use crate::runtime::{
    choose_loopback_port, configure_child_command, probe_coffeepos_health,
    probe_coffeepos_health_with_token, run_command_bounded, wait_for_child_exit,
    CoffeePosHealthState, ProcessContainment, ResolvedRuntime, RuntimeErrorInfo, RuntimeInfo,
    DATABASE_NAME, DATABASE_RUNTIME_SECRET, DATABASE_RUNTIME_USER, DATABASE_WORDPRESS_SECRET,
    DATABASE_WORDPRESS_USER, MACHINE_TOKEN_PENDING_SECRET, MACHINE_TOKEN_SECRET,
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
const WOOCOMMERCE_MANIFEST_SCHEMA_VERSION: u32 = 1;
const WOOCOMMERCE_OWNERSHIP_SCHEMA_VERSION: u32 = 1;
const WOOCOMMERCE_PLUGIN_SLUG: &str = "woocommerce";
const COFFEEPOS_MANIFEST_SCHEMA_VERSION: u32 = 1;
const COFFEEPOS_OWNERSHIP_SCHEMA_VERSION: u32 = 1;
const COFFEEPOS_PLUGIN_SLUG: &str = "coffeepos";
const COFFEEPOS_PHASE_4_9_VERSION: &str = "1.0.0";
const COFFEEPOS_PHASE_4_9_SHA256: &str =
    "ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a";
const COFFEEPOS_PHASE_4_10_VERSION: &str = "1.0.1";
const COFFEEPOS_PHASE_4_10_SHA256: &str =
    "67e3f268ffd29946cfb4fdce13d6e7ad12caaf3ca7af007cff2177940d8e4a64";
const COFFEEPOS_UPGRADE_BACKUP: &str = "coffeepos.previous";
const MANAGED_PLUGIN_OWNERSHIP_FILE: &str = ".coffeepos-managed.json";
const COFFEEPOS_REQUIRED_FILES: [&str; 4] = [
    "coffeepos.php",
    "readme.txt",
    "LICENSE",
    "vendor/autoload.php",
];
const COFFEEPOS_REQUIRED_DIRECTORIES: [&str; 5] =
    ["assets", "includes", "languages", "templates", "vendor"];
const WOOCOMMERCE_DB_VERSION: &str = "11.1.0-1";

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
    pub woocommerce_version: String,
    pub woocommerce_active: bool,
    pub coffeepos_version: String,
    pub coffeepos_active: bool,
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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WooCommerceDevelopmentManifest {
    schema_version: u32,
    target: String,
    woocommerce: WooCommerceArtifactManifest,
    compatibility: WooCommerceCompatibilityManifest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WooCommerceArtifactManifest {
    version: String,
    archive: String,
    source: String,
    release_source: String,
    plugin_directory_source: String,
    archive_sha256: String,
    archive_sha256_source: String,
    license: String,
    plugin_root: PathBuf,
    entry_file: PathBuf,
    license_file: PathBuf,
    readme_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WooCommerceCompatibilityManifest {
    plugin_metadata_source: String,
    server_requirements_source: String,
    minimum_wordpress: String,
    tested_wordpress: String,
    minimum_php: String,
    recommended_php: String,
    recommended_mariadb: String,
    development_wordpress: String,
    development_php: String,
    development_mariadb: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoffeePosDevelopmentManifest {
    schema_version: u32,
    target: String,
    coffeepos: CoffeePosArtifactManifest,
    compatibility: CoffeePosCompatibilityManifest,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoffeePosArtifactManifest {
    version: String,
    archive: String,
    archive_sha256: String,
    archive_sha256_source: String,
    source_kind: String,
    source_git_commit: String,
    build_script: String,
    composer_version: String,
    build_php_version: String,
    license: String,
    requires_plugin: String,
    plugin_root: PathBuf,
    entry_file: PathBuf,
    readme_file: PathBuf,
    license_file: PathBuf,
    autoload_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoffeePosCompatibilityManifest {
    minimum_wordpress: String,
    tested_wordpress: String,
    minimum_php: String,
    development_wordpress: String,
    development_php: String,
    development_mariadb: String,
    development_woocommerce: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedWordPress {
    pub version: String,
    pub core_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ResolvedWooCommerce {
    pub version: String,
    pub plugin_root: PathBuf,
    pub archive_sha256: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedCoffeePos {
    pub version: String,
    pub plugin_root: PathBuf,
    pub archive_sha256: String,
    pub required_wordpress_version: String,
    pub required_php_version: String,
    pub required_mariadb_version: String,
    pub required_woocommerce_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum ProvisioningStage {
    DatabaseReady,
    SiteReady,
    WordPressInstalled,
    WooCommerceProvisioned,
    WooCommerceActivated,
    CoffeePosProvisioned,
    CoffeePosActivated,
    MachineHealthBootstrapped,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProvisioningRecoveryBlocker {
    PartialWordPressInstall,
}

impl ProvisioningRecoveryBlocker {
    fn error(&self) -> RuntimeErrorInfo {
        match self {
            Self::PartialWordPressInstall => provisioning_error(
                "recover partial WordPress install",
                "WordPress has partial database tables from an interrupted install, so automatic provisioning cannot safely decide which data is authoritative.",
                "Preserve the database and site. Repair or restore the partial WordPress schema explicitly, then recheck provisioning state; CoffeePOS Desktop will not delete or reinstall over these tables automatically.",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProvisioningBoundary {
    DatabaseReady,
    SiteReady,
    WordPressInstalled,
    WooCommerceProvisioned,
    WooCommerceActivated,
    CoffeePosProvisioned,
    CoffeePosActivated,
    MachineHealthBootstrapped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvisioningJournal {
    schema_version: u32,
    wordpress_version: String,
    #[serde(default)]
    woocommerce_version: Option<String>,
    #[serde(default)]
    coffeepos_version: Option<String>,
    stage: ProvisioningStage,
    admin_username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_blocker: Option<ProvisioningRecoveryBlocker>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ManagedPluginOwnership {
    schema_version: u32,
    plugin: String,
    version: String,
    archive_sha256: String,
}

pub struct Provisioner {
    runtime: ResolvedRuntime,
    wordpress: ResolvedWordPress,
    woocommerce: ResolvedWooCommerce,
    coffeepos: ResolvedCoffeePos,
    data_root: PathBuf,
    containment: ProcessContainment,
    #[cfg(test)]
    failure_after: Option<ProvisioningBoundary>,
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
        let wordpress = resolve_development_wordpress(project_root, runtime_manifest_path)?;
        let woocommerce = resolve_development_woocommerce(project_root, runtime_manifest_path)?;
        let coffeepos = resolve_development_coffeepos(project_root, runtime_manifest_path)?;
        if coffeepos.required_wordpress_version != wordpress.version
            || coffeepos.required_php_version != runtime.php_version
            || coffeepos.required_mariadb_version != runtime.mariadb_version
            || coffeepos.required_woocommerce_version != woocommerce.version
        {
            return Err(provisioning_error(
                "resolve CoffeePOS baseline",
                format!(
                    "CoffeePOS {} compatibility baseline does not match the staged stack (WordPress {}, PHP {}, MariaDB {}, WooCommerce {}).",
                    coffeepos.version,
                    wordpress.version,
                    runtime.php_version,
                    runtime.mariadb_version,
                    woocommerce.version
                ),
                "Restage the pinned runtime, WordPress, WooCommerce, and CoffeePOS artifacts from compatible manifests before provisioning.",
            ));
        }
        Ok(Self {
            runtime,
            wordpress,
            woocommerce,
            coffeepos,
            data_root,
            containment: ProcessContainment::new()?,
            #[cfg(test)]
            failure_after: None,
        })
    }

    #[cfg(test)]
    fn fail_after_for_test(&mut self, boundary: ProvisioningBoundary) {
        self.failure_after = Some(boundary);
    }

    fn interruption_checkpoint(
        &mut self,
        boundary: ProvisioningBoundary,
    ) -> Result<(), RuntimeErrorInfo> {
        #[cfg(test)]
        {
            if self.failure_after == Some(boundary) {
                self.failure_after = None;
                return Err(provisioning_error(
                    "simulate provisioning interruption",
                    format!(
                        "Injected interruption after {boundary:?} completed and before its journal commit."
                    ),
                    "Recreate the provisioning/runtime state and retry from the persisted journal.",
                ));
            }
        }
        #[cfg(not(test))]
        let _ = boundary;
        Ok(())
    }

    fn recovery_blocker(&self) -> Result<Option<RuntimeErrorInfo>, RuntimeErrorInfo> {
        Ok(self
            .load_journal()?
            .and_then(|journal| journal.recovery_blocker)
            .map(|blocker| blocker.error()))
    }

    #[cfg(test)]
    fn replace_php_executable_for_test(&mut self, executable: PathBuf) -> PathBuf {
        std::mem::replace(&mut self.runtime.php_executable, executable)
    }

    pub fn inspect(&self) -> ProvisioningInfo {
        let journal = match self.load_journal() {
            Ok(journal) => journal,
            Err(error) => {
                return ProvisioningInfo {
                    state: ProvisioningState::NeedsRepair,
                    wordpress_version: self.wordpress.version.clone(),
                    woocommerce_version: self.woocommerce.version.clone(),
                    woocommerce_active: false,
                    coffeepos_version: self.coffeepos.version.clone(),
                    coffeepos_active: false,
                    admin_username: None,
                    can_retry: false,
                    last_error: Some(error),
                };
            }
        };
        let runtime_database_credential_ready =
            secret::load(&self.data_root.join(DATABASE_RUNTIME_SECRET))
                .map(|value| !value.is_empty())
                .unwrap_or(false);
        let wordpress_database_credential_ready =
            secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
                .map(|value| !value.is_empty())
                .unwrap_or(false);
        let wordpress_admin_credential_ready =
            secret::load(&self.data_root.join(WORDPRESS_ADMIN_SECRET))
                .map(|value| !value.is_empty())
                .unwrap_or(false);
        let database_ready = self.data_root.join("database/mysql").is_dir()
            && runtime_database_credential_ready
            && wordpress_database_credential_ready;
        let site_ready = self.data_root.join("site/wp-settings.php").is_file()
            && self.data_root.join("site/wp-config.php").is_file()
            && self.data_root.join("config/wordpress-router.php").is_file();
        let woocommerce_ready = woocommerce_installation_ready(&self.data_root, &self.woocommerce);
        let coffeepos_ready = coffeepos_installation_ready(&self.data_root, &self.coffeepos);
        let machine_token_ready = self.data_root.join(MACHINE_TOKEN_SECRET).is_file()
            && secret::load(&self.data_root.join(MACHINE_TOKEN_SECRET))
                .map(|token| {
                    token.len() == 64
                        && token
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                })
                .unwrap_or(false);
        let recovery_blocker = journal
            .as_ref()
            .and_then(|value| value.recovery_blocker.as_ref())
            .map(ProvisioningRecoveryBlocker::error);
        let woocommerce_activated = journal
            .as_ref()
            .map(|value| value.stage >= ProvisioningStage::WooCommerceActivated)
            .unwrap_or(false);
        let coffeepos_activated = journal
            .as_ref()
            .map(|value| value.stage >= ProvisioningStage::CoffeePosActivated)
            .unwrap_or(false);
        let complete = journal
            .as_ref()
            .map(|value| {
                value.schema_version == PROVISIONING_SCHEMA_VERSION
                    && value.wordpress_version == self.wordpress.version
                    && value.woocommerce_version.as_deref()
                        == Some(self.woocommerce.version.as_str())
                    && value.coffeepos_version.as_deref() == Some(self.coffeepos.version.as_str())
                    && value.stage >= ProvisioningStage::MachineHealthBootstrapped
            })
            .unwrap_or(false)
            && recovery_blocker.is_none()
            && database_ready
            && site_ready
            && wordpress_admin_credential_ready
            && woocommerce_ready
            && coffeepos_ready
            && machine_token_ready;
        if complete {
            return ProvisioningInfo {
                state: ProvisioningState::Ready,
                wordpress_version: self.wordpress.version.clone(),
                woocommerce_version: self.woocommerce.version.clone(),
                woocommerce_active: true,
                coffeepos_version: self.coffeepos.version.clone(),
                coffeepos_active: true,
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
        let database_credential_broken = journal
            .as_ref()
            .map(|value| value.stage >= ProvisioningStage::DatabaseReady)
            .unwrap_or(false)
            && (!runtime_database_credential_ready || !wordpress_database_credential_ready);
        let wordpress_admin_credential_broken = journal
            .as_ref()
            .map(|value| value.stage >= ProvisioningStage::WordPressInstalled)
            .unwrap_or(false)
            && !wordpress_admin_credential_ready;
        let machine_credential_broken = journal
            .as_ref()
            .map(|value| value.stage >= ProvisioningStage::MachineHealthBootstrapped)
            .unwrap_or(false)
            && !machine_token_ready;
        let last_error = if pristine {
            None
        } else if let Some(error) = recovery_blocker.clone() {
            Some(error)
        } else if database_credential_broken {
            Some(provisioning_error(
                "inspect database credentials",
                "Provisioning journal says the database is ready, but one or more protected database credentials are missing or unreadable.",
                "Preserve the database and restore the matching protected runtime/WordPress database credentials. Normal provisioning will not create replacement credentials for an installed store.",
            ))
        } else if wordpress_admin_credential_broken {
            Some(provisioning_error(
                "inspect WordPress administrator credential",
                "Provisioning journal says WordPress is installed, but the protected administrator credential is missing or unreadable.",
                "Preserve the store and restore the matching protected administrator credential. Normal provisioning will not generate a replacement that could diverge from the existing WordPress password.",
            ))
        } else if machine_credential_broken {
            Some(provisioning_error(
                "inspect machine credential",
                "Provisioning journal says CoffeePOS machine health was bootstrapped, but the protected active machine credential is missing or unreadable.",
                "Preserve the store and use the explicit machine-credential repair/rotation flow. Normal provisioning will not create a replacement credential implicitly.",
            ))
        } else {
            Some(provisioning_error(
                "inspect store",
                "WordPress/WooCommerce/CoffeePOS provisioning is incomplete or the managed store layout is inconsistent.",
                "Retry provisioning. Existing site/database/plugin data is preserved; if retry is refused, use an explicit repair flow instead of deleting store data.",
            ))
        };
        ProvisioningInfo {
            state,
            wordpress_version: self.wordpress.version.clone(),
            woocommerce_version: self.woocommerce.version.clone(),
            woocommerce_active: woocommerce_activated,
            coffeepos_version: self.coffeepos.version.clone(),
            coffeepos_active: coffeepos_activated,
            admin_username: journal.map(|value| value.admin_username),
            can_retry: recovery_blocker.is_none()
                && !(database_credential_broken
                    || wordpress_admin_credential_broken
                    || machine_credential_broken),
            last_error,
        }
    }

    pub fn installing_info(&self) -> ProvisioningInfo {
        ProvisioningInfo {
            state: ProvisioningState::Installing,
            wordpress_version: self.wordpress.version.clone(),
            woocommerce_version: self.woocommerce.version.clone(),
            woocommerce_active: false,
            coffeepos_version: self.coffeepos.version.clone(),
            coffeepos_active: false,
            admin_username: Some(WORDPRESS_ADMIN_USER.into()),
            can_retry: false,
            last_error: None,
        }
    }

    pub fn prepare(&mut self) -> Result<ProvisioningInfo, RuntimeErrorInfo> {
        self.prepare_logs()?;
        self.log_event("provisioning prepare requested");
        if let Some(error) = self.recovery_blocker()? {
            return Err(error);
        }
        self.ensure_database_initialized()?;
        self.ensure_database_accounts()?;
        self.interruption_checkpoint(ProvisioningBoundary::DatabaseReady)?;
        self.persist_stage(ProvisioningStage::DatabaseReady)?;
        self.ensure_wordpress_site()?;
        self.interruption_checkpoint(ProvisioningBoundary::SiteReady)?;
        self.persist_stage(ProvisioningStage::SiteReady)?;
        self.log_event("provisioning site prepared");
        Ok(ProvisioningInfo {
            state: ProvisioningState::Installing,
            wordpress_version: self.wordpress.version.clone(),
            woocommerce_version: self.woocommerce.version.clone(),
            woocommerce_active: false,
            coffeepos_version: self.coffeepos.version.clone(),
            coffeepos_active: false,
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
        let wordpress_already_installed = self
            .load_journal()?
            .map(|journal| journal.stage >= ProvisioningStage::WordPressInstalled)
            .unwrap_or(false);
        let admin_password = load_or_create_wordpress_admin_secret(
            &self.data_root.join(WORDPRESS_ADMIN_SECRET),
            wordpress_already_installed,
        )
        .map_err(|error| {
            provisioning_error(
                if wordpress_already_installed {
                    "load WordPress administrator credential"
                } else {
                    "create WordPress administrator credential"
                },
                error,
                if wordpress_already_installed {
                    "Restore the matching protected administrator credential for this installed WordPress store. Provisioning will not generate a replacement password implicitly."
                } else {
                    "Check application-data permissions and Windows DPAPI, then retry."
                },
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
            if matches!(status.code(), Some(6 | 7)) {
                let blocker = ProvisioningRecoveryBlocker::PartialWordPressInstall;
                self.persist_recovery_blocker(blocker.clone())?;
                return Err(blocker.error());
            }
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
        self.interruption_checkpoint(ProvisioningBoundary::WordPressInstalled)?;
        self.persist_stage(ProvisioningStage::WordPressInstalled)?;
        ensure_woocommerce_plugin(&self.data_root, &self.woocommerce)?;
        self.interruption_checkpoint(ProvisioningBoundary::WooCommerceProvisioned)?;
        self.persist_stage(ProvisioningStage::WooCommerceProvisioned)?;
        self.log_event("woocommerce plugin provisioned");
        self.activate_woocommerce(runtime_info)?;
        self.interruption_checkpoint(ProvisioningBoundary::WooCommerceActivated)?;
        self.persist_stage(ProvisioningStage::WooCommerceActivated)?;
        self.log_event("woocommerce plugin activated and verified");
        ensure_coffeepos_plugin(&self.data_root, &self.coffeepos, &self.woocommerce)?;
        self.interruption_checkpoint(ProvisioningBoundary::CoffeePosProvisioned)?;
        self.persist_stage(ProvisioningStage::CoffeePosProvisioned)?;
        self.log_event("coffeepos plugin provisioned");
        self.activate_coffeepos(runtime_info)?;
        self.interruption_checkpoint(ProvisioningBoundary::CoffeePosActivated)?;
        self.persist_stage(ProvisioningStage::CoffeePosActivated)?;
        self.log_event("coffeepos plugin activated and verified");
        self.bootstrap_machine_health(runtime_info)?;
        self.interruption_checkpoint(ProvisioningBoundary::MachineHealthBootstrapped)?;
        self.persist_stage(ProvisioningStage::MachineHealthBootstrapped)?;
        self.log_event("coffeepos machine health credential bootstrapped and verified");
        self.log_event("wordpress + woocommerce + coffeepos provisioning ready");
        Ok(ProvisioningInfo {
            state: ProvisioningState::Ready,
            wordpress_version: self.wordpress.version.clone(),
            woocommerce_version: self.woocommerce.version.clone(),
            woocommerce_active: true,
            coffeepos_version: self.coffeepos.version.clone(),
            coffeepos_active: true,
            admin_username: Some(WORDPRESS_ADMIN_USER.into()),
            can_retry: false,
            last_error: None,
        })
    }

    fn activate_woocommerce(&self, runtime_info: &RuntimeInfo) -> Result<(), RuntimeErrorInfo> {
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "activate WooCommerce",
                "MariaDB port is unavailable while activating WooCommerce.",
                "Keep the provisioned WordPress/plugin files, restart provisioning, and inspect runtime logs if MariaDB is not ready.",
            )
        })?;
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "activate WooCommerce",
                "HTTP port is unavailable while activating WooCommerce.",
                "Keep the provisioned WordPress/plugin files, restart provisioning, and inspect runtime logs if PHP is not ready.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "activate WooCommerce",
                    error,
                    "Retry with the same Windows user profile. Existing WordPress/WooCommerce files and database data are preserved.",
                )
            })?;
        let apply_baseline = self
            .load_journal()?
            .map(|journal| journal.stage < ProvisioningStage::WooCommerceActivated)
            .unwrap_or(true);
        let script = self.write_woocommerce_activation_script()?;
        let log_path = self.data_root.join("logs/woocommerce.log");
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| {
                provisioning_error(
                    "activate WooCommerce",
                    format!("Cannot open WooCommerce activation log: {error}."),
                    "Check application-data permissions and retry.",
                )
            })?;
        let mut command = Command::new(&self.runtime.php_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg(&script)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_DB_PASSWORD", &database_password)
            .env(
                "COFFEEPOS_SITE_URL",
                format!("http://{LOOPBACK}:{http_port}"),
            )
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env("COFFEEPOS_SITE_ROOT", self.data_root.join("site"))
            .env("COFFEEPOS_WOOCOMMERCE_VERSION", &self.woocommerce.version)
            .env("COFFEEPOS_WOOCOMMERCE_DB_VERSION", WOOCOMMERCE_DB_VERSION)
            .env(
                "COFFEEPOS_WOOCOMMERCE_APPLY_BASELINE",
                if apply_baseline { "1" } else { "0" },
            )
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|error| {
                provisioning_error(
                    "activate WooCommerce",
                    format!("Cannot duplicate WooCommerce log handle: {error}."),
                    "Check application-data permissions and retry.",
                )
            })?))
            .stderr(Stdio::from(log))
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let status = run_command_bounded(
            command,
            Duration::from_secs(90),
            "woocommerce",
            "activate and verify",
            &self.containment,
        );
        let _ = fs::remove_file(&script);
        let status = status?;
        if !status.success() {
            return Err(provisioning_error(
                "activate WooCommerce",
                format!("WooCommerce activation/setup exited with status {status}."),
                "Inspect logs/woocommerce.log and retry provisioning. WordPress, WooCommerce files, and existing database data are preserved.",
            ));
        }
        Ok(())
    }

    fn activate_coffeepos(&self, runtime_info: &RuntimeInfo) -> Result<(), RuntimeErrorInfo> {
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "activate CoffeePOS",
                "MariaDB port is unavailable while activating CoffeePOS.",
                "Keep the provisioned WordPress/plugin files, restart provisioning, and inspect runtime logs if MariaDB is not ready.",
            )
        })?;
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "activate CoffeePOS",
                "HTTP port is unavailable while activating CoffeePOS.",
                "Keep the provisioned WordPress/plugin files, restart provisioning, and inspect runtime logs if PHP is not ready.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "activate CoffeePOS",
                    error,
                    "Retry with the same Windows user profile. Existing WordPress/WooCommerce/CoffeePOS files and database data are preserved.",
                )
            })?;
        let apply_baseline = self
            .load_journal()?
            .map(|journal| {
                journal.stage < ProvisioningStage::CoffeePosActivated
                    || journal.coffeepos_version.as_deref() != Some(self.coffeepos.version.as_str())
            })
            .unwrap_or(true);
        let script = self.write_coffeepos_activation_script()?;
        let log_path = self.data_root.join("logs/coffeepos.log");
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| {
                provisioning_error(
                    "activate CoffeePOS",
                    format!("Cannot open CoffeePOS activation log: {error}."),
                    "Check application-data permissions and retry.",
                )
            })?;
        let mut command = Command::new(&self.runtime.php_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg(&script)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_DB_PASSWORD", &database_password)
            .env(
                "COFFEEPOS_SITE_URL",
                format!("http://{LOOPBACK}:{http_port}"),
            )
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env("COFFEEPOS_SITE_ROOT", self.data_root.join("site"))
            .env("COFFEEPOS_EXPECTED_VERSION", &self.coffeepos.version)
            .env(
                "COFFEEPOS_EXPECTED_WOOCOMMERCE_VERSION",
                &self.woocommerce.version,
            )
            .env(
                "COFFEEPOS_APPLY_ACTIVATION_BASELINE",
                if apply_baseline { "1" } else { "0" },
            )
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|error| {
                provisioning_error(
                    "activate CoffeePOS",
                    format!("Cannot duplicate CoffeePOS log handle: {error}."),
                    "Check application-data permissions and retry.",
                )
            })?))
            .stderr(Stdio::from(log))
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let status = run_command_bounded(
            command,
            Duration::from_secs(60),
            "coffeepos",
            "activate lifecycle",
            &self.containment,
        );
        let _ = fs::remove_file(&script);
        let status = status?;
        if !status.success() {
            return Err(provisioning_error(
                "activate CoffeePOS",
                format!("CoffeePOS activation lifecycle exited with status {status}."),
                "Inspect logs/coffeepos.log and retry provisioning. WordPress, WooCommerce, CoffeePOS files, and existing database data are preserved.",
            ));
        }

        let verification_script = self.write_coffeepos_activation_verification_script()?;
        let verification_log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| {
                provisioning_error(
                    "verify CoffeePOS activation",
                    format!("Cannot reopen CoffeePOS activation log: {error}."),
                    "Check application-data permissions and retry.",
                )
            })?;
        let mut verification_command = Command::new(&self.runtime.php_executable);
        verification_command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg(&verification_script)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_DB_PASSWORD", database_password)
            .env(
                "COFFEEPOS_SITE_URL",
                format!("http://{LOOPBACK}:{http_port}"),
            )
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env("COFFEEPOS_SITE_ROOT", self.data_root.join("site"))
            .env("COFFEEPOS_EXPECTED_VERSION", &self.coffeepos.version)
            .env(
                "COFFEEPOS_EXPECTED_WOOCOMMERCE_VERSION",
                &self.woocommerce.version,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::from(verification_log.try_clone().map_err(
                |error| {
                    provisioning_error(
                        "verify CoffeePOS activation",
                        format!("Cannot duplicate CoffeePOS verification log handle: {error}."),
                        "Check application-data permissions and retry.",
                    )
                },
            )?))
            .stderr(Stdio::from(verification_log))
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut verification_command);
        let verification_status = run_command_bounded(
            verification_command,
            Duration::from_secs(60),
            "coffeepos",
            "verify activation baseline",
            &self.containment,
        );
        let _ = fs::remove_file(&verification_script);
        let verification_status = verification_status?;
        if !verification_status.success() {
            return Err(provisioning_error(
                "verify CoffeePOS activation",
                format!(
                    "CoffeePOS fresh-process activation verification exited with status {verification_status}."
                ),
                "Inspect logs/coffeepos.log and retry provisioning. The journal remains at the provisioned stage until the plugin-owned baseline verifies successfully.",
            ));
        }
        Ok(())
    }

    fn bootstrap_machine_health(&self, runtime_info: &RuntimeInfo) -> Result<(), RuntimeErrorInfo> {
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "bootstrap CoffeePOS machine health",
                "HTTP port is unavailable while bootstrapping the CoffeePOS machine credential.",
                "Keep the installed store, restart provisioning, and inspect runtime logs if PHP is not ready.",
            )
        })?;
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "bootstrap CoffeePOS machine health",
                "MariaDB port is unavailable while bootstrapping the CoffeePOS machine credential.",
                "Keep the installed store, restart provisioning, and inspect runtime logs if MariaDB is not ready.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "bootstrap CoffeePOS machine health",
                    error,
                    "Retry with the same Windows user profile. Existing store data and plugin files are preserved.",
                )
            })?;
        let pending_token_path = self.data_root.join(MACHINE_TOKEN_PENDING_SECRET);
        if pending_token_path.is_file() {
            let recovered = probe_coffeepos_health(&self.data_root, http_port);
            if pending_token_path.is_file()
                || !matches!(
                    recovered.state,
                    CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
                )
            {
                return Err(provisioning_error(
                    "recover CoffeePOS machine credential",
                    "A pending CoffeePOS machine credential remains after crash recovery could not identify an accepted credential.",
                    "Preserve the active and pending protected credentials and use explicit machine-credential repair. Provisioning will not guess or reset the server hash.",
                ));
            }
        }
        let token_path = self.data_root.join(MACHINE_TOKEN_SECRET);
        let token = secret::create_machine_token(&token_path).map_err(|error| {
            provisioning_error(
                "bootstrap CoffeePOS machine health",
                error,
                "Check protected application-data storage and operating-system random generation, then retry.",
            )
        })?;
        let script = self.write_machine_health_bootstrap_script()?;
        let log_path = self.data_root.join("logs/coffeepos.log");
        let log = match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(log) => log,
            Err(error) => {
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "bootstrap CoffeePOS machine health",
                    format!("Cannot open CoffeePOS log: {error}."),
                    "Check application-data permissions and retry.",
                ));
            }
        };
        let stdout_log = match log.try_clone() {
            Ok(log) => log,
            Err(error) => {
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "bootstrap CoffeePOS machine health",
                    format!("Cannot duplicate CoffeePOS log handle: {error}."),
                    "Check application-data permissions and retry.",
                ));
            }
        };
        let mut command = Command::new(&self.runtime.php_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg(&script)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_DB_PASSWORD", database_password)
            .env(
                "COFFEEPOS_SITE_URL",
                format!("http://{LOOPBACK}:{http_port}"),
            )
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env("COFFEEPOS_SITE_ROOT", self.data_root.join("site"))
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout_log))
            .stderr(Stdio::from(log))
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "bootstrap CoffeePOS machine health",
                    format!("Cannot start pinned PHP for machine-token bootstrap: {error}."),
                    "Verify the pinned PHP runtime and retry.",
                ));
            }
        };
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(error);
        }
        let stdin_result = match child.stdin.take() {
            Some(mut stdin) => stdin.write_all(token.as_bytes()),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pinned PHP stdin pipe is unavailable",
            )),
        };
        if let Err(error) = stdin_result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(provisioning_error(
                "bootstrap CoffeePOS machine health",
                format!("Cannot pass the machine credential to pinned PHP over stdin: {error}."),
                "Retry provisioning. The protected credential is preserved and is never placed in process arguments or logs.",
            ));
        }
        let status = match wait_for_child_exit(&mut child, Duration::from_secs(30)) {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "bootstrap CoffeePOS machine health",
                    format!("Machine-token bootstrap exceeded its bounded timeout: {}.", error.message),
                    "The pinned PHP bootstrap was terminated. The protected token is preserved because the WordPress hash may already have committed; inspect logs/coffeepos.log and retry.",
                ));
            }
        };
        let _ = fs::remove_file(&script);
        if !status.success() {
            return Err(provisioning_error(
                "bootstrap CoffeePOS machine health",
                format!("Machine-token bootstrap exited with status {status}."),
                "Inspect logs/coffeepos.log and retry. The protected token is preserved for the same store.",
            ));
        }

        let health = probe_coffeepos_health(&self.data_root, http_port);
        if !matches!(
            health.state,
            CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
        ) {
            let details = health
                .error
                .map(|error| format!("{} {}", error.message, error.recovery))
                .unwrap_or_else(|| "CoffeePOS machine-health probe is unavailable.".into());
            return Err(provisioning_error(
                "verify CoffeePOS machine health",
                details,
                "Keep the protected token and installed store, correct the endpoint/auth/bootstrap issue, then retry provisioning.",
            ));
        }
        Ok(())
    }

    // Phase 4.10 implements the explicit repair primitive and its recovery semantics. Phase 6.2
    // will expose the user-facing repair action; normal start/restart/provisioning must not rotate.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rotate_machine_health_token(
        &self,
        runtime_info: &RuntimeInfo,
    ) -> Result<(), RuntimeErrorInfo> {
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "rotate CoffeePOS machine credential",
                "HTTP port is unavailable during rotation.",
                "Start the managed runtime and wait for WordPress health before retrying.",
            )
        })?;
        let active_path = self.data_root.join(MACHINE_TOKEN_SECRET);
        let pending_path = self.data_root.join(MACHINE_TOKEN_PENDING_SECRET);
        if pending_path.is_file() {
            let recovered = probe_coffeepos_health(&self.data_root, http_port);
            if pending_path.is_file()
                || !matches!(
                    recovered.state,
                    CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
                )
            {
                return Err(provisioning_error(
                    "recover CoffeePOS machine credential",
                    "An existing pending machine credential could not be resolved safely.",
                    "Preserve both protected credential files and use explicit repair.",
                ));
            }
        }

        let active = secret::load(&active_path).map_err(|error| {
            provisioning_error(
                "rotate CoffeePOS machine credential",
                error,
                "Repair the active protected credential before rotating it.",
            )
        })?;
        let active_health = probe_coffeepos_health_with_token(http_port, &active);
        if !matches!(
            active_health.state,
            CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
        ) {
            return Err(provisioning_error(
                "rotate CoffeePOS machine credential",
                "The current machine credential is not accepted by the CoffeePOS endpoint.",
                "Repair the active credential first; rotation will not overwrite an unverified server hash.",
            ));
        }

        let pending = secret::create_machine_token(&pending_path).map_err(|error| {
            provisioning_error(
                "rotate CoffeePOS machine credential",
                error,
                "Check protected application-data storage and retry.",
            )
        })?;
        if let Err(switch_error) = self.switch_machine_health_token(runtime_info, &active, &pending)
        {
            let active_after_failure = probe_coffeepos_health_with_token(http_port, &active);
            if matches!(
                active_after_failure.state,
                CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
            ) {
                fs::remove_file(&pending_path).map_err(|error| {
                    provisioning_error(
                        "recover CoffeePOS machine credential",
                        format!(
                            "Credential update failed before replacing the active server hash, but the unused pending credential cannot be removed: {error}."
                        ),
                        "The existing active credential is still authoritative. Remove the stale pending file through explicit repair before rotating again.",
                    )
                })?;
                return Err(provisioning_error(
                    "rotate CoffeePOS machine credential",
                    format!(
                        "Credential update failed ({switch_error}). The previous active credential is still accepted, so the pending credential was discarded."
                    ),
                    "Keep using the previous protected credential and correct the reported update/runtime issue before retrying rotation.",
                ));
            }

            let pending_after_failure = probe_coffeepos_health_with_token(http_port, &pending);
            if !matches!(
                pending_after_failure.state,
                CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
            ) {
                return Err(provisioning_error(
                    "recover CoffeePOS machine credential",
                    format!(
                        "Credential update failed ({switch_error}), and neither the active nor pending protected credential can currently establish server authority."
                    ),
                    "Preserve both protected credentials for explicit recovery; do not start another rotation while the server commit point is unknown.",
                ));
            }

            if let Err(rollback_error) =
                self.switch_machine_health_token(runtime_info, &pending, &active)
            {
                return Err(provisioning_error(
                    "rollback CoffeePOS machine credential",
                    format!(
                        "Credential update failed ({switch_error}); WordPress accepts the pending credential, but restoring the previous hash also failed: {rollback_error}."
                    ),
                    "Preserve both protected credentials for explicit recovery because WordPress still accepts the pending credential.",
                ));
            }
            let restored = probe_coffeepos_health_with_token(http_port, &active);
            if !matches!(
                restored.state,
                CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
            ) {
                let detail = restored
                    .error
                    .as_ref()
                    .map(|error| error.message.as_str())
                    .unwrap_or("the restored credential could not be verified");
                return Err(provisioning_error(
                    "rollback CoffeePOS machine credential",
                    format!(
                        "Pinned PHP restored the previous hash after the failed update, but the active credential could not be verified: {detail}."
                    ),
                    "Preserve both protected credentials and retry recovery when the endpoint is reachable; do not start another rotation.",
                ));
            }
            fs::remove_file(&pending_path).map_err(|error| {
                provisioning_error(
                    "rollback CoffeePOS machine credential",
                    format!(
                        "The previous machine credential hash was restored, but the pending protected credential cannot be removed: {error}."
                    ),
                    "The previous active credential is authoritative. Remove the stale pending file through explicit repair before rotating again.",
                )
            })?;
            return Err(provisioning_error(
                "rotate CoffeePOS machine credential",
                format!(
                    "Credential update failed ({switch_error}). The pending credential had reached WordPress, so pinned PHP restored and verified the previous hash before the pending credential was discarded."
                ),
                "Keep using the previous protected credential and correct the reported update/runtime issue before retrying rotation.",
            ));
        }
        let pending_health = probe_coffeepos_health_with_token(http_port, &pending);
        if matches!(
            pending_health.state,
            CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
        ) {
            secret::store_machine_token(&active_path, &pending).map_err(|error| {
                provisioning_error(
                    "promote CoffeePOS machine credential",
                    error,
                    "The server accepts the pending credential. Preserve the pending protected file and retry recovery.",
                )
            })?;
            fs::remove_file(&pending_path).map_err(|error| {
                provisioning_error(
                    "promote CoffeePOS machine credential",
                    format!("Cannot remove the promoted pending credential: {error}."),
                    "Retry recovery; the active protected credential already matches WordPress.",
                )
            })?;
            return Ok(());
        }

        let pending_failure = pending_health
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "pending credential probe failed".into());
        if let Err(error) = self.switch_machine_health_token(runtime_info, &pending, &active) {
            return Err(provisioning_error(
                "rollback CoffeePOS machine credential",
                format!(
                    "Pending verification failed ({pending_failure}) and rollback could not switch the server hash: {error}."
                ),
                "Preserve both protected credential files for explicit recovery.",
            ));
        }
        let restored = probe_coffeepos_health_with_token(http_port, &active);
        if !matches!(
            restored.state,
            CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
        ) {
            let detail = restored
                .error
                .as_ref()
                .map(|error| error.message.as_str())
                .unwrap_or("the restored credential could not be verified");
            return Err(provisioning_error(
                "rollback CoffeePOS machine credential",
                format!(
                    "Pending verification failed ({pending_failure}); pinned PHP restored the previous hash, but the active credential could not be verified: {detail}."
                ),
                "Preserve both protected credentials and retry recovery when the endpoint is reachable; do not start another rotation.",
            ));
        }
        fs::remove_file(&pending_path).map_err(|error| {
            provisioning_error(
                "rollback CoffeePOS machine credential",
                format!("Rollback restored the previous server hash but the pending credential cannot be removed: {error}."),
                "The previous active credential is authoritative; remove the stale pending file through explicit repair before rotating again.",
            )
        })?;
        Err(provisioning_error(
            "rotate CoffeePOS machine credential",
            format!(
                "The pending credential failed endpoint verification: {pending_failure}. The previous hash was restored and verified before the pending credential was discarded."
            ),
            "Keep using the previous protected credential and correct the endpoint issue before retrying rotation.",
        ))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn switch_machine_health_token(
        &self,
        runtime_info: &RuntimeInfo,
        expected_current: &str,
        replacement: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "switch CoffeePOS machine credential",
                "MariaDB port is unavailable during rotation.",
                "Keep both protected credentials and restart the managed runtime before recovery.",
            )
        })?;
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "switch CoffeePOS machine credential",
                "HTTP port is unavailable during rotation.",
                "Keep both protected credentials and restart the managed runtime before recovery.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "switch CoffeePOS machine credential",
                    error,
                    "Retry with the same Windows user profile; both protected machine credentials are preserved.",
                )
        })?;
        let script = self.write_machine_token_switch_script()?;
        let log_path = self.data_root.join("logs/coffeepos.log");
        let log = match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(log) => log,
            Err(error) => {
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "switch CoffeePOS machine credential",
                    format!("Cannot open CoffeePOS log: {error}."),
                    "Check application-data permissions and retry.",
                ));
            }
        };
        let stdout_log = match log.try_clone() {
            Ok(log) => log,
            Err(error) => {
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "switch CoffeePOS machine credential",
                    format!("Cannot duplicate CoffeePOS log handle: {error}."),
                    "Check application-data permissions and retry.",
                ));
            }
        };
        let mut command = Command::new(&self.runtime.php_executable);
        command
            .arg("-c")
            .arg(&self.runtime.php_ini)
            .arg(&script)
            .env_remove("PHPRC")
            .env("PHP_INI_SCAN_DIR", "")
            .env("COFFEEPOS_DB_HOST", format!("{LOOPBACK}:{database_port}"))
            .env("COFFEEPOS_DB_PASSWORD", database_password)
            .env(
                "COFFEEPOS_SITE_URL",
                format!("http://{LOOPBACK}:{http_port}"),
            )
            .env("COFFEEPOS_UPLOAD_ROOT", self.data_root.join("uploads"))
            .env("COFFEEPOS_SITE_ROOT", self.data_root.join("site"))
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout_log))
            .stderr(Stdio::from(log))
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "switch CoffeePOS machine credential",
                    format!("Cannot start pinned PHP for rotation: {error}."),
                    "Keep both protected credentials and verify the pinned PHP runtime.",
                ));
            }
        };
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(error);
        }
        let input = format!("{expected_current}\n{replacement}\n");
        let stdin_result = match child.stdin.take() {
            Some(mut stdin) => stdin.write_all(input.as_bytes()),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pinned PHP stdin pipe is unavailable",
            )),
        };
        if let Err(error) = stdin_result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(provisioning_error(
                "switch CoffeePOS machine credential",
                format!("Cannot pass rotation credentials to pinned PHP over stdin: {error}."),
                "Both protected credentials are preserved. Retry recovery after checking the runtime.",
            ));
        }
        let status = match wait_for_child_exit(&mut child, Duration::from_secs(30)) {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&script);
                return Err(provisioning_error(
                    "switch CoffeePOS machine credential",
                    format!("Credential switch exceeded its bounded timeout: {}.", error.message),
                    "Both protected credentials are preserved because the server commit point is unknown. Retry recovery, not a new rotation.",
                ));
            }
        };
        let _ = fs::remove_file(&script);
        if !status.success() {
            return Err(provisioning_error(
                "switch CoffeePOS machine credential",
                format!("Credential switch exited with status {status}."),
                "Both protected credentials are preserved. Inspect logs/coffeepos.log and use recovery rather than overwriting the server hash.",
            ));
        }
        Ok(())
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
        copy_tree(
            &self.wordpress.core_root,
            &staging,
            "WordPress core",
            "Restage the verified WordPress archive and retry.",
        )?;
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
                if !existing.contains("define('DISABLE_WP_CRON', true);") {
                    let anchor = "define('AUTOMATIC_UPDATER_DISABLED', true);\n";
                    let Some(position) = existing.find(anchor) else {
                        return Err(provisioning_error(
                            "update wp-config",
                            "CoffeePOS-managed wp-config.php is missing the expected updater anchor for the cron migration.",
                            "Preserve the managed config and use an explicit repair flow instead of rewriting an unexpected wp-config.php layout.",
                        ));
                    };
                    let insertion = position + anchor.len();
                    let mut updated = existing;
                    updated.insert_str(insertion, "define('DISABLE_WP_CRON', true);\n");
                    atomic_write(&path, updated.as_bytes(), "wp-config.php")?;
                }
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
define('DISABLE_WP_CRON', true);\n\
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

    fn write_woocommerce_activation_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare WooCommerce activation",
                format!("Cannot create temporary WooCommerce activation script: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(WOOCOMMERCE_ACTIVATION_BOOTSTRAP.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare WooCommerce activation",
                    format!("Cannot write WooCommerce activation script: {error}."),
                    "Check application-data storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare WooCommerce activation",
                format!(
                    "Cannot retain WooCommerce activation script: {}.",
                    error.error
                ),
                "Check application-data permissions and retry.",
            )
        })?;
        Ok(path)
    }

    fn write_coffeepos_activation_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS activation",
                format!("Cannot create temporary CoffeePOS activation script: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(COFFEEPOS_ACTIVATION_BOOTSTRAP.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare CoffeePOS activation",
                    format!("Cannot write CoffeePOS activation script: {error}."),
                    "Check application-data storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS activation",
                format!(
                    "Cannot retain CoffeePOS activation script: {}.",
                    error.error
                ),
                "Check application-data permissions and retry.",
            )
        })?;
        Ok(path)
    }

    fn write_coffeepos_activation_verification_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS activation verification",
                format!(
                    "Cannot create temporary CoffeePOS activation verification script: {error}."
                ),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(COFFEEPOS_ACTIVATION_VERIFY_BOOTSTRAP.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare CoffeePOS activation verification",
                    format!("Cannot write CoffeePOS activation verification script: {error}."),
                    "Check application-data storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS activation verification",
                format!(
                    "Cannot retain CoffeePOS activation verification script: {}.",
                    error.error
                ),
                "Check application-data permissions and retry.",
            )
        })?;
        Ok(path)
    }

    fn write_machine_health_bootstrap_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS machine health",
                format!("Cannot create temporary machine-health bootstrap script: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(COFFEEPOS_MACHINE_HEALTH_BOOTSTRAP.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare CoffeePOS machine health",
                    format!("Cannot write machine-health bootstrap script: {error}."),
                    "Check application-data storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS machine health",
                format!(
                    "Cannot retain machine-health bootstrap script: {}.",
                    error.error
                ),
                "Check application-data permissions and retry.",
            )
        })?;
        Ok(path)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn write_machine_token_switch_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS machine credential rotation",
                format!("Cannot create temporary credential-switch script: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(COFFEEPOS_MACHINE_TOKEN_SWITCH_BOOTSTRAP.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare CoffeePOS machine credential rotation",
                    format!("Cannot write credential-switch script: {error}."),
                    "Check application-data storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare CoffeePOS machine credential rotation",
                format!("Cannot retain credential-switch script: {}.", error.error),
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
        let existing = self.load_journal()?;
        if let Some(existing) = existing.as_ref() {
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
            woocommerce_version: (stage >= ProvisioningStage::WooCommerceProvisioned)
                .then(|| self.woocommerce.version.clone()),
            coffeepos_version: (stage >= ProvisioningStage::CoffeePosProvisioned)
                .then(|| self.coffeepos.version.clone()),
            stage,
            admin_username: WORDPRESS_ADMIN_USER.into(),
            recovery_blocker: existing.and_then(|value| value.recovery_blocker),
        };
        self.persist_journal(&journal)
    }

    fn persist_recovery_blocker(
        &self,
        blocker: ProvisioningRecoveryBlocker,
    ) -> Result<(), RuntimeErrorInfo> {
        let mut journal = self.load_journal()?.ok_or_else(|| {
            provisioning_error(
                "save recovery state",
                "Cannot record a provisioning recovery blocker because the provisioning journal is missing.",
                "Preserve the database/site and inspect config/provisioning.json before retrying.",
            )
        })?;
        journal.recovery_blocker = Some(blocker);
        self.persist_journal(&journal)
    }

    fn persist_journal(&self, journal: &ProvisioningJournal) -> Result<(), RuntimeErrorInfo> {
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

pub fn resolve_development_woocommerce(
    project_root: &Path,
    runtime_manifest_path: &Path,
) -> Result<ResolvedWooCommerce, RuntimeErrorInfo> {
    if !project_root.is_absolute() || !runtime_manifest_path.is_absolute() {
        return Err(provisioning_error(
            "resolve WooCommerce baseline",
            "Project root and runtime manifest path must both be absolute.",
            "Resolve development paths from the Cargo manifest directory before provisioning.",
        ));
    }
    let development_root =
        fs::canonicalize(project_root.join("runtime/development")).map_err(|error| {
            provisioning_error(
                "resolve WooCommerce baseline",
                format!("Cannot resolve runtime/development: {error}."),
                "Run the WooCommerce development staging script and retry.",
            )
        })?;
    let runtime_manifest = fs::canonicalize(runtime_manifest_path).map_err(|error| {
        provisioning_error(
            "resolve WooCommerce baseline",
            format!("Cannot resolve runtime manifest: {error}."),
            "Stage the pinned development runtime before provisioning.",
        )
    })?;
    if !runtime_manifest.starts_with(&development_root) {
        return Err(provisioning_error(
            "resolve WooCommerce baseline",
            "Runtime manifest resolves outside runtime/development.",
            "Use only the pinned project development runtime.",
        ));
    }
    let target_root = runtime_manifest.parent().ok_or_else(|| {
        provisioning_error(
            "resolve WooCommerce baseline",
            "Runtime manifest does not have a target directory.",
            "Restage the pinned development runtime.",
        )
    })?;
    let manifest_path = target_root.join("woocommerce-manifest.json");
    let manifest: WooCommerceDevelopmentManifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|error| {
            provisioning_error(
                "resolve WooCommerce baseline",
                format!("Cannot read WooCommerce manifest: {error}."),
                "Run scripts/stage-woocommerce-development.ps1 and retry.",
            )
        })?)
        .map_err(|error| {
            provisioning_error(
                "resolve WooCommerce baseline",
                format!("WooCommerce manifest is invalid JSON: {error}."),
                "Restore the checked-in manifest template and restage the artifact.",
            )
        })?;
    validate_woocommerce_manifest(&manifest)?;

    let plugin_root = fs::canonicalize(target_root.join(&manifest.woocommerce.plugin_root))
        .map_err(|error| {
            provisioning_error(
                "resolve WooCommerce baseline",
                format!("Cannot resolve staged WooCommerce plugin: {error}."),
                "Run scripts/stage-woocommerce-development.ps1 and retry.",
            )
        })?;
    if !plugin_root.starts_with(&development_root) || !plugin_root.is_dir() {
        return Err(provisioning_error(
            "resolve WooCommerce baseline",
            "WooCommerce plugin root resolves outside runtime/development or is not a directory.",
            "Restage the pinned WooCommerce artifact from the checked-in manifest.",
        ));
    }
    for required in [
        "woocommerce.php",
        "license.txt",
        "readme.txt",
        "includes",
        "src",
        "vendor",
    ] {
        if !plugin_root.join(required).exists() {
            return Err(provisioning_error(
                "resolve WooCommerce baseline",
                format!("Staged WooCommerce plugin is missing required path '{required}'."),
                "Restage the pinned WooCommerce archive and retry.",
            ));
        }
    }
    let actual_version = read_plugin_header_version(&plugin_root.join("woocommerce.php"))?;
    if actual_version != manifest.woocommerce.version {
        return Err(provisioning_error(
            "resolve WooCommerce baseline",
            format!(
                "Staged WooCommerce plugin version is {actual_version}, expected {}.",
                manifest.woocommerce.version
            ),
            "Restage the exact pinned WooCommerce artifact and retry.",
        ));
    }
    Ok(ResolvedWooCommerce {
        version: manifest.woocommerce.version,
        plugin_root,
        archive_sha256: manifest.woocommerce.archive_sha256,
    })
}

