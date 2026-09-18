use crate::runtime::{
    choose_loopback_port, configure_child_command, run_command_bounded, wait_for_child_exit,
    ProcessContainment, ResolvedRuntime, RuntimeErrorInfo, RuntimeInfo, DATABASE_NAME,
    DATABASE_RUNTIME_SECRET, DATABASE_RUNTIME_USER, DATABASE_WORDPRESS_SECRET,
    DATABASE_WORDPRESS_USER,
};
use crate::secret;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

const LOOPBACK: &str = "127.0.0.1";
const PROVISIONING_SCHEMA_VERSION: u32 = 1;
const WORDPRESS_MANIFEST_SCHEMA_VERSION: u32 = 1;
const DATABASE_BOOTSTRAP_SECRET: &str = "config/database-bootstrap.secret";
const WORDPRESS_ADMIN_SECRET: &str = "config/wordpress-admin.secret";
const WORDPRESS_ADMIN_USER: &str = "coffeepos_admin";
const WORDPRESS_ADMIN_EMAIL: &str = "admin@coffeepos.local";
const MANAGED_CONFIG_MARKER: &str = "CoffeePOS Desktop managed configuration";
const MANAGED_ROUTER_MARKER: &str = "CoffeePOS Desktop managed router";
const MANAGED_MU_PLUGIN_MARKER: &str = "CoffeePOS Desktop managed uploads bridge";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningState {
    NotInstalled,
    Installing,
    Ready,
    NeedsRepair,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProvisioningInfo {
    pub state: ProvisioningState,
    pub wordpress_version: String,
    pub admin_username: Option<String>,
    pub can_retry: bool,
    pub last_error: Option<RuntimeErrorInfo>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WordPressDevelopmentManifest {
    schema_version: u32,
    target: String,
    wordpress: WordPressArtifactManifest,
    compatibility: CompatibilityManifest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WordPressArtifactManifest {
    version: String,
    archive: String,
    source: String,
    release_source: String,
    official_sha1_source: String,
    official_sha1: String,
    archive_sha256: String,
    archive_sha256_source: String,
    license: String,
    core_root: PathBuf,
    license_file: PathBuf,
    readme_file: PathBuf,
    version_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityManifest {
    source: String,
    recommended_php: String,
    recommended_mariadb: String,
    development_php: String,
    development_mariadb: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedWordPress {
    pub version: String,
    pub core_root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum ProvisioningStage {
    DatabaseReady,
    SiteReady,
    WordPressInstalled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvisioningJournal {
    schema_version: u32,
    wordpress_version: String,
    stage: ProvisioningStage,
    admin_username: String,
}

pub struct Provisioner {
    runtime: ResolvedRuntime,
    wordpress: ResolvedWordPress,
    data_root: PathBuf,
    containment: ProcessContainment,
}

enum DatabaseEndpoint {
    Pipe(String),
    Tcp(u16),
}

impl Provisioner {
    pub fn from_development(
        project_root: &Path,
        runtime_manifest_path: &Path,
        runtime: ResolvedRuntime,
        data_root: PathBuf,
    ) -> Result<Self, RuntimeErrorInfo> {
        if !data_root.is_absolute() {
            return Err(provisioning_error(
                "resolve data path",
                "Application data root must be absolute.",
                "Resolve the application data directory through Tauri before provisioning.",
            ));
        }
        Ok(Self {
            runtime,
            wordpress: resolve_development_wordpress(project_root, runtime_manifest_path)?,
            data_root,
            containment: ProcessContainment::new()?,
        })
    }

    pub fn inspect(&self) -> ProvisioningInfo {
        let journal = match self.load_journal() {
            Ok(journal) => journal,
            Err(error) => {
                return ProvisioningInfo {
                    state: ProvisioningState::NeedsRepair,
                    wordpress_version: self.wordpress.version.clone(),
                    admin_username: None,
                    can_retry: false,
                    last_error: Some(error),
                };
            }
        };
        let database_ready = self.data_root.join("database/mysql").is_dir()
            && self.data_root.join(DATABASE_RUNTIME_SECRET).is_file()
            && self.data_root.join(DATABASE_WORDPRESS_SECRET).is_file();
        let site_ready = self.data_root.join("site/wp-settings.php").is_file()
            && self.data_root.join("site/wp-config.php").is_file()
            && self.data_root.join("config/wordpress-router.php").is_file();
        let complete = journal
            .as_ref()
            .map(|value| {
                value.schema_version == PROVISIONING_SCHEMA_VERSION
                    && value.wordpress_version == self.wordpress.version
                    && value.stage == ProvisioningStage::WordPressInstalled
            })
            .unwrap_or(false)
            && database_ready
            && site_ready;
        if complete {
            return ProvisioningInfo {
                state: ProvisioningState::Ready,
                wordpress_version: self.wordpress.version.clone(),
                admin_username: Some(WORDPRESS_ADMIN_USER.into()),
                can_retry: false,
                last_error: None,
            };
        }
        let pristine = !self.data_root.join("database/mysql").exists()
            && directory_is_empty_or_missing(&self.data_root.join("database"))
            && directory_is_empty_or_missing(&self.data_root.join("site"))
            && journal.is_none();
        let state = if pristine {
            ProvisioningState::NotInstalled
        } else {
            ProvisioningState::NeedsRepair
        };
        let last_error = if pristine {
            None
        } else {
            Some(provisioning_error(
                "inspect store",
                "WordPress provisioning is incomplete or the managed store layout is inconsistent.",
                "Retry provisioning. Existing site/database data is preserved; if retry is refused, use an explicit repair flow instead of deleting store data.",
            ))
        };
        ProvisioningInfo {
            state,
            wordpress_version: self.wordpress.version.clone(),
            admin_username: journal.map(|value| value.admin_username),
            can_retry: true,
            last_error,
        }
    }

    pub fn installing_info(&self) -> ProvisioningInfo {
        ProvisioningInfo {
            state: ProvisioningState::Installing,
            wordpress_version: self.wordpress.version.clone(),
            admin_username: Some(WORDPRESS_ADMIN_USER.into()),
            can_retry: false,
            last_error: None,
        }
    }

    pub fn prepare(&mut self) -> Result<ProvisioningInfo, RuntimeErrorInfo> {
        self.prepare_logs()?;
        self.log_event("provisioning prepare requested");
        self.ensure_database_initialized()?;
        self.ensure_database_accounts()?;
        self.persist_stage(ProvisioningStage::DatabaseReady)?;
        self.ensure_wordpress_site()?;
        self.persist_stage(ProvisioningStage::SiteReady)?;
        self.log_event("provisioning site prepared");
        Ok(ProvisioningInfo {
            state: ProvisioningState::Installing,
            wordpress_version: self.wordpress.version.clone(),
            admin_username: Some(WORDPRESS_ADMIN_USER.into()),
            can_retry: false,
            last_error: None,
        })
    }

    pub fn install_wordpress(
        &mut self,
        store_name: &str,
        runtime_info: &RuntimeInfo,
    ) -> Result<ProvisioningInfo, RuntimeErrorInfo> {
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "install WordPress",
                "MariaDB port is unavailable after runtime startup.",
                "Stop the runtime, retry provisioning, and inspect runtime logs if MariaDB is not ready.",
            )
        })?;
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "install WordPress",
                "HTTP port is unavailable after runtime startup.",
                "Stop the runtime, retry provisioning, and inspect PHP logs if the server is not ready.",
            )
        })?;
        let wordpress_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "load WordPress database credential",
                    error,
                    "Retry with the same Windows user profile. Existing store data has been preserved.",
                )
            })?;
        let admin_password =
            secret::create(&self.data_root.join(WORDPRESS_ADMIN_SECRET)).map_err(|error| {
                provisioning_error(
                    "create WordPress administrator credential",
                    error,
                    "Check application-data permissions and Windows DPAPI, then retry.",
                )
            })?;
        let script = self.write_wordpress_bootstrap_script()?;
        let site_url = format!("http://{LOOPBACK}:{http_port}");
        let mut command = Command::new(&self.runtime.php_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg(&script)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_DB_PASSWORD", wordpress_password)
            .env("COFFEEPOS_SITE_URL", &site_url)
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env("COFFEEPOS_SITE_ROOT", self.data_root.join("site"))
            .env("COFFEEPOS_STORE_NAME", store_name)
            .env("COFFEEPOS_ADMIN_USER", WORDPRESS_ADMIN_USER)
            .env("COFFEEPOS_ADMIN_EMAIL", WORDPRESS_ADMIN_EMAIL)
            .env("COFFEEPOS_ADMIN_PASSWORD", admin_password)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let status = run_command_bounded(
            command,
            Duration::from_secs(60),
            "wordpress",
            "install",
            &self.containment,
        );
        let _ = fs::remove_file(&script);
        let status = status?;
        if !status.success() {
            return Err(provisioning_error(
                "install WordPress",
                format!("WordPress bootstrap exited with status {status}."),
                "Inspect logs/wordpress.log and retry. Existing database/site files are preserved.",
            ));
        }
        if !wordpress_http_ready(http_port) {
            return Err(provisioning_error(
                "verify WordPress",
                "WordPress did not return the expected login page after installation.",
                "Inspect logs/php.log and retry provisioning. The existing installation is preserved.",
            ));
        }
        self.persist_stage(ProvisioningStage::WordPressInstalled)?;
        self.log_event("wordpress provisioning ready");
        Ok(ProvisioningInfo {
            state: ProvisioningState::Ready,
            wordpress_version: self.wordpress.version.clone(),
            admin_username: Some(WORDPRESS_ADMIN_USER.into()),
            can_retry: false,
            last_error: None,
        })
    }

    fn ensure_database_initialized(&mut self) -> Result<(), RuntimeErrorInfo> {
        let database = self.data_root.join("database");
        if database.join("mysql").is_dir() {
            return Ok(());
        }
        if !directory_is_empty_or_missing(&database) {
            return Err(provisioning_error(
                "initialize database",
                "The MariaDB directory contains files but is not a valid initialized datadir.",
                "Preserve the database directory and repair/restore it explicitly. CoffeePOS will not delete or reinitialize unknown database files.",
            ));
        }

        let staging = self.data_root.join("database.provisioning");
        reset_owned_staging_dir(&self.data_root, &staging)?;
        fs::create_dir_all(&staging).map_err(|error| {
            provisioning_error(
                "prepare database staging",
                format!("Cannot create database staging directory: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        let _bootstrap_password = secret::create(&self.data_root.join(DATABASE_BOOTSTRAP_SECRET))
            .map_err(|error| {
            provisioning_error(
                "create database bootstrap credential",
                error,
                "Check Windows DPAPI and application-data permissions, then retry.",
            )
        })?;

        let mut command = Command::new(&self.runtime.mariadb_install_db_executable);
        command
            .arg(format!("--datadir={}", staging.to_string_lossy()))
            .arg("--silent")
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(&self.runtime.mariadb_base_dir);
        configure_child_command(&mut command);
        let status = run_command_bounded(
            command,
            Duration::from_secs(90),
            "database",
            "initialize",
            &self.containment,
        )?;
        if !status.success() || !staging.join("mysql").is_dir() {
            return Err(provisioning_error(
                "initialize database",
                format!("mariadb-install-db did not create a complete datadir (status {status})."),
                "Inspect logs/provisioning.log and retry. Only database.provisioning is safe to recreate automatically.",
            ));
        }
        if database.exists() {
            fs::remove_dir(&database).map_err(|error| {
                provisioning_error(
                    "activate database",
                    format!("Cannot replace the empty database directory: {error}."),
                    "Ensure no process is using the application database directory, then retry.",
                )
            })?;
        }
        fs::rename(&staging, &database).map_err(|error| {
            provisioning_error(
                "activate database",
                format!("Cannot atomically activate the initialized datadir: {error}."),
                "Preserve both directories and retry after checking filesystem permissions.",
            )
        })?;
        self.log_event("database datadir initialized");
        Ok(())
    }

    fn ensure_database_accounts(&mut self) -> Result<(), RuntimeErrorInfo> {
        let runtime_secret_path = self.data_root.join(DATABASE_RUNTIME_SECRET);
        let wordpress_secret_path = self.data_root.join(DATABASE_WORDPRESS_SECRET);
        let bootstrap_secret_path = self.data_root.join(DATABASE_BOOTSTRAP_SECRET);

        if runtime_secret_path.is_file()
            && wordpress_secret_path.is_file()
            && !bootstrap_secret_path.is_file()
        {
            return self.verify_existing_database_accounts();
        }
        if !bootstrap_secret_path.is_file() {
            return Err(provisioning_error(
                "configure database accounts",
                "The initialized datadir is missing its bootstrap credential and application accounts are incomplete.",
                "Preserve the datadir and restore the matching protected bootstrap configuration before retrying.",
            ));
        }

        let bootstrap_password = secret::load(&bootstrap_secret_path).map_err(|error| {
            provisioning_error(
                "load database bootstrap credential",
                error,
                "Retry with the same Windows user profile or restore the protected bootstrap credential.",
            )
        })?;
        let runtime_password = secret::create(&runtime_secret_path).map_err(|error| {
            provisioning_error(
                "create runtime database credential",
                error,
                "Check application-data permissions and Windows DPAPI, then retry.",
            )
        })?;
        let wordpress_password = secret::create(&wordpress_secret_path).map_err(|error| {
            provisioning_error(
                "create WordPress database credential",
                error,
                "Check application-data permissions and Windows DPAPI, then retry.",
            )
        })?;

        let pipe = provisioning_pipe_name();
        let endpoint = DatabaseEndpoint::Pipe(pipe.clone());
        let mut database = self.spawn_database(&endpoint)?;
        let root_password = match self.wait_for_database_user(
            &mut database,
            &endpoint,
            "root",
            &bootstrap_password,
            None,
        ) {
            Ok(()) => bootstrap_password.clone(),
            Err(_) => {
                self.wait_for_database_user(&mut database, &endpoint, "root", "", None)?;
                String::new()
            }
        };
        let sql = format!(
            "CREATE DATABASE IF NOT EXISTS `{DATABASE_NAME}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\n\
             CREATE USER IF NOT EXISTS '{DATABASE_WORDPRESS_USER}'@'127.0.0.1' IDENTIFIED BY '{}';\n\
             ALTER USER '{DATABASE_WORDPRESS_USER}'@'127.0.0.1' IDENTIFIED BY '{}';\n\
             GRANT ALL PRIVILEGES ON `{DATABASE_NAME}`.* TO '{DATABASE_WORDPRESS_USER}'@'127.0.0.1';\n\
             CREATE USER IF NOT EXISTS '{DATABASE_RUNTIME_USER}'@'127.0.0.1' IDENTIFIED BY '{}';\n\
             ALTER USER '{DATABASE_RUNTIME_USER}'@'127.0.0.1' IDENTIFIED BY '{}';\n\
             GRANT SHUTDOWN ON *.* TO '{DATABASE_RUNTIME_USER}'@'127.0.0.1';\n\
             ALTER USER CURRENT_USER() IDENTIFIED BY '{}';\n\
             FLUSH PRIVILEGES;\n\
             SHUTDOWN;\n",
            sql_literal(&wordpress_password),
            sql_literal(&wordpress_password),
            sql_literal(&runtime_password),
            sql_literal(&runtime_password),
            sql_literal(&bootstrap_password),
        );
        self.run_database_sql(&endpoint, "root", &root_password, &sql)?;
        let _ = wait_for_child_exit(&mut database, Duration::from_secs(10));

        self.verify_existing_database_accounts()?;
        fs::remove_file(&bootstrap_secret_path).map_err(|error| {
            provisioning_error(
                "retire bootstrap credential",
                format!("Application accounts are ready, but the bootstrap credential could not be removed: {error}."),
                "Close processes using the configuration file and retry provisioning.",
            )
        })?;
        self.log_event("database accounts configured");
        Ok(())
    }

    fn verify_existing_database_accounts(&self) -> Result<(), RuntimeErrorInfo> {
        let runtime_password = secret::load(&self.data_root.join(DATABASE_RUNTIME_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "verify runtime database account",
                    error,
                    "Retry with the Windows user that created this store or restore the matching protected credential.",
                )
            })?;
        let wordpress_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "verify WordPress database account",
                    error,
                    "Retry with the Windows user that created this store or restore the matching protected credential.",
                )
            })?;
        let port = choose_loopback_port(&[])?;
        let endpoint = DatabaseEndpoint::Tcp(port);
        let mut database = self.spawn_database(&endpoint)?;
        self.wait_for_database_user(
            &mut database,
            &endpoint,
            DATABASE_RUNTIME_USER,
            &runtime_password,
            None,
        )?;
        self.wait_for_database_user(
            &mut database,
            &endpoint,
            DATABASE_WORDPRESS_USER,
            &wordpress_password,
            Some(DATABASE_NAME),
        )?;
        self.run_database_sql(
            &endpoint,
            DATABASE_RUNTIME_USER,
            &runtime_password,
            "SHUTDOWN;\n",
        )?;
        let _ = wait_for_child_exit(&mut database, Duration::from_secs(10));
        Ok(())
    }

    fn spawn_database(&self, endpoint: &DatabaseEndpoint) -> Result<Child, RuntimeErrorInfo> {
        let database = self.data_root.join("database");
        let mut command = Command::new(&self.runtime.mariadb_executable);
        command
            .arg("--no-defaults")
            .arg(format!(
                "--basedir={}",
                self.runtime.mariadb_base_dir.to_string_lossy()
            ))
            .arg(format!("--datadir={}", database.to_string_lossy()))
            .arg("--skip-log-bin")
            .arg(format!(
                "--pid-file={}",
                database.join("mariadb-provisioning.pid").to_string_lossy()
            ));
        match endpoint {
            DatabaseEndpoint::Pipe(pipe) => {
                command
                    .arg("--skip-networking")
                    .arg("--named-pipe")
                    .arg(format!("--socket={pipe}"));
            }
            DatabaseEndpoint::Tcp(port) => {
                command
                    .arg(format!("--port={port}"))
                    .arg(format!("--bind-address={LOOPBACK}"))
                    .arg("--skip-name-resolve");
            }
        }
        #[cfg(windows)]
        command.arg("--console");
        command
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .current_dir(&self.runtime.mariadb_base_dir);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.data_root.join("logs/provisioning.log"))
            .map_err(|error| {
                provisioning_error(
                    "open database log",
                    format!("Cannot open provisioning log: {error}."),
                    "Check application-data permissions and retry.",
                )
            })?;
        command.stdout(Stdio::from(log.try_clone().map_err(|error| {
            provisioning_error(
                "open database log",
                format!("Cannot duplicate provisioning log handle: {error}."),
                "Retry after checking application-data storage.",
            )
        })?));
        command.stderr(Stdio::from(log));
        configure_child_command(&mut command);
        let mut child = command.spawn().map_err(|error| {
            provisioning_error(
                "start database bootstrap",
                format!("Cannot start MariaDB for provisioning: {error}."),
                "Verify the pinned MariaDB bundle and inspect logs/provisioning.log before retrying.",
            )
        })?;
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(child)
    }

    fn wait_for_database_user(
        &self,
        database: &mut Child,
        endpoint: &DatabaseEndpoint,
        user: &str,
        password: &str,
        database_name: Option<&str>,
    ) -> Result<(), RuntimeErrorInfo> {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if let Some(status) = database.try_wait().map_err(|error| {
                provisioning_error(
                    "wait for database",
                    format!("Cannot inspect provisioning database process: {error}."),
                    "Inspect logs/provisioning.log and retry.",
                )
            })? {
                return Err(provisioning_error(
                    "wait for database",
                    format!("MariaDB exited during provisioning with status {status}."),
                    "Inspect logs/provisioning.log. Existing database files have been preserved.",
                ));
            }
            if self.database_user_probe(endpoint, user, password, database_name)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(provisioning_error(
                    "wait for database",
                    format!("MariaDB did not authenticate provisioning account '{user}' before the timeout."),
                    "Inspect logs/provisioning.log and preserve the datadir while correcting credentials/runtime state.",
                ));
            }
            thread::sleep(Duration::from_millis(150));
        }
    }

    fn database_user_probe(
        &self,
        endpoint: &DatabaseEndpoint,
        user: &str,
        password: &str,
        database_name: Option<&str>,
    ) -> Result<bool, RuntimeErrorInfo> {
        let mut command = self.database_client_command(endpoint, user, password);
        if let Some(database_name) = database_name {
            command.arg(format!("--database={database_name}"));
        }
        command
            .arg("--execute=SELECT 1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_child_command(&mut command);
        run_command_bounded(
            command,
            Duration::from_secs(3),
            "database",
            "provisioning readiness",
            &self.containment,
        )
        .map(|status| status.success())
    }

    fn run_database_sql(
        &self,
        endpoint: &DatabaseEndpoint,
        user: &str,
        password: &str,
        sql: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let mut command = self.database_client_command(endpoint, user, password);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_child_command(&mut command);
        let mut child = command.spawn().map_err(|error| {
            provisioning_error(
                "execute database bootstrap",
                format!("Cannot start MariaDB client: {error}."),
                "Verify the pinned MariaDB client executable and retry.",
            )
        })?;
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(sql.as_bytes()) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(provisioning_error(
                    "execute database bootstrap",
                    format!("Cannot send SQL to MariaDB client: {error}."),
                    "Retry provisioning; credentials were not written to logs or command arguments.",
                ));
            }
        }
        let status = wait_for_child_exit(&mut child, Duration::from_secs(10))?;
        if !status.success() {
            return Err(provisioning_error(
                "execute database bootstrap",
                format!("MariaDB client rejected provisioning SQL with status {status}."),
                "Inspect logs/provisioning.log and retry. Existing database data is preserved.",
            ));
        }
        Ok(())
    }

    fn database_client_command(
        &self,
        endpoint: &DatabaseEndpoint,
        user: &str,
        password: &str,
    ) -> Command {
        let mut command = Command::new(&self.runtime.mariadb_client_executable);
        command.arg("--no-defaults");
        match endpoint {
            DatabaseEndpoint::Pipe(pipe) => {
                command
                    .arg("--protocol=PIPE")
                    .arg(format!("--socket={pipe}"));
            }
            DatabaseEndpoint::Tcp(port) => {
                command
                    .arg("--protocol=tcp")
                    .arg(format!("--host={LOOPBACK}"))
                    .arg(format!("--port={port}"));
            }
        }
        command
            .arg(format!("--user={user}"))
            .arg("--connect-timeout=1")
            .arg("--batch")
            .arg("--skip-column-names")
            .env("MYSQL_PWD", password)
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME");
        command
    }

    fn ensure_wordpress_site(&mut self) -> Result<(), RuntimeErrorInfo> {
        let site = self.data_root.join("site");
        let journal = self.load_journal()?;
        let core_present = site.join("wp-settings.php").is_file()
            && site.join("wp-includes/version.php").is_file();
        if !core_present {
            if !directory_is_empty_or_missing(&site) {
                return Err(provisioning_error(
                    "stage WordPress",
                    "The site directory contains files but does not contain a complete WordPress core.",
                    "Preserve the site directory and repair/restore it explicitly. CoffeePOS will not overwrite unknown site files.",
                ));
            }
            self.copy_wordpress_core_atomically(&site)?;
        } else if journal.is_none() && !managed_config_exists(&site) {
            return Err(provisioning_error(
                "stage WordPress",
                "An existing WordPress core was found without CoffeePOS provisioning ownership metadata.",
                "Preserve the existing site and use an explicit import/adoption flow before CoffeePOS manages it.",
            ));
        }
        self.ensure_wp_config()?;
        self.ensure_router()?;
        self.ensure_uploads_mu_plugin()?;
        Ok(())
    }

    fn copy_wordpress_core_atomically(&mut self, site: &Path) -> Result<(), RuntimeErrorInfo> {
        let staging = self.data_root.join("site.provisioning");
        reset_owned_staging_dir(&self.data_root, &staging)?;
        copy_tree(&self.wordpress.core_root, &staging)?;
        if !staging.join("wp-settings.php").is_file()
            || !staging.join("wp-includes/version.php").is_file()
        {
            return Err(provisioning_error(
                "stage WordPress",
                "Pinned WordPress baseline was copied incompletely.",
                "Restage the verified WordPress artifact and retry.",
            ));
        }
        if site.exists() {
            fs::remove_dir(site).map_err(|error| {
                provisioning_error(
                    "activate WordPress site",
                    format!("Cannot replace the empty site directory: {error}."),
                    "Check application-data permissions and retry. Unknown site files are never deleted.",
                )
            })?;
        }
        fs::rename(&staging, site).map_err(|error| {
            provisioning_error(
                "activate WordPress site",
                format!("Cannot atomically activate WordPress site files: {error}."),
                "Preserve site.provisioning and retry after checking filesystem permissions.",
            )
        })?;
        self.log_event("wordpress core staged");
        Ok(())
    }

    fn ensure_wp_config(&self) -> Result<(), RuntimeErrorInfo> {
        let path = self.data_root.join("site/wp-config.php");
        if path.is_file() {
            let existing = fs::read_to_string(&path).map_err(|error| {
                provisioning_error(
                    "read wp-config",
                    format!("Cannot read existing wp-config.php: {error}."),
                    "Preserve the file and check its permissions before retrying.",
                )
            })?;
            if existing.contains(MANAGED_CONFIG_MARKER) {
                return Ok(());
            }
            return Err(provisioning_error(
                "generate wp-config",
                "wp-config.php already exists and is not marked as CoffeePOS-managed.",
                "Preserve the existing configuration and explicitly adopt/import the site before retrying.",
            ));
        }
        let salts = generate_salts()?;
        let contents = format!(
            "<?php\n// {MANAGED_CONFIG_MARKER}.\n\
define('DB_NAME', '{DATABASE_NAME}');\n\
define('DB_USER', '{DATABASE_WORDPRESS_USER}');\n\
$coffeepos_db_password = getenv('COFFEEPOS_DB_PASSWORD');\n\
$coffeepos_db_host = getenv('COFFEEPOS_DB_HOST');\n\
$coffeepos_site_url = getenv('COFFEEPOS_SITE_URL');\n\
if (!$coffeepos_db_password || !$coffeepos_db_host || !$coffeepos_site_url) {{ http_response_code(503); exit('CoffeePOS runtime environment is unavailable.'); }}\n\
define('DB_PASSWORD', $coffeepos_db_password);\n\
define('DB_HOST', $coffeepos_db_host);\n\
define('DB_CHARSET', 'utf8mb4');\n\
define('DB_COLLATE', '');\n\
define('WP_HOME', $coffeepos_site_url);\n\
define('WP_SITEURL', $coffeepos_site_url);\n\
define('AUTH_KEY',         '{}');\n\
define('SECURE_AUTH_KEY',  '{}');\n\
define('LOGGED_IN_KEY',    '{}');\n\
define('NONCE_KEY',        '{}');\n\
define('AUTH_SALT',        '{}');\n\
define('SECURE_AUTH_SALT', '{}');\n\
define('LOGGED_IN_SALT',   '{}');\n\
define('NONCE_SALT',       '{}');\n\
$table_prefix = 'wp_';\n\
define('WP_DEBUG', false);\n\
define('DISALLOW_FILE_EDIT', true);\n\
define('AUTOMATIC_UPDATER_DISABLED', true);\n\
if (!defined('ABSPATH')) {{ define('ABSPATH', __DIR__ . '/'); }}\n\
require_once ABSPATH . 'wp-settings.php';\n",
            salts[0], salts[1], salts[2], salts[3], salts[4], salts[5], salts[6], salts[7]
        );
        atomic_write(&path, contents.as_bytes(), "wp-config.php")
    }

    fn ensure_router(&self) -> Result<(), RuntimeErrorInfo> {
        write_managed_file(
            &self.data_root.join("config/wordpress-router.php"),
            MANAGED_ROUTER_MARKER,
            WORDPRESS_ROUTER.as_bytes(),
            "WordPress router",
        )
    }

    fn ensure_uploads_mu_plugin(&self) -> Result<(), RuntimeErrorInfo> {
        let path = self
            .data_root
            .join("site/wp-content/mu-plugins/coffeepos-desktop-runtime.php");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                provisioning_error(
                    "prepare WordPress runtime bridge",
                    format!("Cannot create mu-plugins directory: {error}."),
                    "Check site permissions and retry.",
                )
            })?;
        }
        write_managed_file(
            &path,
            MANAGED_MU_PLUGIN_MARKER,
            WORDPRESS_UPLOADS_MU_PLUGIN.as_bytes(),
            "WordPress uploads bridge",
        )
    }

    fn write_wordpress_bootstrap_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare WordPress bootstrap",
                format!("Cannot create temporary WordPress bootstrap script: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(WORDPRESS_BOOTSTRAP.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare WordPress bootstrap",
                    format!("Cannot write WordPress bootstrap script: {error}."),
                    "Check application-data storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare WordPress bootstrap",
                format!("Cannot retain WordPress bootstrap script: {}.", error.error),
                "Check application-data permissions and retry.",
            )
        })?;
        Ok(path)
    }

    fn prepare_logs(&self) -> Result<(), RuntimeErrorInfo> {
        fs::create_dir_all(self.data_root.join("logs")).map_err(|error| {
            provisioning_error(
                "prepare logs",
                format!("Cannot create provisioning log directory: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })
    }

    fn log_event(&self, event: &str) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or_default();
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.data_root.join("logs/provisioning.log"))
        {
            let _ = writeln!(file, "{timestamp} [event] {event}");
        }
    }

    fn journal_path(&self) -> PathBuf {
        self.data_root.join("config/provisioning.json")
    }

    fn load_journal(&self) -> Result<Option<ProvisioningJournal>, RuntimeErrorInfo> {
        let bytes = match fs::read(self.journal_path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(provisioning_error(
                    "read journal",
                    format!("Cannot read provisioning journal: {error}."),
                    "Preserve config/provisioning.json, fix filesystem permissions, and retry.",
                ));
            }
        };
        let journal: ProvisioningJournal = serde_json::from_slice(&bytes).map_err(|error| {
            provisioning_error(
                "read journal",
                format!("Provisioning journal is invalid and was preserved: {error}."),
                "Restore a valid journal or repair explicitly; store data is not reset automatically.",
            )
        })?;
        if journal.schema_version != PROVISIONING_SCHEMA_VERSION {
            return Err(provisioning_error(
                "read journal",
                "Provisioning journal schema is unsupported.",
                "Use a compatible CoffeePOS Desktop version or migrate the installation explicitly.",
            ));
        }
        Ok(Some(journal))
    }

    fn persist_stage(&self, stage: ProvisioningStage) -> Result<(), RuntimeErrorInfo> {
        if let Some(existing) = self.load_journal()? {
            if existing.wordpress_version != self.wordpress.version {
                return Err(provisioning_error(
                    "save journal",
                    format!(
                        "Existing store uses WordPress {}, staged baseline is {}.",
                        existing.wordpress_version, self.wordpress.version
                    ),
                    "Run an explicit WordPress upgrade flow instead of overwriting the store during provisioning.",
                ));
            }
            if existing.stage > stage {
                return Ok(());
            }
        }
        let journal = ProvisioningJournal {
            schema_version: PROVISIONING_SCHEMA_VERSION,
            wordpress_version: self.wordpress.version.clone(),
            stage,
            admin_username: WORDPRESS_ADMIN_USER.into(),
        };
        let mut bytes = serde_json::to_vec_pretty(&journal).map_err(|error| {
            provisioning_error(
                "save journal",
                format!("Cannot serialize provisioning journal: {error}."),
                "Retry after checking application-data storage.",
            )
        })?;
        bytes.push(b'\n');
        atomic_write(&self.journal_path(), &bytes, "provisioning journal")
    }
}

