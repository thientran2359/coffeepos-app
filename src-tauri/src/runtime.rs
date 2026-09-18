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
const MANIFEST_SCHEMA_VERSION: u32 = 1;
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_LOG_CHUNK_BYTES: usize = 8192;
const PORT_ATTEMPTS: usize = 3;
pub(crate) const DATABASE_RUNTIME_USER: &str = "coffeepos_runtime";
pub(crate) const DATABASE_WORDPRESS_USER: &str = "coffeepos_wp";
pub(crate) const DATABASE_NAME: &str = "coffeepos";
pub(crate) const DATABASE_RUNTIME_SECRET: &str = "config/database-runtime.secret";
pub(crate) const DATABASE_WORDPRESS_SECRET: &str = "config/database-wordpress.secret";

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

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WordPressHealthState {
    Unavailable,
    Checking,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RuntimeErrorInfo {
    pub component: String,
    pub operation: String,
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
    pub mariadb_version: Option<String>,
    pub database_port: Option<u16>,
    pub http_port: Option<u16>,
    pub database_pid: Option<u32>,
    pub php_pid: Option<u32>,
    pub wordpress_health: WordPressHealthState,
    pub wordpress_error: Option<RuntimeErrorInfo>,
    pub last_error: Option<RuntimeErrorInfo>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevelopmentManifest {
    schema_version: u32,
    target: String,
    runtime_version: String,
    php: PhpManifest,
    mariadb: MariaDbManifest,
    http_fixture: HttpFixtureManifest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhpManifest {
    version: String,
    archive: String,
    executable: PathBuf,
    ini: PathBuf,
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
    pub(crate) php_ini: PathBuf,
    pub(crate) mariadb_version: String,
    pub(crate) mariadb_executable: PathBuf,
    pub(crate) mariadb_client_executable: PathBuf,
    pub(crate) mariadb_install_db_executable: PathBuf,
    pub(crate) mariadb_base_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RuntimeSettings {
    schema_version: u32,
    database_port: u16,
    http_port: u16,
}

#[derive(Clone, Debug)]
pub struct RuntimeTimeouts {
    pub database_readiness: Duration,
    pub http_readiness: Duration,
    pub probe_command: Duration,
    pub stop: Duration,
}

impl Default for RuntimeTimeouts {
    fn default() -> Self {
        Self {
            database_readiness: Duration::from_secs(15),
            http_readiness: Duration::from_secs(10),
            probe_command: Duration::from_secs(3),
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

    fn terminate(&mut self, timeout: Duration) -> Result<(), RuntimeErrorInfo> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        self.child.kill().map_err(|error| {
            error_info(
                "runtime",
                "terminate process",
                format!("Cannot terminate child process: {error}."),
                "Close the child process from the operating system, then reopen CoffeePOS Desktop.",
            )
        })?;
        wait_for_child_exit(&mut self.child, timeout).map(|_| ())
    }
}

pub struct RuntimeManager {
    runtime: ResolvedRuntime,
    data_root: PathBuf,
    containment: ProcessContainment,
    state: RuntimeState,
    database_port: Option<u16>,
    http_port: Option<u16>,
    database: Option<ManagedChild>,
    php: Option<ManagedChild>,
    php_probe: Option<PathBuf>,
    wordpress_health: WordPressHealthState,
    wordpress_error: Option<RuntimeErrorInfo>,
    log_lock: Arc<Mutex<()>>,
    last_error: Option<RuntimeErrorInfo>,
    timeouts: RuntimeTimeouts,
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
            database: None,
            php: None,
            php_probe: None,
            wordpress_health: WordPressHealthState::Unavailable,
            wordpress_error: None,
            log_lock: Arc::new(Mutex::new(())),
            last_error: None,
            timeouts: RuntimeTimeouts::default(),
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
            mariadb_version: Some(self.runtime.mariadb_version.clone()),
            database_port: self.database_port,
            http_port: self.http_port,
            database_pid: self.database.as_ref().map(ManagedChild::id),
            php_pid: self.php.as_ref().map(ManagedChild::id),
            wordpress_health: self.wordpress_health.clone(),
            wordpress_error: self.wordpress_error.clone(),
            last_error: self.last_error.clone(),
        }
    }

    pub(crate) fn provisioning_context(&self) -> (ResolvedRuntime, PathBuf) {
        (self.runtime.clone(), self.data_root.clone())
    }

    pub fn refresh(&mut self) -> RuntimeInfo {
        if self.state == RuntimeState::Running {
            let database_exit = child_exit(&mut self.database, "database");
            let php_exit = child_exit(&mut self.php, "php");
            if let Some(error) = database_exit.or(php_exit) {
                let cleanup_error = self.cleanup_started().err();
                self.state = if self.database.is_some() || self.php.is_some() {
                    RuntimeState::Stopping
                } else {
                    RuntimeState::Stopped
                };
                if self.database.is_none() {
                    self.database_port = None;
                }
                if self.php.is_none() {
                    self.http_port = None;
                }
                self.last_error = Some(cleanup_error.unwrap_or(error));
                self.wordpress_health = WordPressHealthState::Unavailable;
                self.wordpress_error = None;
                self.log_event("runtime child exited unexpectedly");
            }
        } else if self.database.is_none() && self.php.is_none() {
            self.state = if installation_ready(&self.data_root) {
                RuntimeState::Stopped
            } else {
                RuntimeState::NotInstalled
            };
            self.wordpress_health = WordPressHealthState::Unavailable;
            self.wordpress_error = None;
        }
        self.info()
    }

    pub fn start(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        self.start_with_wordpress_health(true)
    }

    pub(crate) fn start_for_provisioning(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        self.start_with_wordpress_health(false)
    }

    fn start_with_wordpress_health(
        &mut self,
        check_wordpress_health: bool,
    ) -> Result<RuntimeInfo, RuntimeErrorInfo> {
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
        self.state = RuntimeState::Starting;
        self.wordpress_health = WordPressHealthState::Checking;
        self.wordpress_error = None;
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
            self.database_port = Some(database_port);
            self.http_port = Some(http_port);

            match self.start_attempt(database_port, http_port) {
                Ok(()) => {
                    if let Err(error) = self.persist_runtime_settings(database_port, http_port) {
                        let cleanup_error = self.cleanup_started().err();
                        if self.database.is_none() {
                            self.database_port = None;
                        }
                        if self.php.is_none() {
                            self.http_port = None;
                        }
                        last_error = Some(cleanup_error.unwrap_or(error));
                        break;
                    }
                    self.state = RuntimeState::Running;
                    self.last_error = None;
                    if check_wordpress_health {
                        self.refresh_wordpress_health();
                    } else {
                        self.wordpress_health = WordPressHealthState::Unavailable;
                        self.wordpress_error = None;
                    }
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
                    if self.php.is_none() {
                        self.http_port = None;
                    }
                    last_error = Some(cleanup_error.unwrap_or(error));
                    if self.database.is_some() || self.php.is_some() {
                        break;
                    }
                    if !retryable {
                        break;
                    }
                    self.log_event("runtime readiness retry");
                }
            }
        }

        self.state = if self.database.is_some() || self.php.is_some() {
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
        self.log_event("runtime start failed");
        Err(error)
    }

    pub(crate) fn refresh_wordpress_health(&mut self) -> RuntimeInfo {
        if self.state != RuntimeState::Running || self.php.is_none() || self.database.is_none() {
            self.wordpress_health = WordPressHealthState::Unavailable;
            self.wordpress_error = None;
            return self.info();
        }
        let Some(http_port) = self.http_port else {
            self.wordpress_health = WordPressHealthState::Unavailable;
            self.wordpress_error = None;
            return self.info();
        };
        self.wordpress_health = WordPressHealthState::Checking;
        self.wordpress_error = None;
        match self.wait_wordpress_ready(http_port) {
            Ok(()) => {
                self.wordpress_health = WordPressHealthState::Healthy;
                self.log_event("wordpress healthy");
            }
            Err(error) => {
                self.wordpress_health = WordPressHealthState::Unhealthy;
                self.wordpress_error = Some(error);
                self.log_event("wordpress health check failed");
            }
        }
        self.info()
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
        let port = self.http_port.ok_or_else(|| {
            error_info(
                "wordpress",
                "open",
                "WordPress cannot be opened because the managed HTTP port is unavailable.",
                "Restart the runtime so CoffeePOS Desktop can select and verify a loopback HTTP port.",
            )
        })?;
        Ok(format!("http://{LOOPBACK}:{port}/"))
    }

    pub fn stop(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
        self.refresh();
        if (self.state == RuntimeState::NotInstalled || self.state == RuntimeState::Stopped)
            && self.database.is_none()
            && self.php.is_none()
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

        self.state = RuntimeState::Stopping;
        self.wordpress_health = WordPressHealthState::Unavailable;
        self.wordpress_error = None;
        self.log_event("runtime stop requested");
        let mut failure = None;

        if let Some(php) = self.php.as_mut() {
            if let Err(error) = php.terminate(self.timeouts.stop) {
                failure = Some(error);
            } else {
                self.php = None;
            }
        }

        if self.database.is_some() {
            if let Err(error) = self.shutdown_database_gracefully() {
                failure.get_or_insert(error);
            }
        }
        if let Some(database) = self.database.as_mut() {
            if let Err(error) = database.terminate(self.timeouts.stop) {
                failure.get_or_insert(error);
            } else {
                self.database = None;
            }
        }

        self.remove_php_probe();
        if self.database.is_none() {
            self.database_port = None;
        }
        if self.php.is_none() {
            self.http_port = None;
        }
        self.state = if self.database.is_some() || self.php.is_some() {
            RuntimeState::Stopping
        } else if installation_ready(&self.data_root) {
            RuntimeState::Stopped
        } else {
            RuntimeState::NotInstalled
        };

        if let Some(error) = failure {
            self.last_error = Some(error.clone());
            self.log_event("runtime stop required forced cleanup");
            return Err(error);
        }
        self.last_error = None;
        self.log_event("runtime stopped");
        Ok(self.info())
    }

    fn wait_wordpress_ready(&mut self, port: u16) -> Result<(), RuntimeErrorInfo> {
        let deadline = Instant::now() + self.timeouts.http_readiness;
        loop {
            if child_finished(&mut self.php, "php", "WordPress health")? {
                return Err(error_info(
                    "wordpress",
                    "health",
                    "PHP exited before WordPress health could be verified.",
                    "Inspect logs/php.log, restart the runtime, and retry the WordPress health check.",
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

    pub fn restart(&mut self) -> Result<RuntimeInfo, RuntimeErrorInfo> {
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
                self.start()
            }
            RuntimeState::NotInstalled | RuntimeState::Stopped => self.start(),
        }
    }

    fn start_attempt(
        &mut self,
        database_port: u16,
        http_port: u16,
    ) -> Result<(), RuntimeErrorInfo> {
        self.database = Some(self.spawn_database(database_port)?);
        self.wait_database_ready(database_port)?;
        self.log_event("database ready");

        let nonce = probe_nonce();
        let probe_name = self.write_php_probe(&nonce)?;
        self.php = Some(self.spawn_php(http_port)?);
        let readiness = self.wait_http_ready(http_port, &probe_name, &nonce);
        self.remove_php_probe();
        readiness?;
        self.log_event("php http ready");
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

    fn spawn_php(&self, port: u16) -> Result<ManagedChild, RuntimeErrorInfo> {
        let site = self.data_root.join("site");
        let database_port = self.database_port.ok_or_else(|| {
            error_info(
                "php",
                "spawn",
                "Database port is unavailable while preparing the PHP process.",
                "Restart the runtime so MariaDB can be started before PHP.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                error_info(
                    "php",
                    "load database credential",
                    error,
                    "Run provisioning repair with the same Windows user profile, then retry.",
                )
            })?;
        let mut command = Command::new(&self.runtime.php_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg("-S")
            .arg(format!("{LOOPBACK}:{port}"))
            .arg("-t")
            .arg(&site)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env_remove("PHP_CLI_SERVER_WORKERS")
            .env("COFFEEPOS_DB_PASSWORD", database_password)
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_SITE_URL", format!("http://{LOOPBACK}:{port}"))
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .current_dir(&site);
        let router = self.data_root.join("config/wordpress-router.php");
        if router.is_file() {
            command.arg(router);
        }
        self.spawn_logged(command, "php", "php.log")
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
            if child_finished(&mut self.php, "php", "readiness")? {
                return Err(error_info(
                    "php",
                    "readiness",
                    "PHP exited before the HTTP probe identified the expected runtime instance.",
                    "Inspect logs/php.log and verify php.ini, the site directory, and the selected loopback port.",
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

    fn cleanup_started(&mut self) -> Result<(), RuntimeErrorInfo> {
        let mut failure = None;
        if let Some(php) = self.php.as_mut() {
            if let Err(error) = php.terminate(self.timeouts.stop) {
                failure = Some(error);
            } else {
                self.php = None;
            }
        }
        if let Some(database) = self.database.as_mut() {
            if let Err(error) = database.terminate(self.timeouts.stop) {
                failure.get_or_insert(error);
            } else {
                self.database = None;
            }
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
        for name in ["runtime.log", "database.log", "php.log"] {
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
        if settings.schema_version != 1 || settings.database_port == 0 || settings.http_port == 0 {
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
            schema_version: 1,
            database_port,
            http_port,
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
            "<?php header('Content-Type: text/plain'); echo '{}';\n",
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
        self.php_probe = Some(path);
        file.write_all(body.as_bytes()).map_err(|error| {
            error_info(
                "php",
                "prepare readiness probe",
                format!("Cannot write the temporary PHP readiness probe: {error}."),
                "Check the site directory permissions and free disk space, then retry.",
            )
        })?;
        file.sync_all().map_err(|error| {
            error_info(
                "php",
                "prepare readiness probe",
                format!("Cannot flush the temporary PHP readiness probe: {error}."),
                "Check the site directory storage health, then retry.",
            )
        })?;
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
    Ok(ResolvedRuntime {
        runtime_version: manifest.runtime_version,
        php_version: manifest.php.version,
        php_executable: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.php.executable,
            "PHP executable",
            Some(manifest_root),
        )?),
        php_ini: command_compatible_path(canonical_file_under(
            &development_root,
            &manifest.php.ini,
            "php.ini",
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

fn loopback_port_available(port: u16) -> bool {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
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
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(300)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
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

fn wordpress_probe_response_healthy(response: &[u8]) -> bool {
    let text = String::from_utf8_lossy(response);
    (text.starts_with("HTTP/1.0 200 ") || text.starts_with("HTTP/1.1 200 "))
        && text.contains("loginform")
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
    error_info(
        "runtime",
        "write log",
        format!("Cannot write runtime diagnostics: {error}."),
        "Check application-data permissions and free disk space, then retry.",
    )
}

fn manifest_error(message: impl Into<String>) -> RuntimeErrorInfo {
    error_info(
        "runtime",
        "resolve manifest",
        message,
        "Use a target-specific manifest whose artifact paths resolve canonically inside project runtime/development, then retry.",
    )
}

fn not_installed_error(message: impl Into<String>) -> RuntimeErrorInfo {
    error_info(
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
    RuntimeErrorInfo {
        component: component.into(),
        operation: operation.into(),
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
        let job = unsafe { windows_job::create_kill_on_close_job() }.map_err(|error| {
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
        unsafe { windows_job::assign_process(self.job, child) }.map_err(|error| {
            error_info(
                "runtime",
                "assign process job",
                format!("Cannot attach child process to the Windows runtime job: {error}."),
                "Stop any stale runtime process and retry. CoffeePOS does not leave this child running without containment.",
            )
        })
    }
}

#[cfg(windows)]
impl Drop for ProcessContainment {
    fn drop(&mut self) {
        unsafe { windows_job::close_job(self.job) };
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

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

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
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub unsafe fn create_kill_on_close_job() -> io::Result<usize> {
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
        Ok(job as usize)
    }

    pub unsafe fn assign_process(job: usize, child: &Child) -> io::Result<()> {
        let process = child.as_raw_handle();
        if AssignProcessToJobObject(job as *mut c_void, process) == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub unsafe fn close_job(job: usize) {
        if job != 0 {
            let _ = CloseHandle(job as *mut c_void);
        }
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

    fn manifest_fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
        let development = root.join("runtime/development");
        let php = development.join("php/php-test");
        let php_ini = development.join("php/php.ini");
        let mariadb = development.join("mariadb/bin/mariadbd-test");
        let client = development.join("mariadb/bin/mariadb-test");
        let install_db = development.join("mariadb/bin/mariadb-install-db-test");
        for path in [&php, &php_ini, &mariadb, &client, &install_db] {
            touch(path);
        }
        (development, php, php_ini, mariadb, client, install_db)
    }

    fn write_manifest(
        path: &Path,
        php: &Path,
        php_ini: &Path,
        server: &Path,
        client: &Path,
        install_db: &Path,
    ) {
        let root = path.parent().unwrap();
        touch(&root.join("php/license.txt"));
        touch(&root.join("mariadb/COPYING"));
        touch(&root.join("fixture/router.php"));
        fs::create_dir_all(root.join("fixture/site")).unwrap();
        let manifest = json!({
            "schema_version": 1,
            "target": current_target_triple().unwrap(),
            "runtime_version": "test-runtime",
            "php": {
                "version": "8.4-test",
                "archive": "php-test.zip",
                "executable": php,
                "ini": php_ini,
                "source": "test fixture",
                "checksum_source": "test fixture checksum",
                "archive_sha256": "a".repeat(64),
                "license": "PHP-3.01",
                "license_file": "php/license.txt"
            },
            "mariadb": {
                "version": "11.4-test",
                "archive": "mariadb-test.zip",
                "server": server,
                "client": client,
                "install_db": install_db,
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
        let (development, php, php_ini, mariadb, client, install_db) = manifest_fixture(&project);
        let manifest = development.join("manifest.json");
        write_manifest(&manifest, &php, &php_ini, &mariadb, &client, &install_db);

        let resolved = resolve_development_manifest(&project, &manifest).unwrap();
        assert_eq!(resolved.runtime_version, "test-runtime");
        assert_eq!(resolved.php_version, "8.4-test");
        assert_eq!(resolved.mariadb_version, "11.4-test");
    }

    #[test]
    fn development_manifest_rejects_path_escape() {
        if current_target_triple().is_err() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        let (development, _php, php_ini, mariadb, client, install_db) = manifest_fixture(&project);
        let escaped_php = project.join("outside-php");
        touch(&escaped_php);
        let manifest = development.join("manifest.json");
        write_manifest(
            &manifest,
            &escaped_php,
            &php_ini,
            &mariadb,
            &client,
            &install_db,
        );

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
        let (development, _php, php_ini, mariadb, client, install_db) = manifest_fixture(&project);
        let manifest = development.join("manifest.json");
        write_manifest(
            &manifest,
            Path::new("php/php-test"),
            &php_ini,
            &mariadb,
            &client,
            &install_db,
        );

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

    fn fake_runtime() -> ResolvedRuntime {
        let executable = std::env::current_exe().unwrap();
        let base = executable.parent().unwrap().to_path_buf();
        ResolvedRuntime {
            runtime_version: "test-runtime".into(),
            php_version: "test-php".into(),
            php_executable: executable.clone(),
            php_ini: executable.clone(),
            mariadb_version: "test-db".into(),
            mariadb_executable: executable.clone(),
            mariadb_client_executable: executable.clone(),
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
            probe_command: Duration::from_millis(250),
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
            mariadb_version: Some("test-db".into()),
            database_port: Some(3307),
            http_port: Some(8081),
            database_pid: None,
            php_pid: None,
            wordpress_health: WordPressHealthState::Checking,
            wordpress_error: None,
            last_error: None,
        };
        let value = serde_json::to_value(info).unwrap();
        assert_eq!(value["state"], "starting");
        assert_eq!(value["database_port"], 3307);
        assert_eq!(value["http_port"], 8081);
        assert_eq!(value["wordpress_health"], "checking");
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
        let mut manager = RuntimeManager::from_development(&project, &manifest, data).unwrap();

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
}