fn validate_woocommerce_manifest(
    manifest: &WooCommerceDevelopmentManifest,
) -> Result<(), RuntimeErrorInfo> {
    if manifest.schema_version != WOOCOMMERCE_MANIFEST_SCHEMA_VERSION {
        return Err(provisioning_error(
            "validate WooCommerce manifest",
            format!(
                "Unsupported WooCommerce manifest schema {}.",
                manifest.schema_version
            ),
            "Use the WooCommerce manifest schema shipped with this CoffeePOS Desktop version.",
        ));
    }
    if manifest.target != expected_target()? {
        return Err(provisioning_error(
            "validate WooCommerce manifest",
            format!(
                "WooCommerce manifest target '{}' does not match '{}'.",
                manifest.target,
                expected_target()?
            ),
            "Stage the WooCommerce artifact for the current platform target.",
        ));
    }
    for (label, value) in [
        ("version", manifest.woocommerce.version.as_str()),
        ("archive", manifest.woocommerce.archive.as_str()),
        ("source", manifest.woocommerce.source.as_str()),
        (
            "release_source",
            manifest.woocommerce.release_source.as_str(),
        ),
        (
            "plugin_directory_source",
            manifest.woocommerce.plugin_directory_source.as_str(),
        ),
        (
            "archive_sha256_source",
            manifest.woocommerce.archive_sha256_source.as_str(),
        ),
        ("license", manifest.woocommerce.license.as_str()),
        (
            "compatibility.plugin_metadata_source",
            manifest.compatibility.plugin_metadata_source.as_str(),
        ),
        (
            "compatibility.server_requirements_source",
            manifest.compatibility.server_requirements_source.as_str(),
        ),
        (
            "compatibility.minimum_wordpress",
            manifest.compatibility.minimum_wordpress.as_str(),
        ),
        (
            "compatibility.tested_wordpress",
            manifest.compatibility.tested_wordpress.as_str(),
        ),
        (
            "compatibility.minimum_php",
            manifest.compatibility.minimum_php.as_str(),
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
            "compatibility.development_wordpress",
            manifest.compatibility.development_wordpress.as_str(),
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
                "validate WooCommerce manifest",
                format!("WooCommerce manifest field '{label}' is invalid."),
                "Restore the checked-in WooCommerce manifest template and restage the artifact.",
            ));
        }
    }
    if manifest.woocommerce.archive_sha256.len() != 64
        || !manifest
            .woocommerce
            .archive_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(provisioning_error(
            "validate WooCommerce manifest",
            "WooCommerce archive SHA256 must contain 64 hexadecimal characters.",
            "Restore the pinned checksum and restage the artifact.",
        ));
    }
    for relative in [
        &manifest.woocommerce.plugin_root,
        &manifest.woocommerce.entry_file,
        &manifest.woocommerce.license_file,
        &manifest.woocommerce.readme_file,
    ] {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(provisioning_error(
                "validate WooCommerce manifest",
                "WooCommerce manifest paths must be relative and may not escape the target root.",
                "Restore the checked-in WooCommerce manifest template and restage the artifact.",
            ));
        }
    }
    Ok(())
}

