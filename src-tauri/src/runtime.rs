use crate::config::NetworkMode;
use crate::network::{self, LanCandidate, NetworkProfile};
use crate::secret;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

const LOOPBACK: &str = "127.0.0.1";
const MANIFEST_SCHEMA_VERSION: u32 = 3;
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_LOG_CHUNK_BYTES: usize = 8192;
const PORT_ATTEMPTS: usize = 3;
const WORDPRESS_CRON_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const DATABASE_RUNTIME_USER: &str = "coffeepos_runtime";
pub(crate) const DATABASE_WORDPRESS_USER: &str = "coffeepos_wp";
pub(crate) const DATABASE_NAME: &str = "coffeepos";
pub(crate) const DATABASE_RUNTIME_SECRET: &str = "config/database-runtime.secret";
pub(crate) const DATABASE_WORDPRESS_SECRET: &str = "config/database-wordpress.secret";
pub(crate) const MACHINE_TOKEN_SECRET: &str = "config/machine-token.secret";
pub(crate) const MACHINE_TOKEN_PENDING_SECRET: &str = "config/machine-token.pending.secret";
const COFFEEPOS_HEALTH_SCHEMA_VERSION: u32 = 1;
const COFFEEPOS_HEALTH_INTERVAL: Duration = Duration::from_secs(15);
const PHP_FASTCGI_WORKERS: usize = 4;
const REQUEST_DRAIN_MARKER: &str = "config/runtime-draining.flag";
const RUNTIME_PREPEND_GATE: &str = r#"<?php
$marker = getenv('COFFEEPOS_DRAIN_MARKER');
$requestPath = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH) ?: '/';
$requestPath = rawurldecode($requestPath);
$isDrainProbe = str_starts_with($requestPath, '/.coffeepos-runtime-health-')
    && str_ends_with($requestPath, '.php');