impl Drop for Provisioner {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.data_root.join("database/mariadb-provisioning.pid"));
    }
}

pub fn resolve_development_wordpress(
    project_root: &Path,
    runtime_manifest_path: &Path,
) -> Result<ResolvedWordPress, RuntimeErrorInfo> {
    if !project_root.is_absolute() || !runtime_manifest_path.is_absolute() {
        return Err(provisioning_error(
            "resolve WordPress baseline",
            "Project root and runtime manifest path must both be absolute.",
            "Resolve development paths from the Cargo manifest directory before provisioning.",
        ));
    }
    let development_root =
        fs::canonicalize(project_root.join("runtime/development")).map_err(|error| {
            provisioning_error(
                "resolve WordPress baseline",
                format!("Cannot resolve runtime/development: {error}."),
                "Run the development staging scripts and retry.",
            )
        })?;
    let runtime_manifest = fs::canonicalize(runtime_manifest_path).map_err(|error| {
        provisioning_error(
            "resolve WordPress baseline",
            format!("Cannot resolve runtime manifest: {error}."),
            "Stage the pinned development runtime before provisioning.",
        )
    })?;
    if !runtime_manifest.starts_with(&development_root) {
        return Err(provisioning_error(
            "resolve WordPress baseline",
            "Runtime manifest resolves outside runtime/development.",
            "Use only the pinned project development runtime.",
        ));
    }
    let target_root = runtime_manifest.parent().ok_or_else(|| {
        provisioning_error(
            "resolve WordPress baseline",
            "Runtime manifest does not have a target directory.",
            "Restage the pinned development runtime.",
        )
    })?;
    let manifest_path = target_root.join("wordpress-manifest.json");
    let manifest: WordPressDevelopmentManifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|error| {
            provisioning_error(
                "resolve WordPress baseline",
                format!("Cannot read WordPress manifest: {error}."),
                "Run scripts/stage-wordpress-development.ps1 and retry.",
            )
        })?)
        .map_err(|error| {
            provisioning_error(
                "resolve WordPress baseline",
                format!("WordPress manifest is invalid JSON: {error}."),
                "Restore the checked-in manifest template and restage the baseline.",
            )
        })?;
    validate_wordpress_manifest(&manifest)?;

    let core_root =
        fs::canonicalize(target_root.join(&manifest.wordpress.core_root)).map_err(|error| {
            provisioning_error(
                "resolve WordPress baseline",
                format!("Cannot resolve staged WordPress core: {error}."),
                "Run scripts/stage-wordpress-development.ps1 and retry.",
            )
        })?;
    if !core_root.starts_with(&development_root) || !core_root.is_dir() {
        return Err(provisioning_error(
            "resolve WordPress baseline",
            "WordPress core root resolves outside runtime/development or is not a directory.",
            "Restage the pinned WordPress baseline from the checked-in manifest.",
        ));
    }
    for required in [
        "index.php",
        "wp-settings.php",
        "wp-includes/version.php",
        "wp-admin",
        "wp-content",
        "wp-includes",
    ] {
        if !core_root.join(required).exists() {
            return Err(provisioning_error(
                "resolve WordPress baseline",
                format!("Staged WordPress core is missing required path '{required}'."),
                "Restage the pinned WordPress archive and retry.",
            ));
        }
    }
    Ok(ResolvedWordPress {
        version: manifest.wordpress.version,
        core_root,
    })
}