pub fn resolve_development_coffeepos(
    project_root: &Path,
    runtime_manifest_path: &Path,
) -> Result<ResolvedCoffeePos, RuntimeErrorInfo> {
    if !project_root.is_absolute() || !runtime_manifest_path.is_absolute() {
        return Err(provisioning_error(
            "resolve CoffeePOS baseline",
            "Project root and runtime manifest path must both be absolute.",
            "Resolve development paths from the Cargo manifest directory before provisioning.",
        ));
    }
    let development_root =
        fs::canonicalize(project_root.join("runtime/development")).map_err(|error| {
            provisioning_error(
                "resolve CoffeePOS baseline",
                format!("Cannot resolve runtime/development: {error}."),
                "Run the CoffeePOS development staging script and retry.",
            )
        })?;
    let runtime_manifest = fs::canonicalize(runtime_manifest_path).map_err(|error| {
        provisioning_error(
            "resolve CoffeePOS baseline",
            format!("Cannot resolve runtime manifest: {error}."),
            "Stage the pinned development runtime before provisioning.",
        )
    })?;
    if !runtime_manifest.starts_with(&development_root) {
        return Err(provisioning_error(
            "resolve CoffeePOS baseline",
            "Runtime manifest resolves outside runtime/development.",
            "Use only the pinned project development runtime.",
        ));
    }
    let target_root = runtime_manifest.parent().ok_or_else(|| {
        provisioning_error(
            "resolve CoffeePOS baseline",
            "Runtime manifest does not have a target directory.",
            "Restage the pinned development runtime.",
        )
    })?;
    let manifest_path = target_root.join("coffeepos-manifest.json");
    let manifest: CoffeePosDevelopmentManifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|error| {
            provisioning_error(
                "resolve CoffeePOS baseline",
                format!("Cannot read CoffeePOS manifest: {error}."),
                "Run scripts/stage-coffeepos-development.ps1 and retry.",
            )
        })?)
        .map_err(|error| {
            provisioning_error(
                "resolve CoffeePOS baseline",
                format!("CoffeePOS manifest is invalid JSON: {error}."),
                "Restore the checked-in manifest template and restage the artifact.",
            )
        })?;
    validate_coffeepos_manifest(&manifest)?;

    let plugin_root =
        fs::canonicalize(target_root.join(&manifest.coffeepos.plugin_root)).map_err(|error| {
            provisioning_error(
                "resolve CoffeePOS baseline",
                format!("Cannot resolve staged CoffeePOS plugin: {error}."),
                "Run scripts/stage-coffeepos-development.ps1 and retry.",
            )
        })?;
    if !plugin_root.starts_with(target_root) || !plugin_root.is_dir() {
        return Err(provisioning_error(
            "resolve CoffeePOS baseline",
            "CoffeePOS plugin root resolves outside the current development target or is not a directory.",
            "Restage the pinned CoffeePOS artifact from the checked-in manifest.",
        ));
    }
    if let Some(issue) = coffeepos_tree_issue(&plugin_root) {
        return Err(provisioning_error(
            "resolve CoffeePOS baseline",
            format!("Staged CoffeePOS plugin layout is invalid: {issue}."),
            "Restage the pinned CoffeePOS archive and retry.",
        ));
    }
    let entry_file = plugin_root.join("coffeepos.php");
    let actual_version = read_coffeepos_plugin_header_version(&entry_file)?;
    if actual_version != manifest.coffeepos.version {
        return Err(provisioning_error(
            "resolve CoffeePOS baseline",
            format!(
                "Staged CoffeePOS plugin version is {actual_version}, expected {}.",
                manifest.coffeepos.version
            ),
            "Restage the exact pinned CoffeePOS artifact and retry.",
        ));
    }
    for (field, expected) in [
        (
            "Requires at least",
            manifest.compatibility.minimum_wordpress.as_str(),
        ),
        ("Requires PHP", manifest.compatibility.minimum_php.as_str()),
    ] {
        let actual =
            read_plugin_header_value(&entry_file, field, "CoffeePOS")?.ok_or_else(|| {
                provisioning_error(
                    "resolve CoffeePOS baseline",
                    format!("Staged CoffeePOS plugin is missing the '{field}' header."),
                    "Restore the pinned CoffeePOS artifact and retry.",
                )
            })?;
        if actual != expected {
            return Err(provisioning_error(
                "resolve CoffeePOS baseline",
                format!(
                    "Staged CoffeePOS '{field}' header is {actual}, expected {expected} from the pinned manifest."
                ),
                "Restage the exact pinned CoffeePOS artifact and retry.",
            ));
        }
    }
    let requires_plugins = read_plugin_header_value(&entry_file, "Requires Plugins", "CoffeePOS")?
        .ok_or_else(|| {
            provisioning_error(
                "resolve CoffeePOS baseline",
                "Staged CoffeePOS plugin does not declare its required plugin dependency.",
                "Restore the pinned CoffeePOS artifact and retry.",
            )
        })?;
    let dependency_present = requires_plugins
        .split(',')
        .map(|value| value.trim())
        .any(|value| value.eq_ignore_ascii_case(&manifest.coffeepos.requires_plugin));
    if !dependency_present {
        return Err(provisioning_error(
            "resolve CoffeePOS baseline",
            format!(
                "Staged CoffeePOS plugin does not require the pinned '{}' dependency.",
                manifest.coffeepos.requires_plugin
            ),
            "Restore the pinned CoffeePOS artifact and retry.",
        ));
    }

    Ok(ResolvedCoffeePos {
        version: manifest.coffeepos.version,
        plugin_root,
        archive_sha256: manifest.coffeepos.archive_sha256,
        required_wordpress_version: manifest.compatibility.development_wordpress,
        required_php_version: manifest.compatibility.development_php,
        required_mariadb_version: manifest.compatibility.development_mariadb,
        required_woocommerce_version: manifest.compatibility.development_woocommerce,
    })
}