if ($marker && is_file($marker) && !$isDrainProbe) {
    http_response_code(503);
    header('Retry-After: 1');
    header('Connection: close');
    echo 'CoffeePOS is shutting down';
    exit;
}
"#;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    NotInstalled,
    Installing,
    Stopped,
    Starting,
    Running,
    Stopping,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStartupStage {
    #[default]
    Idle,
    Preparing,
    DatabaseStarting,
    DatabaseReady,
    PhpStarting,
    PhpReady,
    WebServerStarting,
    WebServerReady,
    ApplicationHealth,
    Ready,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WordPressHealthState {
    Unavailable,
    Checking,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoffeePosHealthState {
    Unavailable,
    Checking,
    Healthy,
    Degraded,
    Failed,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoffeePosHealthFailureKind {
    TransportBootstrap,
    Authentication,
    Contract,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LanListenerState {
    #[default]
    Disabled,
    Starting,
    Ready,
    Error,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TlsState {
    #[default]
    Disabled,
    Preparing,
    Ready,
    Error,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct NetworkInfo {
    pub configured_mode: NetworkMode,
    pub effective_mode: NetworkMode,
    pub adapter_id: Option<String>,
    pub adapter_name: Option<String>,
    pub lan_address: Option<String>,
    pub internal_origin: Option<String>,
    pub canonical_origin: Option<String>,
    pub lan_listener_state: LanListenerState,
    pub tls_state: TlsState,
    pub network_profile: Option<NetworkProfile>,
    pub last_error: Option<RuntimeErrorInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoffeePosHealthVersions {
    pub wordpress: String,
    pub woocommerce: String,
    pub coffeepos: String,
    pub coffeepos_schema: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoffeePosHealthStore {
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoffeePosHealthPayload {
    pub schema_version: u32,
    pub status: String,
    pub wordpress: bool,
    pub woocommerce: bool,
    pub coffeepos: bool,
    pub database: bool,
    pub versions: CoffeePosHealthVersions,
    pub store: CoffeePosHealthStore,
    pub pos_path: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CoffeePosHealthInfo {
    pub state: CoffeePosHealthState,
    pub failure_kind: Option<CoffeePosHealthFailureKind>,
    pub payload: Option<CoffeePosHealthPayload>,
    pub error: Option<RuntimeErrorInfo>,
}

impl CoffeePosHealthInfo {
    fn unavailable() -> Self {
        Self {
            state: CoffeePosHealthState::Unavailable,
            failure_kind: None,
            payload: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RuntimeErrorInfo {
    pub component: String,
    pub operation: String,
    pub code: String,
    pub message: String,
    pub recovery: String,
}

impl std::fmt::Display for RuntimeErrorInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {}: {} {}",
            self.component, self.operation, self.message, self.recovery
        )
    }
}

impl std::error::Error for RuntimeErrorInfo {}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub state: RuntimeState,
    pub runtime_version: Option<String>,
    pub php_version: Option<String>,
    pub web_server_version: Option<String>,
    pub mariadb_version: Option<String>,
    pub database_port: Option<u16>,
    pub http_port: Option<u16>,
    pub database_pid: Option<u32>,
    pub php_pid: Option<u32>,
    pub web_server_pid: Option<u32>,
    pub wordpress_health: WordPressHealthState,
    pub wordpress_error: Option<RuntimeErrorInfo>,
    pub coffeepos_health: CoffeePosHealthInfo,
    pub network: NetworkInfo,
    pub last_error: Option<RuntimeErrorInfo>,
}

#[derive(Clone, Debug)]
pub(crate) struct BackgroundHealthProbe {
    data_root: PathBuf,
    http_port: u16,
    generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct DatabaseMaintenanceLease {
    pub(crate) runtime_was_running: bool,
    pub(crate) previous_state: RuntimeState,
    pub(crate) database_port: u16,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentHealthState {
    Unavailable,
    Healthy,
    Unhealthy,
    Unknown,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ComponentHealthInfo {
    pub state: ComponentHealthState,
    pub error: Option<RuntimeErrorInfo>,
}

impl ComponentHealthInfo {
    fn unavailable() -> Self {
        Self {
            state: ComponentHealthState::Unavailable,
            error: None,
        }
    }

    fn healthy() -> Self {
        Self {
            state: ComponentHealthState::Healthy,
            error: None,
        }
    }

    fn unhealthy(error: RuntimeErrorInfo) -> Self {
        Self {
            state: ComponentHealthState::Unhealthy,
            error: Some(error),
        }
    }

    fn unknown(error: Option<RuntimeErrorInfo>) -> Self {
        Self {
            state: ComponentHealthState::Unknown,
            error,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HealthDiagnosticsInfo {
    pub runtime_state: RuntimeState,
    pub database: ComponentHealthInfo,
    pub php: ComponentHealthInfo,
    pub wordpress: ComponentHealthInfo,
    pub woocommerce: ComponentHealthInfo,
    pub coffeepos: ComponentHealthInfo,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevelopmentManifest {
    schema_version: u32,
    target: String,
    runtime_version: String,
    php: PhpManifest,
    web_server: WebServerManifest,
    mariadb: MariaDbManifest,
    http_fixture: HttpFixtureManifest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhpManifest {
    version: String,
    archive: String,
    executable: PathBuf,
    cgi: PathBuf,
    ini: PathBuf,
    source: String,
    checksum_source: String,
    archive_sha256: String,
    license: String,
    license_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebServerManifest {
    version: String,
    archive: String,
    executable: PathBuf,
    source: String,
    checksum_source: String,
    archive_sha256: String,
    license: String,
    license_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MariaDbManifest {
    version: String,
    archive: String,
    server: PathBuf,
    client: PathBuf,
    dump: PathBuf,
    import: PathBuf,
    install_db: PathBuf,
    source: String,
    checksum_source: String,
    archive_sha256: String,
    license: String,
    license_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpFixtureManifest {
    router: PathBuf,
    document_root: PathBuf,
    health_path: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedRuntime {
    pub(crate) runtime_version: String,
    pub(crate) php_version: String,
    pub(crate) php_executable: PathBuf,
    pub(crate) php_cgi_executable: PathBuf,
    pub(crate) php_ini: PathBuf,
    pub(crate) web_server_version: String,
    pub(crate) web_server_executable: PathBuf,
    pub(crate) mariadb_version: String,
    pub(crate) mariadb_executable: PathBuf,
    pub(crate) mariadb_client_executable: PathBuf,
    pub(crate) mariadb_dump_executable: PathBuf,
    pub(crate) mariadb_import_executable: PathBuf,
    pub(crate) mariadb_install_db_executable: PathBuf,
    pub(crate) mariadb_base_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RuntimeSettings {
    schema_version: u32,
    database_port: u16,
    http_port: u16,
    #[serde(default)]
    lan_port: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct RuntimeTimeouts {
    pub database_readiness: Duration,
    pub http_readiness: Duration,
    pub wordpress_readiness: Duration,
    pub probe_command: Duration,
    pub request_drain: Duration,
    pub stop: Duration,
}

impl Default for RuntimeTimeouts {
    fn default() -> Self {
        Self {
            database_readiness: Duration::from_secs(15),
            http_readiness: Duration::from_secs(10),
            wordpress_readiness: Duration::from_secs(45),
            probe_command: Duration::from_secs(3),
            request_drain: Duration::from_secs(3),
            stop: Duration::from_secs(5),
        }
    }
}

struct ManagedChild {
    child: Child,
}

impl ManagedChild {
    fn id(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, RuntimeErrorInfo> {
        self.child.try_wait().map_err(|error| {
            error_info(
                "runtime",
                "inspect process",
                format!("Cannot inspect child process state: {error}."),
                "Stop CoffeePOS Desktop and retry. If the process remains, terminate it from the operating system before reopening the app.",
            )
        })
    }

    fn terminate(
        &mut self,
        component: &'static str,
        timeout: Duration,
    ) -> Result<(), RuntimeErrorInfo> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        let pid = self.id();
        self.child.kill().map_err(|error| {
            error_info(
                component,
                "terminate process",
                format!("Cannot terminate managed {component} process {pid}: {error}."),
                "Close the child process from the operating system, then reopen CoffeePOS Desktop.",
            )
        })?;
        wait_for_child_exit(&mut self.child, timeout)
            .map(|_| ())
            .map_err(|error| {
                error_info(
                    component,
                    "wait for process",
                    format!(
                        "Managed {component} process {pid} did not exit after forced termination: {}",
                        error.message
                    ),
                    error.recovery,
                )
            })
    }
}

pub struct RuntimeManager {
    runtime: ResolvedRuntime,
    data_root: PathBuf,
    containment: ProcessContainment,
    state: RuntimeState,
    database_port: Option<u16>,
    http_port: Option<u16>,
    lan_port: Option<u16>,
    web_server_admin_port: Option<u16>,
    database: Option<ManagedChild>,
    web_server: Option<ManagedChild>,
    php: Option<ManagedChild>,
    php_fastcgi_port: Option<u16>,
    cron: Option<ManagedChild>,
    last_cron_spawn: Option<Instant>,
    php_probe: Option<PathBuf>,
    wordpress_health: WordPressHealthState,
    wordpress_error: Option<RuntimeErrorInfo>,
    coffeepos_health: CoffeePosHealthInfo,
    last_coffeepos_probe: Option<Instant>,
    instance_generation: u64,
    log_lock: Arc<Mutex<()>>,
    last_error: Option<RuntimeErrorInfo>,
    configured_network_mode: NetworkMode,
    configured_lan_adapter_id: Option<String>,
    effective_network_mode: NetworkMode,
    lan_candidate: Option<LanCandidate>,
    lan_listener_state: LanListenerState,
    tls_state: TlsState,
    network_last_error: Option<RuntimeErrorInfo>,
    timeouts: RuntimeTimeouts,
    backup_maintenance_active: bool,
    backup_recovery_lease: Option<DatabaseMaintenanceLease>,
    backup_recovery_staging: Option<PathBuf>,
    backup_recovery_children: Vec<ManagedChild>,
}

impl RuntimeManager {
    pub fn from_development(
        project_root: &Path,
        manifest_path: &Path,
        data_root: PathBuf,
    ) -> Result<Self, RuntimeErrorInfo> {
        let runtime = resolve_development_manifest(project_root, manifest_path)?;
        Self::new(runtime, data_root)
    }

    pub fn new(runtime: ResolvedRuntime, data_root: PathBuf) -> Result<Self, RuntimeErrorInfo> {
        if !data_root.is_absolute() {
            return Err(error_info(
                "runtime",
                "configure data path",
                "Application data root must be an absolute path.",
                "Resolve the path with Tauri app_local_data_dir before constructing the runtime manager.",
            ));
        }
        let state = if installation_ready(&data_root) {
            RuntimeState::Stopped
        } else {
            RuntimeState::NotInstalled
        };
        let containment = ProcessContainment::new()?;
        Ok(Self {
            runtime,
            data_root,
            containment,
            state,
            database_port: None,
            http_port: None,
            lan_port: None,
            web_server_admin_port: None,
            database: None,
            web_server: None,
            php: None,
            php_fastcgi_port: None,
            cron: None,
            last_cron_spawn: None,
            php_probe: None,
            wordpress_health: WordPressHealthState::Unavailable,
            wordpress_error: None,
            coffeepos_health: CoffeePosHealthInfo::unavailable(),
            last_coffeepos_probe: None,
            instance_generation: 0,
            log_lock: Arc::new(Mutex::new(())),
            last_error: None,
            configured_network_mode: NetworkMode::LocalOnly,
            configured_lan_adapter_id: None,
            effective_network_mode: NetworkMode::LocalOnly,
            lan_candidate: None,
            lan_listener_state: LanListenerState::Disabled,
            tls_state: TlsState::Disabled,
            network_last_error: None,
            timeouts: RuntimeTimeouts::default(),
            backup_maintenance_active: false,
            backup_recovery_lease: None,
            backup_recovery_staging: None,
            backup_recovery_children: Vec::new(),
        })
    }

    #[cfg(test)]
    pub fn set_timeouts(&mut self, timeouts: RuntimeTimeouts) {
        self.timeouts = timeouts;
    }

    #[cfg(test)]
    pub fn kill_php_for_test(&mut self) {
        if let Some(php) = self.php.as_mut() {
            php.child.kill().unwrap();
            php.child.wait().unwrap();
        }
    }

    #[cfg(test)]
    pub fn replace_php_executable_for_test(&mut self, executable: PathBuf) -> PathBuf {
        std::mem::replace(&mut self.runtime.php_executable, executable)
    }

    pub fn info(&self) -> RuntimeInfo {
        RuntimeInfo {
            state: self.state.clone(),
            runtime_version: Some(self.runtime.runtime_version.clone()),
            php_version: Some(self.runtime.php_version.clone()),
            web_server_version: Some(self.runtime.web_server_version.clone()),
            mariadb_version: Some(self.runtime.mariadb_version.clone()),
            database_port: self.database_port,
            http_port: self.http_port,
            database_pid: self.database.as_ref().map(ManagedChild::id),
            php_pid: self.php.as_ref().map(ManagedChild::id),
            web_server_pid: self.web_server.as_ref().map(ManagedChild::id),
            wordpress_health: self.wordpress_health.clone(),
            wordpress_error: self.wordpress_error.clone(),
            coffeepos_health: self.coffeepos_health.clone(),
            network: self.network_info(),
            last_error: self.last_error.clone(),
        }
    }

    pub fn configure_network(
        &mut self,
        mode: NetworkMode,
        adapter_id: Option<&str>,
    ) -> Result<Option<String>, RuntimeErrorInfo> {
        if matches!(
            self.state,
            RuntimeState::Installing
                | RuntimeState::Starting
                | RuntimeState::Running
                | RuntimeState::Stopping
        ) {
            return Err(network_error(
                "network_busy",
                "configure",
                "Network mode can only be configured while the managed runtime is stopped.",
                "Stop the managed store before changing its network mode.",
            ));
        }
        match mode {
            NetworkMode::LocalOnly => {
                self.configured_network_mode = NetworkMode::LocalOnly;
                self.configured_lan_adapter_id = None;
                self.effective_network_mode = NetworkMode::LocalOnly;
                self.lan_candidate = None;
                self.lan_port = None;
                self.lan_listener_state = LanListenerState::Disabled;
                self.tls_state = TlsState::Disabled;
                self.network_last_error = None;
                Ok(None)
            }
            NetworkMode::Lan => {
                let candidate = match network::select_lan_candidate(adapter_id) {
                    Ok(candidate) => candidate,
                    Err(message) => {
                        let error = network_error(
                        "network_adapter_unavailable",
                        "preflight",
                        message,
                        "Connect an active Private Windows network, then retry LAN mode. CoffeePOS remains local-only.",
                        );
                        self.network_last_error = Some(error.clone());
                        return Err(error);
                    }
                };
                let selected_id = candidate.adapter_id.clone();
                self.configured_network_mode = NetworkMode::Lan;
                self.configured_lan_adapter_id = Some(selected_id.clone());
                self.effective_network_mode = NetworkMode::LocalOnly;
                self.lan_candidate = Some(candidate);
                self.lan_listener_state = LanListenerState::Disabled;
                self.tls_state = TlsState::Disabled;
                self.network_last_error = None;
                Ok(Some(selected_id))
            }
        }
    }

    pub fn record_network_error(&mut self, error: RuntimeErrorInfo) {
        self.network_last_error = Some(error);
    }

    pub fn sync_network_preference_hint(
        &mut self,
        mode: NetworkMode,
        adapter_id: Option<String>,
    ) {
        if matches!(self.state, RuntimeState::NotInstalled | RuntimeState::Stopped)
            && !self.has_managed_children()
        {
            self.configured_network_mode = mode;
            self.configured_lan_adapter_id = if mode == NetworkMode::Lan {
                adapter_id
            } else {
                None
            };
        }
    }

    pub fn network_info(&self) -> NetworkInfo {
        let internal_origin = self
            .http_port
            .map(|port| format!("http://{LOOPBACK}:{port}"));
        let lan_origin = self.lan_candidate.as_ref().and_then(|candidate| {
            self.lan_port
                .map(|port| format!("https://{}:{port}", candidate.address))
        });
        let canonical_origin = if self.effective_network_mode == NetworkMode::Lan {
            lan_origin.clone()
        } else {
            internal_origin.clone()
        };
        NetworkInfo {
            configured_mode: self.configured_network_mode,
            effective_mode: self.effective_network_mode,
            adapter_id: self
                .lan_candidate
                .as_ref()
                .map(|candidate| candidate.adapter_id.clone())
                .or_else(|| self.configured_lan_adapter_id.clone()),
            adapter_name: self
                .lan_candidate
                .as_ref()
                .map(|candidate| candidate.adapter_name.clone()),
            lan_address: self
                .lan_candidate
                .as_ref()
                .map(|candidate| candidate.address.to_string()),
            internal_origin,
            canonical_origin,
            lan_listener_state: self.lan_listener_state.clone(),
            tls_state: self.tls_state.clone(),
            network_profile: self
                .lan_candidate
                .as_ref()
                .map(|candidate| candidate.network_profile.clone()),
            last_error: self.network_last_error.clone(),
        }
    }

    fn canonical_origin(&self) -> Result<String, RuntimeErrorInfo> {
        self.network_info().canonical_origin.ok_or_else(|| {
            network_error(
                "network_origin_unavailable",
                "resolve origin",
                "The managed runtime does not have a canonical origin yet.",
                "Restart the runtime so CoffeePOS Desktop can establish its listener and canonical origin.",
            )
        })
    }

    fn clear_effective_network(&mut self) {
        self.effective_network_mode = NetworkMode::LocalOnly;
        self.lan_port = None;
        self.lan_listener_state = LanListenerState::Disabled;
        self.tls_state = TlsState::Disabled;
    }

    pub(crate) fn provisioning_context(&self) -> (ResolvedRuntime, PathBuf) {
        (self.runtime.clone(), self.data_root.clone())
    }

    pub(crate) fn backup_maintenance_active(&self) -> bool {
        self.backup_maintenance_active
    }

    pub(crate) fn register_backup_recovery_context(
        &mut self,
        lease: &DatabaseMaintenanceLease,
        staging_dir: &Path,
    ) {
        self.backup_recovery_lease = Some(lease.clone());
        self.backup_recovery_staging = Some(staging_dir.to_path_buf());
    }

    pub(crate) fn backup_recovery_context(&self) -> Option<(DatabaseMaintenanceLease, PathBuf)> {
        Some((
            self.backup_recovery_lease.clone()?,
            self.backup_recovery_staging.clone()?,
        ))
    }

    pub(crate) fn backup_recovery_staging_path(&self) -> Option<&Path> {
        self.backup_recovery_staging.as_deref()
    }

    pub(crate) fn clear_backup_recovery_context(&mut self) {
        self.backup_recovery_lease = None;
        self.backup_recovery_staging = None;
        self.backup_recovery_children.clear();
    }

    pub(crate) fn retain_backup_recovery_child(&mut self, child: Child) {
        self.backup_recovery_children.push(ManagedChild { child });
    }

    pub(crate) fn terminate_retained_backup_children(&mut self) -> Result<(), RuntimeErrorInfo> {
        let mut remaining = Vec::new();
        let mut first_error = None;
        for mut child in self.backup_recovery_children.drain(..) {
            let pid = child.id();
            let kill_error = child.child.kill().err();
            match wait_for_child_exit(&mut child.child, self.timeouts.stop) {
                Ok(_) => {}
                Err(wait_error) => {
                    let kill_detail = kill_error
                        .map(|error| format!(" Termination request error: {error}."))
                        .unwrap_or_default();
                    first_error.get_or_insert_with(|| {
                        error_info(
                            "backup_database",
                            "recover backup child",
                            format!(
                                "Managed backup child {pid} could not be confirmed stopped.{} {}",
                                kill_detail, wait_error.message
                            ),
                            "Backup maintenance remains fenced. Stop the remaining process from Windows, then retry backup cleanup.",
                        )
                    });
                    remaining.push(child);
                }
            }
        }
        self.backup_recovery_children = remaining;
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }

    pub(crate) fn enter_database_backup_maintenance(
        &mut self,
    ) -> Result<DatabaseMaintenanceLease, RuntimeErrorInfo> {
        if self.backup_maintenance_active {
            return Err(error_info(
                "backup_database",
                "quiesce",
                "A database backup maintenance lease is already active.",
                "Finish or cancel the current backup cleanup before starting another managed operation.",
            ));
        }
        let before = self.refresh();
        let previous_state = before.state.clone();
        let runtime_was_running = previous_state == RuntimeState::Running;
        match previous_state {
            RuntimeState::Running | RuntimeState::Stopped => {}
            RuntimeState::NotInstalled => {
                return Err(error_info(
                    "backup_database",
                    "quiesce",
                    "The managed store is not installed, so there is no database to back up.",
                    "Finish CoffeePOS provisioning before creating a database backup.",
                ));
            }
            RuntimeState::Installing | RuntimeState::Starting | RuntimeState::Stopping => {
                return Err(error_info(
                    "backup_database",
                    "quiesce",
                    "The runtime is already changing state and cannot enter backup maintenance.",
                    "Wait for the current lifecycle operation to finish, then retry the backup.",
                ));
            }
        }

        if runtime_was_running || self.has_managed_children() {
            if let Err(stop_error) = self.stop() {
                if runtime_was_running && !self.has_managed_children() {
                    return match self.start() {
                        Ok(_) => Err(stop_error),
                        Err(resume_error) => Err(error_info(
                            "backup_database",
                            "quiesce",
                            format!(
                                "CoffeePOS stopped the normal runtime with an error ({}) and could not restore it after aborting backup maintenance: {}",
                                stop_error.message, resume_error.message
                            ),
                            "Keep CoffeePOS open, inspect runtime health, and restore a healthy running state before retrying backup.",
                        )),
                    };
                }
                return Err(stop_error);
            }
        }
        if self.has_managed_children() {
            return Err(error_info(
                "backup_database",
                "quiesce",
                "CoffeePOS could not confirm that the normal runtime stopped before backup maintenance.",
                "Keep the application open, stop the remaining managed process, then retry the backup.",
            ));
        }

        let setup = (|| {
            self.validate_installed_layout()?;
            self.prepare_logs()?;
            let database_port = choose_runtime_port(None, &[])?;
            self.database_port = Some(database_port);
            self.database = Some(self.spawn_database(database_port)?);
            self.wait_database_ready(database_port)?;
            Ok(database_port)
        })();
        let database_port = match setup {
            Ok(port) => port,
            Err(error) => {
                let cleanup_error = if self.has_managed_children() {
                    self.stop().err()
                } else {
                    self.database_port = None;
                    self.state = if installation_ready(&self.data_root) {
                        RuntimeState::Stopped
                    } else {
                        RuntimeState::NotInstalled
                    };
                    None
                };
                let resume_error = if runtime_was_running && !self.has_managed_children() {
                    self.start().err()
                } else {
                    None
                };
                return match (cleanup_error, resume_error) {
                    (None, None) => Err(error),
                    (cleanup_error, resume_error) => {
                        let cleanup_detail = cleanup_error
                            .map(|value| format!(" Cleanup failed: {}", value.message))
                            .unwrap_or_default();
                        let resume_detail = resume_error
                            .map(|value| format!(" Resume failed: {}", value.message))
                            .unwrap_or_default();
                        Err(error_info(
                            "backup_database",
                            "cleanup",
                            format!(
                                "Database backup maintenance could not start: {}{}{}",
                                error.message, cleanup_detail, resume_detail
                            ),
                            "Keep CoffeePOS open, confirm the managed runtime state, then retry backup only after cleanup/recovery is complete.",
                        ))
                    }
                };
            }
        };
        self.state = RuntimeState::Stopped;
        self.wordpress_health = WordPressHealthState::Unavailable;
        self.wordpress_error = None;
        self.clear_coffeepos_health();
        self.last_error = None;
        self.backup_maintenance_active = true;
        self.log_event("database backup maintenance ready");
        Ok(DatabaseMaintenanceLease {
            runtime_was_running,
            previous_state,
            database_port,
        })
    }

    pub(crate) fn finish_database_backup_maintenance(
        &mut self,
        lease: &DatabaseMaintenanceLease,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        self.finish_database_backup_maintenance_with_policy(lease, true)
    }

    pub(crate) fn finish_database_backup_maintenance_stopped(
        &mut self,
        lease: &DatabaseMaintenanceLease,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        self.finish_database_backup_maintenance_with_policy(lease, false)
    }

    fn finish_database_backup_maintenance_with_policy(
        &mut self,
        lease: &DatabaseMaintenanceLease,
        restore_previous_running_state: bool,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        if !self.backup_recovery_children.is_empty() {
            return Err(error_info(
                "backup_database",
                "cleanup",
                "Backup child cleanup is still pending, so the previous runtime state cannot be restored yet.",
                "Retry backup cleanup after Windows confirms all retained backup children have stopped.",
            ));
        }
        let stop_error = if self.database.is_some() || self.has_managed_children() {
            self.stop_for_backup_maintenance().err()
        } else {
            None
        };
        if self.has_managed_children() {
            let detail = stop_error
                .as_ref()
                .map(|error| format!(" Last stop error: {}", error.message))
                .unwrap_or_default();
            return Err(error_info(
                "backup_database",
                "cleanup",
                format!(
                    "CoffeePOS could not confirm that backup-maintenance processes stopped.{detail}"
                ),
                "Keep the application open and stop the remaining managed process before retrying or exiting.",
            ));
        }
        self.backup_maintenance_active = false;
        self.database_port = None;
        self.state = if installation_ready(&self.data_root) {
            RuntimeState::Stopped
        } else {
            RuntimeState::NotInstalled
        };
        self.log_event("database backup maintenance stopped");
        let restored = if restore_previous_running_state && lease.runtime_was_running {
            self.start()
        } else {
            Ok(self.info())
        };
        match (stop_error, restored) {
            (None, Ok(info)) => Ok(info),
            (Some(error), Ok(_)) => Err(error_info(
                "backup_database",
                "cleanup",
                format!(
                    "Backup maintenance required forced database cleanup before the previous runtime state was restored: {}",
                    error.message
                ),
                "The managed child processes are stopped and the previous runtime state was reconciled. Inspect database health before retrying backup.",
            )),
            (None, Err(error)) => Err(error),
            (Some(stop_error), Err(resume_error)) => Err(error_info(
                "backup_database",
                "cleanup",
                format!(
                    "Backup maintenance cleanup reported an error ({}) and the previous runtime state could not be restored: {}",
                    stop_error.message, resume_error.message
                ),
                "Keep CoffeePOS open, confirm all managed processes are stopped, then restore runtime health before retrying backup.",
            )),
        }
    }

    pub(crate) fn seal_database_backup_snapshot(
        &mut self,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        if !self.backup_maintenance_active {
            return Err(error_info(
                "backup_database",
                "seal snapshot",
                "Database backup maintenance is not active.",
                "Restart the backup from the beginning so CoffeePOS can acquire a fresh maintenance lease.",
            ));
        }
        if !self.backup_recovery_children.is_empty() {
            return Err(error_info(
                "backup_database",
                "seal snapshot",
                "Backup child cleanup is still pending, so the database snapshot cannot be sealed.",
                "Keep backup maintenance fenced and retry cleanup after Windows confirms the retained child has stopped.",
            ));
        }

        let stop_result = if self.database.is_some() || self.has_managed_children() {
            self.stop_for_backup_maintenance()
        } else {
            Ok(self.info())
        };
        if self.has_managed_children() {
            return Err(error_info(
                "backup_database",
                "seal snapshot",
                "CoffeePOS could not confirm that the database-only maintenance process stopped.",
                "Keep the backup maintenance fence active and finish cleanup before retrying backup.",
            ));
        }
        stop_result?;
        // Keep the maintenance lease fenced after MariaDB stops. Phase 7.3 captures uploads and
        // portable metadata only after this point, then `finish_database_backup_maintenance`
        // releases the fence and restores the previous runtime state.
        self.backup_maintenance_active = true;
        self.database_port = None;
        self.state = RuntimeState::Stopped;
        self.wordpress_health = WordPressHealthState::Unavailable;
        self.wordpress_error = None;
        self.clear_coffeepos_health();
        self.last_error = None;
        self.log_event("database backup snapshot sealed; maintenance fence retained");
        Ok(self.info())
    }

    pub(crate) fn contain_backup_child(&self, child: &Child) -> Result<(), RuntimeErrorInfo> {
        self.containment.assign(child)
    }

    fn has_php_workers(&self) -> bool {
        self.php.is_some()
    }

    fn has_managed_children(&self) -> bool {
        self.database.is_some()
            || self.web_server.is_some()
            || self.has_php_workers()
            || self.cron.is_some()
    }

    pub fn refresh(&mut self) -> RuntimeInfo {
        self.refresh_cron_child();
        if self.state == RuntimeState::Stopping {
            let database_error = reap_finished_child(&mut self.database, "database");
            let web_server_error = reap_finished_child(&mut self.web_server, "web server");
            let php_error = reap_finished_child(&mut self.php, "php");
            if let Some(error) = database_error.or(web_server_error).or(php_error) {
                self.last_error = Some(error);
            }
        }
        if self.state == RuntimeState::Running {
            let database_exit = child_exit(&mut self.database, "database");
            let web_server_exit = child_exit(&mut self.web_server, "web server");
            let php_exit = child_exit(&mut self.php, "php");
            if let Some(error) = database_exit.or(web_server_exit).or(php_exit) {
                let cleanup_error = self.cleanup_started().err();
                self.state = if self.has_managed_children() {
                    RuntimeState::Stopping
                } else {
                    RuntimeState::Stopped
                };
                if self.database.is_none() {
                    self.database_port = None;
                }
                if !self.has_php_workers() && self.web_server.is_none() {
                    self.http_port = None;
                    self.php_fastcgi_port = None;
                    self.web_server_admin_port = None;
                    self.clear_effective_network();
                }
                self.last_error = Some(cleanup_error.unwrap_or(error));
                self.wordpress_health = WordPressHealthState::Unavailable;
                self.wordpress_error = None;
                self.clear_coffeepos_health();
                self.log_event("runtime child exited unexpectedly");
            }
        } else if !self.has_managed_children() {
            self.state = if installation_ready(&self.data_root) {
                RuntimeState::Stopped
            } else {
                RuntimeState::NotInstalled
            };
            self.wordpress_health = WordPressHealthState::Unavailable;
            self.wordpress_error = None;
            self.clear_coffeepos_health();
        }
        self.info()
    }

    pub fn start(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        let mut progress = |_| {};
        self.start_with_wordpress_health(true, &mut progress)
    }

    pub(crate) fn start_with_progress<F>(
        &mut self,
        mut progress: F,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo>
    where
        F: FnMut(RuntimeStartupStage),
    {
        self.start_with_wordpress_health(true, &mut progress)
    }

    pub(crate) fn start_for_provisioning(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        let mut progress = |_| {};
        self.start_with_wordpress_health(false, &mut progress)
    }

    fn start_with_wordpress_health<F>(
        &mut self,
        check_wordpress_health: bool,
        progress: &mut F,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo>
    where
        F: FnMut(RuntimeStartupStage),
    {
        if self.backup_maintenance_active {
            return Err(error_info(
                "runtime",
                "start",
                "Runtime start is blocked while database backup maintenance is active.",
                "Finish or cancel the current backup cleanup before starting the store.",
            ));
        }
        self.refresh();
        match self.state {
            RuntimeState::Running => return Ok(self.info()),
            RuntimeState::Installing | RuntimeState::Starting | RuntimeState::Stopping => {
                return Err(error_info(
                    "runtime",
                    "start",
                    "Runtime lifecycle transition is already in progress.",
                    "Wait for the current start or stop operation to finish before retrying.",
                ));
            }
            RuntimeState::NotInstalled | RuntimeState::Stopped => {}
        }

        if let Err(error) = self.validate_installed_layout() {
            self.state = RuntimeState::NotInstalled;
            self.last_error = Some(error.clone());
            return Err(error);
        }

        if let Err(error) = self.prepare_logs() {
            self.last_error = Some(error.clone());
            return Err(error);
        }
        let settings = match self.load_runtime_settings() {
            Ok(settings) => settings,
            Err(error) => {
                self.state = RuntimeState::Stopped;
                self.last_error = Some(error.clone());
                return Err(error);
            }
        };
        if self.configured_network_mode == NetworkMode::Lan {
            let candidate = network::select_lan_candidate(self.configured_lan_adapter_id.as_deref())
                .map_err(|message| {
                    let error = network_error(
                        "network_adapter_unavailable",
                        "preflight",
                        message,
                        "Reconnect the selected Private Windows network or disable LAN mode. CoffeePOS has not exposed a LAN listener.",
                    );
                    self.network_last_error = Some(error.clone());
                    error
                })?;
            self.configured_lan_adapter_id = Some(candidate.adapter_id.clone());
            self.lan_candidate = Some(candidate);
        } else {
            self.lan_candidate = None;
            self.clear_effective_network();
        }
        self.state = RuntimeState::Starting;
        progress(RuntimeStartupStage::Preparing);
        self.wordpress_health = WordPressHealthState::Checking;
        self.wordpress_error = None;
        self.clear_coffeepos_health();
        self.last_error = None;
        self.log_event("runtime start requested");

        let mut last_error = None;
        let mut excluded_ports = Vec::new();
        for attempt in 0..PORT_ATTEMPTS {
            let database_port = match choose_runtime_port(
                settings.as_ref().map(|settings| settings.database_port),
                &excluded_ports,
            ) {
                Ok(port) => port,
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            };
            excluded_ports.push(database_port);
            let http_port = match choose_runtime_port(
                settings.as_ref().map(|settings| settings.http_port),
                &excluded_ports,
            ) {
                Ok(port) => port,
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            };
            excluded_ports.push(http_port);
            let lan_port = if self.configured_network_mode == NetworkMode::Lan {
                let candidate = self.lan_candidate.as_ref().ok_or_else(|| {
                    network_error(
                        "network_adapter_unavailable",
                        "preflight",
                        "The selected LAN adapter disappeared before listener preparation.",
                        "Reconnect the selected Private Windows network or disable LAN mode.",
                    )
                })?;
                match choose_lan_port(
                    candidate.address,
                    settings.as_ref().and_then(|settings| settings.lan_port),
                    &excluded_ports,
                ) {
                    Ok(port) => {
                        excluded_ports.push(port);
                        Some(port)
                    }
                    Err(error) => {
                        last_error = Some(error);
                        break;
                    }
                }
            } else {
                None
            };
            self.database_port = Some(database_port);
            self.http_port = Some(http_port);
            self.lan_port = lan_port;

            match self.start_attempt(database_port, http_port, lan_port, progress) {
                Ok(()) => {
                    if let Err(error) =
                        self.persist_runtime_settings(database_port, http_port, lan_port)
                    {
                        let cleanup_error = self.cleanup_started().err();
                        if self.database.is_none() {
                            self.database_port = None;
                        }
                        if !self.has_php_workers() && self.web_server.is_none() {
                            self.http_port = None;
                            self.php_fastcgi_port = None;
                            self.web_server_admin_port = None;
                            self.clear_effective_network();
                        }
                        last_error = Some(cleanup_error.unwrap_or(error));
                        break;
                    }
                    self.state = RuntimeState::Running;
                    if self.configured_network_mode == NetworkMode::Lan {
                        self.effective_network_mode = NetworkMode::Lan;
                        self.lan_listener_state = LanListenerState::Ready;
                        self.tls_state = TlsState::Ready;
                        self.network_last_error = None;
                    } else {
                        self.clear_effective_network();
                    }
                    self.instance_generation = self.instance_generation.wrapping_add(1);
                    self.last_error = None;
                    if check_wordpress_health {
                        progress(RuntimeStartupStage::ApplicationHealth);
                        self.refresh_wordpress_health();
                    } else {
                        self.wordpress_health = WordPressHealthState::Unavailable;
                        self.wordpress_error = None;
                        self.clear_coffeepos_health();
                    }
                    progress(RuntimeStartupStage::Ready);
                    self.log_event("runtime ready");
                    return Ok(self.info());
                }
                Err(error) => {
                    let retryable = error.operation == "readiness" && attempt + 1 < PORT_ATTEMPTS;
                    let cleanup_error = self.cleanup_started().err();
                    self.remove_php_probe();
                    if self.database.is_none() {
                        self.database_port = None;
                    }
                    if !self.has_php_workers() && self.web_server.is_none() {
                        self.http_port = None;
                        self.php_fastcgi_port = None;
                        self.web_server_admin_port = None;
                        self.clear_effective_network();
                    }
                    last_error = Some(cleanup_error.unwrap_or(error));
                    if self.has_managed_children() {
                        break;
                    }
                    if !retryable {
                        break;
                    }
                    self.log_event("runtime readiness retry");
                }
            }
        }

        self.state = if self.has_managed_children() {
            RuntimeState::Stopping
        } else {
            RuntimeState::Stopped
        };
        let error = last_error.unwrap_or_else(|| {
            error_info(
                "runtime",
                "start",
                "Runtime did not start.",
                "Inspect runtime logs, correct the reported failure, then retry.",
            )
        });
        self.last_error = Some(error.clone());
        self.wordpress_health = WordPressHealthState::Unavailable;
        self.wordpress_error = None;
        self.clear_coffeepos_health();
        self.clear_effective_network();
        self.log_event("runtime start failed");
        Err(error)
    }

    pub(crate) fn refresh_wordpress_health(&mut self) -> RuntimeInfo {
        if self.state != RuntimeState::Running
            || !self.has_php_workers()
            || self.web_server.is_none()
            || self.database.is_none()
        {
            self.wordpress_health = WordPressHealthState::Unavailable;
            self.wordpress_error = None;
            self.clear_coffeepos_health();
            return self.info();
        }
        let Some(http_port) = self.http_port else {
            self.wordpress_health = WordPressHealthState::Unavailable;
            self.wordpress_error = None;
            self.clear_coffeepos_health();
            return self.info();
        };
        self.wordpress_health = WordPressHealthState::Checking;
        self.wordpress_error = None;
        match self.wait_wordpress_ready(http_port) {
            Ok(()) => {
                self.wordpress_health = WordPressHealthState::Healthy;
                self.log_event("wordpress healthy");
                if let Err(error) = self.maybe_spawn_wordpress_cron(true) {
                    self.log_event(&format!("wordpress cron spawn failed: {error}"));
                }
                self.refresh_coffeepos_health();
            }
            Err(error) => {
                self.wordpress_health = WordPressHealthState::Unhealthy;
                self.wordpress_error = Some(error);
                self.clear_coffeepos_health();
                self.log_event("wordpress health check failed");
            }
        }
        self.info()
    }

    pub(crate) fn refresh_coffeepos_health(&mut self) -> RuntimeInfo {
        if self.state != RuntimeState::Running
            || self.wordpress_health != WordPressHealthState::Healthy
            || !self.has_php_workers()
            || self.web_server.is_none()
            || self.database.is_none()
        {
            self.clear_coffeepos_health();
            return self.info();
        }
        let Some(http_port) = self.http_port else {
            self.clear_coffeepos_health();
            return self.info();
        };
        self.coffeepos_health = CoffeePosHealthInfo {
            state: CoffeePosHealthState::Checking,
            failure_kind: None,
            payload: None,
            error: None,
        };
        self.last_coffeepos_probe = Some(Instant::now());
        self.coffeepos_health = probe_coffeepos_health(&self.data_root, http_port);
        match self.coffeepos_health.state {
            CoffeePosHealthState::Healthy => self.log_event("coffeepos application healthy"),
            CoffeePosHealthState::Degraded => self.log_event("coffeepos application degraded"),
            CoffeePosHealthState::Failed => self.log_event("coffeepos application health failed"),
            CoffeePosHealthState::Unavailable | CoffeePosHealthState::Checking => {}
        }
        self.info()
    }

    pub(crate) fn prepare_background_maintenance(
        &mut self,
    ) -> (RuntimeInfo, Option<BackgroundHealthProbe>) {
        if self.backup_maintenance_active {
            return (self.info(), None);
        }
        let runtime = self.refresh();
        if runtime.state != RuntimeState::Running {
            return (runtime, None);
        }
        if let Err(error) = self.maybe_spawn_wordpress_cron(false) {
            self.log_event(&format!("wordpress cron spawn failed: {error}"));
        }
        if self.wordpress_health != WordPressHealthState::Healthy {
            return (self.info(), None);
        }
        let due = self
            .last_coffeepos_probe
            .map(|instant| instant.elapsed() >= COFFEEPOS_HEALTH_INTERVAL)
            .unwrap_or(true);
        if !due {
            return (self.info(), None);
        }
        let Some(http_port) = self.http_port else {
            return (self.info(), None);
        };
        self.last_coffeepos_probe = Some(Instant::now());
        let probe = BackgroundHealthProbe {
            data_root: self.data_root.clone(),
            http_port,
            generation: self.instance_generation,
        };
        (self.info(), Some(probe))
    }

    pub(crate) fn run_background_health_probe(
        probe: &BackgroundHealthProbe,
    ) -> CoffeePosHealthInfo {
        probe_coffeepos_health(&probe.data_root, probe.http_port)
    }

    pub(crate) fn commit_background_health(
        &mut self,
        probe: &BackgroundHealthProbe,
        health: CoffeePosHealthInfo,
    ) -> RuntimeInfo {
        let runtime = self.refresh();
        if runtime.state != RuntimeState::Running
            || self.wordpress_health != WordPressHealthState::Healthy
            || self.instance_generation != probe.generation
            || self.http_port != Some(probe.http_port)
        {
            return runtime;
        }
        self.coffeepos_health = health;
        match self.coffeepos_health.state {
            CoffeePosHealthState::Healthy => self.log_event("coffeepos application healthy"),
            CoffeePosHealthState::Degraded => self.log_event("coffeepos application degraded"),
            CoffeePosHealthState::Failed => self.log_event("coffeepos application health failed"),
            CoffeePosHealthState::Unavailable | CoffeePosHealthState::Checking => {}
        }
        self.info()
    }

    pub(crate) fn health_diagnostics(&mut self) -> HealthDiagnosticsInfo {
        let runtime = self.refresh();
        if runtime.state != RuntimeState::Running {
            return health_diagnostics_for_inactive_runtime(&runtime);
        }

        let mut database = match self.database_port {
            Some(port) => match self.database_probe(port) {
                Ok(true) => ComponentHealthInfo::healthy(),
                Ok(false) => ComponentHealthInfo::unhealthy(error_info(
                    "database",
                    "health",
                    "MariaDB did not accept the authenticated diagnostic query.",
                    "Retry the health check. If it still fails, restart the runtime and inspect the database log before attempting repair.",
                )),
                Err(error) => ComponentHealthInfo::unhealthy(error),
            },
            None => ComponentHealthInfo::unhealthy(error_info(
                "database",
                "health",
                "The running runtime has no database port to probe.",
                "Restart the runtime so MariaDB can be assigned and verified on a managed loopback port.",
            )),
        };

        let php = match self.http_port {
            Some(port) => self.probe_php_health_once(port),
            None => ComponentHealthInfo::unhealthy(error_info(
                "php",
                "health",
                "The running runtime has no HTTP port to probe.",
                "Restart the runtime so PHP can be assigned and verified on a managed loopback port.",
            )),
        };

        let mut wordpress = if database.state == ComponentHealthState::Healthy
            && php.state == ComponentHealthState::Healthy
        {
            let Some(port) = self.http_port else {
                unreachable!("healthy PHP diagnostic requires an HTTP port")
            };
            if wordpress_http_probe(port) {
                self.wordpress_health = WordPressHealthState::Healthy;
                self.wordpress_error = None;
                ComponentHealthInfo::healthy()
            } else {
                let error = error_info(
                    "wordpress",
                    "health",
                    "WordPress did not return the expected login readiness response.",
                    "Retry the health check. If WordPress remains unavailable while Database and PHP are healthy, restart the runtime and inspect WordPress/PHP logs before repair.",
                );
                self.wordpress_health = WordPressHealthState::Unhealthy;
                self.wordpress_error = Some(error.clone());
                self.clear_coffeepos_health();
                ComponentHealthInfo::unhealthy(error)
            }
        } else {
            self.wordpress_health = WordPressHealthState::Unavailable;
            self.wordpress_error = None;
            self.clear_coffeepos_health();
            ComponentHealthInfo::unknown(None)
        };

        let (woocommerce, coffeepos) = if wordpress.state == ComponentHealthState::Healthy {
            self.refresh_coffeepos_health();
            match &self.coffeepos_health {
                CoffeePosHealthInfo {
                    payload: Some(payload),
                    ..
                } => {
                    if !payload.database {
                        database = ComponentHealthInfo::unhealthy(machine_component_error(
                            "database", "Database",
                        ));
                    }
                    if !payload.wordpress {
                        wordpress = ComponentHealthInfo::unhealthy(machine_component_error(
                            "wordpress",
                            "WordPress",
                        ));
                    }
                    (
                        machine_component_health(payload.woocommerce, "woocommerce", "WooCommerce"),
                        machine_component_health(payload.coffeepos, "coffeepos", "CoffeePOS"),
                    )
                }
                CoffeePosHealthInfo {
                    state: CoffeePosHealthState::Failed,
                    error,
                    ..
                } => (
                    ComponentHealthInfo::unknown(None),
                    ComponentHealthInfo::unknown(error.clone()),
                ),
                CoffeePosHealthInfo {
                    state: CoffeePosHealthState::Checking,
                    ..
                }
                | CoffeePosHealthInfo {
                    state: CoffeePosHealthState::Unavailable,
                    ..
                } => (
                    ComponentHealthInfo::unknown(None),
                    ComponentHealthInfo::unknown(None),
                ),
                CoffeePosHealthInfo {
                    state: CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded,
                    payload: None,
                    ..
                } => (
                    ComponentHealthInfo::unknown(None),
                    ComponentHealthInfo::unknown(None),
                ),
            }
        } else {
            (
                ComponentHealthInfo::unknown(None),
                ComponentHealthInfo::unknown(None),
            )
        };

        HealthDiagnosticsInfo {
            runtime_state: self.state.clone(),
            database,
            php,
            wordpress,
            woocommerce,
            coffeepos,
        }
    }

    fn probe_php_health_once(&mut self, port: u16) -> ComponentHealthInfo {
        let nonce = probe_nonce();
        let probe_name = match self.write_php_probe(&nonce) {
            Ok(probe_name) => probe_name,
            Err(error) => return ComponentHealthInfo::unhealthy(error),
        };
        let healthy = http_probe(port, &probe_name, &nonce);
        self.remove_php_probe();
        if healthy {
            ComponentHealthInfo::healthy()
        } else {
            ComponentHealthInfo::unhealthy(error_info(
                "php",
                "health",
                "PHP did not execute the expected nonce diagnostic response.",
                "Retry the health check. If it still fails, restart the runtime and inspect the PHP log before attempting repair.",
            ))
        }
    }

    fn clear_coffeepos_health(&mut self) {
        self.coffeepos_health = CoffeePosHealthInfo::unavailable();
        self.last_coffeepos_probe = None;
    }

    pub fn wordpress_url(&self) -> Result<String, RuntimeErrorInfo> {
        if self.state != RuntimeState::Running {
            return Err(error_info(
                "wordpress",
                "open",
                "WordPress cannot be opened because the local runtime is not running.",
                "Start the local runtime and wait for WordPress health to become healthy before opening the site.",
            ));
        }
        if self.wordpress_health != WordPressHealthState::Healthy {
            return Err(error_info(
                "wordpress",
                "open",
                "WordPress cannot be opened because health has not been verified for the current runtime instance.",
                "Wait for WordPress health to become healthy or restart the runtime if the health check failed.",
            ));
        }
        self.http_port.ok_or_else(|| {
            error_info(
                "wordpress",
                "open",
                "WordPress cannot be opened because the managed HTTP port is unavailable.",
                "Restart the runtime so CoffeePOS Desktop can select and verify a loopback HTTP port.",
            )
        })?;
        Ok(format!("{}/", self.canonical_origin()?.trim_end_matches('/')))
    }

    pub fn pos_url(&self) -> Result<String, RuntimeErrorInfo> {
        if self.state != RuntimeState::Running {
            return Err(error_info(
                "coffeepos",
                "open POS",
                "CoffeePOS cannot be opened because the local runtime is not running.",
                "Start the store and wait until CoffeePOS reports ready before opening the POS.",
            ));
        }
        if self.wordpress_health != WordPressHealthState::Healthy {
            return Err(error_info(
                "coffeepos",
                "open POS",
                "CoffeePOS cannot be opened because WordPress health has not been verified for the current runtime instance.",
                "Wait for the store health check to finish or retry the health check before opening the POS.",
            ));
        }
        if self.coffeepos_health.state != CoffeePosHealthState::Healthy {
            return Err(error_info(
                "coffeepos",
                "open POS",
                "CoffeePOS cannot be opened because application health is not healthy.",
                "Retry the store health check and open Diagnostics if CoffeePOS remains unavailable or degraded.",
            ));
        }
        let payload = self.coffeepos_health.payload.as_ref().ok_or_else(|| {
            error_info(
                "coffeepos",
                "open POS",
                "CoffeePOS reported healthy without a usable POS route.",
                "Retry the store health check. If the problem continues, use a compatible CoffeePOS plugin build.",
            )
        })?;
        if !safe_pos_path(&payload.pos_path) {
            return Err(error_info(
                "coffeepos",
                "open POS",
                "CoffeePOS reported an unsafe POS route.",
                "Restore a compatible CoffeePOS router/settings configuration and retry the health check.",
            ));
        }
        self.http_port.ok_or_else(|| {
            error_info(
                "coffeepos",
                "open POS",
                "CoffeePOS cannot be opened because the managed HTTP port is unavailable.",
                "Restart the runtime so CoffeePOS Desktop can select and verify a loopback HTTP port.",
            )
        })?;
        Ok(format!("{}{}", self.canonical_origin()?.trim_end_matches('/'), payload.pos_path))
    }

    pub fn stop(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        if self.backup_maintenance_active {
            return Err(error_info(
                "runtime",
                "stop",
                "Runtime stop is blocked while database backup maintenance is active.",
                "Finish or cancel the current backup cleanup before stopping the store.",
            ));
        }
        self.stop_internal()
    }

    fn stop_for_backup_maintenance(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        self.stop_internal()
    }

    fn stop_internal(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        self.refresh();
        if (self.state == RuntimeState::NotInstalled || self.state == RuntimeState::Stopped)
            && !self.has_managed_children()
        {
            return Ok(self.info());
        }
        if self.state == RuntimeState::Installing || self.state == RuntimeState::Starting {
            return Err(error_info(
                "runtime",
                "stop",
                "Runtime lifecycle transition is already in progress.",
                "Wait for the current lifecycle operation to finish before retrying.",
            ));
        }

        let mut drain_gate_active = false;
        self.state = RuntimeState::Stopping;
        self.wordpress_health = WordPressHealthState::Unavailable;
        self.wordpress_error = None;
        self.clear_coffeepos_health();
        self.log_event("runtime stop requested");
        let mut failure = None;

        if let Some(cron) = self.cron.as_mut() {
            if let Err(error) = cron.terminate("cron", self.timeouts.stop) {
                failure = Some(error);
            } else {
                self.cron = None;
            }
        }

        self.drain_php_requests();
        if self.web_server.is_some() && !drain_gate_active {
            match self.begin_request_drain() {
                Ok(()) => drain_gate_active = true,
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Some(web_server) = self.web_server.as_mut() {
            if let Err(error) = web_server.terminate("web server", self.timeouts.stop) {
                failure.get_or_insert(error);
            } else {
                self.web_server = None;
            }
        }
        if let Some(php) = self.php.as_mut() {
            if let Err(error) = php.terminate("php", self.timeouts.stop) {
                failure.get_or_insert(error);
            } else {
                self.php = None;
            }
        }

        if self.cron.is_none() && self.web_server.is_none() && self.php.is_none() {
            if self.database.is_some() {
                if let Err(error) = self.shutdown_database_gracefully() {
                    failure.get_or_insert(error);
                }
            }
            if let Some(database) = self.database.as_mut() {
                if let Err(error) = database.terminate("database", self.timeouts.stop) {
                    failure.get_or_insert(error);
                } else {
                    self.database = None;
                }
            }
        } else {
            self.log_event(
                "database shutdown deferred because managed cron/web/PHP work is still alive",
            );
        }

        self.remove_php_probe();
        if self.database.is_none() {
            self.database_port = None;
        }
        if !self.has_php_workers() && self.web_server.is_none() {
            self.http_port = None;
            self.php_fastcgi_port = None;
            self.web_server_admin_port = None;
            self.clear_effective_network();
        }
        self.state = if self.has_managed_children() {
            RuntimeState::Stopping
        } else if installation_ready(&self.data_root) {
            RuntimeState::Stopped
        } else {
            RuntimeState::NotInstalled
        };
        if drain_gate_active && !self.has_php_workers() && self.web_server.is_none() {
            if let Err(error) = self.clear_request_drain_marker() {
                failure.get_or_insert(error);
            }
        } else if drain_gate_active {
            self.log_event("request drain marker retained while managed PHP is still alive");
        }

        if let Some(error) = failure {
            self.last_error = Some(error.clone());
            self.log_event("runtime stop required forced cleanup");
            return Err(error);
        }
        self.last_error = None;
        self.log_event("runtime stopped");
        Ok(self.info())
    }

    pub fn requires_exit_confirmation(&self) -> bool {
        self.backup_maintenance_active
            || !self.backup_recovery_children.is_empty()
            || self.has_managed_children()
            || matches!(
                self.state,
                RuntimeState::Installing
                    | RuntimeState::Starting
                    | RuntimeState::Running
                    | RuntimeState::Stopping
            )
    }

    fn wait_wordpress_ready(&mut self, port: u16) -> Result<(), RuntimeErrorInfo> {
        let deadline = Instant::now() + self.timeouts.wordpress_readiness;
        loop {
            if self.web_stack_finished("WordPress health")? {
                return Err(error_info(
                    "wordpress",
                    "health",
                    "The web/PHP serving stack exited before WordPress health could be verified.",
                    "Inspect logs/web-server.log and logs/php.log, restart the runtime, and retry the WordPress health check.",
                ));
            }
            if wordpress_http_probe(port) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(error_info(
                    "wordpress",
                    "health",
                    "Runtime services are running, but WordPress did not return the expected login page before the timeout.",
                    "Keep the installed store data, inspect logs/php.log and logs/database.log, then restart the runtime. Do not reinstall WordPress for this health failure.",
                ));
            }
            thread::sleep(Duration::from_millis(150));
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn restart(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        let mut progress = |_| {};
        self.restart_with_progress_inner(&mut progress)
    }

    pub(crate) fn restart_with_progress<F>(
        &mut self,
        mut progress: F,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo>
    where
        F: FnMut(RuntimeStartupStage),
    {
        self.restart_with_progress_inner(&mut progress)
    }

    fn restart_with_progress_inner<F>(
        &mut self,
        progress: &mut F,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo>
    where
        F: FnMut(RuntimeStartupStage),
    {
        if self.backup_maintenance_active {
            return Err(error_info(
                "runtime",
                "restart",
                "Runtime restart is blocked while database backup maintenance is active.",
                "Finish or cancel the current backup cleanup before restarting the store.",
            ));
        }
        self.refresh();
        match self.state {
            RuntimeState::Installing | RuntimeState::Starting | RuntimeState::Stopping => {
                Err(error_info(
                    "runtime",
                    "restart",
                    "Runtime lifecycle transition is already in progress.",
                    "Wait for the current lifecycle operation to finish before retrying.",
                ))
            }
            RuntimeState::Running => {
                self.stop()?;
                self.start_with_wordpress_health(true, progress)
            }
            RuntimeState::NotInstalled | RuntimeState::Stopped => {
                self.start_with_wordpress_health(true, progress)
            }
        }
    }

    fn start_attempt<F>(
        &mut self,
        database_port: u16,
        http_port: u16,
        lan_port: Option<u16>,
        progress: &mut F,
    ) -> Result<(), RuntimeErrorInfo>
    where
        F: FnMut(RuntimeStartupStage),
    {
        progress(RuntimeStartupStage::DatabaseStarting);
        self.database = Some(self.spawn_database(database_port)?);
        self.wait_database_ready(database_port)?;
        progress(RuntimeStartupStage::DatabaseReady);
        self.log_event("database ready");

        let prepend_gate = self.prepare_runtime_prepend_gate()?;
        let fastcgi_port = choose_runtime_port(None, &[database_port, http_port])?;
        let web_server_admin_port =
            choose_runtime_port(None, &[database_port, http_port, fastcgi_port])?;
        self.php_fastcgi_port = Some(fastcgi_port);
        self.web_server_admin_port = Some(web_server_admin_port);
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                error_info(
                    "php",
                    "load database credential",
                    error,
                    "Run provisioning repair with the same Windows user profile, then retry.",
                )
            })?;
        let canonical_origin = match (&self.lan_candidate, lan_port) {
            (Some(candidate), Some(port)) if self.configured_network_mode == NetworkMode::Lan => {
                format!("https://{}:{port}", candidate.address)
            }
            _ => format!("http://{LOOPBACK}:{http_port}"),
        };
        progress(RuntimeStartupStage::PhpStarting);
        self.php = Some(self.spawn_php_worker(
            fastcgi_port,
            &canonical_origin,
            &prepend_gate,
            &database_password,
        )?);
        self.wait_fastcgi_workers_ready()?;
        progress(RuntimeStartupStage::PhpReady);
        self.log_event("php fastcgi workers ready");

        progress(RuntimeStartupStage::WebServerStarting);
        if self.configured_network_mode == NetworkMode::Lan {
            self.lan_listener_state = LanListenerState::Starting;
            self.tls_state = TlsState::Preparing;
        }
        let web_server_config = self.prepare_web_server_config(
            http_port,
            web_server_admin_port,
            fastcgi_port,
            lan_port,
            &canonical_origin,
        )?;
        self.validate_web_server_config(&web_server_config)?;
        self.web_server = Some(self.spawn_web_server(&web_server_config)?);
        let nonce = probe_nonce();
        let probe_name = self.write_php_probe(&nonce)?;
        let readiness = self.wait_http_ready(http_port, &probe_name, &nonce);
        self.remove_php_probe();
        readiness?;
        if let (Some(candidate), Some(port)) = (&self.lan_candidate, lan_port) {
            if !lan_port_listening(candidate.address, port) {
                self.lan_listener_state = LanListenerState::Error;
                self.tls_state = TlsState::Error;
                let error = network_error(
                    "network_listener_unavailable",
                    "readiness",
                    "Caddy started, but the selected LAN HTTPS listener did not become reachable.",
                    "CoffeePOS will keep LAN disabled. Verify the selected adapter and local port availability, then retry.",
                );
                self.network_last_error = Some(error.clone());
                return Err(error);
            }
        }
        progress(RuntimeStartupStage::WebServerReady);
        self.log_event("concurrent http runtime ready");
        Ok(())
    }

    fn validate_installed_layout(&self) -> Result<(), RuntimeErrorInfo> {
        let database_dir = self.data_root.join("database");
        if !database_dir.join("mysql").is_dir() {
            return Err(not_installed_error(
                "MariaDB data directory is not initialized. The runtime manager will not initialize or replace store data.",
            ));
        }
        if !self.data_root.join(DATABASE_RUNTIME_SECRET).is_file() {
            return Err(not_installed_error(
                "MariaDB runtime credentials are missing. The runtime manager will not invent database credentials.",
            ));
        }
        if !self.data_root.join(DATABASE_WORDPRESS_SECRET).is_file() {
            return Err(not_installed_error(
                "WordPress database credentials are missing. Run Phase 3 provisioning or repair before starting the runtime.",
            ));
        }
        if !self.data_root.join("site").is_dir() {
            return Err(not_installed_error(
                "WordPress site directory is missing. Phase 2 does not provision WordPress files.",
            ));
        }
        Ok(())
    }

    fn spawn_database(&self, port: u16) -> Result<ManagedChild, RuntimeErrorInfo> {
        let database_dir = self.data_root.join("database");
        let mut command = Command::new(&self.runtime.mariadb_executable);
        command
            .arg("--no-defaults")
            .arg(format!(
                "--basedir={}",
                self.runtime.mariadb_base_dir.to_string_lossy()
            ))
            .arg(format!("--datadir={}", database_dir.to_string_lossy()))
            .arg(format!("--port={port}"))
            .arg(format!("--bind-address={LOOPBACK}"))
            .arg("--skip-name-resolve")
            .arg("--skip-log-bin")
            .arg(format!(
                "--pid-file={}",
                database_dir.join("mariadb.pid").to_string_lossy()
            ));
        #[cfg(windows)]
        command.arg("--console");
        command
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME");
        command.current_dir(&self.runtime.mariadb_base_dir);
        self.spawn_logged(command, "database", "database.log")
    }

    fn spawn_php_worker(
        &self,
        worker_port: u16,
        canonical_origin: &str,
        prepend_gate: &Path,
        database_password: &str,
    ) -> Result<ManagedChild, RuntimeErrorInfo> {
        let site = self.data_root.join("site");
        let database_port = self.database_port.ok_or_else(|| {
            error_info(
                "php",
                "spawn",
                "Database port is unavailable while preparing the PHP process.",
                "Restart the runtime so MariaDB can be started before PHP.",
            )
        })?;
        let mut command = Command::new(&self.runtime.php_cgi_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg("-d")
            .arg(format!(
                "auto_prepend_file=\"{}\"",
                caddy_path(prepend_gate)
            ))
            .arg("-b")
            .arg(format!("{LOOPBACK}:{worker_port}"))
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env_remove("PHP_CLI_SERVER_WORKERS")
            .env("PHP_FCGI_CHILDREN", PHP_FASTCGI_WORKERS.to_string())
            .env("PHP_FCGI_MAX_REQUESTS", "500")
            .env("FCGI_WEB_SERVER_ADDRS", LOOPBACK)
            .env("COFFEEPOS_DB_PASSWORD", database_password)
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env(
                "COFFEEPOS_SITE_URL",
                canonical_origin,
            )
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env(
                "COFFEEPOS_DRAIN_MARKER",
                self.data_root.join(REQUEST_DRAIN_MARKER),
            )
            .current_dir(&site);
        self.spawn_logged(command, "php", "php.log")
    }

    fn spawn_web_server(&self, config: &Path) -> Result<ManagedChild, RuntimeErrorInfo> {
        let caddy_data = self.data_root.join("config/caddy-data");
        let caddy_config = self.data_root.join("config/caddy-config");
        fs::create_dir_all(&caddy_data).map_err(|error| {
            error_info(
                "web server",
                "prepare runtime directories",
                format!("Cannot create Caddy data directory: {error}."),
                "Check application-data permissions and retry startup.",
            )
        })?;
        fs::create_dir_all(&caddy_config).map_err(|error| {
            error_info(
                "web server",
                "prepare runtime directories",
                format!("Cannot create Caddy config directory: {error}."),
                "Check application-data permissions and retry startup.",
            )
        })?;
        let mut command = Command::new(&self.runtime.web_server_executable);
        command
            .arg("run")
            .arg("--config")
            .arg(config)
            .arg("--adapter")
            .arg("caddyfile")
            .env("XDG_DATA_HOME", caddy_data)
            .env("XDG_CONFIG_HOME", caddy_config)
            .current_dir(&self.data_root);
        self.spawn_logged(command, "web server", "web-server.log")
    }

    fn validate_web_server_config(&self, config: &Path) -> Result<(), RuntimeErrorInfo> {
        let caddy_data = self.data_root.join("config/caddy-data");
        let caddy_config = self.data_root.join("config/caddy-config");
        fs::create_dir_all(&caddy_data).map_err(|error| {
            error_info(
                "web server",
                "prepare TLS state",
                format!("Cannot create Caddy data directory: {error}."),
                "Check application-data permissions and retry startup.",
            )
        })?;
        fs::create_dir_all(&caddy_config).map_err(|error| {
            error_info(
                "web server",
                "prepare TLS state",
                format!("Cannot create Caddy config directory: {error}."),
                "Check application-data permissions and retry startup.",
            )
        })?;
        let mut command = Command::new(&self.runtime.web_server_executable);
        command
            .arg("validate")
            .arg("--config")
            .arg(config)
            .arg("--adapter")
            .arg("caddyfile")
            .env("XDG_DATA_HOME", caddy_data)
            .env("XDG_CONFIG_HOME", caddy_config)
            .current_dir(&self.data_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_child_command(&mut command);
        let status = run_command_bounded(
            command,
            self.timeouts.probe_command,
            "web server",
            "validate listener and TLS configuration",
            &self.containment,
        )?;
        if status.success() {
            Ok(())
        } else {
            Err(network_error(
                "network_tls_prepare_failed",
                "prepare TLS listener",
                "Caddy rejected the generated listener or local TLS configuration.",
                "CoffeePOS has not advertised the candidate LAN listener. Keep local-only mode and retry after checking the selected adapter and Caddy runtime.",
            ))
        }
    }

    fn maybe_spawn_wordpress_cron(&mut self, force: bool) -> Result<(), RuntimeErrorInfo> {
        if self.state != RuntimeState::Running || self.cron.is_some() {
            return Ok(());
        }
        if !force
            && self
                .last_cron_spawn
                .is_some_and(|last| last.elapsed() < WORDPRESS_CRON_INTERVAL)
        {
            return Ok(());
        }
        let site = self.data_root.join("site");
        let cron_script = site.join("wp-cron.php");
        if !cron_script.is_file() {
            return Ok(());
        }
        let database_port = self.database_port.ok_or_else(|| {
            error_info(
                "wordpress cron",
                "spawn",
                "Database port is unavailable while preparing the managed WordPress cron runner.",
                "Restart the runtime so MariaDB is ready before cron/background jobs are processed.",
            )
        })?;
        let canonical_origin = self.canonical_origin()?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                error_info(
                    "wordpress cron",
                    "load database credential",
                    error,
                    "Run provisioning repair with the same Windows user profile, then retry.",
                )
            })?;
        let mut command = Command::new(&self.runtime.php_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg(&cron_script)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env("COFFEEPOS_DB_PASSWORD", database_password)
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_SITE_URL", canonical_origin)
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env("COFFEEPOS_DESKTOP_CRON", "1")
            .current_dir(&site);
        self.cron = Some(self.spawn_logged(command, "wordpress cron", "cron.log")?);
        self.last_cron_spawn = Some(Instant::now());
        self.log_event("wordpress cron/background worker started");
        Ok(())
    }

    fn refresh_cron_child(&mut self) {
        let Some(cron) = self.cron.as_mut() else {
            return;
        };
        match cron.try_wait() {
            Ok(Some(status)) => {
                self.cron = None;
                if status.success() {
                    self.log_event("wordpress cron/background worker completed");
                } else {
                    self.log_event(&format!(
                        "wordpress cron/background worker exited with status {status}; inspect logs/cron.log"
                    ));
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.last_error = Some(error.clone());
                self.log_event(&format!(
                    "wordpress cron/background worker monitor failed: {error}"
                ));
            }
        }
    }

    fn spawn_logged(
        &self,
        mut command: Command,
        component: &'static str,
        log_name: &'static str,
    ) -> Result<ManagedChild, RuntimeErrorInfo> {
        configure_child_command(&mut command);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            error_info(
                component,
                "spawn",
                format!("Cannot start {component}: {error}."),
                "Verify the development runtime manifest points to a portable executable and inspect the component log before retrying.",
            )
        })?;
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let log_path = self.data_root.join("logs").join(log_name);
        if let Some(stdout) = child.stdout.take() {
            spawn_log_pump(stdout, log_path.clone(), "stdout", self.log_lock.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_pump(stderr, log_path, "stderr", self.log_lock.clone());
        }
        Ok(ManagedChild { child })
    }

    fn wait_database_ready(&mut self, port: u16) -> Result<(), RuntimeErrorInfo> {
        let deadline = Instant::now() + self.timeouts.database_readiness;
        loop {
            if child_finished(&mut self.database, "database", "readiness")? {
                return Err(error_info(
                    "database",
                    "readiness",
                    "MariaDB exited before authenticated readiness succeeded.",
                    "Inspect logs/database.log and verify the existing datadir, runtime bundle, permissions, and selected port.",
                ));
            }
            if self.database_probe(port)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(error_info(
                    "database",
                    "readiness",
                    "MariaDB did not pass authenticated readiness before the timeout.",
                    "Inspect logs/database.log and the protected runtime credential, then retry. The datadir was not modified by the runtime manager.",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn database_probe(&self, port: u16) -> Result<bool, RuntimeErrorInfo> {
        let password =
            secret::load(&self.data_root.join(DATABASE_RUNTIME_SECRET)).map_err(|error| {
                error_info(
                    "database",
                    "readiness probe",
                    error,
                    "Run provisioning repair with the same Windows user profile, then retry.",
                )
            })?;
        let mut command = Command::new(&self.runtime.mariadb_client_executable);
        command
            .arg("--no-defaults")
            .arg("--protocol=tcp")
            .arg(format!("--host={LOOPBACK}"))
            .arg(format!("--port={port}"))
            .arg(format!("--user={DATABASE_RUNTIME_USER}"))
            .arg("--connect-timeout=1")
            .arg("--batch")
            .arg("--skip-column-names")
            .arg("--execute=SELECT 1")
            .env("MYSQL_PWD", password)
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_child_command(&mut command);
        run_command_bounded(
            command,
            self.timeouts.probe_command,
            "database",
            "readiness probe",
            &self.containment,
        )
        .map(|status| status.success())
    }

    fn wait_http_ready(
        &mut self,
        port: u16,
        probe_name: &str,
        nonce: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let deadline = Instant::now() + self.timeouts.http_readiness;
        loop {
            if self.web_stack_finished("readiness")? {
                return Err(error_info(
                    "php",
                    "readiness",
                    "The web/PHP serving stack exited before the HTTP probe identified the expected runtime instance.",
                    "Inspect logs/web-server.log and logs/php.log, then verify the managed runtime bundle and selected loopback ports.",
                ));
            }
            if http_probe(port, probe_name, nonce) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(error_info(
                    "php",
                    "readiness",
                    "PHP did not return the expected runtime probe before the timeout.",
                    "Inspect logs/php.log and verify the bundled PHP can execute scripts from the existing site directory.",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn shutdown_database_gracefully(&mut self) -> Result<(), RuntimeErrorInfo> {
        let Some(port) = self.database_port else {
            return Ok(());
        };
        let password =
            secret::load(&self.data_root.join(DATABASE_RUNTIME_SECRET)).map_err(|error| {
                error_info(
                    "database",
                    "shutdown",
                    error,
                    "Run provisioning repair with the same Windows user profile, then retry.",
                )
            })?;
        let mut command = Command::new(&self.runtime.mariadb_client_executable);
        command
            .arg("--no-defaults")
            .arg("--protocol=tcp")
            .arg(format!("--host={LOOPBACK}"))
            .arg(format!("--port={port}"))
            .arg(format!("--user={DATABASE_RUNTIME_USER}"))
            .arg("--connect-timeout=1")
            .arg("--batch")
            .arg("--skip-column-names")
            .arg("--execute=SHUTDOWN")
            .env("MYSQL_PWD", password)
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_child_command(&mut command);
        let status = run_command_bounded(
            command,
            self.timeouts.probe_command,
            "database",
            "shutdown",
            &self.containment,
        )?;
        if !status.success() {
            return Err(error_info(
                "database",
                "shutdown",
                "MariaDB rejected the graceful shutdown command.",
                "The runtime manager will force-stop the managed database process; inspect logs/database.log before the next start.",
            ));
        }
        if let Some(database) = self.database.as_mut() {
            wait_for_child_exit(&mut database.child, self.timeouts.stop).map(|_| ())?;
        }
        Ok(())
    }

    fn drain_php_requests(&mut self) {
        if self.web_server.is_none() {
            return;
        }
        let Some(admin_port) = self.web_server_admin_port else {
            self.log_event("web server graceful drain unavailable: admin port missing");
            return;
        };
        if let Err(error) = request_caddy_stop(admin_port, self.timeouts.request_drain) {
            self.log_event(&format!("web server graceful stop request failed: {error}"));
            return;
        }
        let Some(web_server) = self.web_server.as_mut() else {
            return;
        };
        match wait_for_child_exit(
            &mut web_server.child,
            self.timeouts.request_drain + Duration::from_secs(1),
        ) {
            Ok(_) => {
                self.web_server = None;
                self.log_event("web server graceful request drain complete");
            }
            Err(_) => {
                self.log_event("web server graceful drain timeout; forcing managed stop");
            }
        }
    }

    fn prepare_runtime_prepend_gate(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config_dir = self.data_root.join("config");
        fs::create_dir_all(&config_dir).map_err(|error| {
            error_info(
                "php",
                "prepare runtime admission gate",
                format!("Cannot create the runtime config directory: {error}."),
                "Check application-data permissions and free disk space, then retry startup.",
            )
        })?;
        self.clear_request_drain_marker()?;
        let gate = config_dir.join("runtime-prepend.php");
        fs::write(&gate, RUNTIME_PREPEND_GATE.as_bytes()).map_err(|error| {
            error_info(
                "php",
                "prepare runtime admission gate",
                format!("Cannot write the managed PHP runtime admission gate: {error}."),
                "Check application-data permissions and free disk space, then retry startup.",
            )
        })?;
        Ok(gate)
    }

    fn prepare_web_server_config(
        &self,
        http_port: u16,
        admin_port: u16,
        fastcgi_port: u16,
        lan_port: Option<u16>,
        canonical_origin: &str,
    ) -> Result<PathBuf, RuntimeErrorInfo> {
        let config_dir = self.data_root.join("config");
        fs::create_dir_all(&config_dir).map_err(|error| {
            error_info(
                "web server",
                "prepare config",
                format!("Cannot create runtime config directory: {error}."),
                "Check application-data permissions and retry startup.",
            )
        })?;
        let site = caddy_path(&self.data_root.join("site"));
        let uploads = caddy_path(&self.data_root.join("uploads"));
        let grace_millis = self.timeouts.request_drain.as_millis().max(1);
        let internal_fastcgi_env = if let (Some(candidate), Some(port)) =
            (&self.lan_candidate, lan_port)
        {
            format!(
                "            env HTTP_HOST \"{}:{port}\"\n            env HTTPS on\n            env SERVER_PORT \"{port}\"\n",
                candidate.address
            )
        } else {
            String::new()
        };
        let internal_site = format!(
            "http://{LOOPBACK}:{http_port} {{\n    route {{\n        @blocked path_regexp blocked (?i)^/(wp-config\\.php|\\.env|composer\\.(?:json|lock)|\\.htaccess)$\n        respond @blocked 404\n\n        handle_path /wp-content/uploads/* {{\n            root * \"{uploads}\"\n            file_server\n        }}\n\n        root * \"{site}\"\n        php_fastcgi {LOOPBACK}:{fastcgi_port} {{\n            root \"{site}\"\n            capture_stderr\n{internal_fastcgi_env}        }}\n        file_server\n    }}\n}}\n"
        );
        let lan_site = match (&self.lan_candidate, lan_port) {
            (Some(candidate), Some(port)) => format!(
                "\nhttps://{}:{port} {{\n    tls internal\n    route {{\n        @internal path /wp-json/coffeepos/v1/system/status /.coffeepos-runtime-health-*\n        respond @internal 404\n\n        @blocked path_regexp blocked (?i)^/(wp-config\\.php|\\.env|composer\\.(?:json|lock)|\\.htaccess)$\n        respond @blocked 404\n\n        handle_path /wp-content/uploads/* {{\n            root * \"{uploads}\"\n            file_server\n        }}\n\n        root * \"{site}\"\n        php_fastcgi {LOOPBACK}:{fastcgi_port} {{\n            root \"{site}\"\n            capture_stderr\n        }}\n        file_server\n    }}\n}}\n",
                candidate.address
            ),
            _ => String::new(),
        };
        let contents = format!(
            "{{\n    admin {LOOPBACK}:{admin_port}\n    persist_config off\n    auto_https disable_redirects\n    grace_period {grace_millis}ms\n}}\n\n# Canonical origin: {canonical_origin}\n{internal_site}{lan_site}"
        );
        let path = config_dir.join("runtime-Caddyfile");
        fs::write(&path, contents.as_bytes()).map_err(|error| {
            error_info(
                "web server",
                "prepare config",
                format!("Cannot write the managed Caddy configuration: {error}."),
                "Check application-data permissions and retry startup.",
            )
        })?;
        Ok(path)
    }

    fn wait_fastcgi_workers_ready(&mut self) -> Result<(), RuntimeErrorInfo> {
        let deadline = Instant::now() + self.timeouts.http_readiness;
        loop {
            if child_finished(&mut self.php, "php", "fastcgi readiness")? {
                return Err(error_info(
                    "php",
                    "readiness",
                    "A managed PHP FastCGI worker exited before the worker pool became ready.",
                    "Inspect logs/php.log and verify php.ini and the managed FastCGI ports.",
                ));
            }
            if self.php_fastcgi_port.is_some_and(loopback_port_listening) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(error_info(
                    "php",
                    "readiness",
                    "PHP FastCGI workers did not bind all managed ports before the timeout.",
                    "Inspect logs/php.log and verify the bundled php-cgi executable and managed runtime configuration.",
                ));
            }
            thread::sleep(Duration::from_millis(75));
        }
    }

    fn web_stack_finished(&mut self, operation: &str) -> Result<bool, RuntimeErrorInfo> {
        if child_finished(&mut self.web_server, "web server", operation)? {
            return Ok(true);
        }
        if child_finished(&mut self.php, "php", operation)? {
            return Ok(true);
        }
        Ok(false)
    }

    fn begin_request_drain(&self) -> Result<(), RuntimeErrorInfo> {
        let marker = self.data_root.join(REQUEST_DRAIN_MARKER);
        if let Some(parent) = marker.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                error_info(
                    "php",
                    "begin request drain",
                    format!("Cannot prepare the runtime drain marker directory: {error}."),
                    "Keep CoffeePOS running, check application-data permissions, then retry shutdown.",
                )
            })?;
        }
        fs::write(&marker, b"draining\n").map_err(|error| {
            error_info(
                "php",
                "begin request drain",
                format!("Cannot enable the request admission gate: {error}."),
                "Keep CoffeePOS running, check application-data permissions, then retry shutdown.",
            )
        })
    }

    fn clear_request_drain_marker(&self) -> Result<(), RuntimeErrorInfo> {
        let marker = self.data_root.join(REQUEST_DRAIN_MARKER);
        match fs::remove_file(&marker) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error_info(
                "php",
                "clear request drain",
                format!("Cannot remove the runtime request drain marker: {error}."),
                "Check application-data permissions before restarting CoffeePOS.",
            )),
        }
    }

    fn cleanup_started(&mut self) -> Result<(), RuntimeErrorInfo> {
        let mut failure = None;
        if let Some(cron) = self.cron.as_mut() {
            if let Err(error) = cron.terminate("cron", self.timeouts.stop) {
                failure = Some(error);
            } else {
                self.cron = None;
            }
        }
        if let Some(web_server) = self.web_server.as_mut() {
            if let Err(error) = web_server.terminate("web server", self.timeouts.stop) {
                failure.get_or_insert(error);
            } else {
                self.web_server = None;
            }
        }
        if let Some(php) = self.php.as_mut() {
            if let Err(error) = php.terminate("php", self.timeouts.stop) {
                failure.get_or_insert(error);
            } else {
                self.php = None;
            }
        }
        if self.cron.is_none() && self.web_server.is_none() && self.php.is_none() {
            if let Some(database) = self.database.as_mut() {
                if let Err(error) = database.terminate("database", self.timeouts.stop) {
                    failure.get_or_insert(error);
                } else {
                    self.database = None;
                }
            }
        } else {
            self.log_event(
                "database cleanup deferred because managed cron/web/PHP work is still alive",
            );
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn prepare_logs(&self) -> Result<(), RuntimeErrorInfo> {
        let logs = self.data_root.join("logs");
        fs::create_dir_all(&logs).map_err(|error| {
            error_info(
                "runtime",
                "prepare logs",
                format!("Cannot create runtime log directory: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        for name in [
            "runtime.log",
            "database.log",
            "php.log",
            "web-server.log",
            "cron.log",
        ] {
            bound_existing_log(&logs.join(name))?;
        }
        Ok(())
    }

    fn load_runtime_settings(&self) -> Result<Option<RuntimeSettings>, RuntimeErrorInfo> {
        let path = self.data_root.join("config/runtime.json");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error_info(
                    "runtime",
                    "read settings",
                    format!("Cannot read config/runtime.json: {error}."),
                    "Check application-data permissions, preserve the file, and retry.",
                ));
            }
        };
        let settings: RuntimeSettings = serde_json::from_slice(&bytes).map_err(|error| {
            error_info(
                "runtime",
                "read settings",
                format!("config/runtime.json is invalid and was preserved: {error}."),
                "Correct or restore config/runtime.json, then retry. Do not delete store data.",
            )
        })?;
        if !matches!(settings.schema_version, 1 | 2)
            || settings.database_port == 0
            || settings.http_port == 0
            || settings.lan_port == Some(0)
        {
            return Err(error_info(
                "runtime",
                "read settings",
                "config/runtime.json contains an unsupported schema or invalid port.",
                "Restore a compatible runtime settings file, then retry.",
            ));
        }
        Ok(Some(settings))
    }

    fn persist_runtime_settings(
        &self,
        database_port: u16,
        http_port: u16,
        lan_port: Option<u16>,
    ) -> Result<(), RuntimeErrorInfo> {
        let config_dir = self.data_root.join("config");
        fs::create_dir_all(&config_dir).map_err(|error| {
            error_info(
                "runtime",
                "save settings",
                format!("Cannot create runtime settings directory: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        let settings = RuntimeSettings {
            schema_version: 2,
            database_port,
            http_port,
            lan_port,
        };
        let mut temporary = NamedTempFile::new_in(&config_dir).map_err(|error| {
            error_info(
                "runtime",
                "save settings",
                format!("Cannot create temporary runtime settings: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        serde_json::to_writer_pretty(&mut temporary, &settings).map_err(|error| {
            error_info(
                "runtime",
                "save settings",
                format!("Cannot serialize runtime settings: {error}."),
                "Retry startup after checking application-data storage.",
            )
        })?;
        temporary.write_all(b"\n").map_err(|error| {
            error_info(
                "runtime",
                "save settings",
                format!("Cannot write runtime settings: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary.as_file().sync_all().map_err(|error| {
            error_info(
                "runtime",
                "save settings",
                format!("Cannot flush runtime settings: {error}."),
                "Check application-data storage health, then retry.",
            )
        })?;
        temporary
            .persist(config_dir.join("runtime.json"))
            .map_err(|error| {
                error_info(
                    "runtime",
                    "save settings",
                    format!("Cannot replace config/runtime.json: {error}."),
                    "Check application-data permissions, preserve the existing file, and retry.",
                )
            })?;
        Ok(())
    }

    fn log_event(&self, event: &str) {
        if let Ok(_guard) = self.log_lock.lock() {
            let _ = append_bounded_log(&self.data_root.join("logs/runtime.log"), "event", event);
        }
    }

    fn write_php_probe(&mut self, nonce: &str) -> Result<String, RuntimeErrorInfo> {
        if self.php_probe.is_some() {
            return Err(error_info(
                "php",
                "prepare readiness probe",
                "A previous temporary PHP readiness probe could not be removed.",
                "Check site directory permissions, remove only the stale .coffeepos-runtime-health-* probe after verifying CoffeePOS Desktop is stopped, then retry.",
            ));
        }
        let file_name = format!(".coffeepos-runtime-health-{nonce}.php");
        let path = self.data_root.join("site").join(&file_name);
        let body = format!(
            "<?php header('Content-Type: text/plain'); if (!function_exists('opcache_get_status') || opcache_get_status(false) === false) {{ http_response_code(500); exit('opcache-disabled'); }} echo '{}';\n",
            nonce
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                error_info(
                    "php",
                    "prepare readiness probe",
                    format!("Cannot create the temporary PHP readiness probe: {error}."),
                    "Check the site directory permissions and free disk space, then retry.",
                )
            })?;
        if let Err(error) = file.write_all(body.as_bytes()) {
            let failure = error_info(
                "php",
                "prepare readiness probe",
                format!("Cannot write the temporary PHP readiness probe: {error}."),
                "Check the site directory permissions and free disk space, then retry.",
            );
            drop(file);
            self.php_probe = Some(path);
            self.remove_php_probe();
            return Err(failure);
        }
        if let Err(error) = file.sync_all() {
            let failure = error_info(
                "php",
                "prepare readiness probe",
                format!("Cannot flush the temporary PHP readiness probe: {error}."),
                "Check the site directory storage health, then retry.",
            );
            drop(file);
            self.php_probe = Some(path);
            self.remove_php_probe();
            return Err(failure);
        }
        self.php_probe = Some(path);
        Ok(file_name)
    }

    fn remove_php_probe(&mut self) {
        let Some(path) = self.php_probe.take() else {
            return;
        };
        if let Err(error) = fs::remove_file(&path) {
            if error.kind() != io::ErrorKind::NotFound {
                self.php_probe = Some(path);
                self.log_event("temporary php readiness probe cleanup failed");
            }
        }
    }
}

impl Drop for RuntimeManager {
    fn drop(&mut self) {
        let _ = self.cleanup_started();
        self.remove_php_probe();
    }
}

pub fn resolve_development_manifest(
    project_root: &Path,
    manifest_path: &Path,
) -> Result<ResolvedRuntime, RuntimeErrorInfo> {
    if !project_root.is_absolute() || !manifest_path.is_absolute() {
        return Err(manifest_error(
            "Project root and development manifest path must both be absolute.",
        ));
    }
    let development_root =
        fs::canonicalize(project_root.join("runtime/development")).map_err(|error| {
            manifest_error(format!(
                "Cannot resolve runtime/development directory: {error}."
            ))
        })?;
    let manifest_path = canonical_file_under(&development_root, manifest_path, "manifest", None)?;
    let manifest_root = manifest_path
        .parent()
        .ok_or_else(|| manifest_error("Development manifest must have a parent directory."))?;
    let bytes = fs::read(&manifest_path)
        .map_err(|error| manifest_error(format!("Cannot read development manifest: {error}.")))?;
    let manifest: DevelopmentManifest = serde_json::from_slice(&bytes).map_err(|error| {
        manifest_error(format!("Development manifest is invalid JSON: {error}."))
    })?;
    validate_manifest_metadata(&manifest)?;

    let _php_license = canonical_file_under(
        &development_root,
        &manifest.php.license_file,
        "PHP license file",
        Some(manifest_root),
    )?;
    let _web_server_license = canonical_file_under(
        &development_root,
        &manifest.web_server.license_file,
        "web server license file",
        Some(manifest_root),
    )?;
    let _mariadb_license = canonical_file_under(
        &development_root,
        &manifest.mariadb.license_file,
        "MariaDB license file",
        Some(manifest_root),
    )?;
    let _fixture_router = canonical_file_under(
        &development_root,
        &manifest.http_fixture.router,
        "HTTP fixture router",
        Some(manifest_root),
    )?;
    let _fixture_document_root = canonical_dir_under(
        &development_root,
        &manifest.http_fixture.document_root,
        "HTTP fixture document root",
        Some(manifest_root),
    )?;

    let mariadb_install_db = canonical_file_under(
        &development_root,
        &manifest.mariadb.install_db,
        "MariaDB install-db executable",
        Some(manifest_root),
    )?;
    let mariadb_dump = canonical_file_under(
        &development_root,
        &manifest.mariadb.dump,
        "MariaDB dump executable",
        Some(manifest_root),
    )?;
    let mariadb_import = canonical_file_under(
        &development_root,
        &manifest.mariadb.import,
        "MariaDB import executable",
        Some(manifest_root),
    )?;
    Ok(ResolvedRuntime {
        runtime_version: manifest.runtime_version,
        php_version: manifest.php.version,
        php_executable: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.php.executable,
            "PHP executable",
            Some(manifest_root),
        )?),
        php_cgi_executable: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.php.cgi,
            "PHP CGI executable",
            Some(manifest_root),
        )?),
        php_ini: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.php.ini,
            "php.ini",
            Some(manifest_root),
        )?),
        web_server_version: manifest.web_server.version,
        web_server_executable: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.web_server.executable,
            "web server executable",
            Some(manifest_root),
        )?),
        mariadb_version: manifest.mariadb.version,
        mariadb_executable: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.mariadb.server,
            "MariaDB executable",
            Some(manifest_root),
        )?),
        mariadb_client_executable: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.mariadb.client,
            "MariaDB client executable",
            Some(manifest_root),
        )?),
        mariadb_dump_executable: command_compatible_path(mariadb_dump),
        mariadb_import_executable: command_compatible_path(mariadb_import),
        mariadb_install_db_executable: command_compatible_path(mariadb_install_db),
        mariadb_base_dir: command_compatible_path(resolve_mariadb_base_dir(
            &development_root,
            &manifest.mariadb.server,
            manifest_root,
        )?),
    })
}

fn command_compatible_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

fn caddy_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn choose_loopback_port(excluded: &[u16]) -> Result<u16, RuntimeErrorInfo> {
    for _ in 0..32 {
        let listener =
            TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).map_err(|error| {
                error_info(
                    "runtime",
                    "select port",
                    format!("Cannot ask the operating system for a loopback port: {error}."),
                    "Check local networking policy and retry.",
                )
            })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                error_info(
                    "runtime",
                    "select port",
                    format!("Cannot inspect the selected loopback port: {error}."),
                    "Retry startup.",
                )
            })?
            .port();
        if !excluded.contains(&port) {
            return Ok(port);
        }
    }
    Err(error_info(
        "runtime",
        "select port",
        "Could not select a distinct loopback port after bounded retries.",
        "Close stale local services and retry CoffeePOS Desktop.",
    ))
}

fn choose_runtime_port(preferred: Option<u16>, excluded: &[u16]) -> Result<u16, RuntimeErrorInfo> {
    if let Some(port) = preferred {
        if port != 0 && !excluded.contains(&port) && loopback_port_available(port) {
            return Ok(port);
        }
    }
    choose_loopback_port(excluded)
}

fn choose_lan_port(
    address: Ipv4Addr,
    preferred: Option<u16>,
    excluded: &[u16],
) -> Result<u16, RuntimeErrorInfo> {
    if let Some(port) = preferred {
        if port != 0 && !excluded.contains(&port) && lan_port_available(address, port) {
            return Ok(port);
        }
    }
    for _ in 0..PORT_ATTEMPTS {
        let listener = TcpListener::bind(SocketAddrV4::new(address, 0)).map_err(|error| {
            network_error(
                "network_bind_failed",
                "select LAN port",
                format!("Cannot reserve a LAN port on {address}: {error}."),
                "Verify that the selected Private network adapter is still active, then retry.",
            )
        })?;
        let port = listener.local_addr().map_err(|error| {
            network_error(
                "network_bind_failed",
                "select LAN port",
                format!("Cannot read the reserved LAN port on {address}: {error}."),
                "Retry LAN mode after checking the selected network adapter.",
            )
        })?.port();
        drop(listener);
        if !excluded.contains(&port) && lan_port_available(address, port) {
            return Ok(port);
        }
    }
    Err(network_error(
        "network_port_unavailable",
        "select LAN port",
        "CoffeePOS could not select an available port on the selected LAN address.",
        "Close stale local listeners or reconnect the selected Private network, then retry.",
    ))
}

fn loopback_port_available(port: u16) -> bool {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
}

fn lan_port_available(address: Ipv4Addr, port: u16) -> bool {
    TcpListener::bind(SocketAddrV4::new(address, port)).is_ok()
}

fn lan_port_listening(address: Ipv4Addr, port: u16) -> bool {
    let address = SocketAddr::V4(SocketAddrV4::new(address, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

fn loopback_port_listening(port: u16) -> bool {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_ok()
}

fn installation_ready(data_root: &Path) -> bool {
    data_root.join("database/mysql").is_dir()
        && data_root.join(DATABASE_RUNTIME_SECRET).is_file()
        && data_root.join(DATABASE_WORDPRESS_SECRET).is_file()
        && data_root.join("site").is_dir()
}

fn validate_manifest_metadata(manifest: &DevelopmentManifest) -> Result<(), RuntimeErrorInfo> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(manifest_error(format!(
            "Unsupported development manifest schema {}.",
            manifest.schema_version
        )));
    }
    let expected_target = current_target_triple()?;
    if manifest.target != expected_target {
        return Err(manifest_error(format!(
            "Development manifest target '{}' does not match current target '{expected_target}'.",
            manifest.target
        )));
    }
    require_nonempty("runtime_version", &manifest.runtime_version)?;
    validate_artifact_metadata(
        "PHP",
        &manifest.php.version,
        &manifest.php.archive,
        &manifest.php.archive_sha256,
        &manifest.php.source,
        &manifest.php.checksum_source,
        &manifest.php.license,
    )?;
    validate_artifact_metadata(
        "web server",
        &manifest.web_server.version,
        &manifest.web_server.archive,
        &manifest.web_server.archive_sha256,
        &manifest.web_server.source,
        &manifest.web_server.checksum_source,
        &manifest.web_server.license,
    )?;
    validate_artifact_metadata(
        "MariaDB",
        &manifest.mariadb.version,
        &manifest.mariadb.archive,
        &manifest.mariadb.archive_sha256,
        &manifest.mariadb.source,
        &manifest.mariadb.checksum_source,
        &manifest.mariadb.license,
    )?;
    require_nonempty(
        "http_fixture.health_path",
        &manifest.http_fixture.health_path,
    )?;
    if !manifest.http_fixture.health_path.starts_with('/') {
        return Err(manifest_error(
            "Development manifest http_fixture.health_path must be an absolute URL path.",
        ));
    }
    Ok(())
}

fn validate_artifact_metadata(
    component: &str,
    version: &str,
    archive: &str,
    sha256: &str,
    source: &str,
    checksum_source: &str,
    license: &str,
) -> Result<(), RuntimeErrorInfo> {
    require_nonempty(&format!("{component} version"), version)?;
    require_nonempty(&format!("{component} archive"), archive)?;
    validate_sha256(component, sha256)?;
    require_nonempty(&format!("{component} source"), source)?;
    require_nonempty(&format!("{component} checksum_source"), checksum_source)?;
    require_nonempty(&format!("{component} license"), license)
}

fn validate_sha256(component: &str, value: &str) -> Result<(), RuntimeErrorInfo> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(manifest_error(format!(
            "{component} sha256 must contain exactly 64 hexadecimal characters."
        )))
    }
}

fn require_nonempty(field: &str, value: &str) -> Result<(), RuntimeErrorInfo> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(manifest_error(format!(
            "Development manifest field '{field}' must be non-empty and contain no control characters."
        )))
    } else {
        Ok(())
    }
}

fn canonical_file_under(
    root: &Path,
    candidate: &Path,
    label: &str,
    relative_root: Option<&Path>,
) -> Result<PathBuf, RuntimeErrorInfo> {
    let path = canonical_under(root, candidate, label, relative_root)?;
    if !path.is_file() {
        return Err(manifest_error(format!("{label} is not a regular file.")));
    }
    Ok(path)
}

fn canonical_dir_under(
    root: &Path,
    candidate: &Path,
    label: &str,
    relative_root: Option<&Path>,
) -> Result<PathBuf, RuntimeErrorInfo> {
    let path = canonical_under(root, candidate, label, relative_root)?;
    if !path.is_dir() {
        return Err(manifest_error(format!("{label} is not a directory.")));
    }
    Ok(path)
}

fn canonical_under(
    root: &Path,
    candidate: &Path,
    label: &str,
    relative_root: Option<&Path>,
) -> Result<PathBuf, RuntimeErrorInfo> {
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else if let Some(relative_root) = relative_root {
        relative_root.join(candidate)
    } else {
        return Err(manifest_error(format!(
            "{label} must use an absolute development path."
        )));
    };
    let path = fs::canonicalize(&candidate)
        .map_err(|error| manifest_error(format!("Cannot resolve {label}: {error}.")))?;
    if !path.starts_with(root) {
        return Err(manifest_error(format!(
            "{label} resolves outside runtime/development."
        )));
    }
    Ok(path)
}

fn resolve_mariadb_base_dir(
    development_root: &Path,
    server: &Path,
    manifest_root: &Path,
) -> Result<PathBuf, RuntimeErrorInfo> {
    let server = canonical_file_under(
        development_root,
        server,
        "MariaDB executable",
        Some(manifest_root),
    )?;
    let bin_dir = server
        .parent()
        .ok_or_else(|| manifest_error("MariaDB server executable has no parent directory."))?;
    let base_dir = bin_dir.parent().ok_or_else(|| {
        manifest_error("MariaDB server executable must be inside a bundle bin directory.")
    })?;
    canonical_dir_under(development_root, base_dir, "MariaDB base directory", None)
}

fn current_target_triple() -> Result<&'static str, RuntimeErrorInfo> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return Ok("x86_64-pc-windows-msvc");
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Ok("aarch64-apple-darwin");
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return Ok("x86_64-apple-darwin");
    }
    #[allow(unreachable_code)]
    Err(manifest_error(
        "This target is not qualified for the CoffeePOS development runtime.",
    ))
}

fn child_finished(
    child: &mut Option<ManagedChild>,
    component: &str,
    operation: &str,
) -> Result<bool, RuntimeErrorInfo> {
    match child.as_mut() {
        Some(child) => child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|error| error_info(component, operation, error.message, error.recovery)),
        None => Ok(true),
    }
}

fn child_exit(child: &mut Option<ManagedChild>, component: &str) -> Option<RuntimeErrorInfo> {
    let child = child.as_mut()?;
    match child.try_wait() {
        Ok(Some(status)) => Some(error_info(
            component,
            "monitor",
            format!("Managed {component} process exited unexpectedly with status {status}."),
            format!("Inspect logs/{component}.log, correct the failure, then restart the runtime."),
        )),
        Ok(None) => None,
        Err(error) => Some(error),
    }
}

fn reap_finished_child(
    child: &mut Option<ManagedChild>,
    component: &str,
) -> Option<RuntimeErrorInfo> {
    let status = child.as_mut()?.try_wait();
    match status {
        Ok(Some(_)) => {
            *child = None;
            None
        }
        Ok(None) => None,
        Err(error) => Some(error_info(
            component,
            "inspect stopping process",
            error.message,
            error.recovery,
        )),
    }
}

pub(crate) fn run_command_bounded(
    mut command: Command,
    timeout: Duration,
    component: &str,
    operation: &str,
    containment: &ProcessContainment,
) -> Result<ExitStatus, RuntimeErrorInfo> {
    let mut child = command.spawn().map_err(|error| {
        error_info(
            component,
            operation,
            format!("Cannot start the managed command: {error}."),
            "Verify the pinned runtime executable path and component state, then retry.",
        )
    })?;
    if let Err(error) = containment.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    match wait_for_child_exit(&mut child, timeout) {
        Ok(status) => Ok(status),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error_info(
                component,
                operation,
                format!("Managed command exceeded its timeout: {}", error.message),
                "Inspect the runtime bundle and component state, then retry.",
            ))
        }
    }
}

pub(crate) fn wait_for_child_exit(
    child: &mut Child,
    timeout: Duration,
) -> Result<ExitStatus, RuntimeErrorInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                return Err(error_info(
                    "runtime",
                    "wait for process",
                    "Managed process did not exit before the timeout.",
                    "The runtime manager will attempt forced cleanup.",
                ));
            }
            Err(error) => {
                return Err(error_info(
                    "runtime",
                    "wait for process",
                    format!("Cannot wait for managed process: {error}."),
                    "Inspect operating-system process state before retrying.",
                ));
            }
        }
    }
}

fn http_probe(port: u16, probe_name: &str, nonce: &str) -> bool {
    http_probe_with_timeout(port, probe_name, nonce, Duration::from_millis(500))
}

fn request_caddy_stop(port: u16, timeout: Duration) -> Result<(), RuntimeErrorInfo> {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_millis(500)).map_err(|error| {
            error_info(
                "web server",
                "graceful stop",
                format!("Cannot connect to the managed Caddy admin endpoint: {error}."),
                "The runtime manager will fall back to terminating the managed web server process.",
            )
        })?;
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_read_timeout(Some(timeout));
    let request = format!(
        "POST /stop HTTP/1.1\r\nHost: {LOOPBACK}:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).map_err(|error| {
        error_info(
            "web server",
            "graceful stop",
            format!("Cannot send the managed Caddy stop request: {error}."),
            "The runtime manager will fall back to terminating the managed web server process.",
        )
    })?;
    Ok(())
}

fn http_probe_with_timeout(
    port: u16,
    probe_name: &str,
    nonce: &str,
    read_timeout: Duration,
) -> bool {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(300)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let request = format!(
        "GET /{probe_name} HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 2048];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                response.extend_from_slice(&chunk[..count]);
                if response.len() > 64 * 1024 {
                    return false;
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => return false,
        }
    }
    let Ok(text) = std::str::from_utf8(&response) else {
        return false;
    };
    let Some((headers, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let status_ok = headers.starts_with("HTTP/1.0 200 ") || headers.starts_with("HTTP/1.1 200 ");
    status_ok && body.trim() == nonce
}

fn wordpress_http_probe(port: u16) -> bool {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(300)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let request = format!(
        "GET /wp-login.php?doing_wp_cron=coffeepos-health HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::with_capacity(8192);
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                response.extend_from_slice(&chunk[..count]);
                if response.len() > 256 * 1024 {
                    return false;
                }
                if wordpress_probe_response_healthy(&response) {
                    return true;
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => return false,
        }
    }
    wordpress_probe_response_healthy(&response)
}

#[cfg(test)]
fn http_status_probe(port: u16, path: &str, expected_status: u16) -> bool {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(300)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = [0_u8; 1024];
    let Ok(count) = stream.read(&mut response) else {
        return false;
    };
    if count == 0 {
        return false;
    }
    let text = String::from_utf8_lossy(&response[..count]);
    text.starts_with(&format!("HTTP/1.0 {expected_status} "))
        || text.starts_with(&format!("HTTP/1.1 {expected_status} "))
}

fn wordpress_probe_response_healthy(response: &[u8]) -> bool {
    let text = String::from_utf8_lossy(response);
    (text.starts_with("HTTP/1.0 200 ") || text.starts_with("HTTP/1.1 200 "))
        && text.contains("loginform")
}

pub(crate) fn probe_coffeepos_health(data_root: &Path, port: u16) -> CoffeePosHealthInfo {
    let token_path = data_root.join(MACHINE_TOKEN_SECRET);
    let pending_path = data_root.join(MACHINE_TOKEN_PENDING_SECRET);
    let active_token_exists = token_path.is_file();
    let active_health = if active_token_exists {
        match secret::load(&token_path) {
            Ok(token) => probe_coffeepos_health_with_token(port, &token),
            Err(error) => coffeepos_health_failure(
                CoffeePosHealthFailureKind::Authentication,
                "read machine credential",
                error,
                "Retry with the same Windows user profile. Do not reset the machine credential automatically.",
            ),
        }
    } else {
        coffeepos_health_failure(
            CoffeePosHealthFailureKind::Authentication,
            "read machine credential",
            "The protected CoffeePOS machine credential is missing.",
            "Keep the installed store and use the explicit machine-credential repair/rotation flow. Normal runtime start will not invent a replacement credential.",
        )
    };

    if !pending_path.is_file() {
        return active_health;
    }
    if coffeepos_health_accepts_credential(&active_health) {
        if let Err(error) = fs::remove_file(&pending_path) {
            return coffeepos_health_failure(
                CoffeePosHealthFailureKind::Authentication,
                "recover machine credential",
                format!(
                    "The active CoffeePOS machine credential is valid, but the stale pending credential cannot be removed: {error}."
                ),
                "Close processes using protected application data and retry. The active credential remains authoritative.",
            );
        }
        return active_health;
    }

    let pending = match secret::load(&pending_path) {
        Ok(token) => token,
        Err(error) => {
            return coffeepos_health_failure(
                CoffeePosHealthFailureKind::Authentication,
                "recover machine credential",
                format!("Pending CoffeePOS machine credential cannot be read: {error}"),
                "Preserve both protected credential files and use explicit machine-credential repair.",
            );
        }
    };
    let pending_health = probe_coffeepos_health_with_token(port, &pending);
    if !coffeepos_health_accepts_credential(&pending_health) {
        return if !active_token_exists {
            pending_health
        } else {
            active_health
        };
    }

    if let Err(error) = secret::store_machine_token(&token_path, &pending) {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::Authentication,
            "recover machine credential",
            format!(
                "Pending CoffeePOS machine credential is accepted by WordPress but cannot be promoted to active protected storage: {error}"
            ),
            "Preserve the pending credential and retry recovery; do not rotate again.",
        );
    }
    if let Err(error) = fs::remove_file(&pending_path) {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::Authentication,
            "recover machine credential",
            format!(
                "Pending CoffeePOS machine credential was promoted but the pending file cannot be removed: {error}."
            ),
            "Retry recovery. The active protected credential now matches WordPress.",
        );
    }
    pending_health
}

pub(crate) fn probe_coffeepos_health_with_token(port: u16, token: &str) -> CoffeePosHealthInfo {
    probe_coffeepos_health_with_token_timeouts(
        port,
        token,
        Duration::from_millis(500),
        Duration::from_secs(5),
        Duration::from_secs(2),
    )
}

fn probe_coffeepos_health_with_token_timeouts(
    port: u16,
    token: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
) -> CoffeePosHealthInfo {
    if token.len() != 64
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::Authentication,
            "read machine credential",
            "Protected CoffeePOS machine credential has an invalid format.",
            "Preserve the protected credential and use an explicit repair/rotation flow.",
        );
    }

    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let mut stream = match TcpStream::connect_timeout(&address, connect_timeout) {
        Ok(stream) => stream,
        Err(error) => {
            return coffeepos_health_failure(
                CoffeePosHealthFailureKind::TransportBootstrap,
                "health",
                format!("Cannot connect to the managed CoffeePOS endpoint: {error}."),
                "Keep the store installed and restart the local runtime. If WordPress is healthy but this persists, inspect plugin/bootstrap logs.",
            );
        }
    };
    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(write_timeout));
    let request = format!(
        "GET /wp-json/coffeepos/v1/system/status HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\nX-CoffeePOS-Machine-Token: {token}\r\nConnection: close\r\n\r\n"
    );
    if let Err(error) = stream.write_all(request.as_bytes()) {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::TransportBootstrap,
            "health",
            format!("Cannot send the CoffeePOS health request: {error}."),
            "Restart the local runtime and retry.",
        );
    }
    let mut response = Vec::with_capacity(8192);
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                response.extend_from_slice(&chunk[..count]);
                if response.len() > 256 * 1024 {
                    return coffeepos_health_failure(
                        CoffeePosHealthFailureKind::Contract,
                        "health",
                        "CoffeePOS health response exceeded the 256 KiB contract limit.",
                        "Inspect the CoffeePOS plugin response and restore the supported schema.",
                    );
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                return coffeepos_health_failure(
                    CoffeePosHealthFailureKind::TransportBootstrap,
                    "health",
                    "CoffeePOS health endpoint timed out.",
                    "Restart the local runtime and inspect WordPress/PHP/plugin logs if the endpoint remains unavailable.",
                );
            }
            Err(error) => {
                return coffeepos_health_failure(
                    CoffeePosHealthFailureKind::TransportBootstrap,
                    "health",
                    format!("Cannot read the CoffeePOS health response: {error}."),
                    "Restart the local runtime and retry.",
                );
            }
        }
    }
    parse_coffeepos_health_response(&response)
}

fn parse_coffeepos_health_response(response: &[u8]) -> CoffeePosHealthInfo {
    let text = match std::str::from_utf8(response) {
        Ok(text) => text,
        Err(_) => {
            return coffeepos_health_failure(
                CoffeePosHealthFailureKind::Contract,
                "health",
                "CoffeePOS health response is not valid UTF-8.",
                "Restore a compatible CoffeePOS plugin build and retry.",
            );
        }
    };
    let Some((headers, body)) = text.split_once("\r\n\r\n") else {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::TransportBootstrap,
            "health",
            "CoffeePOS health endpoint returned an incomplete HTTP response.",
            "Restart the local runtime and retry.",
        );
    };
    let status_line = headers.lines().next().unwrap_or_default();
    let mut status_parts = status_line.split_whitespace();
    let protocol = status_parts.next().unwrap_or_default();
    let status_code = status_parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|_| matches!(protocol, "HTTP/1.0" | "HTTP/1.1"));
    let Some(status_code) = status_code else {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::TransportBootstrap,
            "health",
            "CoffeePOS health endpoint returned an invalid HTTP status line.",
            "Restart the local runtime and retry.",
        );
    };
    if status_code == 401 {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::Authentication,
            "health",
            "CoffeePOS rejected the protected machine credential.",
            "Do not reinstall the store or rotate automatically. Use the explicit machine-credential repair/rotation flow.",
        );
    }
    if status_code != 200 && status_code != 503 {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::TransportBootstrap,
            "health",
            format!("CoffeePOS machine-health endpoint returned HTTP {status_code}."),
            "Keep the store installed. Verify that CoffeePOS is active and the system/status route is registered, then retry.",
        );
    }
    let payload: CoffeePosHealthPayload = match serde_json::from_str(body.trim()) {
        Ok(payload) => payload,
        Err(error) => {
            return coffeepos_health_failure(
                CoffeePosHealthFailureKind::Contract,
                "health",
                format!(
                    "CoffeePOS health payload does not match the expected JSON schema: {error}."
                ),
                "Use a CoffeePOS plugin build compatible with machine-health schema version 1.",
            );
        }
    };
    if payload.schema_version != COFFEEPOS_HEALTH_SCHEMA_VERSION {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::Contract,
            "health",
            format!(
                "CoffeePOS health schema {} is unsupported; expected {}.",
                payload.schema_version, COFFEEPOS_HEALTH_SCHEMA_VERSION
            ),
            "Use a compatible CoffeePOS Desktop/plugin pair.",
        );
    }
    if (payload.wordpress && payload.versions.wordpress.trim().is_empty())
        || (payload.woocommerce && payload.versions.woocommerce.trim().is_empty())
        || (payload.coffeepos && payload.versions.coffeepos.trim().is_empty())
        || payload.versions.coffeepos_schema.trim().is_empty()
        || payload.store.name.trim().is_empty()
    {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::Contract,
            "health",
            "CoffeePOS health payload is missing required version or store identity fields.",
            "Use a CoffeePOS plugin build compatible with the schema-version-1 health contract.",
        );
    }
    if !safe_pos_path(&payload.pos_path) {
        return coffeepos_health_failure(
            CoffeePosHealthFailureKind::Contract,
            "health",
            "CoffeePOS health payload contains an unsafe POS path.",
            "Restore a compatible CoffeePOS router/settings configuration.",
        );
    }
    let all_ready =
        payload.wordpress && payload.woocommerce && payload.coffeepos && payload.database;
    match (status_code, payload.status.as_str(), all_ready) {
        (200, "healthy", true) => CoffeePosHealthInfo {
            state: CoffeePosHealthState::Healthy,
            failure_kind: None,
            payload: Some(payload),
            error: None,
        },
        (503, "degraded", false) => CoffeePosHealthInfo {
            state: CoffeePosHealthState::Degraded,
            failure_kind: None,
            payload: Some(payload),
            error: None,
        },
        _ => coffeepos_health_failure(
            CoffeePosHealthFailureKind::Contract,
            "health",
            "CoffeePOS HTTP status, health status, and component readiness are inconsistent.",
            "Use a CoffeePOS plugin build compatible with the schema-version-1 health contract.",
        ),
    }
}