fn validate_wordpress_manifest(
    manifest: &WordPressDevelopmentManifest,
) -> Result<(), RuntimeErrorInfo> {
    if manifest.schema_version != WORDPRESS_MANIFEST_SCHEMA_VERSION {
        return Err(provisioning_error(
            "validate WordPress manifest",
            format!(
                "Unsupported WordPress manifest schema {}.",
                manifest.schema_version
            ),
            "Use the manifest schema shipped with this CoffeePOS Desktop version.",
        ));
    }
    if manifest.target != expected_target()? {
        return Err(provisioning_error(
            "validate WordPress manifest",
            format!(
                "WordPress manifest target '{}' does not match '{}'.",
                manifest.target,
                expected_target()?
            ),
            "Stage the WordPress baseline for the current platform target.",
        ));
    }
    for (label, value) in [
        ("version", manifest.wordpress.version.as_str()),
        ("archive", manifest.wordpress.archive.as_str()),
        ("source", manifest.wordpress.source.as_str()),
        ("release_source", manifest.wordpress.release_source.as_str()),
        (
            "official_sha1_source",
            manifest.wordpress.official_sha1_source.as_str(),
        ),
        (
            "archive_sha256_source",
            manifest.wordpress.archive_sha256_source.as_str(),
        ),
        ("license", manifest.wordpress.license.as_str()),
        (
            "compatibility.source",
            manifest.compatibility.source.as_str(),
        ),
        (
            "compatibility.recommended_php",
            manifest.compatibility.recommended_php.as_str(),
        ),
        (
            "compatibility.recommended_mariadb",
            manifest.compatibility.recommended_mariadb.as_str(),
        ),
        (
            "compatibility.development_php",
            manifest.compatibility.development_php.as_str(),
        ),
        (
            "compatibility.development_mariadb",
            manifest.compatibility.development_mariadb.as_str(),
        ),
    ] {
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(provisioning_error(
                "validate WordPress manifest",
                format!("WordPress manifest field '{label}' is invalid."),
                "Restore the checked-in WordPress manifest template and restage the baseline.",
            ));
        }
    }
    if manifest.wordpress.archive_sha256.len() != 64
        || !manifest
            .wordpress
            .archive_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(provisioning_error(
            "validate WordPress manifest",
            "WordPress archive SHA256 must contain 64 hexadecimal characters.",
            "Restore the pinned checksum and restage the baseline.",
        ));
    }
    if manifest.wordpress.official_sha1.len() != 40
        || !manifest
            .wordpress
            .official_sha1
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(provisioning_error(
            "validate WordPress manifest",
            "WordPress official SHA1 must contain 40 hexadecimal characters.",
            "Restore the official checksum and restage the baseline.",
        ));
    }
    for relative in [
        &manifest.wordpress.core_root,
        &manifest.wordpress.license_file,
        &manifest.wordpress.readme_file,
        &manifest.wordpress.version_file,
    ] {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(provisioning_error(
                "validate WordPress manifest",
                "WordPress manifest paths must be relative and may not escape the target root.",
                "Restore the checked-in manifest template and restage the baseline.",
            ));
        }
    }
    Ok(())
}