fn validate_coffeepos_manifest(
    manifest: &CoffeePosDevelopmentManifest,
) -> Result<(), RuntimeErrorInfo> {
    if manifest.schema_version != COFFEEPOS_MANIFEST_SCHEMA_VERSION {
        return Err(provisioning_error(
            "validate CoffeePOS manifest",
            format!(
                "Unsupported CoffeePOS manifest schema {}.",
                manifest.schema_version
            ),
            "Use the CoffeePOS manifest schema shipped with this CoffeePOS Desktop version.",
        ));
    }
    if manifest.target != expected_target()? {
        return Err(provisioning_error(
            "validate CoffeePOS manifest",
            format!(
                "CoffeePOS manifest target '{}' does not match '{}'.",
                manifest.target,
                expected_target()?
            ),
            "Stage the CoffeePOS artifact for the current platform target.",
        ));
    }
    for (label, value) in [
        ("version", manifest.coffeepos.version.as_str()),
        ("archive", manifest.coffeepos.archive.as_str()),
        (
            "archive_sha256_source",
            manifest.coffeepos.archive_sha256_source.as_str(),
        ),
        ("source_kind", manifest.coffeepos.source_kind.as_str()),
        (
            "source_git_commit",
            manifest.coffeepos.source_git_commit.as_str(),
        ),
        ("build_script", manifest.coffeepos.build_script.as_str()),
        (
            "composer_version",
            manifest.coffeepos.composer_version.as_str(),
        ),
        (
            "build_php_version",
            manifest.coffeepos.build_php_version.as_str(),
        ),
        ("license", manifest.coffeepos.license.as_str()),
        (
            "requires_plugin",
            manifest.coffeepos.requires_plugin.as_str(),
        ),
        (
            "compatibility.minimum_wordpress",
            manifest.compatibility.minimum_wordpress.as_str(),
        ),
        (
            "compatibility.tested_wordpress",
            manifest.compatibility.tested_wordpress.as_str(),
        ),
        (
            "compatibility.minimum_php",
            manifest.compatibility.minimum_php.as_str(),
        ),
        (
            "compatibility.development_wordpress",
            manifest.compatibility.development_wordpress.as_str(),
        ),
        (
            "compatibility.development_php",
            manifest.compatibility.development_php.as_str(),
        ),
        (
            "compatibility.development_mariadb",
            manifest.compatibility.development_mariadb.as_str(),
        ),
        (
            "compatibility.development_woocommerce",
            manifest.compatibility.development_woocommerce.as_str(),
        ),
    ] {
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(provisioning_error(
                "validate CoffeePOS manifest",
                format!("CoffeePOS manifest field '{label}' is invalid."),
                "Restore the checked-in CoffeePOS manifest template and restage the artifact.",
            ));
        }
    }
    if manifest.coffeepos.archive_sha256.len() != 64
        || !manifest
            .coffeepos
            .archive_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(provisioning_error(
            "validate CoffeePOS manifest",
            "CoffeePOS archive SHA256 must contain 64 hexadecimal characters.",
            "Restore the pinned checksum and restage the artifact.",
        ));
    }
    if manifest.coffeepos.source_git_commit.len() != 40
        || !manifest
            .coffeepos
            .source_git_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(provisioning_error(
            "validate CoffeePOS manifest",
            "CoffeePOS source Git commit must contain 40 hexadecimal characters.",
            "Restore the pinned CoffeePOS source provenance and restage the artifact.",
        ));
    }
    if !manifest
        .coffeepos
        .requires_plugin
        .eq_ignore_ascii_case(WOOCOMMERCE_PLUGIN_SLUG)
    {
        return Err(provisioning_error(
            "validate CoffeePOS manifest",
            "CoffeePOS manifest dependency must be WooCommerce.",
            "Restore the checked-in CoffeePOS manifest template and restage the artifact.",
        ));
    }
    for relative in [
        &manifest.coffeepos.plugin_root,
        &manifest.coffeepos.entry_file,
        &manifest.coffeepos.readme_file,
        &manifest.coffeepos.license_file,
        &manifest.coffeepos.autoload_file,
    ] {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(provisioning_error(
                "validate CoffeePOS manifest",
                "CoffeePOS manifest paths must be relative and may not escape the target root.",
                "Restore the checked-in CoffeePOS manifest template and restage the artifact.",
            ));
        }
    }
    let expected_plugin_root = PathBuf::from("coffeepos/coffeepos");
    if manifest.coffeepos.plugin_root != expected_plugin_root
        || manifest.coffeepos.entry_file != expected_plugin_root.join("coffeepos.php")
        || manifest.coffeepos.readme_file != expected_plugin_root.join("readme.txt")
        || manifest.coffeepos.license_file != expected_plugin_root.join("LICENSE")
        || manifest.coffeepos.autoload_file != expected_plugin_root.join("vendor/autoload.php")
    {
        return Err(provisioning_error(
            "validate CoffeePOS manifest",
            "CoffeePOS manifest layout does not match the pinned plugin root contract.",
            "Restore the checked-in CoffeePOS manifest template and restage the artifact.",
        ));
    }
    Ok(())
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

fn load_or_create_wordpress_admin_secret(
    path: &Path,
    wordpress_already_installed: bool,
) -> Result<String, String> {
    if wordpress_already_installed {
        secret::load(path)
    } else {
        secret::create(path)
    }
}

fn directory_is_empty_or_missing(path: &Path) -> bool {
    match fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn woocommerce_installation_ready(data_root: &Path, woocommerce: &ResolvedWooCommerce) -> bool {
    let destination = data_root
        .join("site/wp-content/plugins")
        .join(WOOCOMMERCE_PLUGIN_SLUG);
    let ownership = fs::read(destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ManagedPluginOwnership>(&bytes).ok());
    let Some(ownership) = ownership else {
        return false;
    };
    ownership.schema_version == WOOCOMMERCE_OWNERSHIP_SCHEMA_VERSION
        && ownership.plugin == WOOCOMMERCE_PLUGIN_SLUG
        && ownership.version == woocommerce.version
        && ownership
            .archive_sha256
            .eq_ignore_ascii_case(&woocommerce.archive_sha256)
        && read_plugin_header_version(&destination.join("woocommerce.php"))
            .map(|version| version == woocommerce.version)
            .unwrap_or(false)
}

fn ensure_woocommerce_plugin(
    data_root: &Path,
    woocommerce: &ResolvedWooCommerce,
) -> Result<(), RuntimeErrorInfo> {
    let plugins_root = data_root.join("site/wp-content/plugins");
    let destination = plugins_root.join(WOOCOMMERCE_PLUGIN_SLUG);
    if destination.exists() {
        if !destination.is_dir() {
            return Err(provisioning_error(
                "provision WooCommerce",
                "The WooCommerce plugin destination exists but is not a directory.",
                "Preserve the existing path and remove or adopt it explicitly before retrying. CoffeePOS will not overwrite it automatically.",
            ));
        }
        let ownership_path = destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE);
        let bytes = fs::read(&ownership_path).map_err(|error| {
            provisioning_error(
                "provision WooCommerce",
                format!(
                    "An existing WooCommerce directory is not proven CoffeePOS-managed: cannot read {}: {error}.",
                    MANAGED_PLUGIN_OWNERSHIP_FILE
                ),
                "Preserve the existing WooCommerce directory and use an explicit adoption/repair flow. CoffeePOS will not overwrite an unmanaged plugin.",
            )
        })?;
        let ownership: ManagedPluginOwnership = serde_json::from_slice(&bytes).map_err(|error| {
            provisioning_error(
                "provision WooCommerce",
                format!("WooCommerce ownership metadata is invalid and was preserved: {error}."),
                "Repair or restore the ownership metadata explicitly before retrying; CoffeePOS will not replace the existing plugin automatically.",
            )
        })?;
        if ownership.schema_version != WOOCOMMERCE_OWNERSHIP_SCHEMA_VERSION
            || ownership.plugin != WOOCOMMERCE_PLUGIN_SLUG
        {
            return Err(provisioning_error(
                "provision WooCommerce",
                "Existing WooCommerce ownership metadata is not compatible with this CoffeePOS Desktop version.",
                "Preserve the plugin and use an explicit adoption/upgrade flow instead of overwriting it during provisioning.",
            ));
        }
        if ownership.version != woocommerce.version
            || !ownership
                .archive_sha256
                .eq_ignore_ascii_case(&woocommerce.archive_sha256)
        {
            return Err(provisioning_error(
                "provision WooCommerce",
                format!(
                    "Managed WooCommerce {} does not match the pinned {} artifact.",
                    ownership.version, woocommerce.version
                ),
                "Run an explicit WooCommerce upgrade flow; provisioning will not overwrite an existing managed plugin version.",
            ));
        }
        let installed_version = read_plugin_header_version(&destination.join("woocommerce.php"))?;
        if installed_version != woocommerce.version {
            return Err(provisioning_error(
                "provision WooCommerce",
                format!(
                    "Managed WooCommerce files report version {installed_version}, expected {}.",
                    woocommerce.version
                ),
                "Preserve the plugin directory and repair it explicitly; provisioning will not overwrite a modified/corrupt managed plugin.",
            ));
        }
        return Ok(());
    }

    fs::create_dir_all(&plugins_root).map_err(|error| {
        provisioning_error(
            "provision WooCommerce",
            format!("Cannot create the WordPress plugins directory: {error}."),
            "Check site permissions and free disk space, then retry.",
        )
    })?;
    let staging = data_root.join("woocommerce.provisioning");
    reset_owned_staging_dir(data_root, &staging)?;
    copy_tree(
        &woocommerce.plugin_root,
        &staging,
        "WooCommerce plugin",
        "Restage the exact pinned WooCommerce artifact and retry.",
    )?;
    let staged_version = read_plugin_header_version(&staging.join("woocommerce.php"))?;
    if staged_version != woocommerce.version {
        return Err(provisioning_error(
            "provision WooCommerce",
            format!(
                "Copied WooCommerce files report version {staged_version}, expected {}.",
                woocommerce.version
            ),
            "Restage the exact pinned WooCommerce artifact and retry.",
        ));
    }
    let ownership = ManagedPluginOwnership {
        schema_version: WOOCOMMERCE_OWNERSHIP_SCHEMA_VERSION,
        plugin: WOOCOMMERCE_PLUGIN_SLUG.into(),
        version: woocommerce.version.clone(),
        archive_sha256: woocommerce.archive_sha256.clone(),
    };
    let mut ownership_bytes = serde_json::to_vec_pretty(&ownership).map_err(|error| {
        provisioning_error(
            "provision WooCommerce",
            format!("Cannot serialize WooCommerce ownership metadata: {error}."),
            "Retry after checking application-data storage.",
        )
    })?;
    ownership_bytes.push(b'\n');
    atomic_write(
        &staging.join(MANAGED_PLUGIN_OWNERSHIP_FILE),
        &ownership_bytes,
        "WooCommerce ownership metadata",
    )?;
    fs::rename(&staging, &destination).map_err(|error| {
        provisioning_error(
            "provision WooCommerce",
            format!("Cannot atomically activate the staged WooCommerce plugin: {error}."),
            "Preserve the existing site. Only the owned woocommerce.provisioning staging directory may be retried automatically.",
        )
    })?;
    Ok(())
}

fn coffeepos_installation_ready(data_root: &Path, coffeepos: &ResolvedCoffeePos) -> bool {
    let destination = data_root
        .join("site/wp-content/plugins")
        .join(COFFEEPOS_PLUGIN_SLUG);
    if coffeepos_tree_issue(&destination).is_some() {
        return false;
    }
    let ownership = fs::read(destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ManagedPluginOwnership>(&bytes).ok());
    let Some(ownership) = ownership else {
        return false;
    };
    ownership.schema_version == COFFEEPOS_OWNERSHIP_SCHEMA_VERSION
        && ownership.plugin == COFFEEPOS_PLUGIN_SLUG
        && ownership.version == coffeepos.version
        && ownership
            .archive_sha256
            .eq_ignore_ascii_case(&coffeepos.archive_sha256)
        && read_coffeepos_plugin_header_version(&destination.join("coffeepos.php"))
            .map(|version| version == coffeepos.version)
            .unwrap_or(false)
}

fn ensure_coffeepos_plugin(
    data_root: &Path,
    coffeepos: &ResolvedCoffeePos,
    woocommerce: &ResolvedWooCommerce,
) -> Result<(), RuntimeErrorInfo> {
    if woocommerce.version != coffeepos.required_woocommerce_version
        || !woocommerce_installation_ready(data_root, woocommerce)
    {
        return Err(provisioning_error(
            "provision CoffeePOS",
            format!(
                "CoffeePOS {} requires the managed WooCommerce {} baseline before its files can be provisioned.",
                coffeepos.version, coffeepos.required_woocommerce_version
            ),
            "Provision and verify the pinned WooCommerce dependency first, then retry CoffeePOS provisioning.",
        ));
    }

    let plugins_root = data_root.join("site/wp-content/plugins");
    let destination = plugins_root.join(COFFEEPOS_PLUGIN_SLUG);
    recover_coffeepos_upgrade_backup(data_root, coffeepos, &destination)?;
    if destination.exists() {
        if !destination.is_dir() {
            return Err(provisioning_error(
                "provision CoffeePOS",
                "The CoffeePOS plugin destination exists but is not a directory.",
                "Preserve the existing path and remove or adopt it explicitly before retrying. CoffeePOS Desktop will not overwrite it automatically.",
            ));
        }
        let ownership_path = destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE);
        let bytes = fs::read(&ownership_path).map_err(|error| {
            provisioning_error(
                "provision CoffeePOS",
                format!(
                    "An existing CoffeePOS directory is not proven Desktop-managed: cannot read {}: {error}.",
                    MANAGED_PLUGIN_OWNERSHIP_FILE
                ),
                "Preserve the existing CoffeePOS directory and use an explicit adoption/repair flow. Provisioning will not overwrite an unmanaged plugin.",
            )
        })?;
        let ownership: ManagedPluginOwnership = serde_json::from_slice(&bytes).map_err(|error| {
            provisioning_error(
                "provision CoffeePOS",
                format!("CoffeePOS ownership metadata is invalid and was preserved: {error}."),
                "Repair or restore the ownership metadata explicitly before retrying; provisioning will not replace the existing plugin automatically.",
            )
        })?;
        if ownership.schema_version != COFFEEPOS_OWNERSHIP_SCHEMA_VERSION
            || ownership.plugin != COFFEEPOS_PLUGIN_SLUG
        {
            return Err(provisioning_error(
                "provision CoffeePOS",
                "Existing CoffeePOS ownership metadata is not compatible with this CoffeePOS Desktop version.",
                "Preserve the plugin and use an explicit adoption/upgrade flow instead of overwriting it during provisioning.",
            ));
        }
        if ownership.version != coffeepos.version
            || !ownership
                .archive_sha256
                .eq_ignore_ascii_case(&coffeepos.archive_sha256)
        {
            if ownership.version == COFFEEPOS_PHASE_4_9_VERSION
                && ownership
                    .archive_sha256
                    .eq_ignore_ascii_case(COFFEEPOS_PHASE_4_9_SHA256)
                && coffeepos.version == COFFEEPOS_PHASE_4_10_VERSION
                && coffeepos
                    .archive_sha256
                    .eq_ignore_ascii_case(COFFEEPOS_PHASE_4_10_SHA256)
            {
                return upgrade_managed_coffeepos_plugin(
                    data_root,
                    coffeepos,
                    &destination,
                    &ownership,
                );
            }
            return Err(provisioning_error(
                "provision CoffeePOS",
                format!(
                    "Managed CoffeePOS {} does not match the pinned {} artifact.",
                    ownership.version, coffeepos.version
                ),
                "Run an explicit CoffeePOS upgrade flow; provisioning will not overwrite an existing managed plugin version.",
            ));
        }
        let installed_version =
            read_coffeepos_plugin_header_version(&destination.join("coffeepos.php"))?;
        if installed_version != coffeepos.version {
            return Err(provisioning_error(
                "provision CoffeePOS",
                format!(
                    "Managed CoffeePOS files report version {installed_version}, expected {}.",
                    coffeepos.version
                ),
                "Preserve the plugin directory and repair it explicitly; provisioning will not overwrite a modified/corrupt managed plugin.",
            ));
        }
        if let Some(issue) = coffeepos_tree_issue(&destination) {
            return Err(provisioning_error(
                "provision CoffeePOS",
                format!("Managed CoffeePOS plugin layout is incomplete/corrupt: {issue}."),
                "Preserve the existing managed plugin and repair it explicitly; provisioning will not overwrite missing or modified plugin files automatically.",
            ));
        }
        return Ok(());
    }

    fs::create_dir_all(&plugins_root).map_err(|error| {
        provisioning_error(
            "provision CoffeePOS",
            format!("Cannot create the WordPress plugins directory: {error}."),
            "Check site permissions and free disk space, then retry.",
        )
    })?;
    let staging = stage_coffeepos_artifact(data_root, coffeepos)?;
    fs::rename(&staging, &destination).map_err(|error| {
        provisioning_error(
            "provision CoffeePOS",
            format!("Cannot atomically install the staged CoffeePOS plugin: {error}."),
            "Preserve the existing site. Only the owned coffeepos.provisioning staging directory may be retried automatically.",
        )
    })?;
    Ok(())
}

fn stage_coffeepos_artifact(
    data_root: &Path,
    coffeepos: &ResolvedCoffeePos,
) -> Result<PathBuf, RuntimeErrorInfo> {
    let staging = data_root.join("coffeepos.provisioning");
    reset_owned_staging_dir(data_root, &staging)?;
    copy_tree(
        &coffeepos.plugin_root,
        &staging,
        "CoffeePOS plugin",
        "Restage the exact pinned CoffeePOS artifact and retry.",
    )?;
    let staged_version = read_coffeepos_plugin_header_version(&staging.join("coffeepos.php"))?;
    if staged_version != coffeepos.version {
        return Err(provisioning_error(
            "provision CoffeePOS",
            format!(
                "Copied CoffeePOS files report version {staged_version}, expected {}.",
                coffeepos.version
            ),
            "Restage the exact pinned CoffeePOS artifact and retry.",
        ));
    }
    if let Some(issue) = coffeepos_tree_issue(&staging) {
        return Err(provisioning_error(
            "provision CoffeePOS",
            format!("Copied CoffeePOS plugin layout is incomplete: {issue}."),
            "Restage the exact pinned CoffeePOS artifact and retry.",
        ));
    }
    let ownership = ManagedPluginOwnership {
        schema_version: COFFEEPOS_OWNERSHIP_SCHEMA_VERSION,
        plugin: COFFEEPOS_PLUGIN_SLUG.into(),
        version: coffeepos.version.clone(),
        archive_sha256: coffeepos.archive_sha256.clone(),
    };
    let mut ownership_bytes = serde_json::to_vec_pretty(&ownership).map_err(|error| {
        provisioning_error(
            "provision CoffeePOS",
            format!("Cannot serialize CoffeePOS ownership metadata: {error}."),
            "Retry after checking application-data storage.",
        )
    })?;
    ownership_bytes.push(b'\n');
    atomic_write(
        &staging.join(MANAGED_PLUGIN_OWNERSHIP_FILE),
        &ownership_bytes,
        "CoffeePOS ownership metadata",
    )?;
    Ok(staging)
}