fn safe_pos_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains("://")
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
}

fn coffeepos_health_failure(
    kind: CoffeePosHealthFailureKind,
    operation: &str,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> CoffeePosHealthInfo {
    CoffeePosHealthInfo {
        state: CoffeePosHealthState::Failed,
        failure_kind: Some(kind),
        payload: None,
        error: Some(error_info(
            "coffeepos",
            operation,
            message.into(),
            recovery.into(),
        )),
    }
}

fn coffeepos_health_accepts_credential(health: &CoffeePosHealthInfo) -> bool {
    matches!(
        health.state,
        CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
    )
}

fn health_diagnostics_for_inactive_runtime(runtime: &RuntimeInfo) -> HealthDiagnosticsInfo {
    let mut diagnostics = HealthDiagnosticsInfo {
        runtime_state: runtime.state.clone(),
        database: ComponentHealthInfo::unavailable(),
        php: ComponentHealthInfo::unavailable(),
        wordpress: ComponentHealthInfo::unavailable(),
        woocommerce: ComponentHealthInfo::unavailable(),
        coffeepos: ComponentHealthInfo::unavailable(),
    };

    if let Some(error) = runtime.last_error.clone() {
        match error.component.as_str() {
            "database" => diagnostics.database = ComponentHealthInfo::unhealthy(error),
            "php" => diagnostics.php = ComponentHealthInfo::unhealthy(error),
            "wordpress" => diagnostics.wordpress = ComponentHealthInfo::unhealthy(error),
            "coffeepos" => diagnostics.coffeepos = ComponentHealthInfo::unhealthy(error),
            _ => {}
        }
    }
    diagnostics
}

fn machine_component_health(
    ready: bool,
    component: &'static str,
    display_name: &'static str,
) -> ComponentHealthInfo {
    if ready {
        ComponentHealthInfo::healthy()
    } else {
        ComponentHealthInfo::unhealthy(machine_component_error(component, display_name))
    }
}

fn machine_component_error(
    component: &'static str,
    display_name: &'static str,
) -> RuntimeErrorInfo {
    error_info_with_code(
        "runtime_health_error",
        component,
        "application health",
        format!(
            "Authenticated CoffeePOS machine health reports that {display_name} is not ready."
        ),
        "Retry the health check. If the same component remains unhealthy, restart the runtime before using the explicit repair flow.",
    )
}

fn probe_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("coffeepos-{}-{nanos}", std::process::id())
}