fn expected_target() -> Result<&'static str, RuntimeErrorInfo> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return Ok("x86_64-pc-windows-msvc");
    }
    #[allow(unreachable_code)]
    Err(provisioning_error(
        "resolve target",
        "Phase 3 development provisioning is currently qualified only for Windows x64.",
        "Run provisioning on the supported Windows development target.",
    ))
}

fn provisioning_error(
    operation: impl Into<String>,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> RuntimeErrorInfo {
    RuntimeErrorInfo {
        component: "provisioning".into(),
        operation: operation.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

fn directory_is_empty_or_missing(path: &Path) -> bool {
    match fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn reset_owned_staging_dir(data_root: &Path, staging: &Path) -> Result<(), RuntimeErrorInfo> {
    let parent = staging.parent().ok_or_else(|| {
        provisioning_error(
            "reset staging",
            "Provisioning staging directory has no parent.",
            "Restart CoffeePOS Desktop and retry.",
        )
    })?;
    if parent != data_root
        || !matches!(
            staging.file_name().and_then(|value| value.to_str()),
            Some("database.provisioning" | "site.provisioning")
        )
    {
        return Err(provisioning_error(
            "reset staging",
            "Refusing to remove an unexpected provisioning path.",
            "Restart CoffeePOS Desktop and inspect the application-data path before retrying.",
        ));
    }
    if staging.exists() {
        fs::remove_dir_all(staging).map_err(|error| {
            provisioning_error(
                "reset staging",
                format!("Cannot remove owned provisioning staging directory: {error}."),
                "Close processes using the staging directory, then retry.",
            )
        })?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), RuntimeErrorInfo> {
    fs::create_dir_all(destination).map_err(|error| {
        provisioning_error(
            "copy WordPress core",
            format!("Cannot create WordPress staging directory: {error}."),
            "Check application-data permissions and free disk space, then retry.",
        )
    })?;
    for entry in fs::read_dir(source).map_err(|error| {
        provisioning_error(
            "copy WordPress core",
            format!("Cannot read staged WordPress baseline: {error}."),
            "Restage the verified WordPress archive and retry.",
        )
    })? {
        let entry = entry.map_err(|error| {
            provisioning_error(
                "copy WordPress core",
                format!("Cannot inspect WordPress baseline entry: {error}."),
                "Restage the verified WordPress archive and retry.",
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            provisioning_error(
                "copy WordPress core",
                format!("Cannot inspect WordPress baseline file type: {error}."),
                "Restage the verified WordPress archive and retry.",
            )
        })?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target).map_err(|error| {
                provisioning_error(
                    "copy WordPress core",
                    format!("Cannot copy WordPress baseline file: {error}."),
                    "Check application-data permissions/free disk space and retry.",
                )
            })?;
        } else {
            return Err(provisioning_error(
                "copy WordPress core",
                "Pinned WordPress baseline contains an unsupported filesystem entry.",
                "Restage the official WordPress archive; symlink/reparse entries are not accepted.",
            ));
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8], label: &str) -> Result<(), RuntimeErrorInfo> {
    let parent = path.parent().ok_or_else(|| {
        provisioning_error(
            format!("write {label}"),
            format!("{label} has no parent directory."),
            "Restart CoffeePOS Desktop and retry.",
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        provisioning_error(
            format!("write {label}"),
            format!("Cannot create parent directory for {label}: {error}."),
            "Check application-data permissions and retry.",
        )
    })?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        provisioning_error(
            format!("write {label}"),
            format!("Cannot create temporary {label}: {error}."),
            "Check application-data permissions and free disk space, then retry.",
        )
    })?;
    temporary
        .write_all(bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| {
            provisioning_error(
                format!("write {label}"),
                format!("Cannot write {label}: {error}."),
                "Check application-data storage health and retry.",
            )
        })?;
    temporary.persist(path).map_err(|error| {
        provisioning_error(
            format!("write {label}"),
            format!("Cannot replace {label}: {}.", error.error),
            "Preserve the existing file and retry after checking filesystem permissions.",
        )
    })?;
    Ok(())
}

fn write_managed_file(
    path: &Path,
    marker: &str,
    bytes: &[u8],
    label: &str,
) -> Result<(), RuntimeErrorInfo> {
    if path.is_file() {
        let existing = fs::read_to_string(path).map_err(|error| {
            provisioning_error(
                format!("read {label}"),
                format!("Cannot read existing {label}: {error}."),
                "Preserve the file and fix filesystem permissions before retrying.",
            )
        })?;
        if !existing.contains(marker) {
            return Err(provisioning_error(
                format!("write {label}"),
                format!("Existing {label} is not marked as CoffeePOS-managed."),
                "Preserve the file and explicitly adopt/repair the site before retrying.",
            ));
        }
    }
    atomic_write(path, bytes, label)
}

fn managed_config_exists(site: &Path) -> bool {
    fs::read_to_string(site.join("wp-config.php"))
        .map(|contents| contents.contains(MANAGED_CONFIG_MARKER))
        .unwrap_or(false)
}

fn sql_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "''")
}

fn generate_salts() -> Result<[String; 8], RuntimeErrorInfo> {
    let mut salts: [String; 8] = Default::default();
    for salt in &mut salts {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|error| {
            provisioning_error(
                "generate WordPress salts",
                format!("Cannot generate cryptographic random bytes: {error}."),
                "Check operating-system random generation and retry.",
            )
        })?;
        let mut value = String::with_capacity(64);
        for byte in random {
            use std::fmt::Write as _;
            write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
        }
        *salt = value;
    }
    Ok(salts)
}

fn provisioning_pipe_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    format!("CoffeePOSProvisioning_{}_{nanos}", std::process::id())
}