fn upgrade_managed_coffeepos_plugin(
    data_root: &Path,
    coffeepos: &ResolvedCoffeePos,
    destination: &Path,
    ownership: &ManagedPluginOwnership,
) -> Result<(), RuntimeErrorInfo> {
    let installed_version =
        read_coffeepos_plugin_header_version(&destination.join("coffeepos.php"))?;
    if installed_version != ownership.version {
        return Err(provisioning_error(
            "upgrade CoffeePOS",
            format!(
                "Managed CoffeePOS ownership says {}, but installed files report {installed_version}.",
                ownership.version
            ),
            "Preserve the plugin directory and repair the managed installation explicitly before retrying the Phase 4.10 upgrade.",
        ));
    }
    if let Some(issue) = coffeepos_tree_issue(destination) {
        return Err(provisioning_error(
            "upgrade CoffeePOS",
            format!("Managed CoffeePOS 1.0.0 layout is incomplete/corrupt: {issue}."),
            "Preserve the existing plugin and repair it explicitly; the health-endpoint upgrade will not overwrite a corrupt managed plugin.",
        ));
    }

    let staging = stage_coffeepos_artifact(data_root, coffeepos)?;
    let backup = data_root.join(COFFEEPOS_UPGRADE_BACKUP);
    if backup.exists() {
        return Err(provisioning_error(
            "upgrade CoffeePOS",
            "A previous CoffeePOS upgrade backup still exists.",
            "Retry after CoffeePOS Desktop recovers the managed backup, or use the explicit repair flow if both old and new plugin trees are present.",
        ));
    }
    fs::rename(destination, &backup).map_err(|error| {
        provisioning_error(
            "upgrade CoffeePOS",
            format!("Cannot move managed CoffeePOS 1.0.0 into the upgrade backup: {error}."),
            "Close processes using plugin files and retry. The existing plugin directory has not been overwritten.",
        )
    })?;
    if let Err(error) = fs::rename(&staging, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(provisioning_error(
            "upgrade CoffeePOS",
            format!("Cannot atomically activate CoffeePOS {}: {error}.", coffeepos.version),
            "The old managed plugin was restored when possible. Inspect the site and retry the Phase 4.10 upgrade.",
        ));
    }
    if let Err(error) = fs::remove_dir_all(&backup) {
        return Err(provisioning_error(
            "upgrade CoffeePOS",
            format!("CoffeePOS {} is installed, but the old managed plugin backup could not be removed: {error}.", coffeepos.version),
            "Retry provisioning; CoffeePOS Desktop will verify the new plugin before removing the owned backup.",
        ));
    }
    Ok(())
}

fn recover_coffeepos_upgrade_backup(
    data_root: &Path,
    coffeepos: &ResolvedCoffeePos,
    destination: &Path,
) -> Result<(), RuntimeErrorInfo> {
    let backup = data_root.join(COFFEEPOS_UPGRADE_BACKUP);
    if !backup.exists() {
        return Ok(());
    }
    if !backup.is_dir() {
        return Err(provisioning_error(
            "recover CoffeePOS upgrade",
            "The owned CoffeePOS upgrade backup path exists but is not a directory.",
            "Preserve the path and use explicit repair before retrying.",
        ));
    }
    if !destination.exists() {
        fs::rename(&backup, destination).map_err(|error| {
            provisioning_error(
                "recover CoffeePOS upgrade",
                format!("Cannot restore the previous managed CoffeePOS plugin after an interrupted upgrade: {error}."),
                "Close processes using the site and retry. The backup is preserved.",
            )
        })?;
        return Ok(());
    }
    if coffeepos_installation_ready(data_root, coffeepos) {
        fs::remove_dir_all(&backup).map_err(|error| {
            provisioning_error(
                "recover CoffeePOS upgrade",
                format!("The new CoffeePOS plugin is ready, but the old owned upgrade backup cannot be removed: {error}."),
                "Close processes using the backup and retry provisioning.",
            )
        })?;
        return Ok(());
    }
    Err(provisioning_error(
        "recover CoffeePOS upgrade",
        "Both a CoffeePOS upgrade backup and a non-ready destination exist.",
        "Preserve both plugin trees and use explicit repair; CoffeePOS Desktop will not guess which tree is authoritative.",
    ))
}

fn coffeepos_tree_issue(root: &Path) -> Option<String> {
    for relative in COFFEEPOS_REQUIRED_FILES {
        if !root.join(relative).is_file() {
            return Some(format!(
                "required file '{relative}' is missing or is not a file"
            ));
        }
    }
    for relative in COFFEEPOS_REQUIRED_DIRECTORIES {
        if !root.join(relative).is_dir() {
            return Some(format!(
                "required directory '{relative}' is missing or is not a directory"
            ));
        }
    }
    None
}

fn read_plugin_header_version(path: &Path) -> Result<String, RuntimeErrorInfo> {
    read_plugin_header_value(path, "Version", "WooCommerce")?.ok_or_else(|| {
        provisioning_error(
            "inspect WooCommerce version",
            "WooCommerce entry file does not contain a valid Version header.",
            "Restage the exact pinned WooCommerce artifact and retry.",
        )
    })
}

fn read_coffeepos_plugin_header_version(path: &Path) -> Result<String, RuntimeErrorInfo> {
    read_plugin_header_value(path, "Version", "CoffeePOS")?.ok_or_else(|| {
        provisioning_error(
            "inspect CoffeePOS version",
            "CoffeePOS entry file does not contain a valid Version header.",
            "Restage the exact pinned CoffeePOS artifact and retry.",
        )
    })
}