fn spawn_log_pump<R>(mut reader: R, path: PathBuf, stream: &'static str, log_lock: Arc<Mutex<()>>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let text = String::from_utf8_lossy(&buffer[..count]);
                    let redacted = redact_log_text(&text);
                    if let Ok(_guard) = log_lock.lock() {
                        let _ = append_bounded_log(&path, stream, &redacted);
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn redact_log_text(text: &str) -> String {
    let mut output = String::new();
    for line in text.lines() {
        let lowered = line.to_ascii_lowercase();
        if [
            "password",
            "passwd",
            "token",
            "secret",
            "authorization",
            "cookie",
        ]
        .iter()
        .any(|needle| lowered.contains(needle))
        {
            output.push_str("[redacted sensitive log line]\n");
        } else {
            let bytes = line.as_bytes();
            let bounded = if bytes.len() > MAX_LOG_CHUNK_BYTES {
                String::from_utf8_lossy(&bytes[..MAX_LOG_CHUNK_BYTES])
            } else {
                String::from_utf8_lossy(bytes)
            };
            output.push_str(&bounded);
            output.push('\n');
        }
    }
    output
}

fn bound_existing_log(path: &Path) -> Result<(), RuntimeErrorInfo> {
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.len() > MAX_LOG_BYTES {
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)
                .map_err(log_error)?;
        }
    }
    Ok(())
}