fn wordpress_http_ready(port: u16) -> bool {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let Ok(mut stream) = TcpStream::connect_timeout(&address.into(), Duration::from_millis(500))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!(
        "GET /wp-login.php?doing_wp_cron=coffeepos-health HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::new();
    let _ = stream.take(256 * 1024).read_to_end(&mut response);
    let text = String::from_utf8_lossy(&response);
    (text.starts_with("HTTP/1.0 200 ") || text.starts_with("HTTP/1.1 200 "))
        && text.contains("loginform")
}

const WORDPRESS_BOOTSTRAP: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
$storeName = getenv('COFFEEPOS_STORE_NAME');
$adminUser = getenv('COFFEEPOS_ADMIN_USER');
$adminEmail = getenv('COFFEEPOS_ADMIN_EMAIL');
$adminPassword = getenv('COFFEEPOS_ADMIN_PASSWORD');

if (!$siteRoot || !$storeName || !$adminUser || !$adminEmail || !$adminPassword) {
    fwrite(STDERR, "CoffeePOS WordPress bootstrap environment is incomplete.\n");
    exit(2);
}

define('WP_INSTALLING', true);
require_once $siteRoot . '/wp-load.php';
require_once ABSPATH . 'wp-admin/includes/upgrade.php';
require_once ABSPATH . 'wp-admin/includes/user.php';

global $wpdb;
$optionsTable = $wpdb->options;
$tableExists = $wpdb->get_var($wpdb->prepare('SHOW TABLES LIKE %s', $optionsTable)) === $optionsTable;
$installed = false;

if ($tableExists) {
    $siteUrl = $wpdb->get_var("SELECT option_value FROM {$optionsTable} WHERE option_name = 'siteurl' LIMIT 1");
    if (!$siteUrl) {
        fwrite(STDERR, "CoffeePOS detected a partial WordPress options table; refusing to reinstall.\n");
        exit(6);
    }
    $installed = true;
} else {
    $prefix = $wpdb->esc_like($wpdb->prefix) . '%';
    $existingTables = $wpdb->get_col($wpdb->prepare('SHOW TABLES LIKE %s', $prefix));
    if (!empty($existingTables)) {
        fwrite(STDERR, "CoffeePOS detected partial WordPress tables; refusing to overwrite them.\n");
        exit(7);
    }
}

if (!$installed) {
    $result = wp_install($storeName, $adminUser, $adminEmail, false, '', $adminPassword);
    if (is_wp_error($result)) {
        fwrite(STDERR, $result->get_error_message() . "\n");
        exit(3);
    }
} else {
    $existing = get_user_by('login', $adminUser);
    if (!$existing) {
        $userId = wp_create_user($adminUser, $adminPassword, $adminEmail);
        if (is_wp_error($userId)) {
            fwrite(STDERR, $userId->get_error_message() . "\n");
            exit(4);
        }
        $user = new WP_User($userId);
        $user->set_role('administrator');
    }
}

exit(get_option('siteurl') ? 0 : 5);
"#;

const WORDPRESS_ROUTER: &str = r#"<?php
// CoffeePOS Desktop managed router.
declare(strict_types=1);

$siteRoot = getcwd();
$uploadRoot = getenv('COFFEEPOS_UPLOAD_ROOT');
$requestPath = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH) ?: '/';
$requestPath = rawurldecode($requestPath);

