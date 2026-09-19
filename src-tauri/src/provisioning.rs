use crate::runtime::{
    choose_loopback_port, configure_child_command, probe_coffeepos_health,
    probe_coffeepos_health_with_token, run_command_bounded, wait_for_child_exit,
    CoffeePosHealthState, HealthDiagnosticsInfo, ProcessContainment, ResolvedRuntime,
    RuntimeErrorInfo, RuntimeInfo, DATABASE_NAME, DATABASE_RUNTIME_SECRET, DATABASE_RUNTIME_USER,
    DATABASE_WORDPRESS_SECRET, DATABASE_WORDPRESS_USER, MACHINE_TOKEN_PENDING_SECRET,
    MACHINE_TOKEN_SECRET,
};
use crate::secret;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

const LOOPBACK: &str = "127.0.0.1";
pub const PROVISIONING_SCHEMA_VERSION: u32 = 1;
const WORDPRESS_MANIFEST_SCHEMA_VERSION: u32 = 1;
const DATABASE_BOOTSTRAP_SECRET: &str = "config/database-bootstrap.secret";
pub const WORDPRESS_ADMIN_SECRET: &str = "config/wordpress-admin.secret";
pub const WORDPRESS_ADMIN_USER: &str = "coffeepos_admin";
pub const WORDPRESS_ADMIN_EMAIL: &str = "admin@coffeepos.local";
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
const REPAIR_SCHEMA_VERSION: u32 = 2;
const REPAIR_JOURNAL: &str = "config/repair.json";
const REPAIR_ADMIN_PENDING_SECRET: &str = "config/wordpress-admin.repair.pending.secret";
const WOOCOMMERCE_REPAIR_STAGING: &str = "woocommerce.repairing";
const WOOCOMMERCE_REPAIR_BACKUP: &str = "woocommerce.repair-backup";
const COFFEEPOS_REPAIR_STAGING: &str = "coffeepos.repairing";
const COFFEEPOS_REPAIR_BACKUP: &str = "coffeepos.repair-backup";
const REPAIR_ORIGINAL_MISSING_MARKER: &str = ".coffeepos-repair-original-missing";
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

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairClassification {
    Repairable,
    RequiresInput,
    Blocked,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RepairItem {
    pub id: String,
    pub component: String,
    pub target: String,
    pub classification: RepairClassification,
    pub action: String,
    pub reason: String,
    pub impact: String,
    pub requires_runtime_stop: bool,
    pub input_kind: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RepairPlan {
    pub plan_id: String,
    pub generated_at: u64,
    pub store_state: ProvisioningState,
    pub runtime_was_running: bool,
    pub items: Vec<RepairItem>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairItemStatus {
    Repaired,
    Skipped,
    Blocked,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RepairItemResult {
    pub id: String,
    pub status: RepairItemStatus,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairResultStatus {
    Repaired,
    Partial,
    Stale,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RepairApplyResult {
    pub plan_id: String,
    pub status: RepairResultStatus,
    pub items: Vec<RepairItemResult>,
    pub provisioning_info: ProvisioningInfo,
    pub health_diagnostics: Option<HealthDiagnosticsInfo>,
    pub last_error: Option<RuntimeErrorInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RepairJournalStage {
    Planned,
    RuntimeStopped,
    Staged,
    Swapped,
    Verified,
    Committed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairJournal {
    schema_version: u32,
    plan_id: String,
    item_ids: Vec<String>,
    completed_item_ids: Vec<String>,
    active_item_id: Option<String>,
    runtime_was_running: bool,
    stage: RepairJournalStage,
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
    admin_username: String,
    admin_email: String,
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
            admin_username: WORDPRESS_ADMIN_USER.into(),
            admin_email: WORDPRESS_ADMIN_EMAIL.into(),
            #[cfg(test)]
            failure_after: None,
        })
    }

    pub fn configure_initial_admin(
        &mut self,
        admin_username: &str,
        admin_email: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let admin_username = admin_username.trim();
        let admin_email = admin_email.trim();
        if admin_username.is_empty() || admin_email.is_empty() {
            return Err(provisioning_error(
                "configure initial administrator",
                "Initial administrator username and email are required.",
                "Return to setup, enter the administrator account details, then retry.",
            ));
        }
        if let Some(journal) = self.load_journal()? {
            if journal.admin_username != admin_username {
                return Err(provisioning_error(
                    "configure initial administrator",
                    format!(
                        "Provisioning already started with administrator '{}'; refusing to replace it with '{}'.",
                        journal.admin_username, admin_username
                    ),
                    "Continue setup with the existing administrator. CoffeePOS Desktop will not reset an account after provisioning has started.",
                ));
            }
        }
        self.admin_username = admin_username.into();
        self.admin_email = admin_email.into();
        Ok(())
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

    pub fn verify_database_credentials_for_repair(
        &self,
        live_database_port: Option<u16>,
    ) -> Result<(), RuntimeErrorInfo> {
        let Some(journal) = self.load_journal()? else {
            return Ok(());
        };
        if journal.stage < ProvisioningStage::DatabaseReady {
            return Ok(());
        }

        let runtime_password = secret::load(&self.data_root.join(DATABASE_RUNTIME_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "verify repair database credentials",
                    error,
                    "Preserve the MariaDB datadir and restore the matching protected runtime database credential before repair.",
                )
            })?;
        let wordpress_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "verify repair database credentials",
                    error,
                    "Preserve the MariaDB datadir and restore the matching protected WordPress database credential before repair.",
                )
            })?;

        if let Some(port) = live_database_port {
            let endpoint = DatabaseEndpoint::Tcp(port);
            self.require_repair_database_account(
                &endpoint,
                DATABASE_RUNTIME_USER,
                &runtime_password,
                None,
                "runtime",
            )?;
            self.require_repair_database_account(
                &endpoint,
                DATABASE_WORDPRESS_USER,
                &wordpress_password,
                Some(DATABASE_NAME),
                "WordPress",
            )?;
            return Ok(());
        }

        let port = choose_loopback_port(&[])?;
        let endpoint = DatabaseEndpoint::Tcp(port);
        let mut database = self.spawn_database(&endpoint)?;
        let verification = (|| {
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
            Ok(())
        })();
        let cleanup =
            self.stop_repair_preflight_database(&mut database, &endpoint, &runtime_password);

        match (verification, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
            (Err(error), Err(cleanup_error)) => Err(provisioning_error(
                "verify repair database credentials",
                format!(
                    "Database credential verification failed ({}), and the temporary MariaDB preflight process could not be cleaned up safely: {}",
                    error.message, cleanup_error.message
                ),
                "Do not start repair mutation. Preserve the MariaDB datadir, stop the remaining MariaDB process explicitly, then retry credential verification.",
            )),
        }
    }

    pub fn repair_plan(&self, runtime_was_running: bool, machine_auth_failed: bool) -> RepairPlan {
        for (relative, label) in [
            ("config", "repair configuration"),
            ("site", "WordPress site"),
            ("database", "MariaDB data"),
        ] {
            if let Err(error) =
                ensure_repair_path_safe(&self.data_root, &self.data_root.join(relative), label)
            {
                return RepairPlan {
                    plan_id: format!("repair-blocked-{relative}"),
                    generated_at: unix_timestamp(),
                    store_state: ProvisioningState::NeedsRepair,
                    runtime_was_running,
                    items: vec![repair_item(
                        "unsafe_store_path",
                        "repair",
                        label,
                        RepairClassification::Blocked,
                        "Preserve the redirected managed path",
                        &error.message,
                        &error.recovery,
                        false,
                    )],
                    can_apply: false,
                };
            }
        }
        let inspection = self.inspect();
        match self.load_repair_journal() {
            Ok(Some(repair_journal)) => {
                let recovery_check = self.validate_interrupted_repair(&repair_journal);
                let (classification, action, reason, impact) = match recovery_check {
                    Ok(()) => (
                        RepairClassification::Repairable,
                        if matches!(
                            repair_journal.stage,
                            RepairJournalStage::Verified | RepairJournalStage::Committed
                        ) {
                            "Finish the verified repair transaction"
                        } else {
                            "Recover the interrupted repair transaction"
                        },
                        format!(
                            "Repair transaction '{}' was interrupted at stage {:?}; CoffeePOS has enough owned journal/backup evidence to recover deterministically.",
                            repair_journal.plan_id, repair_journal.stage
                        ),
                        if matches!(
                            repair_journal.stage,
                            RepairJournalStage::Verified | RepairJournalStage::Committed
                        ) {
                            "Verified live repair results are kept; remaining owned backup cleanup is committed idempotently before the journal is removed.".to_string()
                        } else {
                            "Unverified plugin swaps are rolled back while the runtime is stopped. Atomic managed-file changes and protected pending credentials are preserved, then the store is inspected again.".to_string()
                        },
                    ),
                    Err(error) => (
                        RepairClassification::Blocked,
                        "Preserve the interrupted repair transaction",
                        error.message,
                        error.recovery,
                    ),
                };
                let item = repair_item(
                    "repair_transaction",
                    "repair",
                    "Repair transaction",
                    classification.clone(),
                    action,
                    &reason,
                    &impact,
                    true,
                );
                return RepairPlan {
                    plan_id: repair_recovery_plan_id(&self.data_root, &repair_journal),
                    generated_at: unix_timestamp(),
                    store_state: inspection.state,
                    runtime_was_running: repair_journal.runtime_was_running,
                    items: vec![item],
                    can_apply: classification == RepairClassification::Repairable,
                };
            }
            Err(error) => {
                let item = repair_item(
                    "repair_transaction",
                    "repair",
                    "Repair transaction",
                    RepairClassification::Blocked,
                    "Preserve the unreadable repair transaction",
                    &error.message,
                    "CoffeePOS will not start another mutation while config/repair.json cannot be validated.",
                    false,
                );
                return self.finish_repair_plan(inspection.state, runtime_was_running, vec![item]);
            }
            Ok(None) => {}
        }
        let mut items = Vec::new();
        let journal = match self.load_journal() {
            Ok(Some(journal)) => journal,
            Ok(None) => {
                if inspection.state == ProvisioningState::NeedsRepair {
                    items.push(repair_item(
                        "store_ownership",
                        "wordpress",
                        "Managed store ownership",
                        RepairClassification::Blocked,
                        "Preserve the existing store",
                        "CoffeePOS cannot prove this non-empty store belongs to the current provisioning journal.",
                        "No files or database data will be changed.",
                        false,
                    ));
                }
                return self.finish_repair_plan(inspection.state, runtime_was_running, items);
            }
            Err(error) => {
                items.push(repair_item(
                    "provisioning_journal",
                    "wordpress",
                    "Provisioning journal",
                    RepairClassification::Blocked,
                    "Preserve and restore the provisioning journal",
                    &error.message,
                    "Repair will not guess ownership while config/provisioning.json is unreadable.",
                    false,
                ));
                return self.finish_repair_plan(inspection.state, runtime_was_running, items);
            }
        };

        if let Some(blocker) = journal.recovery_blocker.as_ref() {
            let error = blocker.error();
            items.push(repair_item(
                "partial_wordpress_install",
                "wordpress",
                "WordPress database installation",
                RepairClassification::Blocked,
                "Preserve the partial WordPress tables",
                &error.message,
                "CoffeePOS will not drop, recreate, or overwrite existing wp_* tables automatically.",
                false,
            ));
            return self.finish_repair_plan(inspection.state, runtime_was_running, items);
        }

        if journal.stage >= ProvisioningStage::DatabaseReady {
            let runtime_db_ready =
                protected_secret_ready(&self.data_root.join(DATABASE_RUNTIME_SECRET));
            let wordpress_db_ready =
                protected_secret_ready(&self.data_root.join(DATABASE_WORDPRESS_SECRET));
            if !runtime_db_ready || !wordpress_db_ready {
                items.push(repair_item(
                    "database_credentials",
                    "database",
                    "Database credentials",
                    RepairClassification::Blocked,
                    "Restore the matching protected database credentials",
                    "The provisioning journal says the database accounts already exist, but one or more protected credentials are missing or unreadable.",
                    "The MariaDB datadir is preserved. Repair will not reinitialize MariaDB or bypass authentication.",
                    false,
                ));
                return self.finish_repair_plan(inspection.state, runtime_was_running, items);
            }
        }

        if journal.stage >= ProvisioningStage::WordPressInstalled {
            let admin_active = protected_secret_ready(&self.data_root.join(WORDPRESS_ADMIN_SECRET));
            let admin_pending =
                protected_secret_ready(&self.data_root.join(REPAIR_ADMIN_PENDING_SECRET));
            if !admin_active || admin_pending {
                items.push(repair_item(
                    "wordpress_admin_password",
                    "wordpress",
                    "WordPress administrator password",
                    if admin_pending {
                        RepairClassification::Repairable
                    } else {
                        RepairClassification::RequiresInput
                    },
                    if admin_pending {
                        "Resume the pending administrator-password repair"
                    } else {
                        "Set a replacement administrator password"
                    },
                    if admin_pending {
                        "A protected pending administrator password remains from an interrupted repair and can be verified/promoted idempotently."
                    } else {
                        "The protected administrator password is missing or unreadable; WordPress hashes cannot recover the previous plaintext password."
                    },
                    "Only the existing administrator account password is changed; store data and account identity are preserved.",
                    false,
                ));
            }
        }

        if journal.stage >= ProvisioningStage::SiteReady {
            self.plan_wordpress_files(&journal, &mut items);
        }

        if journal.stage >= ProvisioningStage::WooCommerceProvisioned {
            self.plan_plugin_repair(
                &journal,
                &mut items,
                "woocommerce_plugin",
                "woocommerce",
                "WooCommerce plugin",
                WOOCOMMERCE_PLUGIN_SLUG,
                &self.woocommerce.version,
                &self.woocommerce.archive_sha256,
                WOOCOMMERCE_OWNERSHIP_SCHEMA_VERSION,
                journal.woocommerce_version.as_deref(),
                &self.woocommerce.plugin_root,
            );
        }

        if journal.stage >= ProvisioningStage::CoffeePosProvisioned {
            self.plan_plugin_repair(
                &journal,
                &mut items,
                "coffeepos_plugin",
                "coffeepos",
                "CoffeePOS plugin",
                COFFEEPOS_PLUGIN_SLUG,
                &self.coffeepos.version,
                &self.coffeepos.archive_sha256,
                COFFEEPOS_OWNERSHIP_SCHEMA_VERSION,
                journal.coffeepos_version.as_deref(),
                &self.coffeepos.plugin_root,
            );
        }

        if journal.stage >= ProvisioningStage::MachineHealthBootstrapped {
            let active = protected_machine_token_ready(&self.data_root.join(MACHINE_TOKEN_SECRET));
            let pending =
                protected_machine_token_ready(&self.data_root.join(MACHINE_TOKEN_PENDING_SECRET));
            if !active || machine_auth_failed {
                items.push(repair_item(
                    if pending {
                        "machine_token_pending"
                    } else {
                        "machine_token"
                    },
                    "coffeepos",
                    "CoffeePOS machine credential",
                    if pending {
                        RepairClassification::Repairable
                    } else {
                        RepairClassification::Blocked
                    },
                    if pending {
                        "Verify and promote the pending machine credential"
                    } else {
                        "Preserve the server credential hash"
                    },
                    if pending {
                        "The active protected token is unavailable, but a pending protected token from an interrupted transaction is available for endpoint verification."
                    } else if machine_auth_failed {
                        "The active protected machine credential exists, but the cached CoffeePOS health result reports an authentication failure and there is no pending credential authority."
                    } else {
                        "The active machine credential is missing or unreadable and no accepted pending authority is available."
                    },
                    if pending {
                        "Repair will start the managed runtime only for verification and promote the pending token only if CoffeePOS accepts it."
                    } else {
                        "CoffeePOS will not overwrite the server-side hash without an independently verified recovery transaction."
                    },
                    false,
                ));
            }
        }

        self.finish_repair_plan(inspection.state, runtime_was_running, items)
    }

    fn finish_repair_plan(
        &self,
        store_state: ProvisioningState,
        runtime_was_running: bool,
        items: Vec<RepairItem>,
    ) -> RepairPlan {
        let plan_id = repair_plan_id(
            &self.data_root,
            &store_state,
            &items,
            &self.wordpress.core_root,
            &self.woocommerce.plugin_root,
            &self.coffeepos.plugin_root,
        );
        let can_apply = items.iter().any(|item| {
            matches!(
                item.classification,
                RepairClassification::Repairable | RepairClassification::RequiresInput
            )
        });
        RepairPlan {
            plan_id,
            generated_at: unix_timestamp(),
            store_state,
            runtime_was_running,
            items,
            can_apply,
        }
    }

    fn plan_wordpress_files(&self, journal: &ProvisioningJournal, items: &mut Vec<RepairItem>) {
        let site = self.data_root.join("site");
        let config_path = site.join("wp-config.php");
        let config_path_safety =
            ensure_repair_path_safe(&self.data_root, &config_path, "wp-config.php");
        if let Err(error) = config_path_safety {
            items.push(repair_item(
                "wordpress_config",
                "wordpress",
                "wp-config.php",
                RepairClassification::Blocked,
                "Preserve the redirected configuration path",
                &error.message,
                &error.recovery,
                true,
            ));
        } else {
            match fs::read_to_string(&config_path) {
            Ok(contents) if contents.contains(MANAGED_CONFIG_MARKER) => {
                if !contents.contains("define('DISABLE_WP_CRON', true);") {
                    let repairable = contents.contains("define('AUTOMATIC_UPDATER_DISABLED', true);");
                    items.push(repair_item(
                        "wordpress_config",
                        "wordpress",
                        "wp-config.php",
                        if repairable {
                            RepairClassification::Repairable
                        } else {
                            RepairClassification::Blocked
                        },
                        if repairable {
                            "Restore the managed runtime anchor"
                        } else {
                            "Preserve the unexpected managed configuration"
                        },
                        if repairable {
                            "The CoffeePOS-managed config is missing DISABLE_WP_CRON but still has the exact updater anchor required for the safe migration."
                        } else {
                            "The managed marker exists but the expected config layout is not recognizable enough for an automatic rewrite."
                        },
                        "Existing database settings and WordPress salts are preserved.",
                        true,
                    ));
                }
            }
            Ok(_) => items.push(repair_item(
                "wordpress_config",
                "wordpress",
                "wp-config.php",
                RepairClassification::Blocked,
                "Preserve the unmanaged configuration",
                "wp-config.php exists but is not marked as CoffeePOS-managed.",
                "CoffeePOS will not adopt or overwrite an unmanaged WordPress configuration.",
                true,
            )),
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && journal.stage >= ProvisioningStage::WordPressInstalled =>
            {
                items.push(repair_item(
                    "wordpress_config",
                    "wordpress",
                    "wp-config.php",
                    RepairClassification::RequiresInput,
                    "Recreate the managed WordPress configuration",
                    "The installed managed store journal is valid, but wp-config.php is missing.",
                    "CoffeePOS will reuse the protected database credentials and generate new WordPress salts. Existing WordPress browser sessions will be signed out; pressing Sửa chữa confirms this impact.",
                    true,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => items.push(repair_item(
                "wordpress_config",
                "wordpress",
                "wp-config.php",
                RepairClassification::Blocked,
                "Fix filesystem access before repair",
                &format!("Cannot read wp-config.php: {error}."),
                "No configuration changes are made while the existing file cannot be inspected.",
                true,
            )),
        }
        }

        plan_managed_file(
            &self.data_root,
            items,
            "wordpress_router",
            "wordpress",
            "WordPress router",
            &self.data_root.join("config/wordpress-router.php"),
            MANAGED_ROUTER_MARKER,
            WORDPRESS_ROUTER.as_bytes(),
        );
        plan_managed_file(
            &self.data_root,
            items,
            "wordpress_uploads_bridge",
            "wordpress",
            "WordPress uploads bridge",
            &site.join("wp-content/mu-plugins/coffeepos-desktop-runtime.php"),
            MANAGED_MU_PLUGIN_MARKER,
            WORDPRESS_UPLOADS_MU_PLUGIN.as_bytes(),
        );

        if journal.wordpress_version != self.wordpress.version {
            items.push(repair_item(
                "wordpress_core",
                "wordpress",
                "WordPress core",
                RepairClassification::Blocked,
                "Use an explicit WordPress upgrade/downgrade flow",
                &format!(
                    "Managed store journal records WordPress {}, while the pinned baseline is {}.",
                    journal.wordpress_version, self.wordpress.version
                ),
                "Repair never upgrades or downgrades WordPress implicitly.",
                true,
            ));
        } else if let Err(error) = ensure_repair_path_safe(&self.data_root, &site, "WordPress core")
        {
            items.push(repair_item(
                "wordpress_core",
                "wordpress",
                "WordPress core",
                RepairClassification::Blocked,
                "Preserve the redirected WordPress site path",
                &error.message,
                &error.recovery,
                true,
            ));
        } else if managed_config_exists(&site) {
            match baseline_tree_differs(
                &self.data_root,
                &self.wordpress.core_root,
                &site,
                Some("wp-content"),
            ) {
                Ok(true) => items.push(repair_item(
                    "wordpress_core",
                    "wordpress",
                    "WordPress core",
                    RepairClassification::Repairable,
                    "Restore the pinned WordPress core files",
                    "One or more managed WordPress core files are missing or differ from the pinned same-version baseline.",
                    "Only baseline core paths are overlaid atomically; wp-content, wp-config.php, uploads, and extra files are preserved.",
                    true,
                )),
                Ok(false) => {}
                Err(error) => items.push(repair_item(
                    "wordpress_core",
                    "wordpress",
                    "WordPress core",
                    RepairClassification::Blocked,
                    "Fix filesystem access before repair",
                    &error.message,
                    "No WordPress core file is changed while ownership/baseline inspection is incomplete.",
                    true,
                )),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_plugin_repair(
        &self,
        _journal: &ProvisioningJournal,
        items: &mut Vec<RepairItem>,
        id: &str,
        component: &str,
        label: &str,
        slug: &str,
        expected_version: &str,
        expected_hash: &str,
        expected_schema: u32,
        journal_version: Option<&str>,
        baseline_root: &Path,
    ) {
        if journal_version != Some(expected_version) {
            items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Blocked,
                "Use an explicit plugin upgrade/downgrade flow",
                &format!(
                    "Provisioning journal version {:?} does not match pinned {label} {expected_version}.",
                    journal_version
                ),
                "Repair never changes plugin versions implicitly.",
                true,
            ));
            return;
        }

        let destination = self.data_root.join("site/wp-content/plugins").join(slug);
        if let Err(error) = ensure_repair_path_safe(&self.data_root, &destination, label) {
            items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Blocked,
                "Preserve the redirected plugin path",
                &error.message,
                &error.recovery,
                true,
            ));
            return;
        }
        if !destination.exists() {
            items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Repairable,
                "Restore the exact managed plugin",
                "The provisioning journal proves this exact plugin version was managed, but the destination directory is missing.",
                "The pinned same-version artifact is restored; database/plugin business data is not deleted or migrated.",
                true,
            ));
            return;
        }
        if !destination.is_dir() {
            items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Blocked,
                "Preserve the conflicting destination",
                "The managed plugin destination exists but is not a directory.",
                "CoffeePOS will not replace an unexpected filesystem object.",
                true,
            ));
            return;
        }
        if let Err(error) = ensure_repair_tree_safe(&self.data_root, &destination, label) {
            items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Blocked,
                "Preserve the redirected plugin tree",
                &error.message,
                &error.recovery,
                true,
            ));
            return;
        }

        let ownership_path = destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE);
        let ownership = fs::read(&ownership_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ManagedPluginOwnership>(&bytes).ok());
        let Some(ownership) = ownership else {
            items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Blocked,
                "Preserve the plugin directory",
                "The existing plugin directory does not have readable CoffeePOS ownership metadata.",
                "Repair will not adopt or overwrite a plugin whose ownership cannot be proven.",
                true,
            ));
            return;
        };
        if ownership.schema_version != expected_schema
            || ownership.plugin != slug
            || ownership.version != expected_version
            || !ownership.archive_sha256.eq_ignore_ascii_case(expected_hash)
        {
            items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Blocked,
                "Preserve the incompatible managed plugin",
                "Plugin ownership metadata does not match the pinned same-version baseline.",
                "Use an explicit adoption/upgrade flow; Phase 6.3 will not rewrite incompatible ownership metadata.",
                true,
            ));
            return;
        }

        match baseline_tree_differs(
            &self.data_root,
            baseline_root,
            &destination,
            Some(MANAGED_PLUGIN_OWNERSHIP_FILE),
        ) {
            Ok(true) => items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Repairable,
                "Restore the pinned same-version plugin files",
                "The plugin is proven CoffeePOS-managed but one or more baseline files are missing or modified.",
                "Repair stages a copy of the current directory, overlays the pinned baseline, then swaps atomically so extra managed-directory files are preserved.",
                true,
            )),
            Ok(false) => {}
            Err(error) => items.push(repair_item(
                id,
                component,
                label,
                RepairClassification::Blocked,
                "Fix filesystem access before repair",
                &error.message,
                "No plugin files are changed while baseline comparison is incomplete.",
                true,
            )),
        }
    }

    fn validate_interrupted_repair(&self, journal: &RepairJournal) -> Result<(), RuntimeErrorInfo> {
        for (path, label) in [
            (
                self.data_root.join(WOOCOMMERCE_REPAIR_STAGING),
                "WooCommerce repair staging",
            ),
            (
                self.data_root.join(WOOCOMMERCE_REPAIR_BACKUP),
                "WooCommerce repair backup",
            ),
            (
                self.data_root.join(COFFEEPOS_REPAIR_STAGING),
                "CoffeePOS repair staging",
            ),
            (
                self.data_root.join(COFFEEPOS_REPAIR_BACKUP),
                "CoffeePOS repair backup",
            ),
        ] {
            ensure_repair_path_safe(&self.data_root, &path, label)?;
            if path.is_dir() {
                ensure_repair_tree_safe(&self.data_root, &path, label)?;
            }
        }

        if matches!(
            journal.stage,
            RepairJournalStage::Verified | RepairJournalStage::Committed
        ) {
            return Ok(());
        }

        for (item_id, backup_name, label) in [
            (
                "woocommerce_plugin",
                WOOCOMMERCE_REPAIR_BACKUP,
                "WooCommerce",
            ),
            ("coffeepos_plugin", COFFEEPOS_REPAIR_BACKUP, "CoffeePOS"),
        ] {
            let completed = journal.completed_item_ids.iter().any(|id| id == item_id);
            let active = journal.active_item_id.as_deref() == Some(item_id);
            let backup_exists = self.data_root.join(backup_name).exists();
            if completed && !backup_exists {
                return Err(provisioning_error(
                    "recover interrupted repair",
                    format!(
                        "{label} was recorded as swapped, but its owned pre-repair backup/sentinel is missing."
                    ),
                    "Preserve the live plugin and repair journal. CoffeePOS cannot infer the pre-repair state after rollback evidence was lost.",
                ));
            }
            if active && !backup_exists {
                return Err(provisioning_error(
                    "recover interrupted repair",
                    format!(
                        "{label} repair was interrupted while active, but no owned rollback evidence is available."
                    ),
                    "Preserve the live plugin and repair journal. Resolve the ambiguous plugin state explicitly before continuing repair.",
                ));
            }
        }
        Ok(())
    }

    pub fn recover_interrupted_repair(
        &self,
        recovery_plan_id: &str,
    ) -> Result<RepairItemResult, RuntimeErrorInfo> {
        let journal = self.load_repair_journal()?.ok_or_else(|| {
            provisioning_error(
                "recover interrupted repair",
                "Repair transaction state is missing.",
                "Inspect the store again before starting another repair.",
            )
        })?;
        self.validate_interrupted_repair(&journal)?;
        let current_recovery_id = repair_recovery_plan_id(&self.data_root, &journal);
        if current_recovery_id != recovery_plan_id {
            return Err(provisioning_error(
                "recover interrupted repair",
                "Repair recovery evidence changed after the recovery plan was inspected.",
                "Inspect the repair plan again before moving or deleting any owned backup evidence.",
            ));
        }

        match journal.stage {
            RepairJournalStage::Verified => {
                commit_pending_plugin_repair_backups(&self.data_root)?;
            }
            RepairJournalStage::Committed => {}
            RepairJournalStage::Planned
            | RepairJournalStage::RuntimeStopped
            | RepairJournalStage::Staged
            | RepairJournalStage::Swapped => {
                rollback_pending_plugin_repair_trees(&self.data_root)?;
            }
        }

        self.advance_repair_journal(&journal.plan_id, RepairJournalStage::Committed)?;
        self.finish_repair(&journal.plan_id)?;
        Ok(RepairItemResult {
            id: "repair_transaction".into(),
            status: RepairItemStatus::Repaired,
            message: if matches!(
                journal.stage,
                RepairJournalStage::Verified | RepairJournalStage::Committed
            ) {
                "Verified interrupted repair transaction committed and cleaned up. Inspect the store again before continuing normal operation."
                    .into()
            } else {
                "Interrupted repair transaction recovered to a deterministic safe state. Unverified plugin swaps were rolled back; inspect the store again for any remaining repair items."
                    .into()
            },
        })
    }

    pub fn begin_repair(&self, plan: &RepairPlan) -> Result<(), RuntimeErrorInfo> {
        if let Some(existing) = self.load_repair_journal()? {
            return Err(provisioning_error(
                "start repair transaction",
                format!(
                    "Repair transaction '{}' is already present at stage {:?}.",
                    existing.plan_id, existing.stage
                ),
                "Preserve the existing repair journal and recover that transaction before starting a new repair.",
            ));
        }
        let journal = RepairJournal {
            schema_version: REPAIR_SCHEMA_VERSION,
            plan_id: plan.plan_id.clone(),
            item_ids: plan.items.iter().map(|item| item.id.clone()).collect(),
            completed_item_ids: Vec::new(),
            active_item_id: None,
            runtime_was_running: plan.runtime_was_running,
            stage: RepairJournalStage::Planned,
        };
        self.persist_repair_journal(&journal)
    }

    pub fn mark_repair_runtime_stopped(&self, plan_id: &str) -> Result<(), RuntimeErrorInfo> {
        self.advance_repair_journal(plan_id, RepairJournalStage::RuntimeStopped)
    }

    pub fn apply_offline_repairs(
        &self,
        plan: &RepairPlan,
    ) -> Result<Vec<RepairItemResult>, RuntimeErrorInfo> {
        self.advance_repair_journal(&plan.plan_id, RepairJournalStage::Staged)?;
        let mut results = Vec::with_capacity(plan.items.len());
        for item in &plan.items {
            if item.classification == RepairClassification::Blocked {
                results.push(RepairItemResult {
                    id: item.id.clone(),
                    status: RepairItemStatus::Blocked,
                    message: item.reason.clone(),
                });
                continue;
            }
            if matches!(
                item.id.as_str(),
                "wordpress_admin_password" | "machine_token_pending"
            ) {
                results.push(RepairItemResult {
                    id: item.id.clone(),
                    status: RepairItemStatus::Skipped,
                    message: "This repair item requires the managed runtime for verification."
                        .into(),
                });
                continue;
            }

            let plugin_item = matches!(item.id.as_str(), "woocommerce_plugin" | "coffeepos_plugin");
            if !plugin_item {
                self.mark_repair_item_started(&plan.plan_id, &item.id)?;
            }
            let repaired = match item.id.as_str() {
                "wordpress_config" => {
                    ensure_repair_path_safe(
                        &self.data_root,
                        &self.data_root.join("site/wp-config.php"),
                        "wp-config.php",
                    )?;
                    self.ensure_wp_config()?;
                    "Managed wp-config.php runtime anchor restored."
                }
                "wordpress_router" => {
                    ensure_repair_path_safe(
                        &self.data_root,
                        &self.data_root.join("config/wordpress-router.php"),
                        "WordPress router",
                    )?;
                    write_managed_file(
                        &self.data_root.join("config/wordpress-router.php"),
                        MANAGED_ROUTER_MARKER,
                        WORDPRESS_ROUTER.as_bytes(),
                        "WordPress router",
                    )?;
                    "Managed WordPress router restored."
                }
                "wordpress_uploads_bridge" => {
                    let path = self
                        .data_root
                        .join("site/wp-content/mu-plugins/coffeepos-desktop-runtime.php");
                    ensure_repair_path_safe(&self.data_root, &path, "WordPress uploads bridge")?;
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(|error| {
                            provisioning_error(
                                "repair WordPress uploads bridge",
                                format!("Cannot create mu-plugins directory: {error}."),
                                "Check site permissions and retry repair.",
                            )
                        })?;
                    }
                    write_managed_file(
                        &path,
                        MANAGED_MU_PLUGIN_MARKER,
                        WORDPRESS_UPLOADS_MU_PLUGIN.as_bytes(),
                        "WordPress uploads bridge",
                    )?;
                    "Managed WordPress uploads bridge restored."
                }
                "wordpress_core" => {
                    ensure_repair_path_safe(
                        &self.data_root,
                        &self.data_root.join("site"),
                        "WordPress core",
                    )?;
                    overlay_baseline_tree(
                        &self.data_root,
                        &self.wordpress.core_root,
                        &self.data_root.join("site"),
                        Some("wp-content"),
                        "WordPress core",
                    )?;
                    "Pinned same-version WordPress core files restored."
                }
                "woocommerce_plugin" => {
                    repair_managed_plugin_tree(
                        &self.data_root,
                        WOOCOMMERCE_PLUGIN_SLUG,
                        &self.woocommerce.version,
                        &self.woocommerce.archive_sha256,
                        WOOCOMMERCE_OWNERSHIP_SCHEMA_VERSION,
                        &self.woocommerce.plugin_root,
                        WOOCOMMERCE_REPAIR_STAGING,
                        WOOCOMMERCE_REPAIR_BACKUP,
                        "WooCommerce",
                    )?;
                    "Pinned same-version WooCommerce files restored."
                }
                "coffeepos_plugin" => {
                    repair_managed_plugin_tree(
                        &self.data_root,
                        COFFEEPOS_PLUGIN_SLUG,
                        &self.coffeepos.version,
                        &self.coffeepos.archive_sha256,
                        COFFEEPOS_OWNERSHIP_SCHEMA_VERSION,
                        &self.coffeepos.plugin_root,
                        COFFEEPOS_REPAIR_STAGING,
                        COFFEEPOS_REPAIR_BACKUP,
                        "CoffeePOS",
                    )?;
                    "Pinned same-version CoffeePOS files restored."
                }
                _ => {
                    self.mark_repair_item_completed(&plan.plan_id, &item.id)?;
                    results.push(RepairItemResult {
                        id: item.id.clone(),
                        status: RepairItemStatus::Skipped,
                        message: "No automatic mutation is defined for this repair item.".into(),
                    });
                    continue;
                }
            };
            if plugin_item {
                self.mark_repair_item_started(&plan.plan_id, &item.id)?;
            }
            self.mark_repair_item_completed(&plan.plan_id, &item.id)?;
            results.push(RepairItemResult {
                id: item.id.clone(),
                status: RepairItemStatus::Repaired,
                message: repaired.into(),
            });
        }
        self.advance_repair_journal(&plan.plan_id, RepairJournalStage::Swapped)?;
        Ok(results)
    }

    pub fn repair_admin_password(
        &self,
        runtime_info: &RuntimeInfo,
        replacement_password: Option<&str>,
    ) -> Result<Option<RepairItemResult>, RuntimeErrorInfo> {
        let pending_path = self.data_root.join(REPAIR_ADMIN_PENDING_SECRET);
        let active_path = self.data_root.join(WORDPRESS_ADMIN_SECRET);
        ensure_repair_path_safe(
            &self.data_root,
            &pending_path,
            "administrator password repair credential",
        )?;
        ensure_repair_path_safe(
            &self.data_root,
            &active_path,
            "administrator password credential",
        )?;
        if !pending_path.exists() && protected_secret_ready(&active_path) {
            return Ok(None);
        }

        let password = if pending_path.is_file() {
            secret::load(&pending_path).map_err(|error| {
                provisioning_error(
                    "resume administrator password repair",
                    error,
                    "Preserve the pending protected credential and retry with the same Windows user profile.",
                )
            })?
        } else {
            let password = replacement_password.ok_or_else(|| {
                provisioning_error(
                    "repair administrator password",
                    "A replacement administrator password is required.",
                    "Enter a 12–128 character replacement password and retry the repair.",
                )
            })?;
            validate_admin_repair_password(password)?;
            secret::store_password(&pending_path, password).map_err(|error| {
                provisioning_error(
                    "stage administrator password repair",
                    error,
                    "Check protected application-data storage and retry. The WordPress account has not been changed yet.",
                )
            })?;
            password.to_string()
        };

        self.apply_admin_password_to_wordpress(runtime_info, &password)?;
        secret::promote_staged_password(&pending_path, &active_path).map_err(|error| {
            provisioning_error(
                "promote administrator password repair",
                error,
                "WordPress already accepts the pending password. Preserve the pending protected credential and retry repair to finish promotion.",
            )
        })?;
        Ok(Some(RepairItemResult {
            id: "wordpress_admin_password".into(),
            status: RepairItemStatus::Repaired,
            message: "Existing WordPress administrator password reset and verified; protected credential promoted.".into(),
        }))
    }

    pub fn recover_pending_machine_token(
        &self,
        runtime_info: &RuntimeInfo,
    ) -> Result<Option<RepairItemResult>, RuntimeErrorInfo> {
        let active_path = self.data_root.join(MACHINE_TOKEN_SECRET);
        let pending_path = self.data_root.join(MACHINE_TOKEN_PENDING_SECRET);
        ensure_repair_path_safe(
            &self.data_root,
            &active_path,
            "CoffeePOS machine credential",
        )?;
        ensure_repair_path_safe(
            &self.data_root,
            &pending_path,
            "CoffeePOS pending machine credential",
        )?;
        if !pending_path.is_file() && !protected_machine_token_ready(&active_path) {
            return Ok(None);
        }
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "recover CoffeePOS machine credential",
                "HTTP port is unavailable during pending credential recovery.",
                "Start the managed runtime and retry repair.",
            )
        })?;
        let health = probe_coffeepos_health(&self.data_root, http_port);
        if !matches!(
            health.state,
            CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
        ) {
            return Err(provisioning_error(
                "recover CoffeePOS machine credential",
                "Neither the active nor pending protected machine credential could be resolved to an accepted CoffeePOS endpoint credential.",
                "Preserve the pending credential. CoffeePOS will not overwrite the server-side hash without independent recovery authority.",
            ));
        }
        if !protected_machine_token_ready(&active_path) || pending_path.exists() {
            return Err(provisioning_error(
                "recover CoffeePOS machine credential",
                "CoffeePOS accepted a machine credential, but protected credential promotion/cleanup is incomplete.",
                "Preserve both protected credential files and retry repair; do not start another rotation.",
            ));
        }
        Ok(Some(RepairItemResult {
            id: "machine_token_pending".into(),
            status: RepairItemStatus::Repaired,
            message:
                "Pending CoffeePOS machine credential verified against the endpoint and promoted."
                    .into(),
        }))
    }

    pub fn mark_repair_verified(&self, plan_id: &str) -> Result<(), RuntimeErrorInfo> {
        self.advance_repair_journal(plan_id, RepairJournalStage::Verified)
    }

    pub fn mark_repair_committed(&self, plan_id: &str) -> Result<(), RuntimeErrorInfo> {
        self.advance_repair_journal(plan_id, RepairJournalStage::Committed)
    }

    pub fn finish_repair(&self, plan_id: &str) -> Result<(), RuntimeErrorInfo> {
        let journal = self.load_repair_journal()?.ok_or_else(|| {
            provisioning_error(
                "complete repair transaction",
                "Repair transaction state is missing before final cleanup.",
                "Inspect the store before starting another repair.",
            )
        })?;
        if journal.plan_id != plan_id || journal.stage != RepairJournalStage::Committed {
            return Err(provisioning_error(
                "complete repair transaction",
                "Repair transaction has not reached its committed state.",
                "Preserve config/repair.json and resume transaction recovery instead of deleting the commit point.",
            ));
        }
        let path = self.data_root.join(REPAIR_JOURNAL);
        fs::remove_file(&path).map_err(|error| {
            provisioning_error(
                "complete repair transaction",
                format!("Repair verified but its journal could not be removed: {error}."),
                "Retry repair cleanup. Store data and the verified repaired files are preserved.",
            )
        })
    }

    fn repair_journal_path(&self) -> PathBuf {
        self.data_root.join(REPAIR_JOURNAL)
    }

    fn load_repair_journal(&self) -> Result<Option<RepairJournal>, RuntimeErrorInfo> {
        let path = self.repair_journal_path();
        ensure_repair_path_safe(&self.data_root, &path, "repair journal")?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| {
            provisioning_error(
                "read repair journal",
                format!("Cannot read repair transaction state: {error}."),
                "Preserve config/repair.json and retry with the same store.",
            )
        })?;
        let journal: RepairJournal = serde_json::from_slice(&bytes).map_err(|error| {
            provisioning_error(
                "read repair journal",
                format!("Repair transaction state is invalid: {error}."),
                "Preserve config/repair.json and repair the journal explicitly before starting another mutation.",
            )
        })?;
        if journal.schema_version != REPAIR_SCHEMA_VERSION {
            return Err(provisioning_error(
                "read repair journal",
                format!(
                    "Repair journal schema {} is not supported by this Desktop build.",
                    journal.schema_version
                ),
                "Use the matching CoffeePOS Desktop version or restore a compatible repair transaction state.",
            ));
        }
        Ok(Some(journal))
    }

    fn persist_repair_journal(&self, journal: &RepairJournal) -> Result<(), RuntimeErrorInfo> {
        ensure_repair_path_safe(
            &self.data_root,
            &self.repair_journal_path(),
            "repair journal",
        )?;
        let mut bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
            provisioning_error(
                "save repair journal",
                format!("Cannot serialize repair transaction state: {error}."),
                "Retry after checking application-data storage.",
            )
        })?;
        bytes.push(b'\n');
        atomic_write(&self.repair_journal_path(), &bytes, "repair journal")
    }

    fn advance_repair_journal(
        &self,
        plan_id: &str,
        stage: RepairJournalStage,
    ) -> Result<(), RuntimeErrorInfo> {
        let mut journal = self.load_repair_journal()?.ok_or_else(|| {
            provisioning_error(
                "advance repair transaction",
                "Repair transaction state is missing.",
                "Inspect the store again and start a new repair plan.",
            )
        })?;
        if journal.plan_id != plan_id {
            return Err(provisioning_error(
                "advance repair transaction",
                "Repair transaction belongs to a different plan.",
                "Preserve the journal and inspect the store before starting another repair.",
            ));
        }
        journal.stage = stage;
        self.persist_repair_journal(&journal)
    }

    fn mark_repair_item_started(
        &self,
        plan_id: &str,
        item_id: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let mut journal = self.load_repair_journal()?.ok_or_else(|| {
            provisioning_error(
                "record repair item",
                "Repair transaction state is missing.",
                "Preserve the store and inspect the repair transaction before retrying.",
            )
        })?;
        if journal.plan_id != plan_id || !journal.item_ids.iter().any(|id| id == item_id) {
            return Err(provisioning_error(
                "record repair item",
                "Repair item does not belong to the active repair transaction.",
                "Inspect the store again instead of applying stale repair state.",
            ));
        }
        journal.active_item_id = Some(item_id.to_string());
        self.persist_repair_journal(&journal)
    }

    fn mark_repair_item_completed(
        &self,
        plan_id: &str,
        item_id: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let mut journal = self.load_repair_journal()?.ok_or_else(|| {
            provisioning_error(
                "record repair item",
                "Repair transaction state is missing.",
                "Preserve the store and inspect the repair transaction before retrying.",
            )
        })?;
        if journal.plan_id != plan_id || journal.active_item_id.as_deref() != Some(item_id) {
            return Err(provisioning_error(
                "record repair item",
                "Repair item completion does not match the active transaction item.",
                "Preserve the repair journal and recover the transaction before continuing.",
            ));
        }
        if !journal.completed_item_ids.iter().any(|id| id == item_id) {
            journal.completed_item_ids.push(item_id.to_string());
        }
        journal.active_item_id = None;
        self.persist_repair_journal(&journal)
    }

    fn apply_admin_password_to_wordpress(
        &self,
        runtime_info: &RuntimeInfo,
        password: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "repair administrator password",
                "MariaDB port is unavailable while updating the WordPress administrator.",
                "Start the managed runtime and retry repair.",
            )
        })?;
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "repair administrator password",
                "HTTP port is unavailable while updating the WordPress administrator.",
                "Start the managed runtime and retry repair.",
            )
        })?;
        let journal = self.load_journal()?.ok_or_else(|| {
            provisioning_error(
                "repair administrator password",
                "Provisioning journal is missing, so the administrator identity cannot be proven.",
                "Preserve the store and restore the provisioning journal before resetting a password.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "repair administrator password",
                    error,
                    "Restore the matching WordPress database credential before resetting the administrator password.",
                )
            })?;
        let script = self.write_admin_password_repair_script()?;
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
            .env("COFFEEPOS_ADMIN_USERNAME", journal.admin_username)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let mut child = command.spawn().map_err(|error| {
            let _ = fs::remove_file(&script);
            provisioning_error(
                "repair administrator password",
                format!("Cannot start pinned PHP for administrator-password repair: {error}."),
                "Verify the pinned PHP runtime and retry; the pending protected password is preserved.",
            )
        })?;
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(error);
        }
        let stdin_result = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PHP stdin unavailable"))
            .and_then(|mut stdin| stdin.write_all(password.as_bytes()));
        if let Err(error) = stdin_result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(provisioning_error(
                "repair administrator password",
                format!("Cannot pass the replacement administrator password over stdin: {error}."),
                "Retry repair. The password is not placed in process arguments or logs.",
            ));
        }
        let status = wait_for_child_exit(&mut child, Duration::from_secs(30));
        let _ = fs::remove_file(&script);
        let status = status?;
        if !status.success() {
            return Err(provisioning_error(
                "repair administrator password",
                format!("Administrator-password repair exited with status {status}."),
                "The pending protected password is preserved. Inspect WordPress/runtime health and retry repair.",
            ));
        }
        Ok(())
    }

    fn write_admin_password_repair_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare administrator password repair",
                format!("Cannot create temporary administrator-password script: {error}."),
                "Check application-data permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(WORDPRESS_ADMIN_PASSWORD_REPAIR.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare administrator password repair",
                    format!("Cannot write administrator-password repair script: {error}."),
                    "Check application-data storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare administrator password repair",
                format!(
                    "Cannot retain administrator-password repair script: {}.",
                    error.error
                ),
                "Check application-data permissions and retry.",
            )
        })?;
        Ok(path)
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
        match self.load_repair_journal() {
            Ok(Some(repair_journal)) => {
                return ProvisioningInfo {
                    state: ProvisioningState::NeedsRepair,
                    wordpress_version: self.wordpress.version.clone(),
                    woocommerce_version: self.woocommerce.version.clone(),
                    woocommerce_active: journal
                        .as_ref()
                        .map(|value| value.stage >= ProvisioningStage::WooCommerceActivated)
                        .unwrap_or(false),
                    coffeepos_version: self.coffeepos.version.clone(),
                    coffeepos_active: journal
                        .as_ref()
                        .map(|value| value.stage >= ProvisioningStage::CoffeePosActivated)
                        .unwrap_or(false),
                    admin_username: journal.as_ref().map(|value| value.admin_username.clone()),
                    can_retry: false,
                    last_error: Some(provisioning_error(
                        "recover repair transaction",
                        format!(
                            "A Phase 6.3 repair transaction is pending at stage {:?}.",
                            repair_journal.stage
                        ),
                        "Open Hệ thống → Sửa chữa and recover the interrupted repair transaction before normal runtime startup.",
                    )),
                };
            }
            Err(error) => {
                return ProvisioningInfo {
                    state: ProvisioningState::NeedsRepair,
                    wordpress_version: self.wordpress.version.clone(),
                    woocommerce_version: self.woocommerce.version.clone(),
                    woocommerce_active: false,
                    coffeepos_version: self.coffeepos.version.clone(),
                    coffeepos_active: false,
                    admin_username: journal.as_ref().map(|value| value.admin_username.clone()),
                    can_retry: false,
                    last_error: Some(error),
                };
            }
            Ok(None) => {}
        }
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
                admin_username: journal.as_ref().map(|value| value.admin_username.clone()),
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
        let journal = self.load_journal().ok().flatten();
        let stage = journal.as_ref().map(|value| value.stage.clone());
        let wordpress_installed = stage
            .as_ref()
            .is_some_and(|value| value >= &ProvisioningStage::WordPressInstalled);
        let woocommerce_provisioned = stage
            .as_ref()
            .is_some_and(|value| value >= &ProvisioningStage::WooCommerceProvisioned);
        let woocommerce_activated = stage
            .as_ref()
            .is_some_and(|value| value >= &ProvisioningStage::WooCommerceActivated);
        let coffeepos_provisioned = stage
            .as_ref()
            .is_some_and(|value| value >= &ProvisioningStage::CoffeePosProvisioned);
        let coffeepos_activated = stage
            .as_ref()
            .is_some_and(|value| value >= &ProvisioningStage::CoffeePosActivated);
        ProvisioningInfo {
            state: ProvisioningState::Installing,
            wordpress_version: self.wordpress.version.clone(),
            woocommerce_version: if woocommerce_provisioned {
                self.woocommerce.version.clone()
            } else {
                String::new()
            },
            woocommerce_active: woocommerce_activated,
            coffeepos_version: if coffeepos_provisioned {
                self.coffeepos.version.clone()
            } else {
                String::new()
            },
            coffeepos_active: coffeepos_activated,
            admin_username: wordpress_installed.then(|| {
                journal
                    .as_ref()
                    .map(|value| value.admin_username.clone())
                    .unwrap_or_else(|| self.admin_username.clone())
            }),
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
            admin_username: Some(self.admin_username.clone()),
            can_retry: false,
            last_error: None,
        })
    }

    pub(crate) fn import_restored_database(
        &self,
        dump_path: &Path,
    ) -> Result<(), RuntimeErrorInfo> {
        let metadata = fs::symlink_metadata(dump_path).map_err(|error| {
            provisioning_error(
                "import restored database",
                format!("Cannot inspect the staged logical dump: {error}."),
                "Re-extract the validated backup into owned restore staging and retry.",
            )
        })?;
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata_is_reparse_point(&metadata)
        {
            return Err(provisioning_error(
                "import restored database",
                "The staged logical dump is not a regular local file.",
                "Discard owned restore staging and re-extract the validated backup before retrying.",
            ));
        }
        let wordpress_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "import restored database",
                    error,
                    "Restore staging must retain the target-generated WordPress database credential before importing the backup.",
                )
            })?;
        let runtime_password = secret::load(&self.data_root.join(DATABASE_RUNTIME_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "import restored database",
                    error,
                    "Restore staging must retain the target-generated runtime database credential before importing the backup.",
                )
            })?;
        let port = choose_loopback_port(&[])?;
        let endpoint = DatabaseEndpoint::Tcp(port);
        let mut database = self.spawn_database(&endpoint)?;
        let import_result = (|| -> Result<(), RuntimeErrorInfo> {
            self.wait_for_database_user(
                &mut database,
                &endpoint,
                DATABASE_WORDPRESS_USER,
                &wordpress_password,
                Some(DATABASE_NAME),
            )?;
            let input = fs::File::open(dump_path).map_err(|error| {
                provisioning_error(
                    "import restored database",
                    format!("Cannot open the staged logical dump: {error}."),
                    "Re-extract the validated backup into owned restore staging and retry.",
                )
            })?;
            let mut command = self.database_client_command(
                &endpoint,
                DATABASE_WORDPRESS_USER,
                &wordpress_password,
            );
            command
                .arg(format!("--database={DATABASE_NAME}"))
                .stdin(Stdio::from(input))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .current_dir(&self.runtime.mariadb_base_dir);
            configure_child_command(&mut command);
            let status = run_command_bounded(
                command,
                Duration::from_secs(180),
                "restore",
                "import restored database",
                &self.containment,
            )?;
            if !status.success() {
                return Err(provisioning_error(
                    "import restored database",
                    format!("The pinned MariaDB client rejected the staged logical dump (status {status})."),
                    "The active store is unchanged. Discard restore staging, re-inspect the backup, and retry.",
                ));
            }
            Ok(())
        })();

        let shutdown_result = self.run_database_sql(
            &endpoint,
            DATABASE_RUNTIME_USER,
            &runtime_password,
            "SHUTDOWN;\n",
        );
        if shutdown_result.is_err() {
            let _ = database.kill();
        }
        let wait_result = wait_for_child_exit(&mut database, Duration::from_secs(15));
        match (import_result, shutdown_result, wait_result) {
            (Ok(()), Ok(()), Ok(_)) => Ok(()),
            (Err(error), _, _) => Err(error),
            (Ok(()), Err(error), _) => Err(provisioning_error(
                "stop restored database staging",
                error.message,
                "Keep the restore admission gate active and retry staging cleanup before another managed operation.",
            )),
            (Ok(()), Ok(()), Err(error)) => Err(provisioning_error(
                "stop restored database staging",
                error.message,
                "Keep the restore admission gate active until the staging MariaDB process is confirmed stopped.",
            )),
        }
    }

    pub(crate) fn verify_restored_wordpress_identity(
        &self,
        runtime_info: &RuntimeInfo,
        store_name: &str,
        administrator_username: &str,
        administrator_email: &str,
        administrator_password: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "verify restored administrator",
                "MariaDB port is unavailable while verifying the restored WordPress identity.",
                "Keep restore staging isolated, restart its verification runtime, and retry.",
            )
        })?;
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "verify restored administrator",
                "HTTP port is unavailable while verifying the restored WordPress identity.",
                "Keep restore staging isolated, restart its verification runtime, and retry.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "verify restored administrator",
                    error,
                    "Restore staging must keep the target-generated WordPress database credential intact.",
                )
            })?;
        let script = self.write_restore_identity_verification_script()?;
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
            .env("COFFEEPOS_RESTORE_STORE_NAME", store_name)
            .env("COFFEEPOS_RESTORE_ADMIN_USER", administrator_username)
            .env("COFFEEPOS_RESTORE_ADMIN_EMAIL", administrator_email)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let mut child = command.spawn().map_err(|error| {
            let _ = fs::remove_file(&script);
            provisioning_error(
                "verify restored administrator",
                format!("Cannot start pinned PHP for restored-identity verification: {error}."),
                "Verify the pinned PHP runtime and retry restore while staging remains isolated.",
            )
        })?;
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(error);
        }
        let stdin_result = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PHP stdin unavailable"))
            .and_then(|mut stdin| stdin.write_all(administrator_password.as_bytes()));
        if let Err(error) = stdin_result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(provisioning_error(
                "verify restored administrator",
                format!("Cannot pass the restored administrator password to pinned PHP over stdin: {error}."),
                "Retry restore. The administrator password is never placed in process arguments or logs.",
            ));
        }
        let status = wait_for_child_exit(&mut child, Duration::from_secs(30));
        let _ = fs::remove_file(&script);
        let status = status?;
        if !status.success() {
            return Err(provisioning_error(
                "verify restored administrator",
                format!("Restored WordPress identity verification exited with status {status}."),
                "The active store is unchanged. Verify that the backup contains the matching administrator credential and retry.",
            ));
        }
        Ok(())
    }

    pub(crate) fn complete_restored_provisioning(
        &self,
        runtime_info: &RuntimeInfo,
        store_name: &str,
    ) -> Result<ProvisioningInfo, RuntimeErrorInfo> {
        let journal = self.load_journal()?.ok_or_else(|| {
            provisioning_error(
                "adopt restored WordPress",
                "Restore staging is missing its provisioning ownership journal.",
                "Discard owned restore staging and rebuild it from the validated backup.",
            )
        })?;
        if journal.stage < ProvisioningStage::SiteReady {
            return Err(provisioning_error(
                "adopt restored WordPress",
                "Restore staging has not completed the target WordPress/core and database preparation boundary.",
                "Rebuild restore staging before importing or activating the restored store.",
            ));
        }
        if journal.stage < ProvisioningStage::WordPressInstalled {
            self.persist_stage(ProvisioningStage::WordPressInstalled)?;
        }
        ensure_woocommerce_plugin(&self.data_root, &self.woocommerce)?;
        if self
            .load_journal()?
            .is_some_and(|value| value.stage < ProvisioningStage::WooCommerceProvisioned)
        {
            self.persist_stage(ProvisioningStage::WooCommerceProvisioned)?;
        }
        self.activate_woocommerce(runtime_info)?;
        if self
            .load_journal()?
            .is_some_and(|value| value.stage < ProvisioningStage::WooCommerceActivated)
        {
            self.persist_stage(ProvisioningStage::WooCommerceActivated)?;
        }
        ensure_coffeepos_plugin(&self.data_root, &self.coffeepos, &self.woocommerce)?;
        if self
            .load_journal()?
            .is_some_and(|value| value.stage < ProvisioningStage::CoffeePosProvisioned)
        {
            self.persist_stage(ProvisioningStage::CoffeePosProvisioned)?;
        }
        self.activate_coffeepos(runtime_info, store_name)?;
        if self
            .load_journal()?
            .is_some_and(|value| value.stage < ProvisioningStage::CoffeePosActivated)
        {
            self.persist_stage(ProvisioningStage::CoffeePosActivated)?;
        }
        self.bootstrap_machine_health_for_restore(runtime_info)?;
        if self
            .load_journal()?
            .is_some_and(|value| value.stage < ProvisioningStage::MachineHealthBootstrapped)
        {
            self.persist_stage(ProvisioningStage::MachineHealthBootstrapped)?;
        }
        Ok(ProvisioningInfo {
            state: ProvisioningState::Ready,
            wordpress_version: self.wordpress.version.clone(),
            woocommerce_version: self.woocommerce.version.clone(),
            woocommerce_active: true,
            coffeepos_version: self.coffeepos.version.clone(),
            coffeepos_active: true,
            admin_username: Some(self.admin_username.clone()),
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
            .env("COFFEEPOS_ADMIN_USER", &self.admin_username)
            .env("COFFEEPOS_ADMIN_EMAIL", &self.admin_email)
            .env("COFFEEPOS_ADMIN_PASSWORD", admin_password)
            .env(
                "COFFEEPOS_VERIFY_INITIAL_SETUP",
                if wordpress_already_installed {
                    "0"
                } else {
                    "1"
                },
            )
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
        self.activate_coffeepos(runtime_info, store_name)?;
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
            admin_username: Some(self.admin_username.clone()),
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

    fn activate_coffeepos(
        &self,
        runtime_info: &RuntimeInfo,
        store_name: &str,
    ) -> Result<(), RuntimeErrorInfo> {
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
            .env("COFFEEPOS_INITIAL_STORE_NAME", store_name)
            .env("COFFEEPOS_INITIAL_ADMIN_USER", &self.admin_username)
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

    pub fn verify_repaired_plugins(
        &self,
        runtime_info: &RuntimeInfo,
        store_name: &str,
        verify_woocommerce: bool,
        verify_coffeepos: bool,
    ) -> Result<(), RuntimeErrorInfo> {
        if verify_woocommerce {
            self.activate_woocommerce(runtime_info)?;
        }
        if verify_coffeepos {
            self.activate_coffeepos(runtime_info, store_name)?;
        }
        Ok(())
    }

    pub fn commit_repaired_plugins(
        &self,
        commit_woocommerce: bool,
        commit_coffeepos: bool,
    ) -> Result<(), RuntimeErrorInfo> {
        if commit_woocommerce {
            commit_managed_plugin_repair(
                &self.data_root,
                WOOCOMMERCE_REPAIR_BACKUP,
                "WooCommerce",
            )?;
        }
        if commit_coffeepos {
            commit_managed_plugin_repair(&self.data_root, COFFEEPOS_REPAIR_BACKUP, "CoffeePOS")?;
        }
        Ok(())
    }

    pub fn rollback_repaired_plugins(
        &self,
        rollback_woocommerce: bool,
        rollback_coffeepos: bool,
    ) -> Result<(), RuntimeErrorInfo> {
        if rollback_woocommerce {
            rollback_managed_plugin_repair(
                &self.data_root,
                WOOCOMMERCE_PLUGIN_SLUG,
                WOOCOMMERCE_REPAIR_STAGING,
                WOOCOMMERCE_REPAIR_BACKUP,
                "WooCommerce",
            )?;
        }
        if rollback_coffeepos {
            rollback_managed_plugin_repair(
                &self.data_root,
                COFFEEPOS_PLUGIN_SLUG,
                COFFEEPOS_REPAIR_STAGING,
                COFFEEPOS_REPAIR_BACKUP,
                "CoffeePOS",
            )?;
        }
        Ok(())
    }

    pub fn rollback_pending_plugin_repairs(&self) -> Result<(), RuntimeErrorInfo> {
        rollback_pending_plugin_repair_trees(&self.data_root)
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

    fn bootstrap_machine_health_for_restore(
        &self,
        runtime_info: &RuntimeInfo,
    ) -> Result<(), RuntimeErrorInfo> {
        let http_port = runtime_info.http_port.ok_or_else(|| {
            provisioning_error(
                "bind restored CoffeePOS machine health",
                "HTTP port is unavailable while binding the target machine credential.",
                "Keep restore staging isolated, restart its verification runtime, and retry.",
            )
        })?;
        let database_port = runtime_info.database_port.ok_or_else(|| {
            provisioning_error(
                "bind restored CoffeePOS machine health",
                "MariaDB port is unavailable while binding the target machine credential.",
                "Keep restore staging isolated, restart its verification runtime, and retry.",
            )
        })?;
        let database_password = secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET))
            .map_err(|error| {
                provisioning_error(
                    "bind restored CoffeePOS machine health",
                    error,
                    "Restore staging must keep the target-generated WordPress database credential intact.",
                )
            })?;
        let token_path = self.data_root.join(MACHINE_TOKEN_SECRET);
        let pending_token_path = self.data_root.join(MACHINE_TOKEN_PENDING_SECRET);
        if token_path.exists() || pending_token_path.exists() {
            return Err(provisioning_error(
                "bind restored CoffeePOS machine health",
                "Restore staging already contains a machine credential before target binding starts.",
                "Keep restore staging isolated and recover or rebuild the transaction-owned staging store before retrying.",
            ));
        }
        let token = secret::create_machine_token(&pending_token_path).map_err(|error| {
            provisioning_error(
                "bind restored CoffeePOS machine health",
                error,
                "Check protected target storage and operating-system random generation, then retry restore.",
            )
        })?;
        let script = self.write_restore_machine_health_script()?;
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
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(self.data_root.join("site"));
        configure_child_command(&mut command);
        let mut child = command.spawn().map_err(|error| {
            let _ = fs::remove_file(&script);
            provisioning_error(
                "bind restored CoffeePOS machine health",
                format!("Cannot start pinned PHP for target machine-token binding: {error}."),
                "Verify the pinned PHP runtime and retry restore while staging remains isolated.",
            )
        })?;
        if let Err(error) = self.containment.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(error);
        }
        let stdin_result = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PHP stdin unavailable"))
            .and_then(|mut stdin| stdin.write_all(token.as_bytes()));
        if let Err(error) = stdin_result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&script);
            return Err(provisioning_error(
                "bind restored CoffeePOS machine health",
                format!("Cannot pass the target machine credential to pinned PHP over stdin: {error}."),
                "Retry restore. The target credential remains protected and is never placed in process arguments or logs.",
            ));
        }
        let status = wait_for_child_exit(&mut child, Duration::from_secs(30));
        let _ = fs::remove_file(&script);
        let status = status?;
        if !status.success() {
            return Err(provisioning_error(
                "bind restored CoffeePOS machine health",
                format!("Target machine-token binding exited with status {status}."),
                "The active store is unchanged. Keep restore staging isolated and retry or abort the restore transaction.",
            ));
        }
        let pending_health = probe_coffeepos_health_with_token(http_port, &token);
        if !matches!(
            pending_health.state,
            CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
        ) {
            let details = pending_health
                .error
                .map(|error| format!("{} {}", error.message, error.recovery))
                .unwrap_or_else(|| "CoffeePOS machine-health probe is unavailable.".into());
            return Err(provisioning_error(
                "verify restored CoffeePOS machine health",
                details,
                "Keep restore staging isolated and repair its target-local credential binding before cutover.",
            ));
        }
        secret::store_machine_token(&token_path, &token).map_err(|error| {
            provisioning_error(
                "promote restored CoffeePOS machine health",
                error,
                "The restored staging endpoint accepts the pending target credential. Preserve pending protected state and retry promotion before cutover.",
            )
        })?;
        fs::remove_file(&pending_token_path).map_err(|error| {
            provisioning_error(
                "promote restored CoffeePOS machine health",
                format!("The target machine credential was promoted but its pending file cannot be removed: {error}."),
                "Keep restore fenced and retry credential cleanup before staging verification completes.",
            )
        })?;
        let promoted_health = probe_coffeepos_health(&self.data_root, http_port);
        if !matches!(
            promoted_health.state,
            CoffeePosHealthState::Healthy | CoffeePosHealthState::Degraded
        ) {
            return Err(provisioning_error(
                "verify promoted CoffeePOS machine health",
                "The promoted target machine credential did not authenticate after staging promotion.",
                "Keep restore staging isolated and preserve its protected credential evidence before retrying recovery.",
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

    fn require_repair_database_account(
        &self,
        endpoint: &DatabaseEndpoint,
        user: &str,
        password: &str,
        database_name: Option<&str>,
        display_name: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        if self.database_user_probe(endpoint, user, password, database_name)? {
            Ok(())
        } else {
            Err(provisioning_error(
                "verify repair database credentials",
                format!(
                    "MariaDB rejected the protected {display_name} database credential."
                ),
                "Preserve the MariaDB datadir and restore the matching protected credential. Repair will not rotate accounts, bypass authentication, or reinitialize the database.",
            ))
        }
    }

    fn stop_repair_preflight_database(
        &self,
        database: &mut Child,
        endpoint: &DatabaseEndpoint,
        runtime_password: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        let inspect_error = match database.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => None,
            Err(error) => Some(error),
        };

        let graceful = inspect_error.is_none()
            && self
                .database_user_probe(endpoint, DATABASE_RUNTIME_USER, runtime_password, None)
                .unwrap_or(false)
            && self
                .run_database_sql(
                    endpoint,
                    DATABASE_RUNTIME_USER,
                    runtime_password,
                    "SHUTDOWN;\n",
                )
                .is_ok();
        if graceful && wait_for_child_exit(database, Duration::from_secs(10)).is_ok() {
            return Ok(());
        }

        let kill_error = database.kill().err();
        match wait_for_child_exit(database, Duration::from_secs(5)) {
            Ok(_) => Ok(()),
            Err(wait_error) => {
                let inspect_detail = inspect_error
                    .map(|error| format!(" Initial process-state inspection failed: {error}."))
                    .unwrap_or_default();
                let kill_detail = kill_error
                    .map(|error| format!(" Termination request failed: {error}."))
                    .unwrap_or_default();
                Err(provisioning_error(
                    "clean up repair database preflight",
                    format!(
                        "Temporary MariaDB preflight process could not be confirmed stopped.{}{} {}",
                        inspect_detail, kill_detail, wait_error.message
                    ),
                    "Confirm no MariaDB process still owns the CoffeePOS datadir before retrying repair.",
                ))
            }
        }
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

    fn write_restore_identity_verification_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare restored identity verification",
                format!("Cannot create temporary restore verification script: {error}."),
                "Check restore-staging permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(WORDPRESS_RESTORE_IDENTITY_VERIFY.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare restored identity verification",
                    format!("Cannot write temporary restore verification script: {error}."),
                    "Check restore-staging storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare restored identity verification",
                format!(
                    "Cannot retain temporary restore verification script: {}.",
                    error.error
                ),
                "Check restore-staging permissions and retry.",
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

    fn write_restore_machine_health_script(&self) -> Result<PathBuf, RuntimeErrorInfo> {
        let config = self.data_root.join("config");
        let mut temporary = NamedTempFile::new_in(&config).map_err(|error| {
            provisioning_error(
                "prepare restored CoffeePOS machine health",
                format!("Cannot create temporary restore machine-health script: {error}."),
                "Check restore-staging permissions and free disk space, then retry.",
            )
        })?;
        temporary
            .write_all(COFFEEPOS_RESTORE_MACHINE_HEALTH_BOOTSTRAP.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| {
                provisioning_error(
                    "prepare restored CoffeePOS machine health",
                    format!("Cannot write temporary restore machine-health script: {error}."),
                    "Check restore-staging storage health and retry.",
                )
            })?;
        let (_file, path) = temporary.keep().map_err(|error| {
            provisioning_error(
                "prepare restored CoffeePOS machine health",
                format!(
                    "Cannot retain temporary restore machine-health script: {}.",
                    error.error
                ),
                "Check restore-staging permissions and retry.",
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
            admin_username: existing
                .as_ref()
                .map(|value| value.admin_username.clone())
                .unwrap_or_else(|| self.admin_username.clone()),
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
        code: "provisioning_error".into(),
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

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn ensure_repair_path_safe(
    data_root: &Path,
    target: &Path,
    label: &str,
) -> Result<(), RuntimeErrorInfo> {
    let relative = target.strip_prefix(data_root).map_err(|_| {
        provisioning_error(
            format!("inspect {label} repair path"),
            format!(
                "Repair target {} is outside the managed store root.",
                target.display()
            ),
            "Preserve the target and inspect the application-data path before retrying repair.",
        )
    })?;
    let mut current = data_root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(provisioning_error(
                format!("inspect {label} repair path"),
                "Repair target contains a non-normal path component.",
                "Preserve the target and inspect the application-data path before retrying repair.",
            ));
        };
        current.push(name);
        if !current.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            provisioning_error(
                format!("inspect {label} repair path"),
                format!("Cannot inspect repair path {}: {error}.", current.display()),
                "Fix application-data permissions before retrying repair.",
            )
        })?;
        if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
            return Err(provisioning_error(
                format!("inspect {label} repair path"),
                format!(
                    "Repair target traverses a symbolic link/reparse point at {}.",
                    current.display()
                ),
                "Preserve the path and resolve the link/junction explicitly; CoffeePOS will not mutate through redirected repair paths.",
            ));
        }
    }
    Ok(())
}

fn ensure_repair_tree_safe(
    data_root: &Path,
    root: &Path,
    label: &str,
) -> Result<(), RuntimeErrorInfo> {
    ensure_repair_path_safe(data_root, root, label)?;
    if !root.exists() || !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| {
        provisioning_error(
            format!("inspect {label} repair tree"),
            format!("Cannot enumerate managed tree {}: {error}.", root.display()),
            "Fix application-data permissions before retrying repair.",
        )
    })? {
        let entry = entry.map_err(|error| {
            provisioning_error(
                format!("inspect {label} repair tree"),
                format!("Cannot enumerate managed tree entry: {error}."),
                "Fix application-data permissions before retrying repair.",
            )
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            provisioning_error(
                format!("inspect {label} repair tree"),
                format!("Cannot inspect managed path {}: {error}.", path.display()),
                "Fix application-data permissions before retrying repair.",
            )
        })?;
        if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
            return Err(provisioning_error(
                format!("inspect {label} repair tree"),
                format!(
                    "Managed repair tree contains a symbolic link/reparse point at {}.",
                    path.display()
                ),
                "Preserve the tree and resolve the redirected path explicitly; CoffeePOS will not copy or replace through links/junctions.",
            ));
        }
        if metadata.is_dir() {
            ensure_repair_tree_safe(data_root, &path, label)?;
        }
    }
    Ok(())
}

fn protected_secret_ready(path: &Path) -> bool {
    path.is_file()
        && secret::load(path)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
}

fn protected_machine_token_ready(path: &Path) -> bool {
    path.is_file()
        && secret::load(path)
            .map(|token| {
                token.len() == 64
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .unwrap_or(false)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn repair_item(
    id: &str,
    component: &str,
    target: &str,
    classification: RepairClassification,
    action: &str,
    reason: &str,
    impact: &str,
    requires_runtime_stop: bool,
) -> RepairItem {
    let input_kind = (id == "wordpress_admin_password"
        && classification == RepairClassification::RequiresInput)
        .then(|| "admin_password".to_string());
    RepairItem {
        id: id.into(),
        component: component.into(),
        target: target.into(),
        classification,
        action: action.into(),
        reason: reason.into(),
        impact: impact.into(),
        requires_runtime_stop,
        input_kind,
    }
}

fn repair_plan_id(
    data_root: &Path,
    store_state: &ProvisioningState,
    items: &[RepairItem],
    wordpress_baseline: &Path,
    woocommerce_baseline: &Path,
    coffeepos_baseline: &Path,
) -> String {
    let mut hash = 14_695_981_039_346_656_037_u64;
    repair_hash_update(&mut hash, format!("{store_state:?}").as_bytes());
    for item in items {
        repair_hash_update(&mut hash, item.id.as_bytes());
        repair_hash_update(&mut hash, format!("{:?}", item.classification).as_bytes());
        repair_hash_update(&mut hash, item.reason.as_bytes());
    }
    for relative in [
        "config/provisioning.json",
        REPAIR_JOURNAL,
        "site/wp-config.php",
        "config/wordpress-router.php",
        "site/wp-content/mu-plugins/coffeepos-desktop-runtime.php",
        "site/wp-content/plugins/woocommerce/.coffeepos-managed.json",
        "site/wp-content/plugins/coffeepos/.coffeepos-managed.json",
        DATABASE_RUNTIME_SECRET,
        DATABASE_WORDPRESS_SECRET,
        WORDPRESS_ADMIN_SECRET,
        REPAIR_ADMIN_PENDING_SECRET,
        MACHINE_TOKEN_SECRET,
        MACHINE_TOKEN_PENDING_SECRET,
    ] {
        let path = data_root.join(relative);
        repair_hash_update(&mut hash, relative.as_bytes());
        if let Err(error) = ensure_repair_path_safe(data_root, &path, "repair-plan evidence") {
            repair_hash_update(&mut hash, b"unsafe-path:");
            repair_hash_update(&mut hash, error.message.as_bytes());
            continue;
        }
        match fs::read(&path) {
            Ok(bytes) => repair_hash_update(&mut hash, &bytes),
            Err(error) => {
                repair_hash_update(&mut hash, format!("missing:{:?}", error.kind()).as_bytes())
            }
        }
    }
    if items.iter().any(|item| item.id == "wordpress_core") {
        hash_baseline_target_evidence(
            &mut hash,
            data_root,
            wordpress_baseline,
            wordpress_baseline,
            &data_root.join("site"),
            Some("wp-content"),
        );
    }
    if items.iter().any(|item| item.id == "woocommerce_plugin") {
        hash_baseline_target_evidence(
            &mut hash,
            data_root,
            woocommerce_baseline,
            woocommerce_baseline,
            &data_root.join("site/wp-content/plugins/woocommerce"),
            Some(MANAGED_PLUGIN_OWNERSHIP_FILE),
        );
    }
    if items.iter().any(|item| item.id == "coffeepos_plugin") {
        hash_baseline_target_evidence(
            &mut hash,
            data_root,
            coffeepos_baseline,
            coffeepos_baseline,
            &data_root.join("site/wp-content/plugins/coffeepos"),
            Some(MANAGED_PLUGIN_OWNERSHIP_FILE),
        );
    }
    format!("repair-{hash:016x}")
}

fn repair_recovery_plan_id(data_root: &Path, journal: &RepairJournal) -> String {
    let mut hash = 14_695_981_039_346_656_037_u64;
    if let Ok(bytes) = serde_json::to_vec(journal) {
        repair_hash_update(&mut hash, &bytes);
    }
    for relative in [
        "site/wp-content/plugins/woocommerce",
        WOOCOMMERCE_REPAIR_STAGING,
        WOOCOMMERCE_REPAIR_BACKUP,
        "site/wp-content/plugins/coffeepos",
        COFFEEPOS_REPAIR_STAGING,
        COFFEEPOS_REPAIR_BACKUP,
        REPAIR_ADMIN_PENDING_SECRET,
        WORDPRESS_ADMIN_SECRET,
        MACHINE_TOKEN_PENDING_SECRET,
        MACHINE_TOKEN_SECRET,
    ] {
        hash_repair_path_evidence(&mut hash, data_root, &data_root.join(relative), relative);
    }
    format!("repair-recovery-{hash:016x}")
}

fn hash_repair_path_evidence(hash: &mut u64, data_root: &Path, path: &Path, label: &str) {
    repair_hash_update(hash, label.as_bytes());
    if let Err(error) = ensure_repair_path_safe(data_root, path, "repair recovery evidence") {
        repair_hash_update(hash, b"unsafe:");
        repair_hash_update(hash, error.message.as_bytes());
        return;
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            repair_hash_update(hash, format!("missing:{:?}", error.kind()).as_bytes());
            return;
        }
    };
    if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
        repair_hash_update(hash, b"reparse");
        return;
    }
    if metadata.is_file() {
        repair_hash_update(hash, b"file:");
        match fs::read(path) {
            Ok(bytes) => repair_hash_update(hash, &bytes),
            Err(error) => {
                repair_hash_update(hash, format!("read-error:{:?}", error.kind()).as_bytes())
            }
        }
        return;
    }
    if !metadata.is_dir() {
        repair_hash_update(hash, b"other");
        return;
    }
    repair_hash_update(hash, b"dir");
    let mut entries = match fs::read_dir(path) {
        Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(error) => {
            repair_hash_update(hash, format!("dir-error:{:?}", error.kind()).as_bytes());
            return;
        }
    };
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let child = entry.path();
        let child_label = format!("{label}/{}", entry.file_name().to_string_lossy());
        hash_repair_path_evidence(hash, data_root, &child, &child_label);
    }
}

fn repair_hash_update(hash: &mut u64, bytes: &[u8]) {
    const FNV_PRIME: u64 = 1_099_511_628_211;
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn hash_baseline_target_evidence(
    hash: &mut u64,
    data_root: &Path,
    baseline_root: &Path,
    current_baseline: &Path,
    destination_root: &Path,
    skip_top: Option<&str>,
) {
    let mut entries = match fs::read_dir(current_baseline) {
        Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(error) => {
            repair_hash_update(
                hash,
                format!("baseline-read-error:{:?}", error.kind()).as_bytes(),
            );
            return;
        }
    };
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source = entry.path();
        let relative = match source.strip_prefix(baseline_root) {
            Ok(relative) => relative,
            Err(_) => {
                repair_hash_update(hash, b"baseline-prefix-error");
                continue;
            }
        };
        if should_skip_baseline_path(relative, skip_top) {
            continue;
        }
        repair_hash_update(hash, b"path:");
        repair_hash_update(hash, relative.to_string_lossy().as_bytes());

        let source_kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) => {
                repair_hash_update(
                    hash,
                    format!("baseline-type-error:{:?}", error.kind()).as_bytes(),
                );
                continue;
            }
        };
        if source_kind.is_dir() {
            repair_hash_update(hash, b"baseline-dir");
        } else if source_kind.is_file() {
            repair_hash_update(hash, b"baseline-file:");
            match fs::read(&source) {
                Ok(bytes) => repair_hash_update(hash, &bytes),
                Err(error) => repair_hash_update(
                    hash,
                    format!("baseline-file-error:{:?}", error.kind()).as_bytes(),
                ),
            }
        } else {
            repair_hash_update(hash, b"baseline-unsupported");
        }

        let target = destination_root.join(relative);
        if let Err(error) = ensure_repair_path_safe(data_root, &target, "repair-plan target") {
            repair_hash_update(hash, b"target-unsafe:");
            repair_hash_update(hash, error.message.as_bytes());
        } else {
            match fs::symlink_metadata(&target) {
                Ok(metadata)
                    if metadata.file_type().is_symlink()
                        || metadata_is_reparse_point(&metadata) =>
                {
                    repair_hash_update(hash, b"target-reparse");
                }
                Ok(metadata) if metadata.is_dir() => {
                    repair_hash_update(hash, b"target-dir");
                }
                Ok(metadata) if metadata.is_file() => {
                    repair_hash_update(hash, b"target-file:");
                    match fs::read(&target) {
                        Ok(bytes) => repair_hash_update(hash, &bytes),
                        Err(error) => repair_hash_update(
                            hash,
                            format!("target-file-error:{:?}", error.kind()).as_bytes(),
                        ),
                    }
                }
                Ok(_) => repair_hash_update(hash, b"target-other"),
                Err(error) => repair_hash_update(
                    hash,
                    format!("target-missing:{:?}", error.kind()).as_bytes(),
                ),
            }
        }

        if source_kind.is_dir() {
            hash_baseline_target_evidence(
                hash,
                data_root,
                baseline_root,
                &source,
                destination_root,
                skip_top,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_managed_file(
    data_root: &Path,
    items: &mut Vec<RepairItem>,
    id: &str,
    component: &str,
    label: &str,
    path: &Path,
    marker: &str,
    expected: &[u8],
) {
    if let Err(error) = ensure_repair_path_safe(data_root, path, label) {
        items.push(repair_item(
            id,
            component,
            label,
            RepairClassification::Blocked,
            "Preserve the redirected managed-file path",
            &error.message,
            &error.recovery,
            true,
        ));
        return;
    }
    if !path.exists() {
        items.push(repair_item(
            id,
            component,
            label,
            RepairClassification::Repairable,
            &format!("Restore the managed {label}"),
            &format!("The CoffeePOS-managed {label} is missing."),
            "The deterministic managed file is recreated atomically from the pinned Desktop baseline.",
            true,
        ));
        return;
    }
    if !path.is_file() {
        items.push(repair_item(
            id,
            component,
            label,
            RepairClassification::Blocked,
            "Preserve the conflicting path",
            &format!("The {label} path exists but is not a regular file."),
            "CoffeePOS will not replace an unexpected filesystem object.",
            true,
        ));
        return;
    }
    match fs::read(path) {
        Ok(bytes) => {
            let marked = String::from_utf8_lossy(&bytes).contains(marker);
            if !marked {
                items.push(repair_item(
                    id,
                    component,
                    label,
                    RepairClassification::Blocked,
                    "Preserve the unmanaged file",
                    &format!("The existing {label} is not marked as CoffeePOS-managed."),
                    "CoffeePOS will not adopt or overwrite an unmanaged file.",
                    true,
                ));
            } else if bytes != expected {
                items.push(repair_item(
                    id,
                    component,
                    label,
                    RepairClassification::Repairable,
                    &format!("Restore the canonical managed {label}"),
                    &format!("The {label} is proven managed but differs from the deterministic baseline."),
                    "The file is replaced atomically; unrelated store data is untouched.",
                    true,
                ));
            }
        }
        Err(error) => items.push(repair_item(
            id,
            component,
            label,
            RepairClassification::Blocked,
            "Fix filesystem access before repair",
            &format!("Cannot read {label}: {error}."),
            "No file is changed while its current contents cannot be inspected.",
            true,
        )),
    }
}

fn should_skip_baseline_path(relative: &Path, skip_top: Option<&str>) -> bool {
    let Some(skip_top) = skip_top else {
        return false;
    };
    relative
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        == Some(skip_top)
}

fn baseline_tree_differs(
    data_root: &Path,
    baseline_root: &Path,
    destination_root: &Path,
    skip_top: Option<&str>,
) -> Result<bool, RuntimeErrorInfo> {
    fn walk(
        data_root: &Path,
        baseline_root: &Path,
        current: &Path,
        destination_root: &Path,
        skip_top: Option<&str>,
    ) -> Result<bool, RuntimeErrorInfo> {
        let entries = fs::read_dir(current).map_err(|error| {
            provisioning_error(
                "inspect repair baseline",
                format!(
                    "Cannot read pinned baseline directory {}: {error}.",
                    current.display()
                ),
                "Restage the verified development artifacts and retry repair.",
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                provisioning_error(
                    "inspect repair baseline",
                    format!("Cannot enumerate pinned baseline: {error}."),
                    "Restage the verified development artifacts and retry repair.",
                )
            })?;
            let source = entry.path();
            let relative = source.strip_prefix(baseline_root).map_err(|_| {
                provisioning_error(
                    "inspect repair baseline",
                    "Pinned repair baseline escaped its resolved artifact root.",
                    "Restage the verified development artifacts before retrying repair.",
                )
            })?;
            if should_skip_baseline_path(relative, skip_top) {
                continue;
            }
            let target = destination_root.join(relative);
            ensure_repair_path_safe(data_root, &target, "repair target")?;
            let kind = entry.file_type().map_err(|error| {
                provisioning_error(
                    "inspect repair baseline",
                    format!("Cannot inspect pinned baseline entry: {error}."),
                    "Restage the verified development artifacts and retry repair.",
                )
            })?;
            if kind.is_dir() {
                if target.exists() && !target.is_dir() {
                    return Ok(true);
                }
                if walk(
                    data_root,
                    baseline_root,
                    &source,
                    destination_root,
                    skip_top,
                )? {
                    return Ok(true);
                }
            } else if kind.is_file() {
                if !target.is_file() {
                    return Ok(true);
                }
                let source_bytes = fs::read(&source).map_err(|error| {
                    provisioning_error(
                        "inspect repair baseline",
                        format!(
                            "Cannot read pinned baseline file {}: {error}.",
                            source.display()
                        ),
                        "Restage the verified development artifacts and retry repair.",
                    )
                })?;
                let target_bytes = fs::read(&target).map_err(|error| {
                    provisioning_error(
                        "inspect repair target",
                        format!(
                            "Cannot read managed target file {}: {error}.",
                            target.display()
                        ),
                        "Fix application-data permissions and retry repair.",
                    )
                })?;
                if source_bytes != target_bytes {
                    return Ok(true);
                }
            } else {
                return Err(provisioning_error(
                    "inspect repair baseline",
                    format!(
                        "Pinned baseline contains unsupported entry {}.",
                        source.display()
                    ),
                    "Restage the verified artifact; repair only accepts regular files/directories.",
                ));
            }
        }
        Ok(false)
    }
    walk(
        data_root,
        baseline_root,
        baseline_root,
        destination_root,
        skip_top,
    )
}

fn overlay_baseline_tree(
    data_root: &Path,
    baseline_root: &Path,
    destination_root: &Path,
    skip_top: Option<&str>,
    label: &str,
) -> Result<(), RuntimeErrorInfo> {
    fn walk(
        data_root: &Path,
        baseline_root: &Path,
        current: &Path,
        destination_root: &Path,
        skip_top: Option<&str>,
        label: &str,
    ) -> Result<(), RuntimeErrorInfo> {
        for entry in fs::read_dir(current).map_err(|error| {
            provisioning_error(
                format!("repair {label}"),
                format!(
                    "Cannot read pinned baseline directory {}: {error}.",
                    current.display()
                ),
                "Restage the exact pinned artifact and retry repair.",
            )
        })? {
            let entry = entry.map_err(|error| {
                provisioning_error(
                    format!("repair {label}"),
                    format!("Cannot enumerate pinned baseline: {error}."),
                    "Restage the exact pinned artifact and retry repair.",
                )
            })?;
            let source = entry.path();
            let relative = source.strip_prefix(baseline_root).map_err(|_| {
                provisioning_error(
                    format!("repair {label}"),
                    "Pinned repair baseline escaped its artifact root.",
                    "Restage the exact pinned artifact and retry repair.",
                )
            })?;
            if should_skip_baseline_path(relative, skip_top) {
                continue;
            }
            let target = destination_root.join(relative);
            ensure_repair_path_safe(data_root, &target, label)?;
            let kind = entry.file_type().map_err(|error| {
                provisioning_error(
                    format!("repair {label}"),
                    format!("Cannot inspect pinned baseline entry: {error}."),
                    "Restage the exact pinned artifact and retry repair.",
                )
            })?;
            if kind.is_dir() {
                if target.exists() && !target.is_dir() {
                    return Err(provisioning_error(
                        format!("repair {label}"),
                        format!("Managed target {} blocks a required directory.", target.display()),
                        "Preserve the conflicting path and resolve it explicitly before retrying repair.",
                    ));
                }
                fs::create_dir_all(&target).map_err(|error| {
                    provisioning_error(
                        format!("repair {label}"),
                        format!(
                            "Cannot create managed directory {}: {error}.",
                            target.display()
                        ),
                        "Check application-data permissions and retry repair.",
                    )
                })?;
                walk(
                    data_root,
                    baseline_root,
                    &source,
                    destination_root,
                    skip_top,
                    label,
                )?;
            } else if kind.is_file() {
                if target.exists() && !target.is_file() {
                    return Err(provisioning_error(
                        format!("repair {label}"),
                        format!("Managed target {} blocks a required file.", target.display()),
                        "Preserve the conflicting path and resolve it explicitly before retrying repair.",
                    ));
                }
                let source_bytes = fs::read(&source).map_err(|error| {
                    provisioning_error(
                        format!("repair {label}"),
                        format!(
                            "Cannot read pinned baseline file {}: {error}.",
                            source.display()
                        ),
                        "Restage the exact pinned artifact and retry repair.",
                    )
                })?;
                let needs_write = fs::read(&target)
                    .map(|bytes| bytes != source_bytes)
                    .unwrap_or(true);
                if needs_write {
                    atomic_write(&target, &source_bytes, label)?;
                }
            } else {
                return Err(provisioning_error(
                    format!("repair {label}"),
                    format!(
                        "Pinned baseline contains unsupported entry {}.",
                        source.display()
                    ),
                    "Restage the verified artifact before retrying repair.",
                ));
            }
        }
        Ok(())
    }
    fs::create_dir_all(destination_root).map_err(|error| {
        provisioning_error(
            format!("repair {label}"),
            format!(
                "Cannot create managed destination {}: {error}.",
                destination_root.display()
            ),
            "Check application-data permissions and retry repair.",
        )
    })?;
    ensure_repair_path_safe(data_root, destination_root, label)?;
    walk(
        data_root,
        baseline_root,
        baseline_root,
        destination_root,
        skip_top,
        label,
    )
}

fn reset_owned_repair_dir(data_root: &Path, path: &Path) -> Result<(), RuntimeErrorInfo> {
    ensure_repair_path_safe(data_root, path, "repair staging/backup")?;
    let parent = path.parent().ok_or_else(|| {
        provisioning_error(
            "reset repair staging",
            "Repair staging path has no parent directory.",
            "Restart CoffeePOS Desktop and inspect the application-data path.",
        )
    })?;
    let name = path.file_name().and_then(|value| value.to_str());
    if parent != data_root
        || !matches!(
            name,
            Some(
                WOOCOMMERCE_REPAIR_STAGING
                    | WOOCOMMERCE_REPAIR_BACKUP
                    | COFFEEPOS_REPAIR_STAGING
                    | COFFEEPOS_REPAIR_BACKUP
            )
        )
    {
        return Err(provisioning_error(
            "reset repair staging",
            "Refusing to remove an unexpected repair path.",
            "Restart CoffeePOS Desktop and inspect the application-data path before retrying.",
        ));
    }
    if path.exists() {
        fs::remove_dir_all(path).map_err(|error| {
            provisioning_error(
                "reset repair staging",
                format!(
                    "Cannot remove owned repair directory {}: {error}.",
                    path.display()
                ),
                "Close processes using the repair directory and retry.",
            )
        })?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plugin_tree_is_exact(
    data_root: &Path,
    destination: &Path,
    slug: &str,
    version: &str,
    archive_sha256: &str,
    ownership_schema: u32,
    baseline_root: &Path,
) -> bool {
    if !destination.is_dir() {
        return false;
    }
    let ownership = fs::read(destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ManagedPluginOwnership>(&bytes).ok());
    let Some(ownership) = ownership else {
        return false;
    };
    ownership.schema_version == ownership_schema
        && ownership.plugin == slug
        && ownership.version == version
        && ownership
            .archive_sha256
            .eq_ignore_ascii_case(archive_sha256)
        && baseline_tree_differs(
            data_root,
            baseline_root,
            destination,
            Some(MANAGED_PLUGIN_OWNERSHIP_FILE),
        )
        .map(|differs| !differs)
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn repair_managed_plugin_tree(
    data_root: &Path,
    slug: &str,
    version: &str,
    archive_sha256: &str,
    ownership_schema: u32,
    baseline_root: &Path,
    staging_name: &str,
    backup_name: &str,
    label: &str,
) -> Result<(), RuntimeErrorInfo> {
    let destination = data_root.join("site/wp-content/plugins").join(slug);
    let staging = data_root.join(staging_name);
    let backup = data_root.join(backup_name);
    ensure_repair_path_safe(data_root, &destination, label)?;
    ensure_repair_path_safe(data_root, &staging, &format!("{label} repair staging"))?;
    ensure_repair_path_safe(data_root, &backup, &format!("{label} repair backup"))?;
    if destination.exists() {
        ensure_repair_tree_safe(data_root, &destination, label)?;
    }

    if backup.exists() {
        if !backup.is_dir() {
            return Err(provisioning_error(
                format!("recover {label} repair"),
                "Owned repair backup path exists but is not a directory.",
                "Preserve the unexpected path and resolve it explicitly before retrying repair.",
            ));
        }
        if backup.join(REPAIR_ORIGINAL_MISSING_MARKER).is_file() {
            return Err(provisioning_error(
                format!("recover {label} repair"),
                "An unfinished repair marker says the managed plugin destination was originally missing.",
                "Preserve the repair journal, live destination, and owned repair backup marker for explicit transaction recovery before starting another repair.",
            ));
        }
        if !destination.exists() {
            fs::rename(&backup, &destination).map_err(|error| {
                provisioning_error(
                    format!("recover {label} repair"),
                    format!("Cannot restore the pre-repair managed plugin backup: {error}."),
                    "Close processes using plugin files and retry; the backup is preserved.",
                )
            })?;
        } else if plugin_tree_is_exact(
            data_root,
            &destination,
            slug,
            version,
            archive_sha256,
            ownership_schema,
            baseline_root,
        ) {
            reset_owned_repair_dir(data_root, &backup)?;
        } else {
            return Err(provisioning_error(
                format!("recover {label} repair"),
                "Both an owned repair backup and a non-verified destination exist.",
                "Preserve both plugin trees. CoffeePOS will not guess which tree is authoritative.",
            ));
        }
    }

    let destination_was_missing = !destination.exists();
    if destination_was_missing {
        reset_owned_repair_dir(data_root, &backup)?;
        fs::create_dir_all(&backup).map_err(|error| {
            provisioning_error(
                format!("prepare {label} repair rollback"),
                format!("Cannot create owned repair backup marker directory: {error}."),
                "Check application-data permissions and retry before activating repaired plugin files.",
            )
        })?;
        atomic_write(
            &backup.join(REPAIR_ORIGINAL_MISSING_MARKER),
            b"original managed plugin destination was missing\n",
            &format!("{label} original-missing repair marker"),
        )?;
    }

    reset_owned_repair_dir(data_root, &staging)?;
    if destination.exists() {
        if !destination.is_dir() {
            return Err(provisioning_error(
                format!("repair {label}"),
                "Managed plugin destination exists but is not a directory.",
                "Preserve the conflicting path and resolve it explicitly before retrying repair.",
            ));
        }
        let ownership =
            fs::read(destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE)).map_err(|error| {
                provisioning_error(
                    format!("repair {label}"),
                    format!("Cannot read managed plugin ownership metadata: {error}."),
                    "Preserve the plugin directory; repair requires proven CoffeePOS ownership.",
                )
            })?;
        let ownership: ManagedPluginOwnership = serde_json::from_slice(&ownership).map_err(|error| {
            provisioning_error(
                format!("repair {label}"),
                format!("Managed plugin ownership metadata is invalid: {error}."),
                "Preserve the plugin directory; repair will not rewrite incompatible ownership metadata.",
            )
        })?;
        if ownership.schema_version != ownership_schema
            || ownership.plugin != slug
            || ownership.version != version
            || !ownership
                .archive_sha256
                .eq_ignore_ascii_case(archive_sha256)
        {
            return Err(provisioning_error(
                format!("repair {label}"),
                "Managed plugin ownership metadata does not match the pinned same-version baseline.",
                "Use an explicit adoption/upgrade flow instead of overwriting this plugin.",
            ));
        }
        copy_tree(
            &destination,
            &staging,
            &format!("{label} repair staging"),
            "Close processes using plugin files and retry repair.",
        )?;
    } else {
        fs::create_dir_all(&staging).map_err(|error| {
            provisioning_error(
                format!("repair {label}"),
                format!("Cannot create repair staging directory: {error}."),
                "Check application-data permissions and retry repair.",
            )
        })?;
    }

    overlay_baseline_tree(
        data_root,
        baseline_root,
        &staging,
        Some(MANAGED_PLUGIN_OWNERSHIP_FILE),
        label,
    )?;
    let ownership = ManagedPluginOwnership {
        schema_version: ownership_schema,
        plugin: slug.into(),
        version: version.into(),
        archive_sha256: archive_sha256.into(),
    };
    let mut ownership_bytes = serde_json::to_vec_pretty(&ownership).map_err(|error| {
        provisioning_error(
            format!("repair {label}"),
            format!("Cannot serialize managed plugin ownership metadata: {error}."),
            "Retry after checking application-data storage.",
        )
    })?;
    ownership_bytes.push(b'\n');
    atomic_write(
        &staging.join(MANAGED_PLUGIN_OWNERSHIP_FILE),
        &ownership_bytes,
        &format!("{label} ownership metadata"),
    )?;
    if baseline_tree_differs(
        data_root,
        baseline_root,
        &staging,
        Some(MANAGED_PLUGIN_OWNERSHIP_FILE),
    )? {
        return Err(provisioning_error(
            format!("repair {label}"),
            "Repaired staging tree still differs from the pinned baseline.",
            "Preserve the live plugin and restage the verified artifact before retrying repair.",
        ));
    }

    if destination.exists() {
        reset_owned_repair_dir(data_root, &backup)?;
        fs::rename(&destination, &backup).map_err(|error| {
            provisioning_error(
                format!("repair {label}"),
                format!("Cannot move the managed plugin into the repair backup: {error}."),
                "Close processes using plugin files and retry; the live plugin has not been overwritten.",
            )
        })?;
    }
    if let Err(error) = fs::rename(&staging, &destination) {
        let activation_error = provisioning_error(
            format!("repair {label}"),
            format!("Cannot atomically activate the repaired plugin tree: {error}."),
            "Restore the proven pre-repair state before retrying after checking filesystem permissions.",
        );
        if let Err(rollback_error) =
            rollback_managed_plugin_repair(data_root, slug, staging_name, backup_name, label)
        {
            return Err(provisioning_error(
                format!("rollback {label} repair"),
                format!(
                    "{} Rollback also failed: {}",
                    activation_error.message, rollback_error.message
                ),
                "Preserve config/repair.json plus the repair staging/backup evidence. Do not start another repair until the plugin destination is reconciled explicitly.",
            ));
        }
        return Err(activation_error);
    }
    if !plugin_tree_is_exact(
        data_root,
        &destination,
        slug,
        version,
        archive_sha256,
        ownership_schema,
        baseline_root,
    ) {
        let verification_error = provisioning_error(
            format!("verify {label} repair"),
            "The activated repaired plugin tree did not verify against the pinned baseline.",
            "The pre-repair state must be restored before retrying after restaging artifacts.",
        );
        if let Err(rollback_error) =
            rollback_managed_plugin_repair(data_root, slug, staging_name, backup_name, label)
        {
            return Err(provisioning_error(
                format!("rollback {label} repair"),
                format!(
                    "{} Rollback also failed: {}",
                    verification_error.message, rollback_error.message
                ),
                "Preserve the repair journal plus staging/backup evidence and reconcile the plugin trees explicitly.",
            ));
        }
        return Err(verification_error);
    }
    Ok(())
}

fn commit_managed_plugin_repair(
    data_root: &Path,
    backup_name: &str,
    label: &str,
) -> Result<(), RuntimeErrorInfo> {
    let backup = data_root.join(backup_name);
    if !backup.exists() {
        return Ok(());
    }
    reset_owned_repair_dir(data_root, &backup).map_err(|error| {
        provisioning_error(
            format!("commit {label} repair"),
            format!(
                "The repaired {label} plugin verified successfully, but its pre-repair backup could not be removed: {}",
                error.message
            ),
            "Keep the verified repaired plugin and retry repair cleanup. The pre-repair backup is preserved.",
        )
    })
}

fn rollback_managed_plugin_repair(
    data_root: &Path,
    slug: &str,
    staging_name: &str,
    backup_name: &str,
    label: &str,
) -> Result<(), RuntimeErrorInfo> {
    let destination = data_root.join("site/wp-content/plugins").join(slug);
    let failed = data_root.join(staging_name);
    let backup = data_root.join(backup_name);
    if !backup.exists() {
        return Err(provisioning_error(
            format!("rollback {label} repair"),
            "The owned pre-repair backup state is missing, so CoffeePOS cannot prove what should be restored.",
            "Preserve the live plugin and repair journal; do not infer that the original destination was absent.",
        ));
    }
    ensure_repair_path_safe(data_root, &destination, label)?;
    ensure_repair_path_safe(data_root, &failed, &format!("{label} failed repair"))?;
    ensure_repair_path_safe(data_root, &backup, &format!("{label} repair backup"))?;
    reset_owned_repair_dir(data_root, &failed)?;
    let original_was_missing = backup.join(REPAIR_ORIGINAL_MISSING_MARKER).is_file();
    if destination.exists() {
        fs::rename(&destination, &failed).map_err(|error| {
            provisioning_error(
                format!("rollback {label} repair"),
                format!("Cannot preserve the failed repaired {label} tree: {error}."),
                "Preserve the destination and repair backup; CoffeePOS will not overwrite either tree.",
            )
        })?;
    }
    if original_was_missing {
        reset_owned_repair_dir(data_root, &backup)?;
        return Ok(());
    }
    if let Err(error) = fs::rename(&backup, &destination) {
        if failed.exists() && !destination.exists() {
            let _ = fs::rename(&failed, &destination);
        }
        return Err(provisioning_error(
            format!("rollback {label} repair"),
            format!("Cannot restore the pre-repair managed {label} plugin: {error}."),
            "Preserve the repair backup and failed repaired tree for explicit recovery.",
        ));
    }
    Ok(())
}

fn rollback_pending_plugin_repair_trees(data_root: &Path) -> Result<(), RuntimeErrorInfo> {
    if data_root.join(WOOCOMMERCE_REPAIR_BACKUP).exists() {
        rollback_managed_plugin_repair(
            data_root,
            WOOCOMMERCE_PLUGIN_SLUG,
            WOOCOMMERCE_REPAIR_STAGING,
            WOOCOMMERCE_REPAIR_BACKUP,
            "WooCommerce",
        )?;
    }
    if data_root.join(COFFEEPOS_REPAIR_BACKUP).exists() {
        rollback_managed_plugin_repair(
            data_root,
            COFFEEPOS_PLUGIN_SLUG,
            COFFEEPOS_REPAIR_STAGING,
            COFFEEPOS_REPAIR_BACKUP,
            "CoffeePOS",
        )?;
    }
    Ok(())
}

fn commit_pending_plugin_repair_backups(data_root: &Path) -> Result<(), RuntimeErrorInfo> {
    if data_root.join(WOOCOMMERCE_REPAIR_BACKUP).exists() {
        commit_managed_plugin_repair(data_root, WOOCOMMERCE_REPAIR_BACKUP, "WooCommerce")?;
    }
    if data_root.join(COFFEEPOS_REPAIR_BACKUP).exists() {
        commit_managed_plugin_repair(data_root, COFFEEPOS_REPAIR_BACKUP, "CoffeePOS")?;
    }
    Ok(())
}

fn validate_admin_repair_password(password: &str) -> Result<(), RuntimeErrorInfo> {
    let count = password.chars().count();
    if !(12..=128).contains(&count) || password.chars().any(char::is_control) {
        return Err(provisioning_error(
            "repair administrator password",
            "Administrator password must contain 12–128 characters without control characters.",
            "Enter a replacement password that meets the same Phase 5.2 password contract.",
        ));
    }
    Ok(())
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
$verifyInitialSetup = getenv('COFFEEPOS_VERIFY_INITIAL_SETUP') === '1';

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

if ($verifyInitialSetup) {
    $createdAdmin = get_user_by('login', $adminUser);
    if (!$createdAdmin || strcasecmp((string) $createdAdmin->user_email, $adminEmail) !== 0) {
        fwrite(STDERR, "CoffeePOS initial administrator identity verification failed.\n");
        exit(8);
    }
    if (!in_array('administrator', (array) $createdAdmin->roles, true)) {
        fwrite(STDERR, "CoffeePOS initial administrator role verification failed.\n");
        exit(9);
    }
    if (!wp_check_password($adminPassword, (string) $createdAdmin->user_pass, (int) $createdAdmin->ID)) {
        fwrite(STDERR, "CoffeePOS initial administrator password verification failed.\n");
        exit(10);
    }
    $expectedBlogName = (string) sanitize_option('blogname', $storeName);
    if ((string) get_option('blogname', '') !== $expectedBlogName) {
        fwrite(STDERR, "CoffeePOS initial WordPress store name verification failed.\n");
        exit(11);
    }
}

exit(get_option('siteurl') ? 0 : 5);
"#;

const WORDPRESS_RESTORE_IDENTITY_VERIFY: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
$storeName = (string) getenv('COFFEEPOS_RESTORE_STORE_NAME');
$adminUser = trim((string) getenv('COFFEEPOS_RESTORE_ADMIN_USER'));
$adminEmail = trim((string) getenv('COFFEEPOS_RESTORE_ADMIN_EMAIL'));
if (!$siteRoot || $storeName === '' || $adminUser === '' || $adminEmail === '') {
    fwrite(STDERR, "CoffeePOS restore identity verification environment is incomplete.\n");
    exit(2);
}
$password = (string) stream_get_contents(STDIN);
if ($password === '') {
    fwrite(STDERR, "CoffeePOS restored administrator credential is empty.\n");
    exit(3);
}

require_once $siteRoot . '/wp-load.php';
$user = get_user_by('login', $adminUser);
if (!$user || !isset($user->ID) || (string) $user->user_login !== $adminUser) {
    $password = '';
    fwrite(STDERR, "CoffeePOS restored administrator identity does not exist.\n");
    exit(4);
}
if (strcasecmp((string) $user->user_email, $adminEmail) !== 0 || !user_can($user, 'manage_options')) {
    $password = '';
    fwrite(STDERR, "CoffeePOS restored administrator metadata or authority does not match the backup.\n");
    exit(5);
}
if (!wp_check_password($password, (string) $user->user_pass, (int) $user->ID)) {
    $password = '';
    fwrite(STDERR, "CoffeePOS restored administrator password does not match the restored WordPress hash.\n");
    exit(6);
}
$password = '';
$expectedBlogName = (string) sanitize_option('blogname', $storeName);
if ((string) get_option('blogname', '') !== $expectedBlogName) {
    fwrite(STDERR, "CoffeePOS restored WordPress store name does not match the backup.\n");
    exit(7);
}
if ((string) get_option('coffeepos_store_name', '') !== $storeName) {
    fwrite(STDERR, "CoffeePOS restored plugin store identity does not match the backup.\n");
    exit(8);
}
exit(0);
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
$initialStoreName = getenv('COFFEEPOS_INITIAL_STORE_NAME');
$initialAdminUser = getenv('COFFEEPOS_INITIAL_ADMIN_USER');
$applyBaseline = getenv('COFFEEPOS_APPLY_ACTIVATION_BASELINE') === '1';

if (!$siteRoot || !$expectedVersion || !$expectedWooCommerceVersion || !$initialStoreName || !$initialAdminUser) {
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
if ($applyBaseline) {
    if (!class_exists('\\CoffeePOS\\Infrastructure\\Settings\\Settings')) {
        fwrite(STDERR, "CoffeePOS settings service is unavailable after activation.\n");
        exit(12);
    }
    $initialAdmin = get_user_by('login', $initialAdminUser);
    if (!$initialAdmin) {
        fwrite(STDERR, "Initial CoffeePOS administrator is unavailable after activation.\n");
        exit(13);
    }
    wp_set_current_user((int) $initialAdmin->ID);
    \CoffeePOS\Infrastructure\Settings\Settings::update(
        \CoffeePOS\Infrastructure\Settings\Settings::OPTION_STORE_NAME,
        $initialStoreName
    );
    if (\CoffeePOS\Infrastructure\Settings\Settings::getStoreName() !== sanitize_text_field($initialStoreName)) {
        fwrite(STDERR, "CoffeePOS initial store name verification failed.\n");
        exit(14);
    }
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

const COFFEEPOS_RESTORE_MACHINE_HEALTH_BOOTSTRAP: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
if (!$siteRoot) {
    fwrite(STDERR, "CoffeePOS restore machine-health environment is incomplete.\n");
    exit(2);
}
$token = trim((string) stream_get_contents(STDIN));
if (strlen($token) !== 64 || !preg_match('/^[0-9a-f]{64}$/', $token)) {
    fwrite(STDERR, "CoffeePOS target machine credential format is invalid.\n");
    exit(3);
}
require_once $siteRoot . '/wp-load.php';
if (!defined('COFFEEPOS_VERSION') || !class_exists('\\CoffeePOS\\REST\\SystemStatusController')) {
    $token = '';
    fwrite(STDERR, "CoffeePOS machine-health controller is unavailable during restore.\n");
    exit(4);
}
$hash = hash('sha256', $token);
$token = '';
$option = \CoffeePOS\REST\SystemStatusController::OPTION_MACHINE_TOKEN_HASH;
$stored = strtolower(trim((string) get_option($option, '')));
if (!hash_equals($stored, $hash) && !update_option($option, $hash, false)) {
    $stored = strtolower(trim((string) get_option($option, '')));
    if (!hash_equals($stored, $hash)) {
        $hash = '';
        fwrite(STDERR, "CoffeePOS target machine credential hash could not be bound to the restored store.\n");
        exit(5);
    }
}
$stored = strtolower(trim((string) get_option($option, '')));
if (!hash_equals($stored, $hash)) {
    $hash = '';
    fwrite(STDERR, "CoffeePOS target machine credential verification failed after restore binding.\n");
    exit(6);
}
$hash = '';
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

const WORDPRESS_ADMIN_PASSWORD_REPAIR: &str = r#"<?php
declare(strict_types=1);

$siteRoot = getenv('COFFEEPOS_SITE_ROOT');
$username = trim((string) getenv('COFFEEPOS_ADMIN_USERNAME'));
if (!$siteRoot || $username === '') {
    fwrite(STDERR, "CoffeePOS administrator-password repair environment is incomplete.\n");
    exit(2);
}
$password = (string) stream_get_contents(STDIN);
$length = function_exists('mb_strlen') ? mb_strlen($password, 'UTF-8') : strlen($password);
if ($length < 12 || $length > 128 || preg_match('/[\x00-\x1f\x7f]/', $password)) {
    fwrite(STDERR, "Replacement administrator password does not meet the Desktop password contract.\n");
    exit(3);
}

require_once $siteRoot . '/wp-load.php';
$user = get_user_by('login', $username);
if (!$user || !isset($user->ID) || (string) $user->user_login !== $username) {
    $password = '';
    fwrite(STDERR, "The provisioned administrator account could not be identified exactly.\n");
    exit(4);
}
if (!user_can($user, 'manage_options')) {
    $password = '';
    fwrite(STDERR, "The provisioned administrator account no longer has administrator authority.\n");
    exit(5);
}

wp_set_password($password, (int) $user->ID);
clean_user_cache((int) $user->ID);
$verified = get_user_by('id', (int) $user->ID);
if (!$verified || (string) $verified->user_login !== $username || !wp_check_password($password, (string) $verified->user_pass, (int) $verified->ID)) {
    $password = '';
    fwrite(STDERR, "WordPress did not verify the replacement administrator password.\n");
    exit(6);
}
$password = '';
fwrite(STDOUT, "CoffeePOS administrator password updated and verified.\n");
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

    fn repair_test_provisioner(data_root: PathBuf) -> Provisioner {
        let artifact_root = data_root.join("test-artifacts");
        let wordpress_root = artifact_root.join("wordpress");
        let woocommerce_root = artifact_root.join("woocommerce");
        let coffeepos_root = artifact_root.join("coffeepos");
        fs::create_dir_all(&wordpress_root).unwrap();
        fs::create_dir_all(&woocommerce_root).unwrap();
        fs::create_dir_all(&coffeepos_root).unwrap();
        Provisioner {
            runtime: ResolvedRuntime {
                runtime_version: "test".into(),
                php_version: "8.4.25".into(),
                php_executable: artifact_root.join("php.exe"),
                php_cgi_executable: artifact_root.join("php-cgi.exe"),
                php_ini: artifact_root.join("php.ini"),
                web_server_version: "2.11.4".into(),
                web_server_executable: artifact_root.join("caddy.exe"),
                mariadb_version: "11.4.13".into(),
                mariadb_executable: artifact_root.join("mariadbd.exe"),
                mariadb_client_executable: artifact_root.join("mariadb.exe"),
                mariadb_dump_executable: artifact_root.join("mariadb-dump.exe"),
                mariadb_import_executable: artifact_root.join("mariadb.exe"),
                mariadb_install_db_executable: artifact_root.join("mariadb-install-db.exe"),
                mariadb_base_dir: artifact_root.join("mariadb"),
            },
            wordpress: ResolvedWordPress {
                version: "7.1".into(),
                core_root: wordpress_root,
            },
            woocommerce: ResolvedWooCommerce {
                version: "11.1.0".into(),
                plugin_root: woocommerce_root,
                archive_sha256: "woo-test-hash".into(),
            },
            coffeepos: ResolvedCoffeePos {
                version: "1.0.1".into(),
                plugin_root: coffeepos_root,
                archive_sha256: "coffeepos-test-hash".into(),
                required_wordpress_version: "7.1".into(),
                required_php_version: "8.4.25".into(),
                required_mariadb_version: "11.4.13".into(),
                required_woocommerce_version: "11.1.0".into(),
            },
            data_root,
            containment: ProcessContainment::new().unwrap(),
            admin_username: WORDPRESS_ADMIN_USER.into(),
            admin_email: WORDPRESS_ADMIN_EMAIL.into(),
            failure_after: None,
        }
    }

    fn write_repair_test_journal(
        provisioner: &Provisioner,
        stage: RepairJournalStage,
        completed_item_ids: Vec<String>,
    ) {
        fs::create_dir_all(provisioner.data_root.join("config")).unwrap();
        fs::create_dir_all(provisioner.data_root.join("site")).unwrap();
        fs::create_dir_all(provisioner.data_root.join("database")).unwrap();
        provisioner
            .persist_repair_journal(&RepairJournal {
                schema_version: REPAIR_SCHEMA_VERSION,
                plan_id: "original-repair-plan".into(),
                item_ids: vec!["woocommerce_plugin".into()],
                completed_item_ids,
                active_item_id: None,
                runtime_was_running: true,
                stage,
            })
            .unwrap();
    }

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
    fn repair_wordpress_core_overlay_preserves_wp_content_and_extra_files() {
        let temp = tempfile::tempdir().unwrap();
        let baseline = temp.path().join("baseline");
        let destination = temp.path().join("site");
        fs::create_dir_all(baseline.join("wp-includes")).unwrap();
        fs::create_dir_all(baseline.join("wp-content")).unwrap();
        fs::create_dir_all(destination.join("wp-content")).unwrap();
        fs::write(baseline.join("wp-settings.php"), b"pinned-core").unwrap();
        fs::write(baseline.join("wp-includes/version.php"), b"pinned-version").unwrap();
        fs::write(baseline.join("wp-content/index.php"), b"pinned-content").unwrap();
        fs::write(destination.join("wp-settings.php"), b"corrupt-core").unwrap();
        fs::write(destination.join("wp-content/index.php"), b"store-content").unwrap();
        fs::write(
            destination.join("wp-content/business-sentinel.txt"),
            b"keep-me",
        )
        .unwrap();
        fs::write(destination.join("unrelated-extra.txt"), b"preserve-extra").unwrap();

        overlay_baseline_tree(
            temp.path(),
            &baseline,
            &destination,
            Some("wp-content"),
            "WordPress core",
        )
        .unwrap();

        assert_eq!(
            fs::read(destination.join("wp-settings.php")).unwrap(),
            b"pinned-core"
        );
        assert_eq!(
            fs::read(destination.join("wp-includes/version.php")).unwrap(),
            b"pinned-version"
        );
        assert_eq!(
            fs::read(destination.join("wp-content/index.php")).unwrap(),
            b"store-content"
        );
        assert_eq!(
            fs::read(destination.join("wp-content/business-sentinel.txt")).unwrap(),
            b"keep-me"
        );
        assert_eq!(
            fs::read(destination.join("unrelated-extra.txt")).unwrap(),
            b"preserve-extra"
        );
    }

    #[test]
    fn repair_managed_plugin_preserves_extra_files_and_restores_pinned_baseline() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let baseline = temp.path().join("plugin-baseline");
        let destination = data_root.join("site/wp-content/plugins/test-plugin");
        fs::create_dir_all(baseline.join("assets")).unwrap();
        fs::create_dir_all(destination.join("assets")).unwrap();
        fs::write(baseline.join("plugin.php"), b"pinned-plugin").unwrap();
        fs::write(baseline.join("assets/app.js"), b"pinned-js").unwrap();
        fs::write(destination.join("plugin.php"), b"corrupt-plugin").unwrap();
        fs::write(destination.join("assets/app.js"), b"corrupt-js").unwrap();
        fs::write(destination.join("business-sentinel.txt"), b"keep-me").unwrap();
        let ownership = ManagedPluginOwnership {
            schema_version: 1,
            plugin: "test-plugin".into(),
            version: "1.0.0".into(),
            archive_sha256: "abc123".into(),
        };
        let mut ownership_bytes = serde_json::to_vec_pretty(&ownership).unwrap();
        ownership_bytes.push(b'\n');
        fs::write(
            destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE),
            ownership_bytes,
        )
        .unwrap();

        repair_managed_plugin_tree(
            &data_root,
            "test-plugin",
            "1.0.0",
            "abc123",
            1,
            &baseline,
            COFFEEPOS_REPAIR_STAGING,
            COFFEEPOS_REPAIR_BACKUP,
            "test plugin",
        )
        .unwrap();

        assert_eq!(
            fs::read(destination.join("plugin.php")).unwrap(),
            b"pinned-plugin"
        );
        assert_eq!(
            fs::read(destination.join("assets/app.js")).unwrap(),
            b"pinned-js"
        );
        assert_eq!(
            fs::read(destination.join("business-sentinel.txt")).unwrap(),
            b"keep-me"
        );
        assert!(!data_root.join(COFFEEPOS_REPAIR_STAGING).exists());
        assert!(data_root.join(COFFEEPOS_REPAIR_BACKUP).exists());
        commit_managed_plugin_repair(&data_root, COFFEEPOS_REPAIR_BACKUP, "test plugin").unwrap();
        assert!(!data_root.join(COFFEEPOS_REPAIR_BACKUP).exists());
    }

    #[test]
    fn failed_plugin_verifier_can_restore_pre_repair_tree_offline() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let baseline = temp.path().join("plugin-baseline");
        let destination = data_root.join("site/wp-content/plugins/test-plugin");
        fs::create_dir_all(&baseline).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(baseline.join("plugin.php"), b"pinned-plugin").unwrap();
        fs::write(destination.join("plugin.php"), b"pre-repair-plugin").unwrap();
        fs::write(destination.join("business-sentinel.txt"), b"keep-me").unwrap();
        let ownership = ManagedPluginOwnership {
            schema_version: 1,
            plugin: "test-plugin".into(),
            version: "1.0.0".into(),
            archive_sha256: "abc123".into(),
        };
        let mut ownership_bytes = serde_json::to_vec_pretty(&ownership).unwrap();
        ownership_bytes.push(b'\n');
        fs::write(
            destination.join(MANAGED_PLUGIN_OWNERSHIP_FILE),
            ownership_bytes,
        )
        .unwrap();

        repair_managed_plugin_tree(
            &data_root,
            "test-plugin",
            "1.0.0",
            "abc123",
            1,
            &baseline,
            COFFEEPOS_REPAIR_STAGING,
            COFFEEPOS_REPAIR_BACKUP,
            "test plugin",
        )
        .unwrap();
        assert_eq!(
            fs::read(destination.join("plugin.php")).unwrap(),
            b"pinned-plugin"
        );

        rollback_managed_plugin_repair(
            &data_root,
            "test-plugin",
            COFFEEPOS_REPAIR_STAGING,
            COFFEEPOS_REPAIR_BACKUP,
            "test plugin",
        )
        .unwrap();

        assert_eq!(
            fs::read(destination.join("plugin.php")).unwrap(),
            b"pre-repair-plugin"
        );
        assert_eq!(
            fs::read(destination.join("business-sentinel.txt")).unwrap(),
            b"keep-me"
        );
        assert_eq!(
            fs::read(data_root.join(COFFEEPOS_REPAIR_STAGING).join("plugin.php")).unwrap(),
            b"pinned-plugin"
        );
        assert!(!data_root.join(COFFEEPOS_REPAIR_BACKUP).exists());
    }

    #[test]
    fn failed_plugin_verifier_restores_originally_missing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let baseline = temp.path().join("plugin-baseline");
        let destination = data_root.join("site/wp-content/plugins/test-plugin");
        fs::create_dir_all(&baseline).unwrap();
        fs::create_dir_all(data_root.join("site/wp-content/plugins")).unwrap();
        fs::write(baseline.join("plugin.php"), b"pinned-plugin").unwrap();

        repair_managed_plugin_tree(
            &data_root,
            "test-plugin",
            "1.0.0",
            "abc123",
            1,
            &baseline,
            COFFEEPOS_REPAIR_STAGING,
            COFFEEPOS_REPAIR_BACKUP,
            "test plugin",
        )
        .unwrap();

        assert_eq!(
            fs::read(destination.join("plugin.php")).unwrap(),
            b"pinned-plugin"
        );
        assert!(data_root
            .join(COFFEEPOS_REPAIR_BACKUP)
            .join(REPAIR_ORIGINAL_MISSING_MARKER)
            .is_file());

        rollback_managed_plugin_repair(
            &data_root,
            "test-plugin",
            COFFEEPOS_REPAIR_STAGING,
            COFFEEPOS_REPAIR_BACKUP,
            "test plugin",
        )
        .unwrap();

        assert!(!destination.exists());
        assert_eq!(
            fs::read(data_root.join(COFFEEPOS_REPAIR_STAGING).join("plugin.php")).unwrap(),
            b"pinned-plugin"
        );
        assert!(!data_root.join(COFFEEPOS_REPAIR_BACKUP).exists());
    }

    #[test]
    fn pending_plugin_rollback_restores_earlier_swap_before_runtime_restart() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let destination = data_root.join("site/wp-content/plugins/woocommerce");
        let backup = data_root.join(WOOCOMMERCE_REPAIR_BACKUP);
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(destination.join("woocommerce.php"), b"repaired-unverified").unwrap();
        fs::write(backup.join("woocommerce.php"), b"pre-repair").unwrap();

        rollback_pending_plugin_repair_trees(&data_root).unwrap();

        assert_eq!(
            fs::read(destination.join("woocommerce.php")).unwrap(),
            b"pre-repair"
        );
        assert_eq!(
            fs::read(
                data_root
                    .join(WOOCOMMERCE_REPAIR_STAGING)
                    .join("woocommerce.php")
            )
            .unwrap(),
            b"repaired-unverified"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn interrupted_swapped_repair_rolls_back_owned_plugin_and_replans() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let provisioner = repair_test_provisioner(data_root.clone());
        let destination = data_root.join("site/wp-content/plugins/woocommerce");
        let backup = data_root.join(WOOCOMMERCE_REPAIR_BACKUP);
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(destination.join("woocommerce.php"), b"repaired-unverified").unwrap();
        fs::write(backup.join("woocommerce.php"), b"pre-repair").unwrap();
        write_repair_test_journal(
            &provisioner,
            RepairJournalStage::Swapped,
            vec!["woocommerce_plugin".into()],
        );

        let plan = provisioner.repair_plan(false, false);
        assert!(plan.can_apply);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].id, "repair_transaction");
        assert_eq!(
            plan.items[0].classification,
            RepairClassification::Repairable
        );

        provisioner
            .recover_interrupted_repair(&plan.plan_id)
            .unwrap();

        assert_eq!(
            fs::read(destination.join("woocommerce.php")).unwrap(),
            b"pre-repair"
        );
        assert_eq!(
            fs::read(
                data_root
                    .join(WOOCOMMERCE_REPAIR_STAGING)
                    .join("woocommerce.php")
            )
            .unwrap(),
            b"repaired-unverified"
        );
        assert!(!data_root.join(REPAIR_JOURNAL).exists());
    }

    #[test]
    fn interrupted_verified_repair_keeps_live_plugin_and_commits_backup_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let provisioner = repair_test_provisioner(data_root.clone());
        let destination = data_root.join("site/wp-content/plugins/woocommerce");
        let backup = data_root.join(WOOCOMMERCE_REPAIR_BACKUP);
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(destination.join("woocommerce.php"), b"verified-repaired").unwrap();
        fs::write(backup.join("woocommerce.php"), b"pre-repair").unwrap();
        write_repair_test_journal(
            &provisioner,
            RepairJournalStage::Verified,
            vec!["woocommerce_plugin".into()],
        );

        let plan = provisioner.repair_plan(false, false);
        assert!(plan.can_apply);
        provisioner
            .recover_interrupted_repair(&plan.plan_id)
            .unwrap();

        assert_eq!(
            fs::read(destination.join("woocommerce.php")).unwrap(),
            b"verified-repaired"
        );
        assert!(!backup.exists());
        assert!(!data_root.join(REPAIR_JOURNAL).exists());
    }

    #[test]
    fn interrupted_swapped_repair_blocks_when_completed_plugin_backup_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let provisioner = repair_test_provisioner(data_root.clone());
        let destination = data_root.join("site/wp-content/plugins/woocommerce");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("woocommerce.php"), b"unknown-live-state").unwrap();
        write_repair_test_journal(
            &provisioner,
            RepairJournalStage::Swapped,
            vec!["woocommerce_plugin".into()],
        );

        let plan = provisioner.repair_plan(false, false);
        assert!(!plan.can_apply);
        assert_eq!(plan.items[0].classification, RepairClassification::Blocked);
        assert!(plan.items[0].reason.contains("backup/sentinel is missing"));
        assert!(data_root.join(REPAIR_JOURNAL).is_file());
    }

    #[test]
    fn repair_plan_id_changes_when_managed_evidence_changes() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        fs::create_dir_all(data_root.join("config")).unwrap();
        fs::write(
            data_root.join("config/provisioning.json"),
            b"first-evidence",
        )
        .unwrap();
        let items = vec![repair_item(
            "wordpress_router",
            "wordpress",
            "WordPress router",
            RepairClassification::Repairable,
            "Restore router",
            "Managed router differs from baseline.",
            "Only the managed router changes.",
            true,
        )];
        let first = repair_plan_id(
            data_root,
            &ProvisioningState::NeedsRepair,
            &items,
            data_root,
            data_root,
            data_root,
        );
        fs::write(
            data_root.join("config/provisioning.json"),
            b"second-evidence",
        )
        .unwrap();
        let second = repair_plan_id(
            data_root,
            &ProvisioningState::NeedsRepair,
            &items,
            data_root,
            data_root,
            data_root,
        );
        assert_ne!(first, second);
    }

    #[test]
    fn repair_plan_id_changes_when_repair_target_bytes_change_but_classification_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("store");
        let wordpress_baseline = temp.path().join("wordpress-baseline");
        let empty_baseline = temp.path().join("empty-baseline");
        fs::create_dir_all(&wordpress_baseline).unwrap();
        fs::create_dir_all(&empty_baseline).unwrap();
        fs::create_dir_all(data_root.join("site")).unwrap();
        fs::write(wordpress_baseline.join("wp-settings.php"), b"pinned").unwrap();
        fs::write(data_root.join("site/wp-settings.php"), b"corrupt-a").unwrap();
        let items = vec![repair_item(
            "wordpress_core",
            "wordpress",
            "WordPress core",
            RepairClassification::Repairable,
            "Restore core",
            "Managed WordPress core differs from baseline.",
            "Only pinned core paths change.",
            true,
        )];

        let first = repair_plan_id(
            &data_root,
            &ProvisioningState::NeedsRepair,
            &items,
            &wordpress_baseline,
            &empty_baseline,
            &empty_baseline,
        );
        fs::write(data_root.join("site/wp-settings.php"), b"corrupt-b").unwrap();
        let second = repair_plan_id(
            &data_root,
            &ProvisioningState::NeedsRepair,
            &items,
            &wordpress_baseline,
            &empty_baseline,
            &empty_baseline,
        );
        assert_ne!(first, second);
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
    fn form_encode(value: &str) -> String {
        let mut encoded = String::new();
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                encoded.push(char::from(byte));
            } else if byte == b' ' {
                encoded.push('+');
            } else {
                use std::fmt::Write as _;
                write!(&mut encoded, "%{byte:02X}").unwrap();
            }
        }
        encoded
    }

    #[cfg(windows)]
    fn http_post_form(port: u16, path: &str, fields: &[(&str, &str)]) -> String {
        let body = fields
            .iter()
            .map(|(name, value)| format!("{}={}", form_encode(name), form_encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let mut stream = std::net::TcpStream::connect((LOOPBACK, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = format!(
            "POST {path} HTTP/1.0\r\nHost: {LOOPBACK}:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
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
                Err(error) => panic!("HTTP acceptance POST read failed: {error}"),
            }
        }
        String::from_utf8_lossy(&response).into_owned()
    }

    #[cfg(windows)]
    fn html_input_value(html: &str, input_name: &str) -> Option<String> {
        let marker = format!("name=\"{input_name}\"");
        let offset = html.find(&marker)?;
        let remainder = &html[offset..];
        let value_offset = remainder.find("value=\"")? + "value=\"".len();
        let value = &remainder[value_offset..];
        let end = value.find('"')?;
        Some(value[..end].to_string())
    }

    #[cfg(windows)]
    fn response_cookies(response: &str) -> String {
        response
            .lines()
            .filter_map(|line| {
                let value = line.strip_prefix("Set-Cookie: ")?;
                value.split(';').next().map(str::to_owned)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[cfg(windows)]
    fn response_location(response: &str) -> Option<String> {
        response.lines().find_map(|line| {
            line.strip_prefix("Location: ")
                .map(|value| value.trim().to_string())
        })
    }

    #[cfg(windows)]
    fn assert_text_files_do_not_contain(root: &Path, secret_value: &str) {
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                for entry in fs::read_dir(&path).unwrap().flatten() {
                    stack.push(entry.path());
                }
                continue;
            }
            if metadata.len() > 2 * 1024 * 1024 {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let text_like = matches!(
                extension.as_str(),
                "json" | "log" | "txt" | "php" | "ini" | "conf" | "html" | "css" | "js"
            );
            if !text_like {
                continue;
            }
            let bytes = fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(secret_value.len())
                    .any(|window| window == secret_value.as_bytes()),
                "plaintext administrator password leaked into {}",
                path.display()
            );
        }
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
    fn staged_runtime_phase_5_2_fresh_onboarding_login_and_secret_hygiene() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap().to_path_buf();
        let runtime_manifest =
            project_root.join("runtime/development/x86_64-pc-windows-msvc/manifest.json");
        let runtime = resolve_development_manifest(&project_root, &runtime_manifest).unwrap();

        let e2e_root = manifest_dir.join("target/phase5-2-e2e");
        fs::create_dir_all(&e2e_root).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("onboarding-")
            .tempdir_in(&e2e_root)
            .unwrap();
        let data_root = temp.path().join("store");

        let store_name = "Coffee & \"Co\" Café 5.2";
        let admin_username = "owner_52";
        let admin_email = "owner52@example.com";
        let admin_password = "Phase52SecurePassword2026";

        let mut config_store = crate::config::Store::open(data_root.clone()).unwrap();
        config_store
            .save_setup_profile(store_name, admin_username, admin_email)
            .unwrap();
        secret::store_password(&data_root.join(WORDPRESS_ADMIN_SECRET), admin_password).unwrap();
        let app_config = fs::read_to_string(data_root.join("config/app.json")).unwrap();
        let parsed_config: crate::config::AppConfig = serde_json::from_str(&app_config).unwrap();
        assert_eq!(parsed_config.store_name, store_name);
        assert_eq!(
            parsed_config.setup_admin_username.as_deref(),
            Some(admin_username)
        );
        assert_eq!(
            parsed_config.setup_admin_email.as_deref(),
            Some(admin_email)
        );
        assert!(!app_config.contains(admin_password));
        drop(config_store);

        let protected_password = fs::read(data_root.join(WORDPRESS_ADMIN_SECRET)).unwrap();
        assert!(!protected_password
            .windows(admin_password.len())
            .any(|window| window == admin_password.as_bytes()));

        let mut provisioner = Provisioner::from_development(
            &project_root,
            &runtime_manifest,
            runtime.clone(),
            data_root.clone(),
        )
        .unwrap();
        provisioner
            .configure_initial_admin(admin_username, admin_email)
            .unwrap();
        assert_eq!(provisioner.inspect().state, ProvisioningState::NotInstalled);
        provisioner.prepare().unwrap();

        let mut manager = RuntimeManager::new(runtime, data_root.clone()).unwrap();
        let running = manager.start().unwrap();
        let installed = provisioner
            .install_wordpress(store_name, &running)
            .unwrap_or_else(|error| {
                let provisioning_log = fs::read_to_string(data_root.join("logs/provisioning.log"))
                    .unwrap_or_else(|read_error| {
                        format!("<cannot read provisioning.log: {read_error}>")
                    });
                panic!(
                    "Phase 5.2 onboarding provisioning failed: {error}\n--- provisioning.log ---\n{provisioning_log}"
                )
            });
        assert_eq!(installed.state, ProvisioningState::Ready);
        assert_eq!(installed.admin_username.as_deref(), Some(admin_username));
        let journal = provisioner.load_journal().unwrap().unwrap();
        assert_eq!(journal.admin_username, admin_username);
        assert_eq!(journal.stage, ProvisioningStage::MachineHealthBootstrapped);

        let database_port = running.database_port.unwrap();
        assert_eq!(
            query_wordpress_option(&provisioner, database_port, "blogname"),
            "Coffee &amp; &quot;Co&quot; Café 5.2"
        );
        assert_eq!(
            query_wordpress_option(&provisioner, database_port, "coffeepos_store_name"),
            store_name
        );
        assert_eq!(
            query_database_scalar(
                &provisioner,
                database_port,
                &format!(
                    "SELECT user_email FROM wp_users WHERE user_login = '{}' LIMIT 1",
                    sql_literal(admin_username)
                ),
            ),
            admin_email
        );
        assert_eq!(
            secret::load(&data_root.join(WORDPRESS_ADMIN_SECRET)).unwrap(),
            admin_password
        );

        let healthy = manager.refresh_wordpress_health();
        assert_eq!(healthy.wordpress_health, WordPressHealthState::Healthy);
        assert_eq!(
            healthy.coffeepos_health.state,
            CoffeePosHealthState::Healthy
        );
        assert_eq!(
            healthy
                .coffeepos_health
                .payload
                .as_ref()
                .unwrap()
                .store
                .name,
            store_name
        );
        let http_port = healthy.http_port.unwrap();
        let login_page = http_get(http_port, "/pos/");
        assert!(
            login_page.starts_with("HTTP/1.0 200 ") || login_page.starts_with("HTTP/1.1 200 "),
            "CoffeePOS login page did not return HTTP 200"
        );
        let nonce = html_input_value(&login_page, "coffeepos_nonce")
            .expect("CoffeePOS login form did not contain a nonce");
        let login_response = http_post_form(
            http_port,
            "/pos/",
            &[
                ("coffeepos_nonce", nonce.as_str()),
                ("log", admin_username),
                ("pwd", admin_password),
                ("rememberme", "1"),
            ],
        );
        assert!(
            login_response.starts_with("HTTP/1.0 302 ")
                || login_response.starts_with("HTTP/1.1 302 "),
            "CoffeePOS login did not redirect after valid onboarding credentials"
        );
        let cookies = response_cookies(&login_response);
        assert!(
            cookies.contains("wordpress_logged_in_"),
            "CoffeePOS login did not issue a WordPress logged-in cookie"
        );
        let location = response_location(&login_response)
            .expect("CoffeePOS login did not include a redirect location");
        let origin = format!("http://{LOOPBACK}:{http_port}");
        assert!(
            location.starts_with(&origin),
            "CoffeePOS login redirected outside the managed origin: {location}"
        );
        let target_path = location.strip_prefix(&origin).unwrap_or("/pos/");
        let authenticated =
            http_get_with_headers(http_port, target_path, &[("Cookie", cookies.as_str())]);
        assert!(
            authenticated.starts_with("HTTP/1.0 200 ")
                || authenticated.starts_with("HTTP/1.1 200 "),
            "Authenticated CoffeePOS landing page did not return HTTP 200"
        );
        assert!(
            !authenticated.contains("data-component=\"staff-login-form\""),
            "Authenticated CoffeePOS request returned the login form"
        );

        assert_text_files_do_not_contain(&data_root.join("config"), admin_password);
        assert_text_files_do_not_contain(&data_root.join("logs"), admin_password);
        assert_text_files_do_not_contain(&data_root.join("site"), admin_password);

        let stopped = manager.stop().unwrap();
        assert_eq!(stopped.state, RuntimeState::Stopped);
        assert!(stopped.database_pid.is_none());
        assert!(stopped.php_pid.is_none());
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

        let recovery_store_name = "CoffeePOS Phase 5.2 Recovery";
        let recovery_admin_username = "recovery_owner_52";
        let recovery_admin_email = "recovery52@example.com";
        let recovery_admin_password = "Phase52RecoveryPassword2026";
        let mut config_store = crate::config::Store::open(data_root.clone()).unwrap();
        config_store
            .save_setup_profile(
                recovery_store_name,
                recovery_admin_username,
                recovery_admin_email,
            )
            .unwrap();
        secret::store_password(
            &data_root.join(WORDPRESS_ADMIN_SECRET),
            recovery_admin_password,
        )
        .unwrap();
        drop(config_store);

        let make_provisioner = || {
            let mut provisioner = Provisioner::from_development(
                &project_root,
                &runtime_manifest,
                runtime.clone(),
                data_root.clone(),
            )
            .unwrap();
            provisioner
                .configure_initial_admin(recovery_admin_username, recovery_admin_email)
                .unwrap();
            provisioner
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
        assert_eq!(
            provisioner.load_journal().unwrap().unwrap().admin_username,
            recovery_admin_username
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
                .install_wordpress(recovery_store_name, &running)
                .unwrap_err();
            assert_eq!(error.operation, "simulate provisioning interruption");
            assert_eq!(
                provisioner.load_journal().unwrap().unwrap().stage,
                expected_stage
            );
            let inspect = provisioner.inspect();
            assert_eq!(inspect.state, ProvisioningState::NeedsRepair);
            assert!(inspect.can_retry);

            if boundary == ProvisioningBoundary::WordPressInstalled {
                let database_port = running.database_port.unwrap();
                let database_password =
                    secret::load(&data_root.join(DATABASE_WORDPRESS_SECRET)).unwrap();
                let endpoint = DatabaseEndpoint::Tcp(database_port);
                provisioner
                    .run_database_sql(
                        &endpoint,
                        DATABASE_WORDPRESS_USER,
                        &database_password,
                        &format!(
                            "UPDATE coffeepos.wp_users SET user_email = 'tampered@example.com' WHERE user_login = '{}';\n",
                            sql_literal(recovery_admin_username)
                        ),
                    )
                    .unwrap();

                let mut retry_provisioner = make_provisioner();
                let retry_error = retry_provisioner
                    .install_wordpress(recovery_store_name, &running)
                    .unwrap_err();
                assert_eq!(retry_error.operation, "install WordPress");
                assert_eq!(
                    retry_provisioner.load_journal().unwrap().unwrap().stage,
                    ProvisioningStage::SiteReady
                );
                drop(retry_provisioner);

                provisioner
                    .run_database_sql(
                        &endpoint,
                        DATABASE_WORDPRESS_USER,
                        &database_password,
                        &format!(
                            "UPDATE coffeepos.wp_users SET user_email = '{}' WHERE user_login = '{}';\n",
                            sql_literal(recovery_admin_email),
                            sql_literal(recovery_admin_username)
                        ),
                    )
                    .unwrap();
            }

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
            .install_wordpress(recovery_store_name, &running)
            .unwrap();
        assert_eq!(ready.state, ProvisioningState::Ready);
        assert!(ready.woocommerce_active);
        assert!(ready.coffeepos_active);
        assert_eq!(
            ready.admin_username.as_deref(),
            Some(recovery_admin_username)
        );
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
            recovery_admin_password
        );
        assert_eq!(wordpress_admin_secret.unwrap(), recovery_admin_password);
        assert_eq!(
            secret::load(&data_root.join(MACHINE_TOKEN_SECRET)).unwrap(),
            machine_token.unwrap()
        );
        assert_eq!(provisioner.inspect().state, ProvisioningState::Ready);
        let final_health = manager.refresh_wordpress_health();
        assert_eq!(
            final_health.coffeepos_health.state,
            CoffeePosHealthState::Healthy
        );
        assert_eq!(
            final_health
                .coffeepos_health
                .payload
                .as_ref()
                .unwrap()
                .store
                .name,
            recovery_store_name
        );

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