fn append_bounded_log(path: &Path, stream: &str, text: &str) -> Result<(), RuntimeErrorInfo> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)
        .map_err(log_error)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let mut bytes = text.as_bytes();
    if bytes.len() > MAX_LOG_CHUNK_BYTES {
        bytes = &bytes[..MAX_LOG_CHUNK_BYTES];
    }
    let prefix = format!("{timestamp} [{stream}] ");
    let entry_len = prefix.len() as u64 + bytes.len() as u64 + 1;
    if file
        .metadata()
        .map_err(log_error)?
        .len()
        .saturating_add(entry_len)
        > MAX_LOG_BYTES
    {
        file.set_len(0).map_err(log_error)?;
        file.write_all(b"[log truncated at size limit]\n")
            .map_err(log_error)?;
    }
    file.write_all(prefix.as_bytes()).map_err(log_error)?;
    file.write_all(bytes).map_err(log_error)?;
    if !bytes.ends_with(b"\n") {
        file.write_all(b"\n").map_err(log_error)?;
    }
    Ok(())
}

fn log_error(error: io::Error) -> RuntimeErrorInfo {
    error_info_with_code(
        "runtime_log_error",
        "runtime",
        "write log",
        format!("Cannot write runtime diagnostics: {error}."),
        "Check application-data permissions and free disk space, then retry.",
    )
}