if (str_contains($requestPath, "\0") || preg_match('#(?:^|/)\.\.(?:/|$)#', $requestPath)) {
    http_response_code(400);
    exit('Bad request');
}

$lower = strtolower($requestPath);
foreach (['/wp-config.php', '/.env', '/composer.json', '/composer.lock'] as $blocked) {
    if ($lower === $blocked) {
        http_response_code(404);
        exit('Not found');
    }
}

$uploadPrefix = '/wp-content/uploads/';
if ($uploadRoot && str_starts_with($requestPath, $uploadPrefix)) {
    $relative = substr($requestPath, strlen($uploadPrefix));
    $rootReal = realpath($uploadRoot);
    $fileReal = realpath($uploadRoot . DIRECTORY_SEPARATOR . str_replace('/', DIRECTORY_SEPARATOR, $relative));
    if (!$rootReal || !$fileReal || !str_starts_with($fileReal, $rootReal . DIRECTORY_SEPARATOR) || !is_file($fileReal)) {
        http_response_code(404);
        exit('Not found');
    }
    $mime = function_exists('mime_content_type') ? mime_content_type($fileReal) : false;
    header('Content-Type: ' . ($mime ?: 'application/octet-stream'));
    header('Content-Length: ' . filesize($fileReal));
    readfile($fileReal);
    return true;
}