fn read_plugin_header_value(
    path: &Path,
    field: &str,
    plugin_name: &str,
) -> Result<Option<String>, RuntimeErrorInfo> {
    let text = fs::read_to_string(path).map_err(|error| {
        provisioning_error(
            format!("inspect {plugin_name} metadata"),
            format!("Cannot read {plugin_name} entry file: {error}."),
            format!("Restage or repair the {plugin_name} plugin files, then retry."),
        )
    })?;
    let prefix = format!("{field}:");
    Ok(text.lines().take(40).find_map(|line| {
        let candidate = line
            .trim()
            .trim_start_matches('*')
            .trim()
            .strip_prefix(&prefix)
            .map(str::trim)?;
        (!candidate.is_empty() && !candidate.chars().any(char::is_control))
            .then(|| candidate.to_string())
    }))
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
            Some(
                "database.provisioning"
                    | "site.provisioning"
                    | "woocommerce.provisioning"
                    | "coffeepos.provisioning"
            )
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

fn copy_tree(
    source: &Path,
    destination: &Path,
    label: &str,
    recovery: &str,
) -> Result<(), RuntimeErrorInfo> {
    fs::create_dir_all(destination).map_err(|error| {
        provisioning_error(
            format!("copy {label}"),
            format!("Cannot create {label} staging directory: {error}."),
            "Check application-data permissions and free disk space, then retry.",
        )
    })?;
    for entry in fs::read_dir(source).map_err(|error| {
        provisioning_error(
            format!("copy {label}"),
            format!("Cannot read staged {label} baseline: {error}."),
            recovery,
        )
    })? {
        let entry = entry.map_err(|error| {
            provisioning_error(
                format!("copy {label}"),
                format!("Cannot inspect {label} baseline entry: {error}."),
                recovery,
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            provisioning_error(
                format!("copy {label}"),
                format!("Cannot inspect {label} baseline file type: {error}."),
                recovery,
            )
        })?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target, label, recovery)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target).map_err(|error| {
                provisioning_error(
                    format!("copy {label}"),
                    format!("Cannot copy {label} baseline file: {error}."),
                    "Check application-data permissions/free disk space and retry.",
                )
            })?;
        } else {
            return Err(provisioning_error(
                format!("copy {label}"),
                format!("Pinned {label} baseline contains an unsupported filesystem entry."),
                recovery,
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
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if wordpress_http_probe_once(port) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(150));
    }
}

fn wordpress_http_probe_once(port: u16) -> bool {
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

const WOOCOMMERCE_ACTIVATION_BOOTSTRAP: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
$expectedVersion = getenv('COFFEEPOS_WOOCOMMERCE_VERSION');
$expectedDbVersion = getenv('COFFEEPOS_WOOCOMMERCE_DB_VERSION');
$applyBaseline = getenv('COFFEEPOS_WOOCOMMERCE_APPLY_BASELINE') === '1';

if (!$siteRoot || !$expectedVersion || !$expectedDbVersion) {
    fwrite(STDERR, "CoffeePOS WooCommerce activation environment is incomplete.\n");
    exit(2);
}

require_once $siteRoot . '/wp-load.php';
require_once ABSPATH . 'wp-admin/includes/plugin.php';

$plugin = 'woocommerce/woocommerce.php';
$pluginFile = WP_PLUGIN_DIR . '/woocommerce/woocommerce.php';
if (!is_file($pluginFile)) {
    fwrite(STDERR, "Managed WooCommerce entry file is missing.\n");
    exit(3);
}

$pluginData = get_plugin_data($pluginFile, false, false);
$pluginVersion = isset($pluginData['Version']) ? trim((string) $pluginData['Version']) : '';
if ($pluginVersion !== $expectedVersion) {
    fwrite(STDERR, "WooCommerce plugin header version mismatch: {$pluginVersion}; expected {$expectedVersion}.\n");
    exit(4);
}

add_filter('woocommerce_enable_setup_wizard', '__return_false');
add_filter('woocommerce_prevent_automatic_wizard_redirect', '__return_true');
add_filter('woocommerce_enable_auto_update_db', '__return_true');

if (!is_plugin_active($plugin)) {
    $result = activate_plugin($plugin, '', false, false);
    if (is_wp_error($result)) {
        fwrite(STDERR, "WooCommerce activation failed: " . $result->get_error_message() . "\n");
        exit(5);
    }
}

if (!is_plugin_active($plugin)) {
    fwrite(STDERR, "WooCommerce is not listed as an active WordPress plugin after activation.\n");
    exit(6);
}
if (!function_exists('WC') || !class_exists('WooCommerce') || !class_exists('WC_Install')) {
    fwrite(STDERR, "WooCommerce runtime classes are unavailable after activation.\n");
    exit(7);
}
if ((string) WC()->version !== $expectedVersion) {
    fwrite(STDERR, "Loaded WooCommerce version does not match the pinned artifact.\n");
    exit(8);
}

global $wpdb;
$requiredTables = array(
    $wpdb->prefix . 'woocommerce_sessions',
    $wpdb->prefix . 'woocommerce_order_items',
    $wpdb->prefix . 'woocommerce_order_itemmeta',
    $wpdb->prefix . 'wc_product_meta_lookup',
    $wpdb->prefix . 'actionscheduler_actions',
    $wpdb->prefix . 'actionscheduler_claims',
    $wpdb->prefix . 'actionscheduler_groups',
    $wpdb->prefix . 'actionscheduler_logs',
);
$requiredPages = array('shop', 'cart', 'checkout', 'myaccount');

$tableExists = static function (string $table) use ($wpdb): bool {
    $found = $wpdb->get_var($wpdb->prepare('SHOW TABLES LIKE %s', $wpdb->esc_like($table)));
    return $found === $table;
};
$baselineIssues = static function () use ($expectedVersion, $expectedDbVersion, $requiredTables, $requiredPages, $tableExists): array {
    $issues = array();
    if ((string) get_option('woocommerce_version', '') !== $expectedVersion) {
        $issues[] = 'woocommerce_version=' . (string) get_option('woocommerce_version', '');
    }
    if ((string) get_option('woocommerce_db_version', '') !== $expectedDbVersion) {
        $issues[] = 'woocommerce_db_version=' . (string) get_option('woocommerce_db_version', '');
    }
    if (WC_Install::needs_db_update()) {
        $issues[] = 'woocommerce_db_update_pending';
    }
    foreach ($requiredTables as $table) {
        if (!$tableExists($table)) {
            $issues[] = 'missing_table:' . $table;
        }
    }
    foreach ($requiredPages as $page) {
        if (wc_get_page_id($page) <= 0) {
            $issues[] = 'missing_page:' . $page;
        }
    }
    if (get_role('shop_manager') === null) {
        $issues[] = 'missing_role:shop_manager';
    }
    return $issues;
};
$baselineReady = static function () use ($baselineIssues): bool {
    return count($baselineIssues()) === 0;
};

if (!$baselineReady()) {
    WC_Install::install();
}

if (!$tableExists($wpdb->prefix . 'actionscheduler_actions') && class_exists('ActionScheduler_StoreSchema')) {
    $schema = new ActionScheduler_StoreSchema();
    $schema->init();
    $schema->register_tables(true);
}
if (!$tableExists($wpdb->prefix . 'actionscheduler_logs') && class_exists('ActionScheduler_LoggerSchema')) {
    $schema = new ActionScheduler_LoggerSchema();
    $schema->init();
    $schema->register_tables(true);
}

if ($applyBaseline) {
    $profile = get_option('woocommerce_onboarding_profile', array());
    if (!is_array($profile)) {
        $profile = array();
    }
    if (empty($profile['completed'])) {
        $profile['skipped'] = true;
        update_option('woocommerce_onboarding_profile', $profile);
    }
    $hiddenLists = get_option('woocommerce_task_list_hidden_lists', array());
    if (!is_array($hiddenLists)) {
        $hiddenLists = array();
    }
    if (!in_array('setup', $hiddenLists, true)) {
        $hiddenLists[] = 'setup';
        update_option('woocommerce_task_list_hidden_lists', array_values(array_unique($hiddenLists)));
    }
    update_option('woocommerce_show_marketplace_suggestions', 'no');
    update_option('woocommerce_allow_tracking', 'no');
}
delete_transient('_wc_activation_redirect');

if (!$baselineReady()) {
    fwrite(STDERR, "WooCommerce activation completed but required baseline is not ready: " . implode(', ', $baselineIssues()) . "\n");
    exit(9);
}

$profile = get_option('woocommerce_onboarding_profile', array());
if (!is_array($profile) || (empty($profile['completed']) && empty($profile['skipped']))) {
    fwrite(STDERR, "WooCommerce onboarding still requires operator interaction.\n");
    exit(10);
}

fwrite(STDOUT, "WooCommerce {$expectedVersion} active; schema/pages/Action Scheduler/onboarding baseline verified.\n");
exit(0);
"#;

const COFFEEPOS_ACTIVATION_BOOTSTRAP: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
$expectedVersion = getenv('COFFEEPOS_EXPECTED_VERSION');
$expectedWooCommerceVersion = getenv('COFFEEPOS_EXPECTED_WOOCOMMERCE_VERSION');
$applyBaseline = getenv('COFFEEPOS_APPLY_ACTIVATION_BASELINE') === '1';

if (!$siteRoot || !$expectedVersion || !$expectedWooCommerceVersion) {
    fwrite(STDERR, "CoffeePOS activation environment is incomplete.\n");
    exit(2);
}

require_once $siteRoot . '/wp-load.php';
require_once ABSPATH . 'wp-admin/includes/plugin.php';

$wooCommercePlugin = 'woocommerce/woocommerce.php';
$wooCommerceFile = WP_PLUGIN_DIR . '/woocommerce/woocommerce.php';
$coffeePosPlugin = 'coffeepos/coffeepos.php';
$coffeePosFile = WP_PLUGIN_DIR . '/coffeepos/coffeepos.php';

if (!is_file($wooCommerceFile) || !is_plugin_active($wooCommercePlugin)) {
    fwrite(STDERR, "CoffeePOS requires the managed WooCommerce plugin to be installed and active before activation.\n");
    exit(3);
}
$wooCommerceData = get_plugin_data($wooCommerceFile, false, false);
$wooCommerceVersion = isset($wooCommerceData['Version']) ? trim((string) $wooCommerceData['Version']) : '';
if ($wooCommerceVersion !== $expectedWooCommerceVersion) {
    fwrite(STDERR, "WooCommerce plugin header version mismatch: {$wooCommerceVersion}; expected {$expectedWooCommerceVersion}.\n");
    exit(4);
}
if (!defined('WC_VERSION') || (string) WC_VERSION !== $expectedWooCommerceVersion || !class_exists('WooCommerce')) {
    fwrite(STDERR, "The active WooCommerce runtime does not match the pinned dependency.\n");
    exit(5);
}

if (!is_file($coffeePosFile)) {
    fwrite(STDERR, "Managed CoffeePOS entry file is missing.\n");
    exit(6);
}
$coffeePosData = get_plugin_data($coffeePosFile, false, false);
$coffeePosVersion = isset($coffeePosData['Version']) ? trim((string) $coffeePosData['Version']) : '';
if ($coffeePosVersion !== $expectedVersion) {
    fwrite(STDERR, "CoffeePOS plugin header version mismatch: {$coffeePosVersion}; expected {$expectedVersion}.\n");
    exit(7);
}

$wasActive = is_plugin_active($coffeePosPlugin);
if (!$wasActive) {
    $result = activate_plugin($coffeePosPlugin, '', false, false);
    if (is_wp_error($result)) {
        fwrite(STDERR, "CoffeePOS activation failed: " . $result->get_error_message() . "\n");
        exit(8);
    }
}
if (!is_plugin_active($coffeePosPlugin)) {
    fwrite(STDERR, "CoffeePOS is not listed as an active WordPress plugin after activation.\n");
    exit(9);
}
if (!defined('COFFEEPOS_VERSION') || (string) COFFEEPOS_VERSION !== $expectedVersion) {
    fwrite(STDERR, "Loaded CoffeePOS version does not match the pinned artifact.\n");
    exit(10);
}
if (!class_exists('\\CoffeePOS\\Core\\Lifecycle')) {
    fwrite(STDERR, "CoffeePOS lifecycle class is unavailable after activation.\n");
    exit(11);
}
if ($wasActive && $applyBaseline) {
    \CoffeePOS\Core\Lifecycle::activate();
}

fwrite(STDOUT, "CoffeePOS {$expectedVersion} activation lifecycle completed.\n");
exit(0);
"#;

const COFFEEPOS_ACTIVATION_VERIFY_BOOTSTRAP: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
$expectedVersion = getenv('COFFEEPOS_EXPECTED_VERSION');
$expectedWooCommerceVersion = getenv('COFFEEPOS_EXPECTED_WOOCOMMERCE_VERSION');

if (!$siteRoot || !$expectedVersion || !$expectedWooCommerceVersion) {
    fwrite(STDERR, "CoffeePOS activation verification environment is incomplete.\n");
    exit(2);
}

require_once $siteRoot . '/wp-load.php';
require_once ABSPATH . 'wp-admin/includes/plugin.php';

$wooCommercePlugin = 'woocommerce/woocommerce.php';
$wooCommerceFile = WP_PLUGIN_DIR . '/woocommerce/woocommerce.php';
$coffeePosPlugin = 'coffeepos/coffeepos.php';
$coffeePosFile = WP_PLUGIN_DIR . '/coffeepos/coffeepos.php';

if (!is_file($wooCommerceFile) || !is_plugin_active($wooCommercePlugin)) {
    fwrite(STDERR, "CoffeePOS verification requires the managed WooCommerce plugin to remain active.\n");
    exit(3);
}
$wooCommerceData = get_plugin_data($wooCommerceFile, false, false);
$wooCommerceVersion = isset($wooCommerceData['Version']) ? trim((string) $wooCommerceData['Version']) : '';
if ($wooCommerceVersion !== $expectedWooCommerceVersion) {
    fwrite(STDERR, "WooCommerce plugin header version mismatch during CoffeePOS verification.\n");
    exit(4);
}
if (!defined('WC_VERSION') || (string) WC_VERSION !== $expectedWooCommerceVersion || !class_exists('WooCommerce')) {
    fwrite(STDERR, "The fresh WordPress process did not load the pinned WooCommerce dependency.\n");
    exit(5);
}
if (!is_file($coffeePosFile) || !is_plugin_active($coffeePosPlugin)) {
    fwrite(STDERR, "CoffeePOS is not active in the fresh WordPress verification process.\n");
    exit(6);
}
$coffeePosData = get_plugin_data($coffeePosFile, false, false);
$coffeePosVersion = isset($coffeePosData['Version']) ? trim((string) $coffeePosData['Version']) : '';
if ($coffeePosVersion !== $expectedVersion) {
    fwrite(STDERR, "CoffeePOS plugin header version mismatch during verification.\n");
    exit(7);
}
if (!defined('COFFEEPOS_VERSION') || (string) COFFEEPOS_VERSION !== $expectedVersion) {
    fwrite(STDERR, "The fresh WordPress process did not load the pinned CoffeePOS version.\n");
    exit(8);
}

$requiredClasses = array(
    '\\CoffeePOS\\Core\\Lifecycle',
    '\\CoffeePOS\\Core\\Bootstrap',
    '\\CoffeePOS\\Infrastructure\\Database\\Migrator',
    '\\CoffeePOS\\Infrastructure\\Database\\Schema',
    '\\CoffeePOS\\Infrastructure\\Settings\\Settings',
    '\\CoffeePOS\\POS\\Router',
    '\\CoffeePOS\\REST\\RouteRegistrar',
    '\\CoffeePOS\\Support\\Capabilities',
);
foreach ($requiredClasses as $requiredClass) {
    if (!class_exists($requiredClass)) {
        fwrite(STDERR, "CoffeePOS runtime class is unavailable in the fresh process: {$requiredClass}.\n");
        exit(9);
    }
}

$server = rest_get_server();
$routes = $server->get_routes();

global $wpdb;
$tableExists = static function (string $table) use ($wpdb): bool {
    $found = $wpdb->get_var($wpdb->prepare('SHOW TABLES LIKE %s', $wpdb->esc_like($table)));
    return $found === $table;
};
$baselineIssues = static function () use ($expectedVersion, $tableExists, $routes): array {
    $issues = array();
    if ((string) get_option('coffeepos_installed_version', '') !== $expectedVersion) {
        $issues[] = 'installed_version=' . (string) get_option('coffeepos_installed_version', '');
    }
    if ((string) get_option(\CoffeePOS\Infrastructure\Database\Migrator::OPTION_DB_VERSION, '') !== \CoffeePOS\Infrastructure\Database\Migrator::SCHEMA_VERSION) {
        $issues[] = 'db_schema=' . (string) get_option(\CoffeePOS\Infrastructure\Database\Migrator::OPTION_DB_VERSION, '');
    }
    foreach (\CoffeePOS\Infrastructure\Database\Schema::tableNames($GLOBALS['wpdb']->prefix) as $table) {
        if (!$tableExists($table)) {
            $issues[] = 'missing_table:' . $table;
        }
    }
    $missingOption = new stdClass();
    foreach (\CoffeePOS\Infrastructure\Settings\Settings::optionNames() as $optionName) {
        if (get_option($optionName, $missingOption) === $missingOption) {
            $issues[] = 'missing_setting:' . $optionName;
        }
    }
    $pluginCapabilities = \CoffeePOS\Support\Capabilities::all();
    foreach (array('coffeepos_cashier', 'coffeepos_kitchen', 'coffeepos_supervisor', 'coffeepos_manager') as $roleName) {
        $role = get_role($roleName);
        if ($role === null) {
            $issues[] = 'missing_role:' . $roleName;
            continue;
        }
        if (!$role->has_cap('read')) {
            $issues[] = 'missing_capability:' . $roleName . ':read';
        }
        $hasCoffeePosCapability = false;
        foreach ($pluginCapabilities as $capability) {
            if ($role->has_cap($capability)) {
                $hasCoffeePosCapability = true;
                break;
            }
        }
        if (!$hasCoffeePosCapability) {
            $issues[] = 'missing_coffeepos_capability:' . $roleName;
        }
    }
    foreach (array('administrator', 'shop_manager') as $roleName) {
        $role = get_role($roleName);
        if ($role === null) {
            $issues[] = 'missing_role:' . $roleName;
            continue;
        }
        foreach ($pluginCapabilities as $capability) {
            if (!$role->has_cap($capability)) {
                $issues[] = 'missing_capability:' . $roleName . ':' . $capability;
            }
        }
    }
    if ((string) get_option('coffeepos_rewrite_version', '') !== \CoffeePOS\POS\Router::rewriteVersion()) {
        $issues[] = 'rewrite_version=' . (string) get_option('coffeepos_rewrite_version', '');
    }
    $rewriteRules = get_option('rewrite_rules', array());
    $hasPosRewrite = false;
    foreach (is_array($rewriteRules) ? $rewriteRules : array() as $rewriteTarget) {
        if (is_string($rewriteTarget) && str_contains($rewriteTarget, 'coffeepos_screen=entry')) {
            $hasPosRewrite = true;
            break;
        }
    }
    if (!$hasPosRewrite) {
        $issues[] = 'missing_pos_rewrite';
    }
    if (!isset($routes['/coffeepos/v1/health'])) {
        $issues[] = 'missing_rest_route:/coffeepos/v1/health';
    }
    $declaredRoutes = \CoffeePOS\REST\RouteRegistrar::registeredRoutes();
    if ($declaredRoutes === []) {
        $issues[] = 'empty_declared_rest_routes';
    }
    foreach ($declaredRoutes as $declaredRoute) {
        $routeKey = '/' . ltrim((string) $declaredRoute, '/');
        if (!isset($routes[$routeKey])) {
            $issues[] = 'missing_declared_rest_route:' . $routeKey;
        }
    }
    return $issues;
};

$issues = $baselineIssues();
if ($issues !== []) {
    fwrite(STDERR, "CoffeePOS fresh-process activation baseline is not ready: " . implode(', ', $issues) . "\n");
    exit(10);
}

fwrite(
    STDOUT,
    "CoffeePOS {$expectedVersion} active; plugin-owned schema/settings/capabilities/rewrite/REST baseline verified.\n"
);
exit(0);
"#;

const COFFEEPOS_MACHINE_HEALTH_BOOTSTRAP: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
if (!$siteRoot) {
    fwrite(STDERR, "CoffeePOS machine-health bootstrap environment is incomplete.\n");
    exit(2);
}
$token = trim((string) stream_get_contents(STDIN));
if (strlen($token) !== 64 || !preg_match('/^[0-9a-f]{64}$/', $token)) {
    fwrite(STDERR, "CoffeePOS machine credential format is invalid.\n");
    exit(3);
}

require_once $siteRoot . '/wp-load.php';

if (!defined('COFFEEPOS_VERSION') || !class_exists('\\CoffeePOS\\REST\\SystemStatusController')) {
    fwrite(STDERR, "CoffeePOS machine-health controller is unavailable.\n");
    exit(4);
}

$hash = hash('sha256', $token);
$token = '';
$option = \CoffeePOS\REST\SystemStatusController::OPTION_MACHINE_TOKEN_HASH;
$stored = strtolower(trim((string) get_option($option, '')));
if ($stored === '') {
    if (!update_option($option, $hash, false)) {
        $stored = strtolower(trim((string) get_option($option, '')));
        if (!hash_equals($stored, $hash)) {
            fwrite(STDERR, "CoffeePOS machine credential hash could not be persisted.\n");
            exit(5);
        }
    }
} elseif (!preg_match('/^[0-9a-f]{64}$/', $stored)) {
    fwrite(STDERR, "Stored CoffeePOS machine credential hash is invalid; refusing implicit reset.\n");
    exit(6);
} elseif (!hash_equals($stored, $hash)) {
    fwrite(STDERR, "Stored CoffeePOS machine credential differs; explicit rotation/repair is required.\n");
    exit(7);
}
$hash = '';
fwrite(STDOUT, "CoffeePOS machine credential hash persisted.\n");
exit(0);
"#;

#[cfg_attr(not(test), allow(dead_code))]
const COFFEEPOS_MACHINE_TOKEN_SWITCH_BOOTSTRAP: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
if (!$siteRoot) {
    fwrite(STDERR, "CoffeePOS machine-credential rotation environment is incomplete.\n");
    exit(2);
}
$input = (string) stream_get_contents(STDIN);
$lines = preg_split('/\R/', trim($input));
$input = '';
if (!is_array($lines) || count($lines) < 2) {
    fwrite(STDERR, "CoffeePOS machine-credential rotation input is incomplete.\n");
    exit(3);
}
$expected = trim((string) $lines[0]);
$replacement = trim((string) $lines[1]);
$lines = array();
foreach (array($expected, $replacement) as $token) {
    if (strlen($token) !== 64 || !preg_match('/^[0-9a-f]{64}$/', $token)) {
        fwrite(STDERR, "CoffeePOS machine-credential rotation token format is invalid.\n");
        exit(4);
    }
}

require_once $siteRoot . '/wp-load.php';
if (!class_exists('\\CoffeePOS\\REST\\SystemStatusController')) {
    fwrite(STDERR, "CoffeePOS machine-health controller is unavailable.\n");
    exit(5);
}

$expectedHash = hash('sha256', $expected);
$replacementHash = hash('sha256', $replacement);
$expected = '';
$replacement = '';
$option = \CoffeePOS\REST\SystemStatusController::OPTION_MACHINE_TOKEN_HASH;
$stored = strtolower(trim((string) get_option($option, '')));
if (hash_equals($stored, $replacementHash)) {
    fwrite(STDOUT, "CoffeePOS machine credential already switched.\n");
    exit(0);
}
if (!preg_match('/^[0-9a-f]{64}$/', $stored) || !hash_equals($stored, $expectedHash)) {
    fwrite(STDERR, "Stored CoffeePOS machine credential does not match the expected active credential.\n");
    exit(6);
}
if (!update_option($option, $replacementHash, false)) {
    $stored = strtolower(trim((string) get_option($option, '')));
    if (!hash_equals($stored, $replacementHash)) {
        fwrite(STDERR, "CoffeePOS machine credential could not be switched.\n");
        exit(7);
    }
}
$expectedHash = '';
$replacementHash = '';
fwrite(STDOUT, "CoffeePOS machine credential switched.\n");
exit(0);
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
        resolve_development_manifest, CoffeePosHealthFailureKind, CoffeePosHealthState,
        RuntimeManager, RuntimeState, WordPressHealthState,
    };

    #[test]
    fn sql_literal_escapes_quotes_and_backslashes() {
        assert_eq!(sql_literal("a'b\\c"), "a''b\\\\c");
    }

    #[cfg(windows)]
    #[test]
    fn installed_wordpress_admin_secret_is_never_regenerated() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wordpress-admin.secret");
        let original = load_or_create_wordpress_admin_secret(&path, false).unwrap();
        assert_eq!(secret::load(&path).unwrap(), original);

        fs::remove_file(&path).unwrap();
        let error = load_or_create_wordpress_admin_secret(&path, true).unwrap_err();
        assert!(error.contains("Cannot read protected database credential"));
        assert!(!path.exists());
    }

    #[test]
    fn staging_reset_refuses_unowned_path() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path();
        assert!(reset_owned_staging_dir(data, &data.join("database")).is_err());
        assert!(reset_owned_staging_dir(data, &data.join("site.provisioning")).is_ok());
        assert!(reset_owned_staging_dir(data, &data.join("coffeepos.provisioning")).is_ok());
    }

    #[test]
    fn phase_4_6_journal_remains_backward_compatible_and_incomplete_for_phase_4_8() {
        let bytes = br#"{
  "schema_version": 1,
  "wordpress_version": "7.1",
  "woocommerce_version": "11.1.0",
  "stage": "woo_commerce_activated",
  "admin_username": "coffeepos_admin"
}"#;
        let journal: ProvisioningJournal = serde_json::from_slice(bytes).unwrap();
        assert_eq!(journal.stage, ProvisioningStage::WooCommerceActivated);
        assert_eq!(journal.woocommerce_version.as_deref(), Some("11.1.0"));
        assert!(journal.coffeepos_version.is_none());
        assert!(journal.stage < ProvisioningStage::CoffeePosProvisioned);
    }

    #[test]
    fn phase_4_8_journal_remains_backward_compatible_and_incomplete_for_phase_4_9() {
        let bytes = br#"{
  "schema_version": 1,
  "wordpress_version": "7.1",
  "woocommerce_version": "11.1.0",
  "coffeepos_version": "1.0.0",
  "stage": "coffee_pos_provisioned",
  "admin_username": "coffeepos_admin"
}"#;
        let journal: ProvisioningJournal = serde_json::from_slice(bytes).unwrap();
        assert_eq!(journal.stage, ProvisioningStage::CoffeePosProvisioned);
        assert_eq!(journal.coffeepos_version.as_deref(), Some("1.0.0"));
        assert!(journal.stage < ProvisioningStage::CoffeePosActivated);
    }

    #[test]
    fn phase_4_9_journal_remains_backward_compatible_and_incomplete_for_phase_4_10() {
        let bytes = br#"{
  "schema_version": 1,
  "wordpress_version": "7.1",
  "woocommerce_version": "11.1.0",
  "coffeepos_version": "1.0.0",
  "stage": "coffee_pos_activated",
  "admin_username": "coffeepos_admin"
}"#;
        let journal: ProvisioningJournal = serde_json::from_slice(bytes).unwrap();
        assert_eq!(journal.stage, ProvisioningStage::CoffeePosActivated);
        assert_eq!(journal.coffeepos_version.as_deref(), Some("1.0.0"));
        assert!(journal.stage < ProvisioningStage::MachineHealthBootstrapped);
    }

    #[cfg(windows)]
    fn query_wordpress_option(
        provisioner: &Provisioner,
        database_port: u16,
        option_name: &str,
    ) -> String {
        let password =
            secret::load(&provisioner.data_root.join(DATABASE_WORDPRESS_SECRET)).unwrap();
        let endpoint = DatabaseEndpoint::Tcp(database_port);
        let mut command =
            provisioner.database_client_command(&endpoint, DATABASE_WORDPRESS_USER, &password);
        command
            .arg(format!("--database={DATABASE_NAME}"))
            .arg(format!(
                "--execute=SELECT option_value FROM wp_options WHERE option_name = '{}' LIMIT 1",
                sql_literal(option_name)
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_child_command(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "database option query failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[cfg(windows)]
    fn query_database_scalar(provisioner: &Provisioner, database_port: u16, sql: &str) -> String {
        let password =
            secret::load(&provisioner.data_root.join(DATABASE_WORDPRESS_SECRET)).unwrap();
        let endpoint = DatabaseEndpoint::Tcp(database_port);
        let mut command =
            provisioner.database_client_command(&endpoint, DATABASE_WORDPRESS_USER, &password);
        command
            .arg(format!("--database={DATABASE_NAME}"))
            .arg(format!("--execute={sql}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_child_command(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "database scalar query failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[cfg(windows)]
    fn http_get(port: u16, path: &str) -> String {
        http_get_with_headers(port, path, &[])
    }

    #[cfg(windows)]
    fn http_get_with_headers(port: u16, path: &str, headers: &[(&str, &str)]) -> String {
        let mut stream = std::net::TcpStream::connect((LOOPBACK, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = format!("GET {path} HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\n");
        for (name, value) in headers {
            request.push_str(name);
            request.push_str(": ");
            request.push_str(value);
            request.push_str("\r\n");
        }
        request.push_str("Connection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        let mut chunk = [0_u8; 8192];
        while response.len() < 2 * 1024 * 1024 {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => response.extend_from_slice(&chunk[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("HTTP acceptance read failed: {error}"),
            }
        }
        String::from_utf8_lossy(&response).into_owned()
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

    #[cfg(windows)]
    #[test]
    fn checked_in_woocommerce_manifest_matches_native_schema() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap();
        let bytes = fs::read(
            project_root.join("scripts/woocommerce-development/woocommerce-11.1.0.manifest.json"),
        )
        .unwrap();
        let manifest: WooCommerceDevelopmentManifest = serde_json::from_slice(&bytes).unwrap();
        validate_woocommerce_manifest(&manifest).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn woocommerce_manifest_rejects_path_escape() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap();
        let bytes = fs::read(
            project_root.join("scripts/woocommerce-development/woocommerce-11.1.0.manifest.json"),
        )
        .unwrap();
        let mut manifest: WooCommerceDevelopmentManifest = serde_json::from_slice(&bytes).unwrap();
        manifest.woocommerce.plugin_root = PathBuf::from("../escape");
        assert!(validate_woocommerce_manifest(&manifest).is_err());
    }

    #[test]
    fn woocommerce_provisioning_preserves_owned_plugin_on_retry() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("woocommerce.php"),
            b"<?php\n/**\n * Plugin Name: WooCommerce\n * Version: 11.1.0\n */\n",
        )
        .unwrap();
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();
        let artifact = ResolvedWooCommerce {
            version: "11.1.0".into(),
            plugin_root: source,
            archive_sha256: "6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19"
                .into(),
        };

        ensure_woocommerce_plugin(&data_root, &artifact).unwrap();
        let destination = data_root.join("site/wp-content/plugins/woocommerce");
        assert!(woocommerce_installation_ready(&data_root, &artifact));
        let sentinel = destination.join("preserve-user-file.txt");
        fs::write(&sentinel, b"preserve me").unwrap();

        ensure_woocommerce_plugin(&data_root, &artifact).unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve me");
    }

    #[test]
    fn woocommerce_provisioning_refuses_unmanaged_existing_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("woocommerce.php"),
            b"<?php\n/**\n * Plugin Name: WooCommerce\n * Version: 11.1.0\n */\n",
        )
        .unwrap();
        let data_root = temp.path().join("store");
        let destination = data_root.join("site/wp-content/plugins/woocommerce");
        fs::create_dir_all(&destination).unwrap();
        let sentinel = destination.join("unmanaged.txt");
        fs::write(&sentinel, b"do not overwrite").unwrap();
        let artifact = ResolvedWooCommerce {
            version: "11.1.0".into(),
            plugin_root: source,
            archive_sha256: "6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19"
                .into(),
        };

        let error = ensure_woocommerce_plugin(&data_root, &artifact).unwrap_err();
        assert_eq!(error.operation, "provision WooCommerce");
        assert_eq!(fs::read(&sentinel).unwrap(), b"do not overwrite");
        assert!(!destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE).exists());
    }

    #[cfg(windows)]
    #[test]
    fn checked_in_coffeepos_manifest_matches_native_schema() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap();
        let bytes = fs::read(
            project_root.join("scripts/coffeepos-development/coffeepos-1.0.1.manifest.json"),
        )
        .unwrap();
        let manifest: CoffeePosDevelopmentManifest = serde_json::from_slice(&bytes).unwrap();
        validate_coffeepos_manifest(&manifest).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn coffeepos_manifest_rejects_path_escape() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap();
        let bytes = fs::read(
            project_root.join("scripts/coffeepos-development/coffeepos-1.0.1.manifest.json"),
        )
        .unwrap();
        let mut manifest: CoffeePosDevelopmentManifest = serde_json::from_slice(&bytes).unwrap();
        manifest.coffeepos.plugin_root = PathBuf::from("../escape");
        assert!(validate_coffeepos_manifest(&manifest).is_err());
    }

    fn create_test_coffeepos_source(root: &Path) {
        create_test_coffeepos_source_version(root, "1.0.0");
    }

    fn create_test_coffeepos_source_version(root: &Path, version: &str) {
        for directory in ["assets", "includes", "languages", "templates", "vendor"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(
            root.join("coffeepos.php"),
            format!(
                "<?php\n/**\n * Plugin Name: CoffeePOS\n * Version: {version}\n * Requires at least: 6.4\n * Requires PHP: 7.4\n * Requires Plugins: woocommerce\n */\n"
            ),
        )
        .unwrap();
        fs::write(root.join("readme.txt"), format!("Stable tag: {version}\n")).unwrap();
        fs::write(root.join("LICENSE"), b"GPL-2.0-or-later\n").unwrap();
        fs::write(root.join("vendor/autoload.php"), b"<?php\n").unwrap();
    }

    #[test]
    fn coffeepos_phase_4_10_upgrades_only_exact_managed_phase_4_9_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let woocommerce_source = temp.path().join("woocommerce-source");
        fs::create_dir_all(&woocommerce_source).unwrap();
        fs::write(
            woocommerce_source.join("woocommerce.php"),
            b"<?php\n/**\n * Plugin Name: WooCommerce\n * Version: 11.1.0\n */\n",
        )
        .unwrap();
        let old_source = temp.path().join("coffeepos-old");
        let new_source = temp.path().join("coffeepos-new");
        create_test_coffeepos_source_version(&old_source, COFFEEPOS_PHASE_4_9_VERSION);
        create_test_coffeepos_source_version(&new_source, COFFEEPOS_PHASE_4_10_VERSION);
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();
        let woocommerce = ResolvedWooCommerce {
            version: "11.1.0".into(),
            plugin_root: woocommerce_source,
            archive_sha256: "6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19"
                .into(),
        };
        let old = ResolvedCoffeePos {
            version: COFFEEPOS_PHASE_4_9_VERSION.into(),
            plugin_root: old_source,
            archive_sha256: COFFEEPOS_PHASE_4_9_SHA256.into(),
            required_wordpress_version: "7.1".into(),
            required_php_version: "8.4.25".into(),
            required_mariadb_version: "11.4.13".into(),
            required_woocommerce_version: "11.1.0".into(),
        };
        let new = ResolvedCoffeePos {
            version: COFFEEPOS_PHASE_4_10_VERSION.into(),
            plugin_root: new_source,
            archive_sha256: COFFEEPOS_PHASE_4_10_SHA256.into(),
            required_wordpress_version: "7.1".into(),
            required_php_version: "8.4.25".into(),
            required_mariadb_version: "11.4.13".into(),
            required_woocommerce_version: "11.1.0".into(),
        };
        ensure_woocommerce_plugin(&data_root, &woocommerce).unwrap();
        ensure_coffeepos_plugin(&data_root, &old, &woocommerce).unwrap();
        let site_sentinel = data_root.join("site/keep-store-data.txt");
        fs::write(&site_sentinel, b"preserve store data").unwrap();

        ensure_coffeepos_plugin(&data_root, &new, &woocommerce).unwrap();

        let destination = data_root.join("site/wp-content/plugins/coffeepos");
        assert!(coffeepos_installation_ready(&data_root, &new));
        assert_eq!(
            read_coffeepos_plugin_header_version(&destination.join("coffeepos.php")).unwrap(),
            COFFEEPOS_PHASE_4_10_VERSION
        );
        assert_eq!(fs::read(site_sentinel).unwrap(), b"preserve store data");
        assert!(!data_root.join(COFFEEPOS_UPGRADE_BACKUP).exists());
        assert!(!data_root.join("coffeepos.provisioning").exists());
    }

    #[test]
    fn coffeepos_provisioning_preserves_owned_plugin_on_retry() {
        let temp = tempfile::tempdir().unwrap();
        let woocommerce_source = temp.path().join("woocommerce-source");
        fs::create_dir_all(&woocommerce_source).unwrap();
        fs::write(
            woocommerce_source.join("woocommerce.php"),
            b"<?php\n/**\n * Plugin Name: WooCommerce\n * Version: 11.1.0\n */\n",
        )
        .unwrap();
        let coffeepos_source = temp.path().join("coffeepos-source");
        create_test_coffeepos_source(&coffeepos_source);
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();
        let woocommerce = ResolvedWooCommerce {
            version: "11.1.0".into(),
            plugin_root: woocommerce_source,
            archive_sha256: "6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19"
                .into(),
        };
        let coffeepos = ResolvedCoffeePos {
            version: "1.0.0".into(),
            plugin_root: coffeepos_source,
            archive_sha256: "ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a"
                .into(),
            required_wordpress_version: "7.1".into(),
            required_php_version: "8.4.25".into(),
            required_mariadb_version: "11.4.13".into(),
            required_woocommerce_version: "11.1.0".into(),
        };
        ensure_woocommerce_plugin(&data_root, &woocommerce).unwrap();
        ensure_coffeepos_plugin(&data_root, &coffeepos, &woocommerce).unwrap();
        let destination = data_root.join("site/wp-content/plugins/coffeepos");
        assert!(coffeepos_installation_ready(&data_root, &coffeepos));
        let sentinel = destination.join("preserve-user-file.txt");
        fs::write(&sentinel, b"preserve me").unwrap();

        ensure_coffeepos_plugin(&data_root, &coffeepos, &woocommerce).unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve me");
    }

    #[test]
    fn coffeepos_provisioning_refuses_corrupt_managed_plugin_without_recopy() {
        let temp = tempfile::tempdir().unwrap();
        let woocommerce_source = temp.path().join("woocommerce-source");
        fs::create_dir_all(&woocommerce_source).unwrap();
        fs::write(
            woocommerce_source.join("woocommerce.php"),
            b"<?php\n/**\n * Plugin Name: WooCommerce\n * Version: 11.1.0\n */\n",
        )
        .unwrap();
        let coffeepos_source = temp.path().join("coffeepos-source");
        create_test_coffeepos_source(&coffeepos_source);
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();
        let woocommerce = ResolvedWooCommerce {
            version: "11.1.0".into(),
            plugin_root: woocommerce_source,
            archive_sha256: "6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19"
                .into(),
        };
        let coffeepos = ResolvedCoffeePos {
            version: "1.0.0".into(),
            plugin_root: coffeepos_source,
            archive_sha256: "ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a"
                .into(),
            required_wordpress_version: "7.1".into(),
            required_php_version: "8.4.25".into(),
            required_mariadb_version: "11.4.13".into(),
            required_woocommerce_version: "11.1.0".into(),
        };
        ensure_woocommerce_plugin(&data_root, &woocommerce).unwrap();
        ensure_coffeepos_plugin(&data_root, &coffeepos, &woocommerce).unwrap();
        let destination = data_root.join("site/wp-content/plugins/coffeepos");
        let sentinel = destination.join("preserve-user-file.txt");
        fs::write(&sentinel, b"preserve me").unwrap();
        fs::remove_file(destination.join("vendor/autoload.php")).unwrap();

        assert!(!coffeepos_installation_ready(&data_root, &coffeepos));
        let error = ensure_coffeepos_plugin(&data_root, &coffeepos, &woocommerce).unwrap_err();
        assert_eq!(error.operation, "provision CoffeePOS");
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve me");
        assert!(!destination.join("vendor/autoload.php").exists());
    }

    #[test]
    fn coffeepos_provisioning_refuses_unmanaged_existing_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let woocommerce_source = temp.path().join("woocommerce-source");
        fs::create_dir_all(&woocommerce_source).unwrap();
        fs::write(
            woocommerce_source.join("woocommerce.php"),
            b"<?php\n/**\n * Plugin Name: WooCommerce\n * Version: 11.1.0\n */\n",
        )
        .unwrap();
        let coffeepos_source = temp.path().join("coffeepos-source");
        fs::create_dir_all(&coffeepos_source).unwrap();
        fs::write(
            coffeepos_source.join("coffeepos.php"),
            b"<?php\n/**\n * Plugin Name: CoffeePOS\n * Version: 1.0.0\n * Requires Plugins: woocommerce\n */\n",
        )
        .unwrap();
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();
        let woocommerce = ResolvedWooCommerce {
            version: "11.1.0".into(),
            plugin_root: woocommerce_source,
            archive_sha256: "6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19"
                .into(),
        };
        let coffeepos = ResolvedCoffeePos {
            version: "1.0.0".into(),
            plugin_root: coffeepos_source,
            archive_sha256: "ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a"
                .into(),
            required_wordpress_version: "7.1".into(),
            required_php_version: "8.4.25".into(),
            required_mariadb_version: "11.4.13".into(),
            required_woocommerce_version: "11.1.0".into(),
        };
        ensure_woocommerce_plugin(&data_root, &woocommerce).unwrap();
        let destination = data_root.join("site/wp-content/plugins/coffeepos");
        fs::create_dir_all(&destination).unwrap();
        let sentinel = destination.join("unmanaged.txt");
        fs::write(&sentinel, b"do not overwrite").unwrap();

        let error = ensure_coffeepos_plugin(&data_root, &coffeepos, &woocommerce).unwrap_err();
        assert_eq!(error.operation, "provision CoffeePOS");
        assert_eq!(fs::read(&sentinel).unwrap(), b"do not overwrite");
        assert!(!destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE).exists());
    }

    #[test]
    fn coffeepos_provisioning_requires_managed_woocommerce_dependency() {
        let temp = tempfile::tempdir().unwrap();
        let coffeepos_source = temp.path().join("coffeepos-source");
        fs::create_dir_all(&coffeepos_source).unwrap();
        fs::write(
            coffeepos_source.join("coffeepos.php"),
            b"<?php\n/**\n * Plugin Name: CoffeePOS\n * Version: 1.0.0\n * Requires Plugins: woocommerce\n */\n",
        )
        .unwrap();
        let woocommerce = ResolvedWooCommerce {
            version: "11.1.0".into(),
            plugin_root: temp.path().join("unused-woocommerce-source"),
            archive_sha256: "6bae9bf74d722b6deb15f049687c311cfafc26e3a5d8fa55ac6ea4b9a3a8df19"
                .into(),
        };
        let coffeepos = ResolvedCoffeePos {
            version: "1.0.0".into(),
            plugin_root: coffeepos_source,
            archive_sha256: "ee9f241a516e7c6ddddc6e84ad26515d0a9cd9ccd5d6fd101d078a738e598f2a"
                .into(),
            required_wordpress_version: "7.1".into(),
            required_php_version: "8.4.25".into(),
            required_mariadb_version: "11.4.13".into(),
            required_woocommerce_version: "11.1.0".into(),
        };
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();

        let error = ensure_coffeepos_plugin(&data_root, &coffeepos, &woocommerce).unwrap_err();
        assert_eq!(error.operation, "provision CoffeePOS");
        assert!(!data_root.join("site/wp-content/plugins/coffeepos").exists());
    }

    #[test]
    fn provisioning_info_serializes_ui_state_and_retry_contract() {
        let value = serde_json::to_value(ProvisioningInfo {
            state: ProvisioningState::NeedsRepair,
            wordpress_version: "7.1".into(),
            woocommerce_version: "11.1.0".into(),
            woocommerce_active: false,
            coffeepos_version: "1.0.0".into(),
            coffeepos_active: false,
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
        assert_eq!(value["woocommerce_version"], "11.1.0");
        assert_eq!(value["woocommerce_active"], false);
        assert_eq!(value["coffeepos_version"], "1.0.0");
        assert_eq!(value["coffeepos_active"], false);
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
            .unwrap_or_else(|error| {
                let woocommerce_log = fs::read_to_string(data_root.join("logs/woocommerce.log"))
                    .unwrap_or_else(|read_error| {
                        format!("<cannot read woocommerce.log: {read_error}>")
                    });
                let provisioning_log =
                    fs::read_to_string(data_root.join("logs/provisioning.log")).unwrap_or_else(
                        |read_error| format!("<cannot read provisioning.log: {read_error}>"),
                    );
                panic!(
                    "install WordPress/WooCommerce/CoffeePOS failed: {error}\n--- woocommerce.log ---\n{woocommerce_log}\n--- provisioning.log ---\n{provisioning_log}"
                );
            });
        assert_eq!(installed.state, ProvisioningState::Ready);
        assert!(installed.woocommerce_active);
        assert!(installed.coffeepos_active);
        let inspected = provisioner.inspect();
        assert_eq!(inspected.state, ProvisioningState::Ready);
        assert!(inspected.woocommerce_active);
        assert!(inspected.coffeepos_active);
        assert_eq!(
            provisioner.load_journal().unwrap().unwrap().stage,
            ProvisioningStage::MachineHealthBootstrapped
        );
        assert!(fs::read_to_string(data_root.join("site/wp-config.php"))
            .unwrap()
            .contains("define('DISABLE_WP_CRON', true);"));
        let woocommerce = data_root.join("site/wp-content/plugins/woocommerce");
        assert_eq!(
            read_plugin_header_version(&woocommerce.join("woocommerce.php")).unwrap(),
            "11.1.0"
        );
        assert!(woocommerce.join(MANAGED_PLUGIN_OWNERSHIP_FILE).is_file());
        assert!(!data_root.join("woocommerce.provisioning").exists());
        let coffeepos = data_root.join("site/wp-content/plugins/coffeepos");
        assert_eq!(
            read_coffeepos_plugin_header_version(&coffeepos.join("coffeepos.php")).unwrap(),
            "1.0.1"
        );
        assert!(coffeepos.join(MANAGED_PLUGIN_OWNERSHIP_FILE).is_file());
        assert!(!data_root.join("coffeepos.provisioning").exists());
        let active_plugins = query_wordpress_option(
            &provisioner,
            running.database_port.unwrap(),
            "active_plugins",
        );
        assert!(
            active_plugins.contains("woocommerce/woocommerce.php"),
            "WooCommerce is not active after Phase 4.6 baseline: {active_plugins}"
        );
        assert!(
            active_plugins.contains("coffeepos/coffeepos.php"),
            "CoffeePOS is not active after Phase 4.9 activation: {active_plugins}"
        );
        assert_eq!(
            query_wordpress_option(
                &provisioner,
                running.database_port.unwrap(),
                "coffeepos_installed_version",
            ),
            "1.0.1"
        );
        assert_eq!(
            query_wordpress_option(
                &provisioner,
                running.database_port.unwrap(),
                "coffeepos_db_version",
            ),
            "0.0.1"
        );
        assert_eq!(
            query_wordpress_option(
                &provisioner,
                running.database_port.unwrap(),
                "coffeepos_rewrite_version",
            ),
            "1.0.1:3"
        );
        let machine_token_path = data_root.join(MACHINE_TOKEN_SECRET);
        let mut machine_token = secret::load(&machine_token_path).unwrap();
        assert_eq!(machine_token.len(), 64);
        assert_eq!(machine_token.to_ascii_lowercase(), machine_token);
        let protected_machine_token = fs::read(&machine_token_path).unwrap();
        assert!(!protected_machine_token
            .windows(machine_token.len())
            .any(|window| window == machine_token.as_bytes()));

        let post_install_health = manager.refresh_wordpress_health();
        if post_install_health.wordpress_health != WordPressHealthState::Healthy {
            let php_log = fs::read_to_string(data_root.join("logs/php.log"))
                .unwrap_or_else(|error| format!("<cannot read php.log: {error}>"));
            let database_log = fs::read_to_string(data_root.join("logs/database.log"))
                .unwrap_or_else(|error| format!("<cannot read database.log: {error}>"));
            let runtime_log = fs::read_to_string(data_root.join("logs/runtime.log"))
                .unwrap_or_else(|error| format!("<cannot read runtime.log: {error}>"));
            panic!(
                "WordPress health after CoffeePOS provisioning was {:?}: {:?}\n--- php.log ---\n{}\n--- database.log ---\n{}\n--- runtime.log ---\n{}",
                post_install_health.wordpress_health,
                post_install_health.wordpress_error,
                php_log,
                database_log,
                runtime_log
            );
        }
        assert!(post_install_health.wordpress_error.is_none());
        assert_eq!(
            post_install_health.coffeepos_health.state,
            CoffeePosHealthState::Healthy
        );
        let app_health = post_install_health
            .coffeepos_health
            .payload
            .as_ref()
            .unwrap();
        assert_eq!(app_health.schema_version, 1);
        assert_eq!(app_health.versions.wordpress, "7.1");
        assert_eq!(app_health.versions.woocommerce, "11.1.0");
        assert_eq!(app_health.versions.coffeepos, "1.0.1");
        assert_eq!(app_health.versions.coffeepos_schema, "0.0.1");
        assert!(app_health.wordpress);
        assert!(app_health.woocommerce);
        assert!(app_health.coffeepos);
        assert!(app_health.database);
        assert!(app_health.pos_path.starts_with('/'));
        let rest_response = http_get(
            post_install_health.http_port.unwrap(),
            "/wp-json/wc/store/v1/products?per_page=1",
        );
        assert!(
            rest_response.starts_with("HTTP/1.0 200 ")
                || rest_response.starts_with("HTTP/1.1 200 "),
            "WooCommerce Store API did not return HTTP 200 after activation:\n{rest_response}"
        );
        let coffeepos_health_response = http_get(
            post_install_health.http_port.unwrap(),
            "/wp-json/coffeepos/v1/health",
        );
        assert!(
            coffeepos_health_response.starts_with("HTTP/1.0 401 ")
                || coffeepos_health_response.starts_with("HTTP/1.1 401 ")
                || coffeepos_health_response.starts_with("HTTP/1.0 403 ")
                || coffeepos_health_response.starts_with("HTTP/1.1 403 "),
            "CoffeePOS health route did not enforce its normal permission contract after activation:\n{coffeepos_health_response}"
        );
        assert!(
            coffeepos_health_response.contains("coffeepos_rest_forbidden"),
            "CoffeePOS health route did not return the plugin-owned permission error:\n{coffeepos_health_response}"
        );
        let machine_health_without_token = http_get(
            post_install_health.http_port.unwrap(),
            "/wp-json/coffeepos/v1/system/status",
        );
        assert!(
            machine_health_without_token.starts_with("HTTP/1.0 401 ")
                || machine_health_without_token.starts_with("HTTP/1.1 401 "),
            "Machine-health request without token did not return HTTP 401:\n{machine_health_without_token}"
        );
        let wrong_token = "0".repeat(64);
        let machine_health_wrong_token = http_get_with_headers(
            post_install_health.http_port.unwrap(),
            "/wp-json/coffeepos/v1/system/status",
            &[("X-CoffeePOS-Machine-Token", wrong_token.as_str())],
        );
        assert!(
            machine_health_wrong_token.starts_with("HTTP/1.0 401 ")
                || machine_health_wrong_token.starts_with("HTTP/1.1 401 "),
            "Machine-health request with wrong token did not return HTTP 401:\n{machine_health_wrong_token}"
        );
        let machine_health_authenticated = http_get_with_headers(
            post_install_health.http_port.unwrap(),
            "/wp-json/coffeepos/v1/system/status",
            &[("X-CoffeePOS-Machine-Token", machine_token.as_str())],
        );
        assert!(
            machine_health_authenticated.starts_with("HTTP/1.0 200 ")
                || machine_health_authenticated.starts_with("HTTP/1.1 200 "),
            "Authenticated machine-health request did not return HTTP 200:\n{machine_health_authenticated}"
        );
        assert!(machine_health_authenticated.contains(r#""schema_version":1"#));
        assert!(machine_health_authenticated.contains(r#""status":"healthy""#));

        let original_rotation_php = provisioner
            .replace_php_executable_for_test(data_root.join("missing-phase4-10-rotation-php.exe"));
        let failed_rotation = provisioner
            .rotate_machine_health_token(&post_install_health)
            .unwrap_err();
        assert_eq!(
            failed_rotation.operation,
            "rotate CoffeePOS machine credential"
        );
        assert_eq!(secret::load(&machine_token_path).unwrap(), machine_token);
        assert!(!data_root.join(MACHINE_TOKEN_PENDING_SECRET).exists());
        assert_eq!(
            probe_coffeepos_health_with_token(
                post_install_health.http_port.unwrap(),
                machine_token.as_str()
            )
            .state,
            CoffeePosHealthState::Healthy
        );
        provisioner.replace_php_executable_for_test(original_rotation_php);

        let previous_machine_token = machine_token.clone();
        provisioner
            .rotate_machine_health_token(&post_install_health)
            .unwrap();
        machine_token = secret::load(&machine_token_path).unwrap();
        assert_ne!(machine_token, previous_machine_token);
        assert!(!data_root.join(MACHINE_TOKEN_PENDING_SECRET).exists());
        let old_machine_health = http_get_with_headers(
            post_install_health.http_port.unwrap(),
            "/wp-json/coffeepos/v1/system/status",
            &[("X-CoffeePOS-Machine-Token", previous_machine_token.as_str())],
        );
        assert!(
            old_machine_health.starts_with("HTTP/1.0 401 ")
                || old_machine_health.starts_with("HTTP/1.1 401 "),
            "Previous machine token remained valid after rotation:\n{old_machine_health}"
        );
        let rotated_machine_health = http_get_with_headers(
            post_install_health.http_port.unwrap(),
            "/wp-json/coffeepos/v1/system/status",
            &[("X-CoffeePOS-Machine-Token", machine_token.as_str())],
        );
        assert!(
            rotated_machine_health.starts_with("HTTP/1.0 200 ")
                || rotated_machine_health.starts_with("HTTP/1.1 200 "),
            "Rotated machine token was not accepted:\n{rotated_machine_health}"
        );
        assert_eq!(
            manager.refresh_coffeepos_health().coffeepos_health.state,
            CoffeePosHealthState::Healthy
        );

        let woocommerce_unavailable =
            data_root.join("site/wp-content/plugins/woocommerce.health-unavailable");
        fs::rename(&woocommerce, &woocommerce_unavailable).unwrap();
        let unavailable = manager.refresh_coffeepos_health();
        assert_eq!(
            unavailable.coffeepos_health.state,
            CoffeePosHealthState::Degraded
        );
        assert!(
            !unavailable
                .coffeepos_health
                .payload
                .as_ref()
                .unwrap()
                .woocommerce
        );
        fs::rename(&woocommerce_unavailable, &woocommerce).unwrap();
        assert_eq!(
            manager.refresh_coffeepos_health().coffeepos_health.state,
            CoffeePosHealthState::Healthy
        );

        let coffeepos_unavailable =
            data_root.join("site/wp-content/plugins/coffeepos.health-unavailable");
        fs::rename(&coffeepos, &coffeepos_unavailable).unwrap();
        let plugin_unavailable = manager.refresh_coffeepos_health();
        assert_eq!(
            plugin_unavailable.coffeepos_health.state,
            CoffeePosHealthState::Failed
        );
        assert_eq!(
            plugin_unavailable.coffeepos_health.failure_kind,
            Some(CoffeePosHealthFailureKind::TransportBootstrap)
        );
        assert!(plugin_unavailable.coffeepos_health.payload.is_none());
        fs::rename(&coffeepos_unavailable, &coffeepos).unwrap();
        assert_eq!(
            manager.refresh_coffeepos_health().coffeepos_health.state,
            CoffeePosHealthState::Healthy
        );

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
        assert_eq!(
            healthy.coffeepos_health.state,
            CoffeePosHealthState::Healthy
        );
        assert_eq!(secret::load(&machine_token_path).unwrap(), machine_token);
        let active_plugins_after_restart = query_wordpress_option(
            &provisioner,
            healthy.database_port.unwrap(),
            "active_plugins",
        );
        assert!(
            active_plugins_after_restart.contains("coffeepos/coffeepos.php"),
            "CoffeePOS activation did not persist across runtime restart: {active_plugins_after_restart}"
        );

        manager.kill_php_for_test();
        let child_failed = manager.refresh();
        assert_eq!(child_failed.state, RuntimeState::Stopped);
        assert_eq!(
            child_failed.wordpress_health,
            WordPressHealthState::Unavailable
        );
        assert_eq!(
            child_failed.coffeepos_health.state,
            CoffeePosHealthState::Unavailable
        );
        assert!(child_failed.last_error.is_some());

        let recovered_after_child_exit = manager.start().unwrap();
        assert_eq!(recovered_after_child_exit.state, RuntimeState::Running);
        assert_eq!(
            recovered_after_child_exit.wordpress_health,
            WordPressHealthState::Healthy
        );
        assert_eq!(
            recovered_after_child_exit.coffeepos_health.state,
            CoffeePosHealthState::Healthy
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
        assert_eq!(
            failed_start.coffeepos_health.state,
            CoffeePosHealthState::Unavailable
        );
        manager.replace_php_executable_for_test(original_php);
        let retried_start = manager.start().unwrap();
        assert_eq!(retried_start.state, RuntimeState::Running);
        assert_eq!(
            retried_start.wordpress_health,
            WordPressHealthState::Healthy
        );
        assert_eq!(
            retried_start.coffeepos_health.state,
            CoffeePosHealthState::Healthy
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
            moved_runtime.coffeepos_health.state,
            CoffeePosHealthState::Healthy
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
        let woocommerce_sentinel = woocommerce.join("coffeepos-phase4-5-sentinel.txt");
        fs::write(&woocommerce_sentinel, b"preserve plugin data").unwrap();
        let coffeepos_sentinel = coffeepos.join("coffeepos-phase4-8-sentinel.txt");
        fs::write(&coffeepos_sentinel, b"preserve coffeepos plugin data").unwrap();
        let unrelated_plugin = data_root.join("site/wp-content/plugins/existing-plugin");
        fs::create_dir_all(&unrelated_plugin).unwrap();
        let unrelated_sentinel = unrelated_plugin.join("keep.txt");
        fs::write(&unrelated_sentinel, b"keep unrelated plugin").unwrap();

        let uploads_sentinel = data_root.join("uploads/phase-4-11/keep.txt");
        fs::create_dir_all(uploads_sentinel.parent().unwrap()).unwrap();
        fs::write(&uploads_sentinel, b"preserve uploaded media").unwrap();

        let phase_4_11_database_port = moved_runtime.database_port.unwrap();
        let phase_4_11_database_password =
            secret::load(&data_root.join(DATABASE_WORDPRESS_SECRET)).unwrap();
        provisioner
            .run_database_sql(
                &DatabaseEndpoint::Tcp(phase_4_11_database_port),
                DATABASE_WORDPRESS_USER,
                &phase_4_11_database_password,
                "UPDATE coffeepos.wp_options SET option_value = 'Phase 4.11 preserved store' WHERE option_name = 'coffeepos_store_name';\n",
            )
            .expect("failed to seed Phase 4.11 CoffeePOS store option sentinel");
        provisioner
            .run_database_sql(
                &DatabaseEndpoint::Tcp(phase_4_11_database_port),
                DATABASE_WORDPRESS_USER,
                &phase_4_11_database_password,
                "INSERT INTO coffeepos.wp_coffeepos_suspended_carts (user_id, label, cart_payload, created_at, updated_at) VALUES (1, 'phase-4-11-sentinel', '{\"phase\":\"4.11\",\"items\":[{\"sku\":\"sentinel\",\"quantity\":2}]}', NOW(), NOW());\n",
            )
            .expect("failed to seed Phase 4.11 CoffeePOS suspended-cart sentinel");
        assert_eq!(
            query_wordpress_option(
                &provisioner,
                phase_4_11_database_port,
                "coffeepos_store_name",
            ),
            "Phase 4.11 preserved store"
        );
        assert_eq!(
            query_database_scalar(
                &provisioner,
                phase_4_11_database_port,
                "SELECT cart_payload FROM wp_coffeepos_suspended_carts WHERE label = 'phase-4-11-sentinel' ORDER BY id DESC LIMIT 1",
            ),
            r#"{"phase":"4.11","items":[{"sku":"sentinel","quantity":2}]}"#
        );

        let credential_snapshots = [
            data_root.join(DATABASE_RUNTIME_SECRET),
            data_root.join(DATABASE_WORDPRESS_SECRET),
            data_root.join(WORDPRESS_ADMIN_SECRET),
            data_root.join(MACHINE_TOKEN_SECRET),
        ]
        .into_iter()
        .map(|path| {
            let value = secret::load(&path).unwrap();
            (path, value)
        })
        .collect::<Vec<_>>();
        let journal_before_second_provisioning = fs::read(provisioner.journal_path()).unwrap();
        let woocommerce_ownership_before_second_provisioning =
            fs::read(woocommerce.join(MANAGED_PLUGIN_OWNERSHIP_FILE)).unwrap();
        let coffeepos_ownership_before_second_provisioning =
            fs::read(coffeepos.join(MANAGED_PLUGIN_OWNERSHIP_FILE)).unwrap();
        let wp_config_before_second_provisioning =
            fs::read(data_root.join("site/wp-config.php")).unwrap();
        let active_plugins_before_second_provisioning =
            query_wordpress_option(&provisioner, phase_4_11_database_port, "active_plugins");
        let admin_password_hash_before_second_provisioning = query_database_scalar(
            &provisioner,
            phase_4_11_database_port,
            "SELECT user_pass FROM wp_users WHERE user_login = 'coffeepos_admin' LIMIT 1",
        );

        let stopped = manager.stop().unwrap();
        assert_eq!(stopped.state, RuntimeState::Stopped);
        assert_eq!(stopped.wordpress_health, WordPressHealthState::Unavailable);
        assert_eq!(
            stopped.coffeepos_health.state,
            CoffeePosHealthState::Unavailable
        );
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
        assert!(installed_again.woocommerce_active);
        assert!(installed_again.coffeepos_active);
        assert_eq!(secret::load(&machine_token_path).unwrap(), machine_token);
        assert!(sentinel.is_file());
        assert_eq!(
            fs::read(&uploads_sentinel).unwrap(),
            b"preserve uploaded media"
        );
        assert_eq!(
            fs::read(&woocommerce_sentinel).unwrap(),
            b"preserve plugin data"
        );
        assert_eq!(
            fs::read(&coffeepos_sentinel).unwrap(),
            b"preserve coffeepos plugin data"
        );
        assert_eq!(
            fs::read(&unrelated_sentinel).unwrap(),
            b"keep unrelated plugin"
        );
        for (path, expected) in &credential_snapshots {
            assert_eq!(
                secret::load(path).unwrap(),
                *expected,
                "credential changed during second provisioning: {}",
                path.display()
            );
        }
        assert_eq!(
            fs::read(provisioner.journal_path()).unwrap(),
            journal_before_second_provisioning
        );
        assert_eq!(
            fs::read(woocommerce.join(MANAGED_PLUGIN_OWNERSHIP_FILE)).unwrap(),
            woocommerce_ownership_before_second_provisioning
        );
        assert_eq!(
            fs::read(coffeepos.join(MANAGED_PLUGIN_OWNERSHIP_FILE)).unwrap(),
            coffeepos_ownership_before_second_provisioning
        );
        assert_eq!(
            fs::read(data_root.join("site/wp-config.php")).unwrap(),
            wp_config_before_second_provisioning
        );
        assert_eq!(
            query_wordpress_option(
                &provisioner,
                running_again.database_port.unwrap(),
                "active_plugins",
            ),
            active_plugins_before_second_provisioning
        );
        assert_eq!(
            query_database_scalar(
                &provisioner,
                running_again.database_port.unwrap(),
                "SELECT user_pass FROM wp_users WHERE user_login = 'coffeepos_admin' LIMIT 1",
            ),
            admin_password_hash_before_second_provisioning
        );
        assert_eq!(
            query_wordpress_option(
                &provisioner,
                running_again.database_port.unwrap(),
                "coffeepos_store_name",
            ),
            "Phase 4.11 preserved store"
        );
        assert_eq!(
            query_database_scalar(
                &provisioner,
                running_again.database_port.unwrap(),
                "SELECT CONCAT(label, '|', cart_payload) FROM wp_coffeepos_suspended_carts WHERE label = 'phase-4-11-sentinel' ORDER BY id DESC LIMIT 1",
            ),
            r#"phase-4-11-sentinel|{"phase":"4.11","items":[{"sku":"sentinel","quantity":2}]}"#
        );
        assert_eq!(
            query_database_scalar(
                &provisioner,
                running_again.database_port.unwrap(),
                "SELECT COUNT(*) FROM wp_coffeepos_suspended_carts WHERE label = 'phase-4-11-sentinel'",
            ),
            "1"
        );
        let inspected_again = provisioner.inspect();
        assert_eq!(inspected_again.state, ProvisioningState::Ready);
        assert!(inspected_again.woocommerce_active);
        assert!(inspected_again.coffeepos_active);

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
        if fs::read_dir(&e2e_root).unwrap().next().is_none() {
            fs::remove_dir(&e2e_root).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "uses the staged real PHP/MariaDB/WordPress development runtime"]
    fn staged_runtime_recovers_across_first_run_journal_boundaries() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap().to_path_buf();
        let runtime_manifest =
            project_root.join("runtime/development/x86_64-pc-windows-msvc/manifest.json");
        let runtime = resolve_development_manifest(&project_root, &runtime_manifest).unwrap();

        let e2e_root = manifest_dir.join("target/phase4-12-e2e");
        fs::create_dir_all(&e2e_root).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("recovery-")
            .tempdir_in(&e2e_root)
            .unwrap();
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();

        let make_provisioner = || {
            Provisioner::from_development(
                &project_root,
                &runtime_manifest,
                runtime.clone(),
                data_root.clone(),
            )
            .unwrap()
        };

        let mut provisioner = make_provisioner();
        provisioner.fail_after_for_test(ProvisioningBoundary::DatabaseReady);
        let error = provisioner.prepare().unwrap_err();
        assert_eq!(error.operation, "simulate provisioning interruption");
        assert!(provisioner.load_journal().unwrap().is_none());
        let inspect = provisioner.inspect();
        assert_eq!(inspect.state, ProvisioningState::NeedsRepair);
        assert!(inspect.can_retry);
        let database_runtime_secret =
            secret::load(&data_root.join(DATABASE_RUNTIME_SECRET)).unwrap();
        let database_wordpress_secret =
            secret::load(&data_root.join(DATABASE_WORDPRESS_SECRET)).unwrap();
        drop(provisioner);

        let mut provisioner = make_provisioner();
        provisioner.fail_after_for_test(ProvisioningBoundary::SiteReady);
        let error = provisioner.prepare().unwrap_err();
        assert_eq!(error.operation, "simulate provisioning interruption");
        assert_eq!(
            provisioner.load_journal().unwrap().unwrap().stage,
            ProvisioningStage::DatabaseReady
        );
        assert!(provisioner.inspect().can_retry);
        drop(provisioner);

        let install_boundaries = [
            (
                ProvisioningBoundary::WordPressInstalled,
                ProvisioningStage::SiteReady,
            ),
            (
                ProvisioningBoundary::WooCommerceProvisioned,
                ProvisioningStage::WordPressInstalled,
            ),
            (
                ProvisioningBoundary::WooCommerceActivated,
                ProvisioningStage::WooCommerceProvisioned,
            ),
            (
                ProvisioningBoundary::CoffeePosProvisioned,
                ProvisioningStage::WooCommerceActivated,
            ),
            (
                ProvisioningBoundary::CoffeePosActivated,
                ProvisioningStage::CoffeePosProvisioned,
            ),
            (
                ProvisioningBoundary::MachineHealthBootstrapped,
                ProvisioningStage::CoffeePosActivated,
            ),
        ];

        let mut wordpress_admin_secret = None;
        let mut machine_token = None;
        for (boundary, expected_stage) in install_boundaries {
            let mut provisioner = make_provisioner();
            provisioner.prepare().unwrap();
            let mut manager = RuntimeManager::new(runtime.clone(), data_root.clone()).unwrap();
            let running = manager.start().unwrap();
            provisioner.fail_after_for_test(boundary);
            let error = provisioner
                .install_wordpress("CoffeePOS Phase 4.12 Recovery", &running)
                .unwrap_err();
            assert_eq!(error.operation, "simulate provisioning interruption");
            assert_eq!(
                provisioner.load_journal().unwrap().unwrap().stage,
                expected_stage
            );
            let inspect = provisioner.inspect();
            assert_eq!(inspect.state, ProvisioningState::NeedsRepair);
            assert!(inspect.can_retry);

            if data_root.join(WORDPRESS_ADMIN_SECRET).is_file() {
                let current = secret::load(&data_root.join(WORDPRESS_ADMIN_SECRET)).unwrap();
                if let Some(expected) = wordpress_admin_secret.as_ref() {
                    assert_eq!(&current, expected);
                } else {
                    wordpress_admin_secret = Some(current);
                }
            }
            if data_root.join(MACHINE_TOKEN_SECRET).is_file() {
                let current = secret::load(&data_root.join(MACHINE_TOKEN_SECRET)).unwrap();
                if let Some(expected) = machine_token.as_ref() {
                    assert_eq!(&current, expected);
                } else {
                    machine_token = Some(current);
                }
            }

            let stopped = manager.stop().unwrap();
            assert_eq!(stopped.state, RuntimeState::Stopped);
            drop(manager);
            drop(provisioner);
        }

        let mut provisioner = make_provisioner();
        provisioner.prepare().unwrap();
        let mut manager = RuntimeManager::new(runtime, data_root.clone()).unwrap();
        let running = manager.start().unwrap();
        let ready = provisioner
            .install_wordpress("CoffeePOS Phase 4.12 Recovery", &running)
            .unwrap();
        assert_eq!(ready.state, ProvisioningState::Ready);
        assert!(ready.woocommerce_active);
        assert!(ready.coffeepos_active);
        assert_eq!(
            provisioner.load_journal().unwrap().unwrap().stage,
            ProvisioningStage::MachineHealthBootstrapped
        );
        assert_eq!(
            secret::load(&data_root.join(DATABASE_RUNTIME_SECRET)).unwrap(),
            database_runtime_secret
        );
        assert_eq!(
            secret::load(&data_root.join(DATABASE_WORDPRESS_SECRET)).unwrap(),
            database_wordpress_secret
        );
        assert_eq!(
            secret::load(&data_root.join(WORDPRESS_ADMIN_SECRET)).unwrap(),
            wordpress_admin_secret.unwrap()
        );
        assert_eq!(
            secret::load(&data_root.join(MACHINE_TOKEN_SECRET)).unwrap(),
            machine_token.unwrap()
        );
        assert_eq!(provisioner.inspect().state, ProvisioningState::Ready);

        manager.stop().unwrap();
        drop(manager);
        drop(provisioner);
        temp.close().unwrap();
        if fs::read_dir(&e2e_root).unwrap().next().is_none() {
            fs::remove_dir(&e2e_root).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "uses the staged real PHP/MariaDB/WordPress development runtime"]
    fn staged_runtime_blocks_retry_for_partial_wordpress_tables() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap().to_path_buf();
        let runtime_manifest =
            project_root.join("runtime/development/x86_64-pc-windows-msvc/manifest.json");
        let runtime = resolve_development_manifest(&project_root, &runtime_manifest).unwrap();

        let e2e_root = manifest_dir.join("target/phase4-12-e2e");
        fs::create_dir_all(&e2e_root).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("partial-wordpress-")
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
        provisioner.prepare().unwrap();
        let mut manager = RuntimeManager::new(runtime, data_root.clone()).unwrap();
        let running = manager.start().unwrap();
        let database_port = running.database_port.unwrap();
        let database_password = secret::load(&data_root.join(DATABASE_WORDPRESS_SECRET)).unwrap();
        provisioner
            .run_database_sql(
                &DatabaseEndpoint::Tcp(database_port),
                DATABASE_WORDPRESS_USER,
                &database_password,
                "CREATE TABLE coffeepos.wp_phase_4_12_partial (id BIGINT UNSIGNED NOT NULL PRIMARY KEY);\n",
            )
            .unwrap();

        let error = provisioner
            .install_wordpress("CoffeePOS Phase 4.12 Partial", &running)
            .unwrap_err();
        assert_eq!(error.operation, "recover partial WordPress install");
        assert_eq!(
            query_database_scalar(
                &provisioner,
                database_port,
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'coffeepos' AND table_name = 'wp_phase_4_12_partial'",
            ),
            "1"
        );
        let journal = provisioner.load_journal().unwrap().unwrap();
        assert_eq!(journal.stage, ProvisioningStage::SiteReady);
        assert_eq!(
            journal.recovery_blocker,
            Some(ProvisioningRecoveryBlocker::PartialWordPressInstall)
        );

        manager.stop().unwrap();
        drop(manager);
        drop(provisioner);

        let mut relaunched = Provisioner::from_development(
            &project_root,
            &runtime_manifest,
            resolve_development_manifest(&project_root, &runtime_manifest).unwrap(),
            data_root.clone(),
        )
        .unwrap();
        let inspect = relaunched.inspect();
        assert_eq!(inspect.state, ProvisioningState::NeedsRepair);
        assert!(!inspect.can_retry);
        assert_eq!(
            inspect
                .last_error
                .as_ref()
                .map(|error| error.operation.as_str()),
            Some("recover partial WordPress install")
        );
        let retry_error = relaunched.prepare().unwrap_err();
        assert_eq!(retry_error.operation, "recover partial WordPress install");
        assert!(data_root.join("database/mysql").is_dir());
        assert!(data_root.join("site/wp-settings.php").is_file());

        drop(relaunched);
        temp.close().unwrap();
        if fs::read_dir(&e2e_root).unwrap().next().is_none() {
            fs::remove_dir(&e2e_root).unwrap();
        }
    }
}