fn manifest_error(message: impl Into<String>) -> RuntimeErrorInfo {
    error_info_with_code(
        "runtime_manifest_error",
        "runtime",
        "resolve manifest",
        message,
        "Use a target-specific manifest whose artifact paths resolve canonically inside project runtime/development, then retry.",
    )
}

fn not_installed_error(message: impl Into<String>) -> RuntimeErrorInfo {
    error_info_with_code(
        "runtime_not_installed",
        "runtime",
        "preflight",
        message,
        "Run the provisioning/repair phase to create a valid existing store. Phase 2 runtime start does not initialize or overwrite store data.",
    )
}

fn error_info(
    component: impl Into<String>,
    operation: impl Into<String>,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> RuntimeErrorInfo {
    error_info_with_code("runtime_error", component, operation, message, recovery)
}

fn network_error(
    code: impl Into<String>,
    operation: impl Into<String>,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> RuntimeErrorInfo {
    error_info_with_code(code, "network", operation, message, recovery)
}

fn error_info_with_code(
    code: impl Into<String>,
    component: impl Into<String>,
    operation: impl Into<String>,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> RuntimeErrorInfo {
    RuntimeErrorInfo {
        component: component.into(),
        operation: operation.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

#[cfg(windows)]
pub(crate) struct ProcessContainment {
    job: usize,
}

#[cfg(windows)]
impl ProcessContainment {
    pub(crate) fn new() -> Result<Self, RuntimeErrorInfo> {
        let job = unsafe { windows_job::process_lifetime_job() }.map_err(|error| {
            error_info(
                "runtime",
                "create process job",
                format!("Cannot create Windows process containment: {error}."),
                "Check Windows process policy and retry. Runtime services are not started without crash containment.",
            )
        })?;
        Ok(Self { job })
    }

    pub(crate) fn assign(&self, child: &Child) -> Result<(), RuntimeErrorInfo> {
        unsafe { windows_job::verify_process_in_job(self.job, child) }.map_err(|error| {
            error_info(
                "runtime",
                "verify process job",
                format!("Cannot verify child process inheritance in the Windows runtime job: {error}."),
                "Stop any stale runtime process and retry. CoffeePOS only starts managed children that inherit process-lifetime crash containment.",
            )
        })
    }
}

#[cfg(not(windows))]
pub(crate) struct ProcessContainment;

#[cfg(not(windows))]
impl ProcessContainment {
    pub(crate) fn new() -> Result<Self, RuntimeErrorInfo> {
        Ok(Self)
    }

    pub(crate) fn assign(&self, _child: &Child) -> Result<(), RuntimeErrorInfo> {
        Ok(())
    }
}

#[cfg(windows)]
mod windows_job {
    use super::*;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::ptr;
    use std::sync::OnceLock;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    static PROCESS_LIFETIME_JOB: OnceLock<Result<usize, String>> = OnceLock::new();

    #[repr(C)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            info_class: i32,
            info: *const c_void,
            info_length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
        fn IsProcessInJob(process: *mut c_void, job: *mut c_void, result: *mut i32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub unsafe fn process_lifetime_job() -> io::Result<usize> {
        match PROCESS_LIFETIME_JOB
            .get_or_init(|| create_process_lifetime_job().map_err(|error| error.to_string()))
        {
            Ok(job) => Ok(*job),
            Err(message) => Err(io::Error::other(message.clone())),
        }
    }

    unsafe fn create_process_lifetime_job() -> io::Result<usize> {
        let job = CreateJobObjectW(ptr::null(), ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info: JobObjectExtendedLimitInformation = zeroed();
        info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            &info as *const JobObjectExtendedLimitInformation as *const c_void,
            size_of::<JobObjectExtendedLimitInformation>() as u32,
        );
        if configured == 0 {
            let error = io::Error::last_os_error();
            let _ = CloseHandle(job);
            return Err(error);
        }
        // Associate the CoffeePOS process itself with this process-lifetime job. Windows then
        // places every child in the same job atomically at CreateProcess time unless the child is
        // explicitly created with CREATE_BREAKAWAY_FROM_JOB (CoffeePOS never sets that flag).
        // This closes the spawn -> AssignProcessToJobObject crash window for backup helpers.
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            let error = io::Error::last_os_error();
            let _ = CloseHandle(job);
            return Err(error);
        }
        Ok(job as usize)
    }

    pub unsafe fn verify_process_in_job(job: usize, child: &Child) -> io::Result<()> {
        let process = child.as_raw_handle();
        let mut in_job = 0_i32;
        if IsProcessInJob(process, job as *mut c_void, &mut in_job) == 0 {
            return Err(io::Error::last_os_error());
        }
        if in_job == 0 {
            return Err(io::Error::other(
                "child did not inherit the CoffeePOS process-lifetime job",
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn configure_child_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn configure_child_command(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"test").unwrap();
    }

    fn manifest_fixture(root: &Path) -> (PathBuf, PathBuf) {
        let development = root.join("runtime/development");
        let php = development.join("php/php-test");
        let php_cgi = development.join("php/php-cgi-test");
        let php_ini = development.join("php/php.ini");
        let web_server = development.join("caddy/caddy-test");
        let mariadb = development.join("mariadb/bin/mariadbd-test");
        let client = development.join("mariadb/bin/mariadb-test");
        let dump = development.join("mariadb/bin/mariadb-dump-test");
        let install_db = development.join("mariadb/bin/mariadb-install-db-test");
        for path in [
            &php,
            &php_cgi,
            &php_ini,
            &web_server,
            &mariadb,
            &client,
            &dump,
            &install_db,
        ] {
            touch(path);
        }
        (development, php)
    }

    fn write_manifest(path: &Path, php: &Path) {
        let root = path.parent().unwrap();
        let php_cgi = root.join("php/php-cgi-test");
        let php_ini = root.join("php/php.ini");
        let web_server = root.join("caddy/caddy-test");
        let server = root.join("mariadb/bin/mariadbd-test");
        let client = root.join("mariadb/bin/mariadb-test");
        let dump = root.join("mariadb/bin/mariadb-dump-test");
        let install_db = root.join("mariadb/bin/mariadb-install-db-test");
        touch(&root.join("php/license.txt"));
        touch(&root.join("caddy/LICENSE"));
        touch(&root.join("mariadb/COPYING"));
        touch(&root.join("fixture/router.php"));
        fs::create_dir_all(root.join("fixture/site")).unwrap();
        let manifest = json!({
            "schema_version": 3,
            "target": current_target_triple().unwrap(),
            "runtime_version": "test-runtime",
            "php": {
                "version": "8.4-test",
                "archive": "php-test.zip",
                "executable": php,
                "cgi": &php_cgi,
                "ini": &php_ini,
                "source": "test fixture",
                "checksum_source": "test fixture checksum",
                "archive_sha256": "a".repeat(64),
                "license": "PHP-3.01",
                "license_file": "php/license.txt"
            },
            "web_server": {
                "version": "2.11-test",
                "archive": "caddy-test.zip",
                "executable": &web_server,
                "source": "test fixture",
                "checksum_source": "test fixture checksum",
                "archive_sha256": "c".repeat(64),
                "license": "Apache-2.0",
                "license_file": "caddy/LICENSE"
            },
            "mariadb": {
                "version": "11.4-test",
                "archive": "mariadb-test.zip",
                "server": &server,
                "client": &client,
                "dump": &dump,
                "import": &client,
                "install_db": &install_db,
                "source": "test fixture",
                "checksum_source": "test fixture checksum",
                "archive_sha256": "b".repeat(64),
                "license": "GPL-2.0-only",
                "license_file": "mariadb/COPYING"
            },
            "http_fixture": {
                "router": "fixture/router.php",
                "document_root": "fixture/site",
                "health_path": "/__coffeepos_runtime_health"
            }
        });
        fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    }

    #[test]
    fn development_manifest_resolves_absolute_paths_inside_development_root() {
        if current_target_triple().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        let (development, php) = manifest_fixture(&project);
        let manifest = development.join("manifest.json");
        write_manifest(&manifest, &php);

        let resolved = resolve_development_manifest(&project, &manifest).unwrap();
        assert_eq!(resolved.runtime_version, "test-runtime");
        assert_eq!(resolved.php_version, "8.4-test");
        assert_eq!(resolved.mariadb_version, "11.4-test");
        assert!(resolved.mariadb_dump_executable.is_absolute());
        assert!(resolved.mariadb_import_executable.is_absolute());
        assert!(resolved
            .mariadb_dump_executable
            .starts_with(command_compatible_path(development.clone())));
        assert!(resolved
            .mariadb_import_executable
            .starts_with(command_compatible_path(development)));
    }

    #[test]
    fn development_manifest_rejects_missing_database_dump_tool() {
        if current_target_triple().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        let (development, php) = manifest_fixture(&project);
        let manifest = development.join("manifest.json");
        write_manifest(&manifest, &php);
        fs::remove_file(development.join("mariadb/bin/mariadb-dump-test")).unwrap();

        let error = resolve_development_manifest(&project, &manifest).unwrap_err();
        assert!(error.message.contains("MariaDB dump executable"));
    }

    #[test]
    fn development_manifest_rejects_database_dump_path_escape() {
        if current_target_triple().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        let (development, php) = manifest_fixture(&project);
        let escaped_dump = project.join("outside-mariadb-dump");
        touch(&escaped_dump);
        let manifest = development.join("manifest.json");
        write_manifest(&manifest, &php);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["mariadb"]["dump"] = serde_json::json!(escaped_dump);
        fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let error = resolve_development_manifest(&project, &manifest).unwrap_err();
        assert!(error.message.contains("outside runtime/development"));
    }

    #[test]
    fn development_manifest_rejects_path_escape() {
        if current_target_triple().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        let (development, _php) = manifest_fixture(&project);
        let escaped_php = project.join("outside-php");
        touch(&escaped_php);
        let manifest = development.join("manifest.json");
        write_manifest(&manifest, &escaped_php);

        let error = resolve_development_manifest(&project, &manifest).unwrap_err();
        assert!(error.message.contains("outside runtime/development"));
    }

    #[test]
    fn development_manifest_resolves_relative_artifact_path_inside_manifest_root() {
        if current_target_triple().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        let (development, _php) = manifest_fixture(&project);
        let manifest = development.join("manifest.json");
        write_manifest(&manifest, Path::new("php/php-test"));

        let resolved = resolve_development_manifest(&project, &manifest).unwrap();
        assert!(resolved.php_executable.is_absolute());
        assert!(resolved
            .php_executable
            .starts_with(command_compatible_path(development)));
    }

    #[test]
    fn loopback_port_selection_returns_distinct_bindable_ports() {
        let first = choose_loopback_port(&[]).unwrap();
        let second = choose_loopback_port(&[first]).unwrap();
        assert_ne!(first, second);
        let _first_listener = TcpListener::bind((LOOPBACK, first)).unwrap();
        let _second_listener = TcpListener::bind((LOOPBACK, second)).unwrap();
    }

    #[test]
    fn occupied_preferred_port_falls_back_to_another_loopback_port() {
        let occupied = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let preferred = occupied.local_addr().unwrap().port();
        let selected = choose_runtime_port(Some(preferred), &[]).unwrap();
        assert_ne!(selected, preferred);
        let _selected_listener = TcpListener::bind((LOOPBACK, selected)).unwrap();
    }

    #[test]
    fn lan_caddy_config_keeps_internal_services_loopback_and_blocks_native_routes() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        fs::create_dir_all(data.join("site")).unwrap();
        fs::create_dir_all(data.join("uploads")).unwrap();
        let mut manager = RuntimeManager::new(fake_runtime(), data).unwrap();
        manager.configured_network_mode = NetworkMode::Lan;
        manager.lan_candidate = Some(LanCandidate {
            adapter_id: "00112233445566778899aabbccddeeff".into(),
            adapter_name: "Private Ethernet".into(),
            address: Ipv4Addr::new(192, 168, 50, 25),
            network_profile: NetworkProfile::Private,
            recommended: true,
        });

        let path = manager
            .prepare_web_server_config(
                48100,
                48101,
                48102,
                Some(48103),
                "https://192.168.50.25:48103",
            )
            .unwrap();
        let caddyfile = fs::read_to_string(path).unwrap();

        assert!(caddyfile.contains("admin 127.0.0.1:48101"));
        assert!(caddyfile.contains("http://127.0.0.1:48100"));
        assert!(caddyfile.contains("php_fastcgi 127.0.0.1:48102"));
        assert!(caddyfile.contains("https://192.168.50.25:48103"));
        assert!(caddyfile.contains("tls internal"));
        assert!(caddyfile.contains("/wp-json/coffeepos/v1/system/status"));
        assert!(caddyfile.contains("/.coffeepos-runtime-health-*"));
        assert!(caddyfile.contains("respond @internal 404"));
        assert!(!caddyfile.contains("0.0.0.0"));
    }

    fn fake_runtime() -> ResolvedRuntime {
        let executable = std::env::current_exe().unwrap();
        let base = executable.parent().unwrap().to_path_buf();
        ResolvedRuntime {
            runtime_version: "test-runtime".into(),
            php_version: "test-php".into(),
            php_executable: executable.clone(),
            php_cgi_executable: executable.clone(),
            php_ini: executable.clone(),
            web_server_version: "test-web-server".into(),
            web_server_executable: executable.clone(),
            mariadb_version: "test-db".into(),
            mariadb_executable: executable.clone(),
            mariadb_client_executable: executable.clone(),
            mariadb_dump_executable: executable.clone(),
            mariadb_import_executable: executable.clone(),
            mariadb_install_db_executable: executable.clone(),
            mariadb_base_dir: base,
        }
    }

    #[test]
    fn uninitialized_database_is_not_installed_and_is_never_initialized() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        fs::create_dir_all(data.join("database")).unwrap();
        fs::create_dir_all(data.join("site")).unwrap();
        let mut manager = RuntimeManager::new(fake_runtime(), data.clone()).unwrap();
        assert_eq!(manager.info().state, RuntimeState::NotInstalled);

        let error = manager.start().unwrap_err();
        assert_eq!(error.operation, "preflight");
        assert!(!data.join("database/mysql").exists());
        assert!(manager.database.is_none());
        assert!(manager.php.is_none());
    }

    #[test]
    fn restart_does_not_turn_not_installed_into_provisioning() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        fs::create_dir_all(data.join("database")).unwrap();
        fs::create_dir_all(data.join("site")).unwrap();
        let mut manager = RuntimeManager::new(fake_runtime(), data.clone()).unwrap();

        let error = manager.restart().unwrap_err();
        assert_eq!(error.operation, "preflight");
        assert_eq!(manager.info().state, RuntimeState::NotInstalled);
        assert!(!data.join("database/mysql").exists());
    }

    #[test]
    fn failed_child_start_is_cleaned_and_returns_to_stopped() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        fs::create_dir_all(data.join("database/mysql")).unwrap();
        fs::create_dir_all(data.join("site")).unwrap();
        crate::secret::create(&data.join(DATABASE_RUNTIME_SECRET)).unwrap();
        crate::secret::create(&data.join(DATABASE_WORDPRESS_SECRET)).unwrap();
        let mut manager = RuntimeManager::new(fake_runtime(), data).unwrap();
        manager.set_timeouts(RuntimeTimeouts {
            database_readiness: Duration::from_millis(500),
            http_readiness: Duration::from_millis(500),
            wordpress_readiness: Duration::from_millis(500),
            probe_command: Duration::from_millis(250),
            request_drain: Duration::from_millis(250),
            stop: Duration::from_millis(500),
        });
        assert_eq!(manager.info().state, RuntimeState::Stopped);

        assert!(manager.start().is_err());
        let info = manager.info();
        assert_eq!(info.state, RuntimeState::Stopped);
        assert!(manager.database.is_none());
        assert!(manager.php.is_none());
        assert!(info.database_pid.is_none());
        assert!(info.php_pid.is_none());
    }

    #[test]
    fn log_redaction_removes_sensitive_lines() {
        let redacted = redact_log_text("ready\npassword=hunter2\nTOKEN abc\nnext");
        assert!(redacted.contains("ready"));
        assert!(redacted.contains("next"));
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("TOKEN abc"));
    }

    #[test]
    fn runtime_info_serializes_stable_state_names() {
        let info = RuntimeInfo {
            state: RuntimeState::Starting,
            runtime_version: Some("test-runtime".into()),
            php_version: Some("test-php".into()),
            web_server_version: Some("test-web-server".into()),
            mariadb_version: Some("test-db".into()),
            database_port: Some(3307),
            http_port: Some(8081),
            database_pid: None,
            php_pid: None,
            web_server_pid: None,
            wordpress_health: WordPressHealthState::Checking,
            wordpress_error: None,
            coffeepos_health: CoffeePosHealthInfo::unavailable(),
            network: NetworkInfo {
                configured_mode: NetworkMode::LocalOnly,
                effective_mode: NetworkMode::LocalOnly,
                adapter_id: None,
                adapter_name: None,
                lan_address: None,
                internal_origin: None,
                canonical_origin: None,
                lan_listener_state: LanListenerState::Disabled,
                tls_state: TlsState::Disabled,
                network_profile: None,
                last_error: None,
            },
            last_error: None,
        };
        let value = serde_json::to_value(info).unwrap();
        assert_eq!(value["state"], "starting");
        assert_eq!(value["database_port"], 3307);
        assert_eq!(value["http_port"], 8081);
        assert_eq!(value["wordpress_health"], "checking");
        assert_eq!(value["coffeepos_health"]["state"], "unavailable");
        assert_eq!(
            serde_json::to_value(RuntimeStartupStage::WebServerStarting).unwrap(),
            "web_server_starting"
        );
    }

    #[test]
    fn inactive_health_diagnostics_preserves_failed_component_without_blame_spread() {
        let database_error = error_info(
            "database",
            "health",
            "Database probe failed.",
            "Retry the database probe.",
        );
        let info = RuntimeInfo {
            state: RuntimeState::Stopped,
            runtime_version: Some("test-runtime".into()),
            php_version: Some("test-php".into()),
            web_server_version: Some("test-web-server".into()),
            mariadb_version: Some("test-db".into()),
            database_port: None,
            http_port: None,
            database_pid: None,
            php_pid: None,
            web_server_pid: None,
            wordpress_health: WordPressHealthState::Unavailable,
            wordpress_error: None,
            coffeepos_health: CoffeePosHealthInfo::unavailable(),
            network: NetworkInfo {
                configured_mode: NetworkMode::LocalOnly,
                effective_mode: NetworkMode::LocalOnly,
                adapter_id: None,
                adapter_name: None,
                lan_address: None,
                internal_origin: None,
                canonical_origin: None,
                lan_listener_state: LanListenerState::Disabled,
                tls_state: TlsState::Disabled,
                network_profile: None,
                last_error: None,
            },
            last_error: Some(database_error.clone()),
        };

        let diagnostics = health_diagnostics_for_inactive_runtime(&info);
        assert_eq!(diagnostics.database.state, ComponentHealthState::Unhealthy);
        assert_eq!(diagnostics.database.error, Some(database_error));
        assert_eq!(diagnostics.php.state, ComponentHealthState::Unavailable);
        assert_eq!(
            diagnostics.wordpress.state,
            ComponentHealthState::Unavailable
        );
        assert_eq!(
            diagnostics.woocommerce.state,
            ComponentHealthState::Unavailable
        );
        assert_eq!(
            diagnostics.coffeepos.state,
            ComponentHealthState::Unavailable
        );
    }

    #[test]
    fn machine_component_health_only_marks_the_reported_component_unhealthy() {
        let woocommerce = machine_component_health(false, "woocommerce", "WooCommerce");
        let coffeepos = machine_component_health(true, "coffeepos", "CoffeePOS");

        assert_eq!(woocommerce.state, ComponentHealthState::Unhealthy);
        assert_eq!(
            woocommerce
                .error
                .as_ref()
                .map(|error| error.component.as_str()),
            Some("woocommerce")
        );
        assert_eq!(coffeepos.state, ComponentHealthState::Healthy);
        assert!(coffeepos.error.is_none());
    }

    fn machine_health_response(status: u16, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}"
        )
        .into_bytes()
    }

    const HEALTHY_MACHINE_BODY: &str = r#"{"schema_version":1,"status":"healthy","wordpress":true,"woocommerce":true,"coffeepos":true,"database":true,"versions":{"wordpress":"7.1","woocommerce":"11.1.0","coffeepos":"1.0.1","coffeepos_schema":"0.0.1"},"store":{"name":"CoffeePOS"},"pos_path":"/pos/"}"#;

    #[test]
    fn coffeepos_health_parser_accepts_healthy_and_degraded_contracts() {
        let healthy =
            parse_coffeepos_health_response(&machine_health_response(200, HEALTHY_MACHINE_BODY));
        assert_eq!(healthy.state, CoffeePosHealthState::Healthy);
        assert_eq!(
            healthy.payload.as_ref().unwrap().versions.coffeepos,
            "1.0.1"
        );

        let degraded_body = HEALTHY_MACHINE_BODY
            .replace(r#""status":"healthy""#, r#""status":"degraded""#)
            .replace(r#""woocommerce":true"#, r#""woocommerce":false"#);
        let degraded =
            parse_coffeepos_health_response(&machine_health_response(503, &degraded_body));
        assert_eq!(degraded.state, CoffeePosHealthState::Degraded);
        assert!(!degraded.payload.as_ref().unwrap().woocommerce);
    }

    #[test]
    fn coffeepos_health_parser_classifies_auth_transport_and_contract_failures() {
        let auth = parse_coffeepos_health_response(&machine_health_response(
            401,
            r#"{"code":"coffeepos_machine_auth_required"}"#,
        ));
        assert_eq!(auth.state, CoffeePosHealthState::Failed);
        assert_eq!(
            auth.failure_kind,
            Some(CoffeePosHealthFailureKind::Authentication)
        );

        let missing = parse_coffeepos_health_response(&machine_health_response(
            404,
            r#"{"code":"rest_no_route"}"#,
        ));
        assert_eq!(
            missing.failure_kind,
            Some(CoffeePosHealthFailureKind::TransportBootstrap)
        );

        let malformed = parse_coffeepos_health_response(&machine_health_response(200, "{"));
        assert_eq!(
            malformed.failure_kind,
            Some(CoffeePosHealthFailureKind::Contract)
        );

        let schema = parse_coffeepos_health_response(&machine_health_response(
            200,
            &HEALTHY_MACHINE_BODY.replace(r#""schema_version":1"#, r#""schema_version":2"#),
        ));
        assert_eq!(
            schema.failure_kind,
            Some(CoffeePosHealthFailureKind::Contract)
        );

        let inconsistent = parse_coffeepos_health_response(&machine_health_response(
            200,
            &HEALTHY_MACHINE_BODY.replace(r#""database":true"#, r#""database":false"#),
        ));
        assert_eq!(
            inconsistent.failure_kind,
            Some(CoffeePosHealthFailureKind::Contract)
        );

        let unsafe_path = parse_coffeepos_health_response(&machine_health_response(
            200,
            &HEALTHY_MACHINE_BODY.replace(r#""/pos/""#, r#""//evil/""#),
        ));
        assert_eq!(
            unsafe_path.failure_kind,
            Some(CoffeePosHealthFailureKind::Contract)
        );

        let nested_unknown = parse_coffeepos_health_response(&machine_health_response(
            200,
            &HEALTHY_MACHINE_BODY.replace(
                r#""wordpress":"7.1""#,
                r#""wordpress":"7.1","unexpected":"value""#,
            ),
        ));
        assert_eq!(
            nested_unknown.failure_kind,
            Some(CoffeePosHealthFailureKind::Contract)
        );

        let invalid_protocol_response = format!(
            "NOTHTTP 200 OK\r\nContent-Type: application/json\r\n\r\n{HEALTHY_MACHINE_BODY}"
        );
        let invalid_protocol =
            parse_coffeepos_health_response(invalid_protocol_response.as_bytes());
        assert_eq!(
            invalid_protocol.failure_kind,
            Some(CoffeePosHealthFailureKind::TransportBootstrap)
        );
    }

    #[test]
    fn coffeepos_health_probe_classifies_missing_protected_token_as_authentication_failure() {
        let temp = tempfile::tempdir().unwrap();
        let health = probe_coffeepos_health(temp.path(), 43129);
        assert_eq!(health.state, CoffeePosHealthState::Failed);
        assert_eq!(
            health.failure_kind,
            Some(CoffeePosHealthFailureKind::Authentication)
        );
        assert!(health
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("credential is missing"));
    }

    #[test]
    fn coffeepos_health_probe_classifies_read_timeout_as_transport_failure() {
        let listener = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            thread::sleep(Duration::from_millis(100));
        });

        let token = "a".repeat(64);
        let health = probe_coffeepos_health_with_token_timeouts(
            port,
            &token,
            Duration::from_millis(200),
            Duration::from_millis(20),
            Duration::from_millis(200),
        );
        assert_eq!(health.state, CoffeePosHealthState::Failed);
        assert_eq!(
            health.failure_kind,
            Some(CoffeePosHealthFailureKind::TransportBootstrap)
        );
        assert!(health.error.as_ref().unwrap().message.contains("timed out"));
        server.join().unwrap();
    }

    #[test]
    fn wordpress_probe_response_accepts_valid_partial_login_page() {
        assert!(wordpress_probe_response_healthy(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<form id=\"loginform\""
        ));
        assert!(!wordpress_probe_response_healthy(
            b"HTTP/1.1 302 Found\r\nLocation: /wp-login.php\r\n\r\n"
        ));
        assert!(!wordpress_probe_response_healthy(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\nloading"
        ));
    }

    #[test]
    fn wordpress_url_requires_current_healthy_running_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let mut manager = RuntimeManager::new(fake_runtime(), data).unwrap();

        assert_eq!(manager.wordpress_url().unwrap_err().operation, "open");

        manager.state = RuntimeState::Running;
        manager.http_port = Some(43127);
        manager.wordpress_health = WordPressHealthState::Unhealthy;
        assert_eq!(manager.wordpress_url().unwrap_err().operation, "open");

        manager.wordpress_health = WordPressHealthState::Healthy;
        assert_eq!(manager.wordpress_url().unwrap(), "http://127.0.0.1:43127/");

        manager.state = RuntimeState::Stopped;
        assert_eq!(manager.wordpress_url().unwrap_err().operation, "open");
    }

    #[test]
    fn pos_url_requires_current_healthy_machine_route_and_port() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let mut manager = RuntimeManager::new(fake_runtime(), data).unwrap();

        assert_eq!(manager.pos_url().unwrap_err().operation, "open POS");

        manager.state = RuntimeState::Running;
        manager.http_port = Some(43127);
        manager.wordpress_health = WordPressHealthState::Healthy;
        assert_eq!(manager.pos_url().unwrap_err().operation, "open POS");

        manager.coffeepos_health = parse_coffeepos_health_response(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{HEALTHY_MACHINE_BODY}"
            )
            .as_bytes(),
        );
        assert_eq!(manager.pos_url().unwrap(), "http://127.0.0.1:43127/pos/");

        manager.http_port = Some(43999);
        assert_eq!(manager.pos_url().unwrap(), "http://127.0.0.1:43999/pos/");

        manager.state = RuntimeState::Stopped;
        assert_eq!(manager.pos_url().unwrap_err().operation, "open POS");
    }

    #[test]
    fn exit_confirmation_tracks_active_runtime_states() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let mut manager = RuntimeManager::new(fake_runtime(), data).unwrap();

        assert!(!manager.requires_exit_confirmation());
        manager.state = RuntimeState::Running;
        assert!(manager.requires_exit_confirmation());
        manager.state = RuntimeState::Stopping;
        assert!(manager.requires_exit_confirmation());
        manager.state = RuntimeState::Stopped;
        assert!(!manager.requires_exit_confirmation());
    }

    #[test]
    fn request_drain_gate_marker_is_recoverable_across_startup() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let manager = RuntimeManager::new(fake_runtime(), data.clone()).unwrap();

        let gate = manager.prepare_runtime_prepend_gate().unwrap();
        let gate_body = fs::read_to_string(gate).unwrap();
        assert!(gate_body.contains("COFFEEPOS_DRAIN_MARKER"));
        assert!(gate_body.contains("503"));
        assert!(gate_body.contains("exit"));

        manager.begin_request_drain().unwrap();
        assert!(data.join(REQUEST_DRAIN_MARKER).is_file());
        manager.clear_request_drain_marker().unwrap();
        assert!(!data.join(REQUEST_DRAIN_MARKER).exists());

        fs::write(data.join(REQUEST_DRAIN_MARKER), b"stale\n").unwrap();
        manager.prepare_runtime_prepend_gate().unwrap();
        assert!(!data.join(REQUEST_DRAIN_MARKER).exists());
    }

    #[test]
    #[ignore = "requires staged Windows runtime and a disposable pre-provisioned datadir"]
    fn staged_runtime_start_stop_restart_smoke() {
        let data = PathBuf::from(
            std::env::var_os("COFFEEPOS_PHASE2_SMOKE_DATA")
                .expect("COFFEEPOS_PHASE2_SMOKE_DATA must point to disposable test data"),
        );
        assert!(data.is_absolute());
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project = manifest_dir.parent().unwrap().canonicalize().unwrap();
        let manifest = project
            .join("runtime/development")
            .join(current_target_triple().unwrap())
            .join("manifest.json");
        let mut manager =
            RuntimeManager::from_development(&project, &manifest, data.clone()).unwrap();

        let started = manager.start().unwrap();
        assert_eq!(started.state, RuntimeState::Running);
        assert!(started.database_pid.is_some());
        assert!(started.php_pid.is_some());
        assert!(started.database_port.is_some());
        assert!(started.http_port.is_some());

        let stopped = manager.stop().unwrap();
        assert_eq!(stopped.state, RuntimeState::Stopped);
        assert!(stopped.database_pid.is_none());
        assert!(stopped.php_pid.is_none());

        let restarted_from_stopped = manager.restart().unwrap();
        assert_eq!(restarted_from_stopped.state, RuntimeState::Running);
        let restarted = manager.restart().unwrap();
        assert_eq!(restarted.state, RuntimeState::Running);
        assert!(restarted.database_pid.is_some());
        assert!(restarted.php_pid.is_some());

        let final_state = manager.stop().unwrap();
        assert_eq!(final_state.state, RuntimeState::Stopped);
    }

    #[test]
    #[ignore = "requires staged Windows runtime and a disposable pre-provisioned datadir"]
    fn staged_runtime_concurrent_http_smoke() {
        let data = PathBuf::from(
            std::env::var_os("COFFEEPOS_PHASE62_SMOKE_DATA")
                .expect("COFFEEPOS_PHASE62_SMOKE_DATA must point to disposable test data"),
        );
        assert!(data.is_absolute());
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project = manifest_dir.parent().unwrap().canonicalize().unwrap();
        let manifest = project
            .join("runtime/development")
            .join(current_target_triple().unwrap())
            .join("manifest.json");
        let mut manager =
            RuntimeManager::from_development(&project, &manifest, data.clone()).unwrap();
        let info = manager.start().unwrap();
        assert_eq!(info.state, RuntimeState::Running);
        let port = info.http_port.unwrap();

        let concurrency_probe = data.join("site/.coffeepos-concurrency-probe.php");
        fs::write(
            &concurrency_probe,
            b"<?php usleep(250000); header('Content-Type: text/plain'); echo 'ok';\n",
        )
        .unwrap();
        assert!(http_status_probe(
            port,
            "/.coffeepos-concurrency-probe.php",
            200
        ));

        let synthetic_sequential_started = Instant::now();
        for _ in 0..4 {
            assert!(http_status_probe(
                port,
                "/.coffeepos-concurrency-probe.php",
                200
            ));
        }
        let synthetic_sequential = synthetic_sequential_started.elapsed();
        let synthetic_parallel_started = Instant::now();
        let synthetic_parallel_ok = thread::scope(|scope| {
            let handles = (0..4)
                .map(|_| {
                    scope
                        .spawn(|| http_status_probe(port, "/.coffeepos-concurrency-probe.php", 200))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .all(|handle| handle.join().unwrap_or(false))
        });
        let synthetic_parallel = synthetic_parallel_started.elapsed();

        for _ in 0..2 {
            assert!(wordpress_http_probe(port));
        }

        let mut sequential = Vec::new();
        for _ in 0..8 {
            let started = Instant::now();
            assert!(wordpress_http_probe(port));
            sequential.push(started.elapsed());
        }
        let four_sequential: Duration = sequential.iter().take(4).copied().sum();

        let parallel_started = Instant::now();
        let parallel_ok = thread::scope(|scope| {
            let handles = (0..4)
                .map(|_| scope.spawn(|| wordpress_http_probe(port)))
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .all(|handle| handle.join().unwrap_or(false))
        });
        let parallel_wall = parallel_started.elapsed();

        let static_path = "/wp-content/plugins/coffeepos/assets/js/app.js";
        for _ in 0..2 {
            assert!(http_status_probe(port, static_path, 200));
        }
        let mut static_samples = Vec::new();
        for _ in 0..8 {
            let started = Instant::now();
            assert!(http_status_probe(port, static_path, 200));
            static_samples.push(started.elapsed());
        }
        let _ = fs::remove_file(&concurrency_probe);
        manager.stop().unwrap();

        eprintln!(
            "phase6.2 raw concurrency php_probe_sequential_ms={} php_probe_parallel_ms={} wordpress_sequential_ms={} wordpress_parallel_ms={}",
            synthetic_sequential.as_millis(),
            synthetic_parallel.as_millis(),
            four_sequential.as_millis(),
            parallel_wall.as_millis()
        );
        assert!(synthetic_parallel_ok);
        assert!(synthetic_parallel * 2 < synthetic_sequential);
        assert!(parallel_ok);
        assert!(parallel_wall < four_sequential);

        sequential.sort();
        static_samples.sort();
        let p50 = sequential[sequential.len() / 2];
        let p95 = sequential[sequential.len() - 1];
        let static_p50 = static_samples[static_samples.len() / 2];
        let static_p95 = static_samples[static_samples.len() - 1];
        eprintln!(
            "phase6.2 benchmark dynamic_p50_ms={} dynamic_p95_ms={} four_sequential_ms={} four_parallel_ms={} static_p50_ms={} static_p95_ms={}",
            p50.as_millis(),
            p95.as_millis(),
            four_sequential.as_millis(),
            parallel_wall.as_millis(),
            static_p50.as_millis(),
            static_p95.as_millis()
        );
    }
}