$local = $siteRoot . DIRECTORY_SEPARATOR . ltrim(str_replace('/', DIRECTORY_SEPARATOR, $requestPath), DIRECTORY_SEPARATOR);
if ($requestPath !== '/' && (is_file($local) || is_dir($local))) {
    return false;
}

require $siteRoot . DIRECTORY_SEPARATOR . 'index.php';
return true;
"#;

const WORDPRESS_UPLOADS_MU_PLUGIN: &str = r#"<?php
/** CoffeePOS Desktop managed uploads bridge. */
if (!defined('ABSPATH')) {
    exit;
}

add_filter('upload_dir', static function (array $uploads): array {
    $root = getenv('COFFEEPOS_UPLOAD_ROOT');
    $origin = getenv('COFFEEPOS_SITE_URL');
    if (!$root || !$origin) {
        return $uploads;
    }
    $subdir = $uploads['subdir'] ?? '';
    $baseUrl = rtrim($origin, '/') . '/wp-content/uploads';
    $uploads['basedir'] = $root;
    $uploads['path'] = rtrim($root, '/\\') . $subdir;
    $uploads['baseurl'] = $baseUrl;
    $uploads['url'] = $baseUrl . $subdir;
    return $uploads;
});
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    use crate::runtime::{
        resolve_development_manifest, RuntimeManager, RuntimeState, WordPressHealthState,
    };

    #[test]
    fn sql_literal_escapes_quotes_and_backslashes() {
        assert_eq!(sql_literal("a'b\\c"), "a''b\\\\c");
    }

    #[test]
    fn staging_reset_refuses_unowned_path() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        assert!(reset_owned_staging_dir(data, &data.join("database")).is_err());
        assert!(reset_owned_staging_dir(data, &data.join("site.provisioning")).is_ok());
    }

    #[cfg(windows)]
    fn http_get(port: u16, path: &str) -> String {
        let mut stream = std::net::TcpStream::connect((LOOPBACK, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request =
            format!("GET {path} HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[cfg(windows)]
    #[test]
    fn checked_in_wordpress_manifest_matches_native_schema() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap();
        let bytes = fs::read(
            project_root.join("scripts/wordpress-development/wordpress-7.1.manifest.json"),
        )
        .unwrap();
        let manifest: WordPressDevelopmentManifest = serde_json::from_slice(&bytes).unwrap();
        validate_wordpress_manifest(&manifest).unwrap();
    }

    #[test]
    fn managed_runtime_files_use_environment_for_dynamic_values() {
        assert!(WORDPRESS_ROUTER.contains("COFFEEPOS_UPLOAD_ROOT"));
        assert!(WORDPRESS_ROUTER.contains("is_file($local) || is_dir($local)"));
        assert!(WORDPRESS_UPLOADS_MU_PLUGIN.contains("COFFEEPOS_SITE_URL"));
        assert!(WORDPRESS_BOOTSTRAP.contains("COFFEEPOS_ADMIN_PASSWORD"));
    }

    #[test]
    fn provisioning_info_serializes_ui_state_and_retry_contract() {
        let value = serde_json::to_value(ProvisioningInfo {
            state: ProvisioningState::NeedsRepair,
            wordpress_version: "7.1".into(),
            admin_username: Some(WORDPRESS_ADMIN_USER.into()),
            can_retry: false,
            last_error: Some(provisioning_error(
                "read journal",
                "Provisioning journal is invalid.",
                "Repair the journal before retrying.",
            )),
        })
        .unwrap();
        assert_eq!(value["state"], "needs_repair");
        assert_eq!(value["can_retry"], false);
        assert_eq!(value["last_error"]["operation"], "read journal");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "uses the staged real PHP/MariaDB/WordPress development runtime"]
    fn staged_runtime_provisions_twice_stops_and_cleans_temp_store() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap().to_path_buf();
        let runtime_manifest =
            project_root.join("runtime/development/x86_64-pc-windows-msvc/manifest.json");
        let runtime = resolve_development_manifest(&project_root, &runtime_manifest).unwrap();

        let e2e_root = manifest_dir.join("target/phase3-e2e");
        fs::create_dir_all(&e2e_root).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("store-")
            .tempdir_in(&e2e_root)
            .unwrap();
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();

        let mut provisioner = Provisioner::from_development(
            &project_root,
            &runtime_manifest,
            runtime.clone(),
            data_root.clone(),
        )
        .unwrap();
        assert_eq!(provisioner.inspect().state, ProvisioningState::NotInstalled);

        let prepared = provisioner.prepare().unwrap_or_else(|error| {
            let log = fs::read_to_string(data_root.join("logs/provisioning.log")).unwrap_or_else(
                |read_error| format!("<cannot read provisioning log: {read_error}>"),
            );
            panic!("provisioning prepare failed: {error}\n--- provisioning.log ---\n{log}");
        });
        assert_eq!(prepared.state, ProvisioningState::Installing);
        assert!(data_root.join("database/mysql").is_dir());
        assert!(data_root.join("site/wp-settings.php").is_file());
        assert!(!data_root.join("database.provisioning").exists());
        assert!(!data_root.join("site.provisioning").exists());
        assert!(!data_root.join(DATABASE_BOOTSTRAP_SECRET).exists());

        let mut manager = RuntimeManager::new(runtime, data_root.clone()).unwrap();
        let running = manager.start().unwrap();
        assert_eq!(running.state, RuntimeState::Running);

        let installed = provisioner
            .install_wordpress("CoffeePOS Phase 3 E2E", &running)
            .unwrap();
        assert_eq!(installed.state, ProvisioningState::Ready);
        assert_eq!(provisioner.inspect().state, ProvisioningState::Ready);

        let post_install_health = manager.refresh_wordpress_health();
        assert_eq!(
            post_install_health.wordpress_health,
            WordPressHealthState::Healthy
        );
        assert!(post_install_health.wordpress_error.is_none());

        let healthy = manager.restart().unwrap();
        assert_eq!(healthy.state, RuntimeState::Running);
        if healthy.wordpress_health != WordPressHealthState::Healthy {
            let php_log = fs::read_to_string(data_root.join("logs/php.log"))
                .unwrap_or_else(|error| format!("<cannot read php.log: {error}>"));
            let database_log = fs::read_to_string(data_root.join("logs/database.log"))
                .unwrap_or_else(|error| format!("<cannot read database.log: {error}>"));
            panic!(
                "WordPress health after restart was {:?}: {:?}\n--- php.log ---\n{}\n--- database.log ---\n{}",
                healthy.wordpress_health, healthy.wordpress_error, php_log, database_log
            );
        }
        assert!(healthy.wordpress_error.is_none());

        manager.kill_php_for_test();
        let child_failed = manager.refresh();
        assert_eq!(child_failed.state, RuntimeState::Stopped);
        assert_eq!(
            child_failed.wordpress_health,
            WordPressHealthState::Unavailable
        );
        assert!(child_failed.last_error.is_some());

        let recovered_after_child_exit = manager.start().unwrap();
        assert_eq!(recovered_after_child_exit.state, RuntimeState::Running);
        assert_eq!(
            recovered_after_child_exit.wordpress_health,
            WordPressHealthState::Healthy
        );

        manager.stop().unwrap();
        let original_php =
            manager.replace_php_executable_for_test(data_root.join("missing-phase4-2-php.exe"));
        assert!(manager.start().is_err());
        let failed_start = manager.info();
        assert_eq!(failed_start.state, RuntimeState::Stopped);
        assert_eq!(
            failed_start.wordpress_health,
            WordPressHealthState::Unavailable
        );
        manager.replace_php_executable_for_test(original_php);
        let retried_start = manager.start().unwrap();
        assert_eq!(retried_start.state, RuntimeState::Running);
        assert_eq!(
            retried_start.wordpress_health,
            WordPressHealthState::Healthy
        );

        let old_http_port = retried_start.http_port.unwrap();
        assert_eq!(
            manager.wordpress_url().unwrap(),
            format!("http://{LOOPBACK}:{old_http_port}/")
        );
        manager.stop().unwrap();
        assert!(manager.wordpress_url().is_err());
        let occupied_old_http = std::net::TcpListener::bind((LOOPBACK, old_http_port)).unwrap();
        let moved_runtime = manager.start().unwrap();
        let moved_http_port = moved_runtime.http_port.unwrap();
        assert_ne!(moved_http_port, old_http_port);
        assert_eq!(
            moved_runtime.wordpress_health,
            WordPressHealthState::Healthy
        );
        assert_eq!(
            manager.wordpress_url().unwrap(),
            format!("http://{LOOPBACK}:{moved_http_port}/")
        );
        let admin_response = http_get(moved_http_port, "/wp-admin/");
        assert!(
            admin_response.contains(&format!(
                "Location: http://{LOOPBACK}:{moved_http_port}/wp-login.php"
            )),
            "unexpected /wp-admin/ response after moving HTTP port:\n{admin_response}"
        );
        assert!(!admin_response.contains(&format!(
            "Location: http://{LOOPBACK}:{old_http_port}/wp-login.php"
        )));
        let login_response = http_get(moved_http_port, "/wp-login.php");
        assert!(login_response.contains(&format!("http://{LOOPBACK}:{moved_http_port}/")));
        assert!(!login_response.contains(&format!("http://{LOOPBACK}:{old_http_port}/")));
        assert!(
            login_response.contains(&format!("http://{LOOPBACK}:{moved_http_port}/wp-includes/"))
        );
        let static_asset_response = http_get(moved_http_port, "/wp-includes/css/dashicons.min.css");
        assert!(
            static_asset_response.starts_with("HTTP/1.0 200 ")
                || static_asset_response.starts_with("HTTP/1.1 200 "),
            "WordPress static asset did not return HTTP 200 on the moved port"
        );
        drop(occupied_old_http);

        let sentinel = data_root.join("site/wp-content/coffeepos-phase3-e2e-sentinel.txt");
        fs::write(&sentinel, b"preserve me").unwrap();

        let stopped = manager.stop().unwrap();
        assert_eq!(stopped.state, RuntimeState::Stopped);
        assert_eq!(stopped.wordpress_health, WordPressHealthState::Unavailable);
        assert!(stopped.database_pid.is_none());
        assert!(stopped.php_pid.is_none());
        assert!(stopped.database_port.is_none());
        assert!(stopped.http_port.is_none());

        let prepared_again = provisioner.prepare().unwrap();
        assert_eq!(prepared_again.state, ProvisioningState::Installing);
        assert!(sentinel.is_file());

        let running_again = manager.start().unwrap();
        assert_eq!(running_again.state, RuntimeState::Running);
        assert_eq!(
            running_again.wordpress_health,
            WordPressHealthState::Healthy
        );
        let installed_again = provisioner
            .install_wordpress("CoffeePOS Phase 3 E2E", &running_again)
            .unwrap();
        assert_eq!(installed_again.state, ProvisioningState::Ready);
        assert!(sentinel.is_file());

        let stopped_again = manager.stop().unwrap();
        assert_eq!(stopped_again.state, RuntimeState::Stopped);
        assert!(stopped_again.database_pid.is_none());
        assert!(stopped_again.php_pid.is_none());
        assert!(!data_root.join("database/mariadb-provisioning.pid").exists());
        assert!(!data_root.join("database/mariadb.pid").exists());
        assert!(!fs::read_dir(data_root.join("site"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".coffeepos-runtime-health-")));

        drop(manager);
        drop(provisioner);
        temp.close().unwrap();
        fs::remove_dir(&e2e_root).unwrap();
    }
}
