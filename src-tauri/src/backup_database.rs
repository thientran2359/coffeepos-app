use crate::runtime::{
    choose_loopback_port, configure_child_command, DatabaseMaintenanceLease, ResolvedRuntime,
    RuntimeManager, DATABASE_NAME, DATABASE_WORDPRESS_SECRET, DATABASE_WORDPRESS_USER,
};
use crate::secret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const LOOPBACK: &str = "127.0.0.1";
const STAGING_ROOT: &str = "backups/.database-staging";
const OWNERSHIP_MARKER: &str = ".coffeepos-database-backup-staging";
const OWNERSHIP_MARKER_VALUE: &[u8] = b"CoffeePOS database backup staging v1\n";
const DUMP_FILE_NAME: &str = "store.sql";
const OPTION_FILE_NAME: &str = "client.cnf";
const DUMP_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_VERIFY_QUERY_BYTES: usize = 1024 * 1024;
const CHILD_RECOVERY_SCHEMA_VERSION: u32 = 1;
const CHILD_RECOVERY_PREFIX: &str = ".coffeepos-backup-child-";
const CHILD_RECOVERY_JSON_SUFFIX: &str = ".json";
const CHILD_RECOVERY_PENDING_SUFFIX: &str = ".pending";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BackupChildRecoveryRecord {
    schema_version: u32,
    pid: u32,
    creation_time: u64,
    image_path: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct BackupDatabaseErrorInfo {
    pub(crate) component: String,
    pub(crate) action: String,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) recovery: String,
}

impl std::fmt::Display for BackupDatabaseErrorInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {}: {} {}",
            self.component, self.action, self.message, self.recovery
        )
    }
}

impl std::error::Error for BackupDatabaseErrorInfo {}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupDatabaseStage {
    Planned,
    Quiesced,
    DatabaseReady,
    Dumping,
    Dumped,
    Verifying,
    Verified,
    Cleanup,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct DatabaseBackupArtifact {
    pub(crate) size_bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DatabaseSnapshotFingerprint {
    tables: String,
    triggers: String,
    wp_options_rows: String,
    wp_users_rows: String,
    wp_usermeta_rows: String,
    store_name_hex: String,
    woocommerce_table_count: String,
    woocommerce_order_rows: String,
    woocommerce_order_digest: String,
    coffeepos_table_count: String,
    coffeepos_option_count: String,
    coffeepos_db_version_hex: String,
    coffeepos_shift_rows: String,
    coffeepos_shift_digest: String,
    coffeepos_suspended_cart_rows: String,
    coffeepos_suspended_cart_digest: String,
}

pub(crate) struct DatabaseBackupSession {
    operation_id: String,
    maintenance: DatabaseMaintenanceLease,
    runtime: ResolvedRuntime,
    data_root: PathBuf,
    staging_dir: PathBuf,
    dump_path: PathBuf,
    stage: BackupDatabaseStage,
    artifact: Option<DatabaseBackupArtifact>,
    verified_store_name: Option<String>,
}

impl DatabaseBackupSession {
    pub(crate) fn begin(
        runtime_manager: &mut RuntimeManager,
        cancelled: &AtomicBool,
    ) -> Result<Self, BackupDatabaseErrorInfo> {
        let operation_id = random_operation_id()?;
        Self::begin_with_operation_id(runtime_manager, cancelled, operation_id)
    }

    pub(crate) fn begin_with_operation_id(
        runtime_manager: &mut RuntimeManager,
        cancelled: &AtomicBool,
        operation_id: String,
    ) -> Result<Self, BackupDatabaseErrorInfo> {
        if cancelled.load(Ordering::Acquire) {
            return Err(cancelled_error("quiesce"));
        }
        if operation_id.len() != 32
            || !operation_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(backup_error(
                "quiesce",
                "operation_id_invalid",
                "CoffeePOS refused an invalid database-backup operation id.",
                "Restart the backup so CoffeePOS can generate a fresh operation id.",
            ));
        }
        if runtime_manager.backup_maintenance_active() {
            return Err(backup_error(
                "quiesce",
                "backup_in_progress",
                "A database backup maintenance lease is already active.",
                "Finish or cancel the current backup cleanup before starting another backup.",
            ));
        }

        let (runtime, data_root) = runtime_manager.provisioning_context();
        require_managed_backup_tools(&runtime)?;
        let password = Zeroizing::new(
            secret::load(&data_root.join(DATABASE_WORDPRESS_SECRET)).map_err(|_| {
                backup_error(
                    "quiesce",
                    "database_credential_unavailable",
                    "The protected WordPress database credential cannot be read for backup.",
                    "Repair the protected database credential with the same Windows profile, then retry backup.",
                )
            })?,
        );
        if password.is_empty() {
            return Err(backup_error(
                "quiesce",
                "database_credential_unavailable",
                "The protected WordPress database credential is empty.",
                "Repair the protected database credential before retrying backup.",
            ));
        }

        cleanup_stale_owned_staging(&data_root)?;
        let staging_dir = prepare_staging_dir(&data_root, &operation_id)?;
        let dump_path = staging_dir.join(DUMP_FILE_NAME);

        let maintenance = match runtime_manager.enter_database_backup_maintenance() {
            Ok(lease) => lease,
            Err(error) => {
                let _ = remove_owned_staging_dir(&data_root, &staging_dir);
                return Err(runtime_error("quiesce", "runtime_quiesce_failed", error));
            }
        };
        runtime_manager.register_backup_recovery_context(&maintenance, &staging_dir);

        let mut session = Self {
            operation_id,
            maintenance,
            runtime,
            data_root,
            staging_dir,
            dump_path,
            stage: BackupDatabaseStage::Quiesced,
            artifact: None,
            verified_store_name: None,
        };

        session.stage = BackupDatabaseStage::DatabaseReady;
        if cancelled.load(Ordering::Acquire) {
            let error = cancelled_error("quiesce");
            return Err(session.cleanup_after_failure(runtime_manager, error));
        }

        if let Err(error) = session.verify_wordpress_database_credential(
            runtime_manager,
            cancelled,
            password.as_str(),
        ) {
            return Err(session.cleanup_after_failure(runtime_manager, error));
        }
        Ok(session)
    }

    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub(crate) fn stage(&self) -> BackupDatabaseStage {
        self.stage.clone()
    }

    pub(crate) fn dump_path(&self) -> &Path {
        &self.dump_path
    }

    pub(crate) fn artifact(&self) -> Option<&DatabaseBackupArtifact> {
        self.artifact.as_ref()
    }

    pub(crate) fn verified_store_name(&self) -> Option<&str> {
        self.verified_store_name.as_deref()
    }

    pub(crate) fn capture_administrator_email(
        &self,
        runtime_manager: &mut RuntimeManager,
        cancelled: &AtomicBool,
        admin_username: &str,
    ) -> Result<String, BackupDatabaseErrorInfo> {
        if self.stage != BackupDatabaseStage::Verified {
            return Err(backup_error(
                "database",
                "database_snapshot_not_verified",
                "CoffeePOS cannot capture portable administrator identity before the database snapshot is verified.",
                "Retry backup from the beginning.",
            ));
        }
        if admin_username.is_empty()
            || !admin_username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(backup_error(
                "database",
                "administrator_identity_invalid",
                "The provisioned administrator username is invalid for portable backup metadata.",
                "Repair the managed administrator identity before retrying backup.",
            ));
        }
        let password = Zeroizing::new(
            secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET)).map_err(|_| {
                backup_error(
                    "database",
                    "database_credential_unavailable",
                    "The protected WordPress database credential became unavailable while capturing backup metadata.",
                    "Restore the protected database credential and retry backup.",
                )
            })?,
        );
        let option_file = PrivateOptionFile::create(
            &self.staging_dir,
            self.maintenance.database_port,
            DATABASE_WORDPRESS_USER,
            password.as_str(),
        )?;
        let sql = format!(
            "SELECT COALESCE(HEX((SELECT user_email FROM coffeepos.wp_users WHERE user_login='{admin_username}' ORDER BY ID LIMIT 1)), '')"
        );
        let args = vec![
            OsString::from(format!(
                "--defaults-extra-file={}",
                option_file.path().to_string_lossy()
            )),
            OsString::from(format!("--database={DATABASE_NAME}")),
            OsString::from("--batch"),
            OsString::from("--skip-column-names"),
            OsString::from(format!("--execute={sql}")),
        ];
        let bytes = run_client_capture(
            runtime_manager,
            &self.runtime,
            &args,
            cancelled,
            CLIENT_TIMEOUT,
        )?;
        drop(option_file);
        let encoded = String::from_utf8(bytes).map_err(|_| {
            backup_error(
                "database",
                "administrator_identity_invalid",
                "MariaDB returned invalid administrator identity text.",
                "Repair the managed administrator account and retry backup.",
            )
        })?;
        let email = decode_utf8_hex(encoded.trim())?;
        if email.trim().is_empty()
            || email.chars().count() > 100
            || email.chars().any(char::is_control)
        {
            return Err(backup_error(
                "database",
                "administrator_identity_invalid",
                "The managed administrator email is invalid for portable backup metadata.",
                "Repair the WordPress administrator email and retry backup.",
            ));
        }
        Ok(email)
    }

    pub(crate) fn runtime_was_running(&self) -> bool {
        self.maintenance.runtime_was_running
    }

    pub(crate) fn previous_runtime_state(&self) -> crate::runtime::RuntimeState {
        self.maintenance.previous_state.clone()
    }

    pub(crate) fn seal_snapshot(
        &mut self,
        runtime_manager: &mut RuntimeManager,
    ) -> Result<(), BackupDatabaseErrorInfo> {
        runtime_manager
            .seal_database_backup_snapshot()
            .map(|_| ())
            .map_err(|error| runtime_error("database", "database_snapshot_seal_failed", error))
    }

    pub(crate) fn create_dump(
        &mut self,
        runtime_manager: &mut RuntimeManager,
        cancelled: &AtomicBool,
    ) -> Result<DatabaseBackupArtifact, BackupDatabaseErrorInfo> {
        if self.artifact.is_some() {
            return Err(backup_error(
                "dump",
                "dump_already_created",
                "This database backup session already created its logical dump.",
                "Continue the same backup transaction or clean it up before starting another dump.",
            ));
        }
        if cancelled.load(Ordering::Acquire) {
            self.stage = BackupDatabaseStage::Cancelled;
            return Err(cancelled_error("dump"));
        }

        let password = Zeroizing::new(
            secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET)).map_err(|_| {
                backup_error(
                    "dump",
                    "database_credential_unavailable",
                    "The protected WordPress database credential became unavailable before the dump started.",
                    "Keep the database maintenance window active, repair the credential state, and retry through a fresh backup operation.",
                )
            })?,
        );
        let option_file = PrivateOptionFile::create(
            &self.staging_dir,
            self.maintenance.database_port,
            DATABASE_WORDPRESS_USER,
            password.as_str(),
        )?;
        self.stage = BackupDatabaseStage::Dumping;

        let output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&self.dump_path)
            .map_err(|_| {
                backup_error(
                    "dump",
                    "dump_output_unavailable",
                    "CoffeePOS could not create the private SQL staging file.",
                    "Check free disk space and application-data permissions, then retry backup.",
                )
            })?;
        let args = dump_arguments(option_file.path());
        let mut command = Command::new(&self.runtime.mariadb_dump_executable);
        command
            .args(&args)
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::piped())
            .current_dir(&self.runtime.mariadb_base_dir);
        configure_child_command(&mut command);
        let child = command.spawn().map_err(|_| {
            backup_error(
                "dump",
                "dump_start_failed",
                "CoffeePOS could not start the pinned MariaDB dump tool.",
                "Repair the managed MariaDB runtime artifact, then retry backup.",
            )
        })?;
        let mut child =
            contain_child_or_cleanup(runtime_manager, child, "dump", "dump_containment_failed")?;
        let stderr = child.stderr.take().map(spawn_discard_reader);
        let status =
            wait_owned_backup_child(runtime_manager, child, DUMP_TIMEOUT, cancelled, "dump")?;
        if let Some(reader) = stderr {
            let _ = reader.join();
        }
        drop(option_file);

        if !status.success() {
            let _ = fs::remove_file(&self.dump_path);
            return Err(backup_error(
                "dump",
                "dump_failed",
                "The pinned MariaDB dump tool did not complete successfully.",
                "The live datadir was preserved. Check database health and available disk space, then retry backup.",
            ));
        }
        let metadata = fs::metadata(&self.dump_path).map_err(|_| {
            backup_error(
                "verify",
                "dump_missing",
                "The MariaDB dump process completed without a readable SQL artifact.",
                "Retry backup after checking application-data storage.",
            )
        })?;
        if !metadata.is_file() || metadata.len() == 0 {
            let _ = fs::remove_file(&self.dump_path);
            return Err(backup_error(
                "verify",
                "dump_empty",
                "The MariaDB logical dump is empty.",
                "Verify the managed CoffeePOS database and retry backup.",
            ));
        }
        let sha256 = sha256_file(&self.dump_path)?;
        let artifact = DatabaseBackupArtifact {
            size_bytes: metadata.len(),
            sha256,
        };
        self.artifact = Some(artifact.clone());
        self.stage = BackupDatabaseStage::Dumped;
        Ok(artifact)
    }

    pub(crate) fn cleanup(
        mut self,
        runtime_manager: &mut RuntimeManager,
    ) -> Result<crate::runtime::RuntimeInfo, BackupDatabaseErrorInfo> {
        self.cleanup_with_resume_policy(runtime_manager, true)
    }

    pub(crate) fn cleanup_without_resume(
        mut self,
        runtime_manager: &mut RuntimeManager,
    ) -> Result<crate::runtime::RuntimeInfo, BackupDatabaseErrorInfo> {
        self.cleanup_with_resume_policy(runtime_manager, false)
    }

    fn cleanup_with_resume_policy(
        &mut self,
        runtime_manager: &mut RuntimeManager,
        restore_previous_running_state: bool,
    ) -> Result<crate::runtime::RuntimeInfo, BackupDatabaseErrorInfo> {
        self.stage = BackupDatabaseStage::Cleanup;
        remove_owned_staging_dir(&self.data_root, &self.staging_dir)?;
        let runtime_result = if restore_previous_running_state {
            runtime_manager.finish_database_backup_maintenance(&self.maintenance)
        } else {
            runtime_manager.finish_database_backup_maintenance_stopped(&self.maintenance)
        }
        .map_err(|error| runtime_error("resume", "runtime_resume_failed", error));
        match runtime_result {
            Ok(info) => {
                runtime_manager.clear_backup_recovery_context();
                self.stage = BackupDatabaseStage::Completed;
                Ok(info)
            }
            Err(error) => {
                if !runtime_manager.backup_maintenance_active() {
                    runtime_manager.clear_backup_recovery_context();
                }
                Err(error)
            }
        }
    }

    fn verify_wordpress_database_credential(
        &self,
        runtime_manager: &mut RuntimeManager,
        cancelled: &AtomicBool,
        password: &str,
    ) -> Result<(), BackupDatabaseErrorInfo> {
        let option_file = PrivateOptionFile::create(
            &self.staging_dir,
            self.maintenance.database_port,
            DATABASE_WORDPRESS_USER,
            password,
        )?;
        let mut command = Command::new(&self.runtime.mariadb_import_executable);
        command
            .arg(format!(
                "--defaults-extra-file={}",
                option_file.path().to_string_lossy()
            ))
            .arg(format!("--database={DATABASE_NAME}"))
            .arg("--batch")
            .arg("--skip-column-names")
            .arg("--execute=SELECT 1")
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(&self.runtime.mariadb_base_dir);
        configure_child_command(&mut command);
        let child = command.spawn().map_err(|_| {
            backup_error(
                "quiesce",
                "database_probe_start_failed",
                "CoffeePOS could not start the pinned MariaDB client for backup credential verification.",
                "Repair the managed MariaDB client artifact, then retry backup.",
            )
        })?;
        let child = contain_child_or_cleanup(
            runtime_manager,
            child,
            "quiesce",
            "database_probe_containment_failed",
        )?;
        let status =
            wait_owned_backup_child(runtime_manager, child, CLIENT_TIMEOUT, cancelled, "quiesce")?;
        drop(option_file);
        if status.success() {
            Ok(())
        } else {
            Err(backup_error(
                "quiesce",
                "database_credential_rejected",
                "MariaDB rejected the protected WordPress database credential before backup.",
                "Repair the matching protected database credential; no logical dump was created.",
            ))
        }
    }

    pub(crate) fn verify_dump_import(
        &mut self,
        runtime_manager: &mut RuntimeManager,
        cancelled: &AtomicBool,
    ) -> Result<(), BackupDatabaseErrorInfo> {
        if self.artifact.is_none() || !self.dump_path.is_file() {
            return Err(backup_error(
                "verify",
                "dump_missing",
                "CoffeePOS cannot verify a database snapshot before a complete SQL dump exists.",
                "Retry the database backup from the beginning.",
            ));
        }
        if cancelled.load(Ordering::Acquire) {
            self.stage = BackupDatabaseStage::Cancelled;
            return Err(cancelled_error("verify"));
        }
        self.stage = BackupDatabaseStage::Verifying;

        let password = Zeroizing::new(
            secret::load(&self.data_root.join(DATABASE_WORDPRESS_SECRET)).map_err(|_| {
                backup_error(
                    "verify",
                    "database_credential_unavailable",
                    "The protected WordPress database credential became unavailable before dump verification.",
                    "Restore the protected credential and retry the backup from the beginning.",
                )
            })?,
        );
        let source_option = PrivateOptionFile::create(
            &self.staging_dir,
            self.maintenance.database_port,
            DATABASE_WORDPRESS_USER,
            password.as_str(),
        )?;
        let source = capture_fingerprint(
            runtime_manager,
            &self.runtime,
            Some(source_option.path()),
            self.maintenance.database_port,
            cancelled,
        )?;
        let source_store_name = decode_utf8_hex(&source.store_name_hex)?;
        drop(source_option);

        let verify_datadir = self.staging_dir.join("verify-database");
        initialize_disposable_datadir(runtime_manager, &self.runtime, &verify_datadir, cancelled)?;
        let verify_port = choose_loopback_port(&[self.maintenance.database_port])
            .map_err(|error| runtime_error("verify", "verify_port_unavailable", error))?;
        let mut disposable = DisposableDatabase::start(
            runtime_manager,
            &self.runtime,
            &verify_datadir,
            verify_port,
        )?;
        let verification = (|| {
            disposable.wait_root_ready(runtime_manager, &self.runtime, cancelled)?;
            import_dump(
                runtime_manager,
                &self.runtime,
                verify_port,
                &self.dump_path,
                cancelled,
            )?;
            let restored =
                capture_fingerprint(runtime_manager, &self.runtime, None, verify_port, cancelled)?;
            if source != restored {
                return Err(backup_error(
                    "verify",
                    "dump_invariant_mismatch",
                    "The disposable MariaDB import did not reproduce the source database baseline.",
                    "CoffeePOS discarded this snapshot candidate. Inspect database health and retry backup before trusting the dump.",
                ));
            }
            Ok(())
        })();
        if let Err(error) = &verification {
            if error.code == "child_cleanup_unconfirmed" {
                disposable.retain_for_recovery(runtime_manager);
                return Err(error.clone());
            }
        }
        let shutdown = disposable.shutdown_root(runtime_manager, &self.runtime, cancelled);
        if let Err(error) = &shutdown {
            if error.code == "child_cleanup_unconfirmed" {
                return Err(error.clone());
            }
        }
        let remove = remove_private_tree(&verify_datadir);
        match (verification, shutdown, remove) {
            (Ok(()), Ok(()), Ok(())) => {
                self.verified_store_name = Some(source_store_name);
                self.stage = BackupDatabaseStage::Verified;
                Ok(())
            }
            (Err(error), Ok(()), Ok(())) => Err(error),
            (verification, shutdown, remove) => {
                let primary = verification.err();
                let shutdown_message = shutdown.err().map(|error| error.message);
                let remove_message = remove.err().map(|error| error.message);
                Err(backup_error(
                    "verify",
                    "verify_cleanup_failed",
                    "CoffeePOS could not fully clean the disposable database used to verify the logical dump.",
                    &format!(
                        "Keep CoffeePOS open until verification cleanup is complete, then retry. Verification: {} Shutdown: {} Cleanup: {}",
                        primary
                            .map(|error| error.message)
                            .unwrap_or_else(|| "completed".into()),
                        shutdown_message.unwrap_or_else(|| "completed".into()),
                        remove_message.unwrap_or_else(|| "completed".into())
                    ),
                ))
            }
        }
    }

    fn cleanup_after_failure(
        &mut self,
        runtime_manager: &mut RuntimeManager,
        original: BackupDatabaseErrorInfo,
    ) -> BackupDatabaseErrorInfo {
        if original.code == "child_cleanup_unconfirmed" {
            self.stage = BackupDatabaseStage::Cleanup;
            return original;
        }
        self.stage = if original.code == "cancelled" {
            BackupDatabaseStage::Cancelled
        } else {
            BackupDatabaseStage::Cleanup
        };
        if let Err(staging_error) = remove_owned_staging_dir(&self.data_root, &self.staging_dir) {
            return backup_error(
                "cleanup",
                "cleanup_failed",
                "The database backup operation failed and CoffeePOS could not remove its private staging safely.",
                &format!(
                    "Backup maintenance remains fenced. Original failure: {} Staging cleanup: {}",
                    original.message, staging_error.message
                ),
            );
        }
        match runtime_manager.finish_database_backup_maintenance(&self.maintenance) {
            Ok(_) => {
                runtime_manager.clear_backup_recovery_context();
                original
            }
            Err(runtime_error) => {
                if !runtime_manager.backup_maintenance_active() {
                    runtime_manager.clear_backup_recovery_context();
                }
                backup_error(
                    "cleanup",
                    "cleanup_failed",
                    "The database backup operation failed and CoffeePOS could not fully restore the previous runtime state.",
                    &format!(
                        "Original failure: {} Runtime reconciliation: {}",
                        original.message, runtime_error.message
                    ),
                )
            }
        }
    }
}

fn decode_utf8_hex(value: &str) -> Result<String, BackupDatabaseErrorInfo> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(backup_error(
            "verify",
            "store_identity_invalid",
            "The managed database returned an invalid encoded store identity.",
            "Repair the CoffeePOS store-name option and retry backup.",
        ));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    for pair in pairs {
        let high = (pair[0] as char).to_digit(16).unwrap_or(0) as u8;
        let low = (pair[1] as char).to_digit(16).unwrap_or(0) as u8;
        bytes.push((high << 4) | low);
    }
    let store_name = String::from_utf8(bytes).map_err(|_| {
        backup_error(
            "verify",
            "store_identity_invalid",
            "The managed database returned a non-UTF-8 store identity.",
            "Repair the CoffeePOS store-name option and retry backup.",
        )
    })?;
    if store_name.trim().is_empty()
        || store_name.chars().count() > 80
        || store_name.chars().any(char::is_control)
    {
        return Err(backup_error(
            "verify",
            "store_identity_invalid",
            "The managed database returned an invalid CoffeePOS store identity.",
            "Repair the CoffeePOS store-name option and retry backup.",
        ));
    }
    Ok(store_name)
}

pub(crate) fn retry_database_backup_cleanup(
    runtime_manager: &mut RuntimeManager,
) -> Result<crate::runtime::RuntimeInfo, BackupDatabaseErrorInfo> {
    let (lease, staging_dir) = runtime_manager.backup_recovery_context().ok_or_else(|| {
        backup_error(
            "cleanup",
            "recovery_context_missing",
            "CoffeePOS has no retained database-backup cleanup context to retry.",
            "Start a new backup only after the current runtime state is healthy and no backup maintenance fence is active.",
        )
    })?;
    runtime_manager
        .terminate_retained_backup_children()
        .map_err(|error| runtime_error("cleanup", "child_cleanup_unconfirmed", error))?;
    let (_, data_root) = runtime_manager.provisioning_context();
    remove_owned_staging_dir(&data_root, &staging_dir)?;
    let result = runtime_manager
        .finish_database_backup_maintenance(&lease)
        .map_err(|error| runtime_error("resume", "runtime_resume_failed", error));
    if !runtime_manager.backup_maintenance_active() {
        runtime_manager.clear_backup_recovery_context();
    }
    result
}

pub(crate) fn create_database_snapshot(
    runtime_manager: &mut RuntimeManager,
    cancelled: &AtomicBool,
) -> Result<DatabaseBackupSession, BackupDatabaseErrorInfo> {
    let mut session = DatabaseBackupSession::begin(runtime_manager, cancelled)?;
    if let Err(error) = session.create_dump(runtime_manager, cancelled) {
        return Err(session.cleanup_after_failure(runtime_manager, error));
    }
    if let Err(error) = session.verify_dump_import(runtime_manager, cancelled) {
        return Err(session.cleanup_after_failure(runtime_manager, error));
    }
    Ok(session)
}

struct DisposableDatabase {
    child: Option<Child>,
    port: u16,
}

impl DisposableDatabase {
    fn start(
        runtime_manager: &mut RuntimeManager,
        runtime: &ResolvedRuntime,
        datadir: &Path,
        port: u16,
    ) -> Result<Self, BackupDatabaseErrorInfo> {
        let mut command = Command::new(&runtime.mariadb_executable);
        command
            .arg("--no-defaults")
            .arg(format!(
                "--basedir={}",
                runtime.mariadb_base_dir.to_string_lossy()
            ))
            .arg(format!("--datadir={}", datadir.to_string_lossy()))
            .arg(format!("--port={port}"))
            .arg(format!("--bind-address={LOOPBACK}"))
            .arg("--skip-name-resolve")
            .arg("--skip-log-bin")
            .arg(format!(
                "--pid-file={}",
                datadir.join("verify.pid").to_string_lossy()
            ));
        #[cfg(windows)]
        command.arg("--console");
        command
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(&runtime.mariadb_base_dir);
        configure_child_command(&mut command);
        let child = command.spawn().map_err(|_| {
            backup_error(
                "verify",
                "verify_database_start_failed",
                "CoffeePOS could not start disposable MariaDB for logical-dump verification.",
                "Repair the pinned MariaDB runtime and retry backup.",
            )
        })?;
        let child = contain_child_or_cleanup(
            runtime_manager,
            child,
            "verify",
            "verify_database_containment_failed",
        )?;
        Ok(Self {
            child: Some(child),
            port,
        })
    }

    fn child_mut(&mut self) -> Result<&mut Child, BackupDatabaseErrorInfo> {
        self.child.as_mut().ok_or_else(|| {
            backup_error(
                "verify",
                "verify_database_handle_missing",
                "CoffeePOS lost the disposable MariaDB process handle during backup verification.",
                "Keep backup maintenance fenced and retry cleanup before another backup.",
            )
        })
    }

    fn retain_for_recovery(&mut self, runtime_manager: &mut RuntimeManager) {
        if let Some(child) = self.child.take() {
            runtime_manager.retain_backup_recovery_child(child);
        }
    }

    fn terminate_for_cleanup(
        &mut self,
        runtime_manager: &mut RuntimeManager,
        action: &str,
    ) -> Result<(), BackupDatabaseErrorInfo> {
        let result = match self.child.as_mut() {
            Some(child) => terminate_child(child, action),
            None => return Ok(()),
        };
        match result {
            Ok(()) => {
                self.child.take();
                Ok(())
            }
            Err(error) if error.code == "child_cleanup_unconfirmed" => {
                self.retain_for_recovery(runtime_manager);
                Err(error)
            }
            Err(error) => {
                self.child.take();
                Err(error)
            }
        }
    }

    fn wait_root_ready(
        &mut self,
        runtime_manager: &mut RuntimeManager,
        runtime: &ResolvedRuntime,
        cancelled: &AtomicBool,
    ) -> Result<(), BackupDatabaseErrorInfo> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if cancelled.load(Ordering::Acquire) {
                self.terminate_for_cleanup(runtime_manager, "verify")?;
                return Err(cancelled_error("verify"));
            }
            let process_state = self.child_mut()?.try_wait();
            match process_state {
                Ok(Some(_status)) => {
                    self.child.take();
                    return Err(backup_error(
                        "verify",
                        "verify_database_exited",
                        "Disposable MariaDB exited before logical-dump verification could begin.",
                        "Repair the pinned MariaDB runtime and retry backup.",
                    ));
                }
                Ok(None) => {}
                Err(_) => {
                    self.terminate_for_cleanup(runtime_manager, "verify")?;
                    return Err(backup_error(
                        "verify",
                        "verify_database_state_unavailable",
                        "CoffeePOS could not inspect the disposable MariaDB process.",
                        "The disposable process was terminated. Retry backup after checking the managed MariaDB runtime.",
                    ));
                }
            }
            let args = root_client_arguments(self.port, "SELECT 1");
            match run_client_capture(runtime_manager, runtime, &args, cancelled, CLIENT_TIMEOUT) {
                Ok(_) => return Ok(()),
                Err(error) if error.code == "client_failed" && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                self.terminate_for_cleanup(runtime_manager, "verify")?;
                return Err(backup_error(
                    "verify",
                    "verify_database_timeout",
                    "Disposable MariaDB did not become ready before the verification timeout.",
                    "Retry backup after checking the pinned MariaDB runtime.",
                ));
            }
        }
    }

    fn shutdown_root(
        &mut self,
        runtime_manager: &mut RuntimeManager,
        runtime: &ResolvedRuntime,
        cancelled: &AtomicBool,
    ) -> Result<(), BackupDatabaseErrorInfo> {
        if self.child.is_none() {
            return Ok(());
        }
        match self.child_mut()?.try_wait() {
            Ok(Some(_)) => {
                self.child.take();
                return Ok(());
            }
            Ok(None) => {}
            Err(_) => {
                self.terminate_for_cleanup(runtime_manager, "verify")?;
                return Err(backup_error(
                    "verify",
                    "verify_database_state_unavailable",
                    "CoffeePOS could not inspect disposable MariaDB while cleaning backup verification.",
                    "The disposable process was terminated. Retry the backup operation from the beginning.",
                ));
            }
        }
        let args = root_client_arguments(self.port, "SHUTDOWN");
        let command_result =
            run_client_capture(runtime_manager, runtime, &args, cancelled, CLIENT_TIMEOUT);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let process_state = self.child_mut()?.try_wait();
            match process_state {
                Ok(Some(_)) => {
                    self.child.take();
                    return command_result.map(|_| ());
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
                _ => {
                    self.terminate_for_cleanup(runtime_manager, "verify")?;
                    return command_result.map(|_| ());
                }
            }
        }
    }
}

impl Drop for DisposableDatabase {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = terminate_child(child, "verify");
            }
        }
    }
}

fn initialize_disposable_datadir(
    runtime_manager: &mut RuntimeManager,
    runtime: &ResolvedRuntime,
    datadir: &Path,
    cancelled: &AtomicBool,
) -> Result<(), BackupDatabaseErrorInfo> {
    fs::create_dir(datadir).map_err(|_| {
        backup_error(
            "verify",
            "verify_staging_failed",
            "CoffeePOS could not create the disposable MariaDB verification directory.",
            "Check application-data permissions and free disk space, then retry backup.",
        )
    })?;
    let mut command = Command::new(&runtime.mariadb_install_db_executable);
    command
        .arg(format!("--datadir={}", datadir.to_string_lossy()))
        .arg("--silent")
        .env_remove("MYSQL_PWD")
        .env_remove("MYSQL_HOME")
        .env_remove("MARIADB_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(&runtime.mariadb_base_dir);
    configure_child_command(&mut command);
    let child = command.spawn().map_err(|_| {
        backup_error(
            "verify",
            "verify_initialize_failed",
            "CoffeePOS could not start the pinned MariaDB initializer for dump verification.",
            "Repair the pinned MariaDB runtime and retry backup.",
        )
    })?;
    let child = contain_child_or_cleanup(
        runtime_manager,
        child,
        "verify",
        "verify_initialize_containment_failed",
    )?;
    let status =
        wait_owned_backup_child(runtime_manager, child, VERIFY_TIMEOUT, cancelled, "verify")?;
    if !status.success() || !datadir.join("mysql").is_dir() {
        return Err(backup_error(
            "verify",
            "verify_initialize_failed",
            "The pinned MariaDB initializer did not create a usable disposable verification database.",
            "Repair the pinned MariaDB runtime and retry backup.",
        ));
    }
    Ok(())
}

fn import_dump(
    runtime_manager: &mut RuntimeManager,
    runtime: &ResolvedRuntime,
    port: u16,
    dump_path: &Path,
    cancelled: &AtomicBool,
) -> Result<(), BackupDatabaseErrorInfo> {
    let input = File::open(dump_path).map_err(|_| {
        backup_error(
            "verify",
            "dump_unreadable",
            "CoffeePOS could not reopen the SQL dump for disposable import verification.",
            "Retry backup after checking application-data storage.",
        )
    })?;
    let mut command = Command::new(&runtime.mariadb_import_executable);
    command
        .args(root_connection_arguments(port))
        .env_remove("MYSQL_PWD")
        .env_remove("MYSQL_HOME")
        .env_remove("MARIADB_HOME")
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(&runtime.mariadb_base_dir);
    configure_child_command(&mut command);
    let child = command.spawn().map_err(|_| {
        backup_error(
            "verify",
            "import_start_failed",
            "CoffeePOS could not start the pinned MariaDB import tool for verification.",
            "Repair the managed MariaDB runtime and retry backup.",
        )
    })?;
    let child = contain_child_or_cleanup(
        runtime_manager,
        child,
        "verify",
        "import_containment_failed",
    )?;
    let status =
        wait_owned_backup_child(runtime_manager, child, VERIFY_TIMEOUT, cancelled, "verify")?;
    if status.success() {
        Ok(())
    } else {
        Err(backup_error(
            "verify",
            "import_failed",
            "The disposable MariaDB import rejected the logical dump.",
            "CoffeePOS discarded this snapshot candidate. Check database health and retry backup.",
        ))
    }
}

fn capture_fingerprint(
    runtime_manager: &mut RuntimeManager,
    runtime: &ResolvedRuntime,
    option_file: Option<&Path>,
    port: u16,
    cancelled: &AtomicBool,
) -> Result<DatabaseSnapshotFingerprint, BackupDatabaseErrorInfo> {
    let mut query = |sql: &str| -> Result<String, BackupDatabaseErrorInfo> {
        let mut args = if let Some(option_file) = option_file {
            vec![OsString::from(format!(
                "--defaults-extra-file={}",
                option_file.to_string_lossy()
            ))]
        } else {
            root_connection_arguments(port)
        };
        if option_file.is_some() {
            args.push(OsString::from(format!("--database={DATABASE_NAME}")));
            args.push(OsString::from("--batch"));
            args.push(OsString::from("--skip-column-names"));
        }
        args.push(OsString::from(format!("--execute={sql}")));
        let bytes = run_client_capture(runtime_manager, runtime, &args, cancelled, CLIENT_TIMEOUT)?;
        String::from_utf8(bytes)
            .map(|value| value.trim().to_string())
            .map_err(|_| {
                backup_error(
                    "verify",
                    "verify_output_invalid",
                    "MariaDB returned invalid text while CoffeePOS compared backup invariants.",
                    "Retry backup after checking the managed MariaDB runtime.",
                )
            })
    };

    let woocommerce_orders_present = query(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='coffeepos' AND table_name='wp_wc_orders'",
    )? == "1";
    let (woocommerce_order_rows, woocommerce_order_digest) = if woocommerce_orders_present {
        let rows = query("SELECT COUNT(*) FROM coffeepos.wp_wc_orders")?;
        let selected = query(
            "SELECT SHA2(CONCAT_WS(0x1F, CAST(id AS CHAR), COALESCE(status,''), COALESCE(CAST(total_amount AS CHAR),'NULL')),256) FROM coffeepos.wp_wc_orders ORDER BY id LIMIT 128",
        )?;
        (rows, sha256_text(&selected))
    } else {
        ("absent".into(), "absent".into())
    };

    let coffeepos_shifts_present = query(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='coffeepos' AND table_name='wp_coffeepos_shifts'",
    )? == "1";
    let (coffeepos_shift_rows, coffeepos_shift_digest) = if coffeepos_shifts_present {
        let rows = query("SELECT COUNT(*) FROM coffeepos.wp_coffeepos_shifts")?;
        let selected = query(
            "SELECT SHA2(CONCAT_WS(0x1F, CAST(id AS CHAR), CAST(user_id AS CHAR), COALESCE(status,''), COALESCE(CAST(opening_cash AS CHAR),'NULL'), COALESCE(CAST(actual_cash AS CHAR),'NULL')),256) FROM coffeepos.wp_coffeepos_shifts ORDER BY id LIMIT 128",
        )?;
        (rows, sha256_text(&selected))
    } else {
        ("absent".into(), "absent".into())
    };

    let coffeepos_suspended_carts_present = query(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='coffeepos' AND table_name='wp_coffeepos_suspended_carts'",
    )? == "1";
    let (coffeepos_suspended_cart_rows, coffeepos_suspended_cart_digest) =
        if coffeepos_suspended_carts_present {
            let rows = query("SELECT COUNT(*) FROM coffeepos.wp_coffeepos_suspended_carts")?;
            let selected = query(
                "SELECT SHA2(CONCAT_WS(0x1F, CAST(id AS CHAR), CAST(user_id AS CHAR), COALESCE(label,''), COALESCE(SHA2(cart_payload,256),''), COALESCE(CAST(created_at AS CHAR),''), COALESCE(CAST(updated_at AS CHAR),'')),256) FROM coffeepos.wp_coffeepos_suspended_carts ORDER BY id LIMIT 128",
            )?;
            (rows, sha256_text(&selected))
        } else {
            ("absent".into(), "absent".into())
        };

    Ok(DatabaseSnapshotFingerprint {
        tables: query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema='coffeepos' ORDER BY table_name",
        )?,
        triggers: query(
            "SELECT trigger_name FROM information_schema.triggers WHERE trigger_schema='coffeepos' ORDER BY trigger_name",
        )?,
        wp_options_rows: query("SELECT COUNT(*) FROM coffeepos.wp_options")?,
        wp_users_rows: query("SELECT COUNT(*) FROM coffeepos.wp_users")?,
        wp_usermeta_rows: query("SELECT COUNT(*) FROM coffeepos.wp_usermeta")?,
        store_name_hex: query("SELECT COALESCE(HEX((SELECT option_value FROM coffeepos.wp_options WHERE option_name='coffeepos_store_name' ORDER BY option_id LIMIT 1)), '')")?,
        woocommerce_table_count: query("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='coffeepos' AND (LEFT(table_name, 6)='wp_wc_' OR LEFT(table_name, 15)='wp_woocommerce_')")?,
        woocommerce_order_rows,
        woocommerce_order_digest,
        coffeepos_table_count: query("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='coffeepos' AND LEFT(table_name, 13)='wp_coffeepos_'")?,
        coffeepos_option_count: query("SELECT COUNT(*) FROM coffeepos.wp_options WHERE LEFT(option_name, 10)='coffeepos_'")?,
        coffeepos_db_version_hex: query("SELECT COALESCE(HEX((SELECT option_value FROM coffeepos.wp_options WHERE option_name='coffeepos_db_version' ORDER BY option_id LIMIT 1)), '')")?,
        coffeepos_shift_rows,
        coffeepos_shift_digest,
        coffeepos_suspended_cart_rows,
        coffeepos_suspended_cart_digest,
    })
}

fn sha256_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn root_connection_arguments(port: u16) -> Vec<OsString> {
    vec![
        OsString::from("--no-defaults"),
        OsString::from("--protocol=tcp"),
        OsString::from(format!("--host={LOOPBACK}")),
        OsString::from(format!("--port={port}")),
        OsString::from("--user=root"),
        OsString::from("--connect-timeout=1"),
        OsString::from("--batch"),
        OsString::from("--skip-column-names"),
    ]
}

fn root_client_arguments(port: u16, sql: &str) -> Vec<OsString> {
    let mut args = root_connection_arguments(port);
    args.push(OsString::from(format!("--execute={sql}")));
    args
}

fn run_client_capture(
    runtime_manager: &mut RuntimeManager,
    runtime: &ResolvedRuntime,
    args: &[OsString],
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<Vec<u8>, BackupDatabaseErrorInfo> {
    let mut command = Command::new(&runtime.mariadb_import_executable);
    command
        .args(args)
        .env_remove("MYSQL_PWD")
        .env_remove("MYSQL_HOME")
        .env_remove("MARIADB_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .current_dir(&runtime.mariadb_base_dir);
    configure_child_command(&mut command);
    let child = command.spawn().map_err(|_| {
        backup_error(
            "verify",
            "client_start_failed",
            "CoffeePOS could not start the pinned MariaDB client for backup verification.",
            "Repair the managed MariaDB runtime and retry backup.",
        )
    })?;
    let mut child = contain_child_or_cleanup(
        runtime_manager,
        child,
        "verify",
        "client_containment_failed",
    )?;
    let stdout = child.stdout.take().ok_or_else(|| {
        backup_error(
            "verify",
            "client_output_unavailable",
            "CoffeePOS could not capture MariaDB verification output.",
            "Retry backup after checking the managed runtime.",
        )
    })?;
    let reader = thread::spawn(move || read_bounded(stdout, MAX_VERIFY_QUERY_BYTES));
    let status = wait_owned_backup_child(runtime_manager, child, timeout, cancelled, "verify")?;
    let bytes = reader.join().map_err(|_| {
        backup_error(
            "verify",
            "client_output_unavailable",
            "CoffeePOS could not collect MariaDB verification output.",
            "Retry backup after checking the managed runtime.",
        )
    })??;
    if status.success() {
        Ok(bytes)
    } else {
        Err(backup_error(
            "verify",
            "client_failed",
            "The pinned MariaDB client rejected a backup verification query.",
            "Check database health and retry backup. Query output was not exposed.",
        ))
    }
}

fn read_bounded(
    mut reader: impl Read,
    max_bytes: usize,
) -> Result<Vec<u8>, BackupDatabaseErrorInfo> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| {
            backup_error(
                "verify",
                "client_output_unavailable",
                "CoffeePOS could not read MariaDB verification output.",
                "Retry backup after checking the managed runtime.",
            )
        })?;
        if read == 0 {
            break;
        }
        if output.len().saturating_add(read) > max_bytes {
            return Err(backup_error(
                "verify",
                "client_output_too_large",
                "MariaDB verification output exceeded the bounded safety limit.",
                "Inspect the database schema for an unexpected number of managed objects before retrying backup.",
            ));
        }
        output.extend_from_slice(&buffer[..read]);
    }
    Ok(output)
}

fn remove_private_tree(path: &Path) -> Result<(), BackupDatabaseErrorInfo> {
    if !path.exists() {
        return Ok(());
    }
    reject_reparse_point(path)?;
    fs::remove_dir_all(path).map_err(|_| {
        backup_error(
            "cleanup",
            "verify_staging_cleanup_failed",
            "CoffeePOS could not remove disposable MariaDB verification data.",
            "Close processes using backup verification staging before retrying.",
        )
    })
}

struct PrivateOptionFile {
    path: PathBuf,
}

impl PrivateOptionFile {
    fn create(
        staging_dir: &Path,
        port: u16,
        user: &str,
        password: &str,
    ) -> Result<Self, BackupDatabaseErrorInfo> {
        let path = staging_dir.join(OPTION_FILE_NAME);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| {
                backup_error(
                    "dump",
                    "credential_staging_failed",
                    "CoffeePOS could not create the private MariaDB client option file.",
                    "Check application-data permissions, then retry backup.",
                )
            })?;
        let contents = format!(
            "[client]\nprotocol=tcp\nhost={LOOPBACK}\nport={port}\nuser={}\npassword={}\ndefault-character-set=utf8mb4\n",
            quote_option_value(user),
            quote_option_value(password)
        );
        if let Err(error) = file
            .write_all(contents.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = fs::remove_file(&path);
            return Err(backup_error(
                "dump",
                "credential_staging_failed",
                "CoffeePOS could not write the private MariaDB client option file.",
                &format!("Check application-data storage and retry backup. OS error: {error}."),
            ));
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateOptionFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn require_managed_backup_tools(runtime: &ResolvedRuntime) -> Result<(), BackupDatabaseErrorInfo> {
    for (path, label) in [
        (&runtime.mariadb_dump_executable, "dump"),
        (&runtime.mariadb_import_executable, "import"),
    ] {
        if !path.is_file() {
            return Err(backup_error(
                "quiesce",
                "managed_tool_missing",
                &format!("The pinned MariaDB {label} tool is missing from the managed runtime."),
                "Restage or repair the exact managed MariaDB runtime artifact before retrying backup.",
            ));
        }
        if !path.starts_with(&runtime.mariadb_base_dir) {
            return Err(backup_error(
                "quiesce",
                "managed_tool_invalid",
                &format!(
                    "The pinned MariaDB {label} tool resolved outside the managed MariaDB runtime."
                ),
                "Repair the runtime manifest and staged artifact before retrying backup.",
            ));
        }
    }
    Ok(())
}

fn prepare_staging_dir(
    data_root: &Path,
    operation_id: &str,
) -> Result<PathBuf, BackupDatabaseErrorInfo> {
    let root = ensure_staging_root(data_root)?;
    let staging = root.join(format!("db-{operation_id}"));
    fs::create_dir(&staging).map_err(|_| {
        backup_error(
            "quiesce",
            "staging_unavailable",
            "CoffeePOS could not create a unique database-backup staging directory.",
            "Retry backup after checking application-data permissions.",
        )
    })?;
    if let Err(error) = restrict_private_directory(&staging) {
        let _ = fs::remove_dir(&staging);
        return Err(error);
    }
    if let Err(error) = fs::write(staging.join(OWNERSHIP_MARKER), OWNERSHIP_MARKER_VALUE) {
        let _ = fs::remove_dir(&staging);
        return Err(backup_error(
            "quiesce",
            "staging_unavailable",
            "CoffeePOS could not mark ownership of its database-backup staging directory.",
            &format!("Check application-data storage and retry backup. OS error: {error}."),
        ));
    }
    Ok(staging)
}

#[cfg(windows)]
fn restrict_private_directory(path: &Path) -> Result<(), BackupDatabaseErrorInfo> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, GENERIC_ALL, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        BuildTrusteeWithSidW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
        SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
        OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct LocalAcl(*mut windows_sys::Win32::Security::ACL);
    impl Drop for LocalAcl {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0.cast::<c_void>());
                }
            }
        }
    }

    let mut token: HANDLE = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(private_acl_error());
    }
    let _token = HandleGuard(token);

    let mut required = 0u32;
    unsafe {
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required);
    }
    if required < std::mem::size_of::<TOKEN_USER>() as u32 {
        return Err(private_acl_error());
    }
    let mut buffer = vec![0u8; required as usize];
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(private_acl_error());
    }
    let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    if token_user.User.Sid.is_null() {
        return Err(private_acl_error());
    }

    let mut trustee = TRUSTEE_W::default();
    unsafe {
        BuildTrusteeWithSidW(&mut trustee, token_user.User.Sid);
    }
    let explicit = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: SET_ACCESS,
        grfInheritance: OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
        Trustee: trustee,
    };
    let mut acl = ptr::null_mut();
    if unsafe { SetEntriesInAclW(1, &explicit, ptr::null(), &mut acl) } != 0 || acl.is_null() {
        return Err(private_acl_error());
    }
    let _acl = LocalAcl(acl);
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let result = unsafe {
        SetNamedSecurityInfoW(
            wide.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    if result != 0 {
        return Err(private_acl_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn restrict_private_directory(_path: &Path) -> Result<(), BackupDatabaseErrorInfo> {
    Ok(())
}

fn private_acl_error() -> BackupDatabaseErrorInfo {
    backup_error(
        "quiesce",
        "staging_acl_failed",
        "CoffeePOS could not restrict database-backup staging to the current operating-system user.",
        "Repair application-data permissions before retrying; no plaintext database dump was created.",
    )
}

fn cleanup_stale_owned_staging(data_root: &Path) -> Result<(), BackupDatabaseErrorInfo> {
    let Some(root) = existing_staging_root(data_root)? else {
        return Ok(());
    };
    let entries = fs::read_dir(&root).map_err(|_| {
        backup_error(
            "cleanup",
            "stale_staging_unreadable",
            "CoffeePOS could not inspect stale database-backup staging data.",
            "Check application-data permissions before retrying backup.",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            backup_error(
                "cleanup",
                "stale_staging_unreadable",
                "CoffeePOS could not inspect a database-backup staging entry.",
                "Check application-data permissions before retrying backup.",
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("db-") {
            continue;
        }
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        reject_reparse_point(&path)?;
        let marker = path.join(OWNERSHIP_MARKER);
        if fs::read(&marker).ok().as_deref() != Some(OWNERSHIP_MARKER_VALUE) {
            continue;
        }
        let operation_id = &name[3..];
        if operation_id.len() == 32
            && operation_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            recover_interrupted_backup_children(data_root, operation_id)?;
        } else {
            return Err(backup_error(
                "cleanup",
                "stale_staging_identity_invalid",
                "CoffeePOS found owned database-backup staging with an invalid operation identifier.",
                "Do not delete it automatically; inspect the owned staging before starting another backup.",
            ));
        }
        fs::remove_dir_all(&path).map_err(|_| {
            backup_error(
                "cleanup",
                "stale_staging_cleanup_failed",
                "CoffeePOS found owned stale database-backup staging but could not remove it safely.",
                "Close processes using CoffeePOS backup staging and retry.",
            )
        })?;
    }
    Ok(())
}

fn remove_owned_staging_dir(
    data_root: &Path,
    staging_dir: &Path,
) -> Result<(), BackupDatabaseErrorInfo> {
    if !staging_dir.exists() {
        return Ok(());
    }
    let Some(root) = existing_staging_root(data_root)? else {
        return Err(backup_error(
            "cleanup",
            "staging_path_invalid",
            "CoffeePOS could not resolve the database-backup staging root safely.",
            "Do not delete unknown paths; repair application-data storage and retry cleanup.",
        ));
    };
    reject_reparse_point(staging_dir)?;
    let canonical_root = fs::canonicalize(&root).map_err(|_| staging_path_error())?;
    let canonical_staging = fs::canonicalize(staging_dir).map_err(|_| {
        backup_error(
            "cleanup",
            "staging_path_invalid",
            "CoffeePOS could not resolve its database-backup staging directory safely.",
            "Do not delete unknown paths; repair application-data storage and retry cleanup.",
        )
    })?;
    if canonical_staging.parent() != Some(canonical_root.as_path())
        || fs::read(canonical_staging.join(OWNERSHIP_MARKER))
            .ok()
            .as_deref()
            != Some(OWNERSHIP_MARKER_VALUE)
    {
        return Err(backup_error(
            "cleanup",
            "staging_ownership_invalid",
            "CoffeePOS refused to remove a database-backup staging path without valid ownership evidence.",
            "Inspect the backup staging directory manually and preserve unknown data.",
        ));
    }
    fs::remove_dir_all(&canonical_staging).map_err(|_| {
        backup_error(
            "cleanup",
            "staging_cleanup_failed",
            "CoffeePOS could not remove the private database-backup staging directory.",
            "Close processes using the staging files and retry cleanup before another backup.",
        )
    })
}

pub(crate) fn cleanup_interrupted_database_staging(
    data_root: &Path,
    operation_id: &str,
) -> Result<(), BackupDatabaseErrorInfo> {
    if operation_id.len() != 32
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(backup_error(
            "cleanup",
            "invalid_recovery_operation_id",
            "CoffeePOS refused an invalid database-backup recovery identifier.",
            "Keep the recovery marker for inspection and do not delete unrelated backup staging.",
        ));
    }
    let staging_dir = data_root
        .join(STAGING_ROOT)
        .join(format!("db-{operation_id}"));
    remove_owned_staging_dir(data_root, &staging_dir)
}

pub(crate) fn recover_interrupted_backup_children(
    data_root: &Path,
    operation_id: &str,
) -> Result<(), BackupDatabaseErrorInfo> {
    if operation_id.len() != 32
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(backup_error(
            "cleanup",
            "invalid_recovery_operation_id",
            "CoffeePOS refused an invalid backup-child recovery identifier.",
            "Keep the recovery marker for inspection and do not start the managed runtime.",
        ));
    }
    let staging_dir = data_root
        .join(STAGING_ROOT)
        .join(format!("db-{operation_id}"));
    if !staging_dir.exists() {
        return Ok(());
    }
    let Some(root) = existing_staging_root(data_root)? else {
        return Err(staging_path_error());
    };
    reject_reparse_point(&staging_dir)?;
    let canonical_root = fs::canonicalize(&root).map_err(|_| staging_path_error())?;
    let canonical_staging = fs::canonicalize(&staging_dir).map_err(|_| staging_path_error())?;
    if canonical_staging.parent() != Some(canonical_root.as_path())
        || fs::read(canonical_staging.join(OWNERSHIP_MARKER))
            .ok()
            .as_deref()
            != Some(OWNERSHIP_MARKER_VALUE)
    {
        return Err(staging_path_error());
    }

    let mut records = Vec::new();
    let mut pending_pids = Vec::new();
    for entry in fs::read_dir(&canonical_staging).map_err(|_| child_recovery_scan_error())? {
        let entry = entry.map_err(|_| child_recovery_scan_error())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(CHILD_RECOVERY_PREFIX) {
            continue;
        }
        let path = entry.path();
        reject_reparse_point(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| child_recovery_scan_error())?;
        if !metadata.file_type().is_file() || metadata.len() > 64 * 1024 {
            return Err(child_recovery_scan_error());
        }
        if name.ends_with(CHILD_RECOVERY_JSON_SUFFIX) {
            let bytes = fs::read(&path).map_err(|_| child_recovery_scan_error())?;
            let record: BackupChildRecoveryRecord =
                serde_json::from_slice(&bytes).map_err(|_| child_recovery_scan_error())?;
            if record.schema_version != CHILD_RECOVERY_SCHEMA_VERSION
                || record.pid == 0
                || record.creation_time == 0
                || record.image_path.is_empty()
                || record.image_path.chars().any(char::is_control)
            {
                return Err(child_recovery_scan_error());
            }
            records.push(record);
        } else if let Some(pid) = name
            .strip_prefix(CHILD_RECOVERY_PREFIX)
            .and_then(|value| value.strip_suffix(CHILD_RECOVERY_PENDING_SUFFIX))
            .and_then(|value| value.parse::<u32>().ok())
        {
            pending_pids.push(pid);
        } else {
            return Err(child_recovery_scan_error());
        }
    }

    for record in &records {
        recover_recorded_backup_child(record)?;
    }
    for pid in pending_pids {
        if !records.iter().any(|record| record.pid == pid) {
            return Err(backup_error(
                "cleanup",
                "child_recovery_evidence_incomplete",
                "CoffeePOS found an incomplete durable backup-child recovery record.",
                "Managed startup remains blocked because CoffeePOS cannot safely prove whether that child process survived the interrupted backup.",
            ));
        }
    }
    Ok(())
}

fn child_recovery_scan_error() -> BackupDatabaseErrorInfo {
    backup_error(
        "cleanup",
        "child_recovery_evidence_invalid",
        "CoffeePOS cannot safely validate durable backup-child recovery evidence.",
        "Keep the managed runtime stopped and inspect the owned database-backup staging before retrying recovery.",
    )
}

#[cfg(windows)]
fn recover_recorded_backup_child(
    record: &BackupChildRecoveryRecord,
) -> Result<(), BackupDatabaseErrorInfo> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
        WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };

    struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
            0,
            record.pid,
        )
    };
    if handle.is_null() {
        if std::io::Error::last_os_error().raw_os_error() == Some(87) {
            return Ok(());
        }
        return Err(child_recovery_process_error());
    }
    let handle = HandleGuard(handle);
    match unsafe { WaitForSingleObject(handle.0, 0) } {
        WAIT_OBJECT_0 => return Ok(()),
        WAIT_TIMEOUT => {}
        _ => return Err(child_recovery_process_error()),
    }

    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) } == 0
    {
        return Err(child_recovery_process_error());
    }
    let mut image = vec![0_u16; 32_768];
    let mut image_len = image.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, image.as_mut_ptr(), &mut image_len) } == 0
        || image_len == 0
    {
        return Err(child_recovery_process_error());
    }
    let image_path = String::from_utf16_lossy(&image[..image_len as usize]);
    if filetime_u64(created) != record.creation_time
        || !image_path.eq_ignore_ascii_case(&record.image_path)
    {
        // The recorded process object has exited and Windows reused the PID. Never terminate a
        // different process merely because its numeric PID matches stale recovery evidence.
        return Ok(());
    }

    let _ = unsafe { TerminateProcess(handle.0, 1) };
    match unsafe { WaitForSingleObject(handle.0, 5_000) } {
        WAIT_OBJECT_0 => Ok(()),
        _ => Err(child_recovery_process_error()),
    }
}

#[cfg(not(windows))]
fn recover_recorded_backup_child(
    _record: &BackupChildRecoveryRecord,
) -> Result<(), BackupDatabaseErrorInfo> {
    Err(child_recovery_process_error())
}

fn child_recovery_process_error() -> BackupDatabaseErrorInfo {
    backup_error(
        "cleanup",
        "child_cleanup_unconfirmed",
        "CoffeePOS could not safely confirm termination of a recorded backup child process after interrupted-backup recovery.",
        "Managed startup remains blocked. Close the recorded managed backup process, then retry recovery.",
    )
}

fn ensure_staging_root(data_root: &Path) -> Result<PathBuf, BackupDatabaseErrorInfo> {
    let canonical_data_root = validate_data_root(data_root)?;
    let backups = data_root.join("backups");
    if !backups.exists() {
        fs::create_dir(&backups).map_err(|_| {
            backup_error(
                "quiesce",
                "staging_unavailable",
                "CoffeePOS could not create the managed backups directory.",
                "Check application-data permissions and free disk space, then retry backup.",
            )
        })?;
    }
    let canonical_backups = validate_direct_child(&canonical_data_root, &backups)?;
    let root = backups.join(".database-staging");
    if !root.exists() {
        fs::create_dir(&root).map_err(|_| {
            backup_error(
                "quiesce",
                "staging_unavailable",
                "CoffeePOS could not create the private database-backup staging root.",
                "Check application-data permissions and free disk space, then retry backup.",
            )
        })?;
    }
    validate_direct_child(&canonical_backups, &root)?;
    Ok(root)
}

fn existing_staging_root(data_root: &Path) -> Result<Option<PathBuf>, BackupDatabaseErrorInfo> {
    let canonical_data_root = validate_data_root(data_root)?;
    let backups = data_root.join("backups");
    if !backups.exists() {
        return Ok(None);
    }
    let canonical_backups = validate_direct_child(&canonical_data_root, &backups)?;
    let root = backups.join(".database-staging");
    if !root.exists() {
        return Ok(None);
    }
    validate_direct_child(&canonical_backups, &root)?;
    Ok(Some(root))
}

fn validate_data_root(data_root: &Path) -> Result<PathBuf, BackupDatabaseErrorInfo> {
    reject_reparse_point(data_root)?;
    fs::canonicalize(data_root).map_err(|_| staging_path_error())
}

fn validate_direct_child(
    canonical_parent: &Path,
    child: &Path,
) -> Result<PathBuf, BackupDatabaseErrorInfo> {
    reject_reparse_point(child)?;
    let canonical_child = fs::canonicalize(child).map_err(|_| staging_path_error())?;
    if canonical_child.parent() != Some(canonical_parent) {
        return Err(staging_path_error());
    }
    Ok(canonical_child)
}

fn staging_path_error() -> BackupDatabaseErrorInfo {
    backup_error(
        "cleanup",
        "staging_path_invalid",
        "CoffeePOS refused a database-backup staging path outside the managed application-data tree.",
        "Restore the managed backups path to normal local directories before retrying backup.",
    )
}

fn reject_reparse_point(path: &Path) -> Result<(), BackupDatabaseErrorInfo> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        backup_error(
            "cleanup",
            "staging_path_invalid",
            "CoffeePOS could not inspect a database-backup staging path safely.",
            "Repair application-data storage before retrying backup.",
        )
    })?;
    if metadata.file_type().is_symlink() || is_windows_reparse_point(&metadata) {
        return Err(backup_error(
            "cleanup",
            "staging_reparse_point",
            "CoffeePOS refused to use a reparse-point database-backup staging path.",
            "Restore the managed backups directory to a normal local directory before retrying.",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn dump_arguments(option_file: &Path) -> Vec<OsString> {
    vec![
        OsString::from(format!(
            "--defaults-extra-file={}",
            option_file.to_string_lossy()
        )),
        OsString::from("--single-transaction"),
        OsString::from("--quick"),
        OsString::from("--skip-lock-tables"),
        OsString::from("--default-character-set=utf8mb4"),
        OsString::from("--hex-blob"),
        OsString::from("--triggers"),
        OsString::from("--routines"),
        OsString::from("--events"),
        OsString::from("--skip-comments"),
        OsString::from("--skip-dump-date"),
        OsString::from("--databases"),
        OsString::from(DATABASE_NAME),
    ]
}

fn quote_option_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{0008}' => escaped.push_str("\\b"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

fn wait_child_bounded(
    child: &mut Child,
    timeout: Duration,
    cancelled: &AtomicBool,
    action: &str,
) -> Result<ExitStatus, BackupDatabaseErrorInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled.load(Ordering::Acquire) {
            terminate_child(child, action)?;
            return Err(cancelled_error(action));
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(_) => {
                terminate_child(child, action)?;
                return Err(backup_error(
                    action,
                    "child_state_unavailable",
                    "CoffeePOS could not inspect the MariaDB backup child process state.",
                    "Confirm no backup child remains, then retry the operation.",
                ));
            }
        }
        if Instant::now() >= deadline {
            terminate_child(child, action)?;
            return Err(backup_error(
                action,
                "child_timeout",
                "The MariaDB backup child process exceeded its bounded execution time.",
                "The child was terminated. Check database health and retry backup.",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_owned_backup_child(
    runtime_manager: &mut RuntimeManager,
    mut child: Child,
    timeout: Duration,
    cancelled: &AtomicBool,
    action: &str,
) -> Result<ExitStatus, BackupDatabaseErrorInfo> {
    match wait_child_bounded(&mut child, timeout, cancelled, action) {
        Ok(status) => Ok(status),
        Err(error) if error.code == "child_cleanup_unconfirmed" => {
            runtime_manager.retain_backup_recovery_child(child);
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn contain_child_or_cleanup(
    runtime_manager: &mut RuntimeManager,
    mut child: Child,
    action: &str,
    containment_code: &str,
) -> Result<Child, BackupDatabaseErrorInfo> {
    if let Err(record_error) = record_backup_child_recovery(runtime_manager, &child) {
        return match terminate_child(&mut child, action) {
            Ok(()) => Err(record_error),
            Err(cleanup_error) => {
                runtime_manager.retain_backup_recovery_child(child);
                Err(backup_error(
                    "cleanup",
                    "child_cleanup_unconfirmed",
                    &format!(
                        "CoffeePOS could not durably record backup-child recovery evidence ({}) and could not confirm that the child stopped: {}",
                        record_error.message, cleanup_error.message
                    ),
                    "Backup maintenance remains fenced. Keep CoffeePOS open and retry retained cleanup; relaunch recovery will fail closed if durable child evidence is incomplete.",
                ))
            }
        };
    }
    match runtime_manager.contain_backup_child(&child) {
        Ok(()) => Ok(child),
        Err(containment_error) => match terminate_child(&mut child, action) {
            Ok(()) => Err(runtime_error(action, containment_code, containment_error)),
            Err(cleanup_error) => {
                runtime_manager.retain_backup_recovery_child(child);
                Err(backup_error(
                    "cleanup",
                    "child_cleanup_unconfirmed",
                    &format!(
                        "CoffeePOS could not attach a backup child to process containment ({}) and could not confirm that the child stopped: {}",
                        containment_error.message, cleanup_error.message
                    ),
                    "Backup maintenance remains fenced. Confirm the child process has stopped, then retry the retained backup cleanup.",
                ))
            }
        },
    }
}

fn record_backup_child_recovery(
    runtime_manager: &RuntimeManager,
    child: &Child,
) -> Result<(), BackupDatabaseErrorInfo> {
    let staging = runtime_manager.backup_recovery_staging_path().ok_or_else(|| {
        backup_error(
            "cleanup",
            "child_recovery_staging_missing",
            "CoffeePOS cannot persist backup-child recovery evidence because the owned staging context is unavailable.",
            "Keep backup maintenance fenced and retry cleanup before another backup.",
        )
    })?;
    let pid = child.id();
    let pending = staging.join(format!(
        "{CHILD_RECOVERY_PREFIX}{pid}{CHILD_RECOVERY_PENDING_SUFFIX}"
    ));
    let mut pending_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .map_err(|_| child_recovery_record_error())?;
    writeln!(pending_file, "{pid}").map_err(|_| child_recovery_record_error())?;
    pending_file
        .sync_all()
        .map_err(|_| child_recovery_record_error())?;

    let record = capture_backup_child_record(child)?;
    let record_path = staging.join(format!(
        "{CHILD_RECOVERY_PREFIX}{}-{}{CHILD_RECOVERY_JSON_SUFFIX}",
        record.pid, record.creation_time
    ));
    let encoded = serde_json::to_vec(&record).map_err(|_| child_recovery_record_error())?;
    let mut record_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&record_path)
        .map_err(|_| child_recovery_record_error())?;
    record_file
        .write_all(&encoded)
        .map_err(|_| child_recovery_record_error())?;
    record_file
        .write_all(b"\n")
        .map_err(|_| child_recovery_record_error())?;
    record_file
        .sync_all()
        .map_err(|_| child_recovery_record_error())?;
    fs::remove_file(&pending).map_err(|_| child_recovery_record_error())?;
    Ok(())
}

fn child_recovery_record_error() -> BackupDatabaseErrorInfo {
    backup_error(
        "cleanup",
        "child_recovery_record_failed",
        "CoffeePOS could not persist bounded recovery evidence for a backup child process.",
        "The operation will stop and keep backup maintenance fenced until child cleanup is confirmed.",
    )
}

#[cfg(windows)]
fn capture_backup_child_record(
    child: &Child,
) -> Result<BackupChildRecoveryRecord, BackupDatabaseErrorInfo> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetProcessTimes, QueryFullProcessImageNameW};

    let handle = child.as_raw_handle();
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(child_recovery_record_error());
    }
    let creation_time = filetime_u64(created);
    let mut image = vec![0_u16; 32_768];
    let mut image_len = image.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle, 0, image.as_mut_ptr(), &mut image_len) } == 0
        || image_len == 0
    {
        return Err(child_recovery_record_error());
    }
    let image_path = String::from_utf16_lossy(&image[..image_len as usize]);
    if image_path.is_empty() || image_path.chars().any(char::is_control) {
        return Err(child_recovery_record_error());
    }
    Ok(BackupChildRecoveryRecord {
        schema_version: CHILD_RECOVERY_SCHEMA_VERSION,
        pid: child.id(),
        creation_time,
        image_path,
    })
}

#[cfg(not(windows))]
fn capture_backup_child_record(
    _child: &Child,
) -> Result<BackupChildRecoveryRecord, BackupDatabaseErrorInfo> {
    Err(child_recovery_record_error())
}

#[cfg(windows)]
fn filetime_u64(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

fn terminate_child(child: &mut Child, action: &str) -> Result<(), BackupDatabaseErrorInfo> {
    let kill_error = child.kill().err().map(|error| error.to_string());
    confirm_child_exit_bounded(action, Duration::from_secs(5), kill_error, || {
        child.try_wait()
    })
}

fn confirm_child_exit_bounded<F>(
    action: &str,
    timeout: Duration,
    kill_error: Option<String>,
    mut inspect: F,
) -> Result<(), BackupDatabaseErrorInfo>
where
    F: FnMut() -> io::Result<Option<ExitStatus>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        match inspect() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let kill_detail = kill_error
                    .as_deref()
                    .map(|error| format!(" Termination request error: {error}."))
                    .unwrap_or_default();
                return Err(backup_error(
                    action,
                    "child_cleanup_unconfirmed",
                    &format!(
                        "CoffeePOS could not confirm that the MariaDB backup child exited within the cleanup timeout.{kill_detail}"
                    ),
                    "Backup maintenance remains fenced. Confirm the child process has stopped before restarting or retrying CoffeePOS.",
                ));
            }
            Err(error) => {
                let kill_detail = kill_error
                    .as_deref()
                    .map(|value| format!(" Termination request error: {value}."))
                    .unwrap_or_default();
                return Err(backup_error(
                    action,
                    "child_cleanup_unconfirmed",
                    &format!(
                        "CoffeePOS could not inspect the MariaDB backup child after requesting termination: {error}.{kill_detail}"
                    ),
                    "Backup maintenance remains fenced. Confirm the child process has stopped before restarting or retrying CoffeePOS.",
                ));
            }
        }
    }
}

fn spawn_discard_reader(mut reader: impl Read + Send + 'static) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
}

fn sha256_file(path: &Path) -> Result<String, BackupDatabaseErrorInfo> {
    let mut file = File::open(path).map_err(|_| {
        backup_error(
            "verify",
            "dump_unreadable",
            "CoffeePOS could not reopen the SQL dump for integrity hashing.",
            "Check application-data storage and retry backup.",
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            backup_error(
                "verify",
                "dump_unreadable",
                "CoffeePOS could not read the SQL dump while computing its integrity hash.",
                "Check application-data storage and retry backup.",
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn random_operation_id() -> Result<String, BackupDatabaseErrorInfo> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        backup_error(
            "quiesce",
            "operation_id_failed",
            "CoffeePOS could not create a secure database-backup operation id.",
            "Retry backup after checking operating-system random generation.",
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn cancelled_error(action: &str) -> BackupDatabaseErrorInfo {
    backup_error(
        action,
        "cancelled",
        "The database backup was cancelled.",
        "CoffeePOS will finish bounded cleanup before another backup can start.",
    )
}

fn runtime_error(
    action: &str,
    code: &str,
    error: crate::runtime::RuntimeErrorInfo,
) -> BackupDatabaseErrorInfo {
    backup_error(action, code, &error.message, &error.recovery)
}

fn backup_error(
    action: &str,
    code: &str,
    message: &str,
    recovery: &str,
) -> BackupDatabaseErrorInfo {
    BackupDatabaseErrorInfo {
        component: "backup_database".into(),
        action: action.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_database_dump_arguments_never_contain_password() {
        let password = "Phase72-Database-Password-Canary";
        let option_path = Path::new(r"C:\private\client.cnf");
        let args = dump_arguments(option_path);
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(rendered.contains("--defaults-extra-file="));
        assert!(!rendered.contains(password));
        assert!(rendered.contains("--databases coffeepos"));
        assert!(rendered.contains("--single-transaction"));
        assert!(rendered.contains("--hex-blob"));
    }

    #[test]
    fn backup_database_option_values_escape_control_syntax() {
        assert_eq!(quote_option_value("p\\\"a\nss"), "\"p\\\\\\\"a\\nss\"");
    }

    #[test]
    fn backup_database_stale_cleanup_requires_owned_marker() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let staging_root = data_root.join(STAGING_ROOT);
        fs::create_dir_all(&staging_root).unwrap();
        let owned = staging_root.join("db-00112233445566778899aabbccddeeff");
        let unknown = staging_root.join("db-unknown");
        fs::create_dir_all(&owned).unwrap();
        fs::create_dir_all(&unknown).unwrap();
        fs::write(owned.join(OWNERSHIP_MARKER), OWNERSHIP_MARKER_VALUE).unwrap();
        fs::write(owned.join("secret.tmp"), b"sensitive").unwrap();
        fs::write(unknown.join("keep.txt"), b"keep").unwrap();

        cleanup_stale_owned_staging(data_root).unwrap();

        assert!(!owned.exists());
        assert!(unknown.join("keep.txt").exists());
    }

    #[test]
    fn backup_database_incomplete_child_recovery_evidence_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let operation_id = "00112233445566778899aabbccddeeff";
        let staging = data_root
            .join(STAGING_ROOT)
            .join(format!("db-{operation_id}"));
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join(OWNERSHIP_MARKER), OWNERSHIP_MARKER_VALUE).unwrap();
        fs::write(
            staging.join(format!(
                "{CHILD_RECOVERY_PREFIX}4242{CHILD_RECOVERY_PENDING_SUFFIX}"
            )),
            b"4242\n",
        )
        .unwrap();

        let error = recover_interrupted_backup_children(data_root, operation_id).unwrap_err();
        assert_eq!(error.code, "child_recovery_evidence_incomplete");
        assert!(staging.exists());
    }

    #[test]
    fn backup_database_hashes_exact_dump_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let dump = temp.path().join("store.sql");
        fs::write(&dump, b"CREATE TABLE test (id INT);\n").unwrap();
        assert_eq!(
            sha256_file(&dump).unwrap(),
            "9b29b2b613f98d10c6b10ddde2a77b9e9070592ed02b5de31ae2e91b4e8f13e7"
        );
    }

    #[test]
    fn backup_database_unconfirmed_child_exit_is_cleanup_error() {
        let error = confirm_child_exit_bounded(
            "dump",
            Duration::ZERO,
            Some("injected termination failure".into()),
            || Ok::<Option<ExitStatus>, io::Error>(None),
        )
        .unwrap_err();
        assert_eq!(error.code, "child_cleanup_unconfirmed");
        assert!(error.message.contains("could not confirm"));
    }

    #[cfg(windows)]
    #[test]
    fn backup_database_private_staging_acl_can_be_applied() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join("private");
        fs::create_dir(&private).unwrap();
        restrict_private_directory(&private).unwrap();
        fs::write(private.join("probe.txt"), b"private").unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn backup_database_rejects_backups_parent_junction_escape() {
        let managed = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let data_root = managed.path().join("store");
        let external_backups = external.path().join("redirected-backups");
        fs::create_dir(&data_root).unwrap();
        fs::create_dir(&external_backups).unwrap();
        let backups = data_root.join("backups");

        let status = Command::new("cmd.exe")
            .arg("/D")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&backups)
            .arg(&external_backups)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "test junction creation failed");

        let result = prepare_staging_dir(&data_root, "junction-escape");
        let escaped = external_backups.join(".database-staging").exists();
        fs::remove_dir(&backups).unwrap();

        let error = result.unwrap_err();
        assert_eq!(error.code, "staging_reparse_point");
        assert!(!escaped);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the staged Windows MariaDB development runtime"]
    fn backup_database_real_dump_import_round_trip() {
        use crate::runtime::{choose_loopback_port, resolve_development_manifest, RuntimeManager};

        const WORDPRESS_PASSWORD: &str = "Phase72WpPassword_2026";
        const RUNTIME_PASSWORD: &str = "Phase72RuntimePassword_2026";
        const STORE_SENTINEL: &str = "Phase 7.2 Café 東京";

        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let manifest =
            project_root.join("runtime/development/x86_64-pc-windows-msvc/manifest.json");
        let resolved = resolve_development_manifest(&project_root, &manifest).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let target_root = temp.path().join("target");

        initialize_test_datadir(&resolved, &source_root.join("database"));
        fs::create_dir_all(source_root.join("site")).unwrap();
        fs::create_dir_all(source_root.join("config")).unwrap();
        secret::store_password(
            &source_root.join(crate::runtime::DATABASE_WORDPRESS_SECRET),
            WORDPRESS_PASSWORD,
        )
        .unwrap();
        secret::store_password(
            &source_root.join(crate::runtime::DATABASE_RUNTIME_SECRET),
            RUNTIME_PASSWORD,
        )
        .unwrap();

        let source_port = choose_loopback_port(&[]).unwrap();
        let mut source_db =
            TestMariaDb::start(&resolved, &source_root.join("database"), source_port);
        source_db.wait_root_ready(&resolved);
        let setup_sql = format!(
            "CREATE DATABASE `{DATABASE_NAME}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\n\
             CREATE USER '{DATABASE_WORDPRESS_USER}'@'127.0.0.1' IDENTIFIED BY '{WORDPRESS_PASSWORD}';\n\
             GRANT ALL PRIVILEGES ON `{DATABASE_NAME}`.* TO '{DATABASE_WORDPRESS_USER}'@'127.0.0.1';\n\
             CREATE USER '{}'@'127.0.0.1' IDENTIFIED BY '{RUNTIME_PASSWORD}';\n\
             GRANT SHUTDOWN ON *.* TO '{}'@'127.0.0.1';\n\
             USE `{DATABASE_NAME}`;\n\
             CREATE TABLE wp_options (option_id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, option_name VARCHAR(191) NOT NULL UNIQUE, option_value LONGTEXT NOT NULL, autoload VARCHAR(20) NOT NULL DEFAULT 'yes') ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n\
             INSERT INTO wp_options(option_name, option_value) VALUES ('coffeepos_store_name', '{}'), ('coffeepos_db_version', '0.0.2');\n\
             CREATE TABLE wp_users (ID BIGINT UNSIGNED NOT NULL PRIMARY KEY, user_login VARCHAR(60) NOT NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n\
             INSERT INTO wp_users VALUES (1, 'owner');\n\
             CREATE TABLE wp_usermeta (umeta_id BIGINT UNSIGNED NOT NULL PRIMARY KEY, user_id BIGINT UNSIGNED NOT NULL, meta_key VARCHAR(255), meta_value LONGTEXT) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n\
             INSERT INTO wp_usermeta VALUES (1, 1, 'wp_capabilities', 'administrator');\n\
             CREATE TABLE wp_wc_orders (id BIGINT UNSIGNED NOT NULL PRIMARY KEY, status VARCHAR(32) NOT NULL, total_amount DECIMAL(10,2) NOT NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n\
             CREATE TABLE wp_coffeepos_shifts (id BIGINT UNSIGNED NOT NULL PRIMARY KEY, user_id BIGINT UNSIGNED NOT NULL, status VARCHAR(20) NOT NULL, opening_cash DECIMAL(20,6) NOT NULL DEFAULT 0, actual_cash DECIMAL(20,6) NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n\
             INSERT INTO wp_coffeepos_shifts VALUES (1, 1, 'open', 100.000000, NULL);\n\
             CREATE TABLE wp_coffeepos_suspended_carts (id BIGINT UNSIGNED NOT NULL PRIMARY KEY, user_id BIGINT UNSIGNED NOT NULL, label VARCHAR(191) NOT NULL, cart_payload LONGTEXT NOT NULL, created_at DATETIME NOT NULL, updated_at DATETIME NOT NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n\
             INSERT INTO wp_coffeepos_suspended_carts VALUES (72001, 1, 'Phase 7.2 cart', '{{\"items\":[{{\"product_id\":72,\"qty\":2}}]}}', '2026-09-19 10:00:00', '2026-09-19 10:00:00');\n\
             CREATE TABLE wp_coffeepos_state (id BIGINT UNSIGNED NOT NULL PRIMARY KEY, state_key VARCHAR(64) NOT NULL, state_value VARCHAR(191) NOT NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n\
             INSERT INTO wp_coffeepos_state VALUES (1, 'phase72', 'sentinel-ok');\n\
             CREATE TABLE wp_binary_fixture (id BIGINT UNSIGNED NOT NULL PRIMARY KEY, payload LONGBLOB NOT NULL) ENGINE=InnoDB;\n\
             INSERT INTO wp_binary_fixture VALUES (1, X'000102FEFF');\n\
             CREATE TABLE wp_trigger_log (order_id BIGINT UNSIGNED NOT NULL PRIMARY KEY) ENGINE=InnoDB;\n\
             CREATE TRIGGER coffeepos_phase72_order AFTER INSERT ON wp_wc_orders FOR EACH ROW INSERT INTO wp_trigger_log(order_id) VALUES (NEW.id);\n\
             INSERT INTO wp_wc_orders VALUES (72001, 'wc-completed', 12.34);\n\
             FLUSH PRIVILEGES;",
            crate::runtime::DATABASE_RUNTIME_USER,
            crate::runtime::DATABASE_RUNTIME_USER,
            STORE_SENTINEL.replace('\\', "\\\\").replace('\'', "''")
        );
        run_root_sql(&resolved, source_port, &setup_sql);
        source_db.shutdown_root(&resolved);

        let mut manager = RuntimeManager::new(resolved.clone(), source_root.clone()).unwrap();
        assert_eq!(manager.info().state, crate::runtime::RuntimeState::Stopped);
        let cancelled = AtomicBool::new(false);
        let session = create_database_snapshot(&mut manager, &cancelled).unwrap();
        let artifact = session.artifact().unwrap();
        assert!(artifact.size_bytes > 0);
        assert_eq!(artifact.sha256.len(), 64);
        let dump_path = session.dump_path().to_path_buf();
        assert!(dump_path.is_file());

        initialize_test_datadir(&resolved, &target_root.join("database"));
        let target_port = choose_loopback_port(&[source_port]).unwrap();
        let mut target_db =
            TestMariaDb::start(&resolved, &target_root.join("database"), target_port);
        target_db.wait_root_ready(&resolved);
        import_dump_as_root(&resolved, target_port, &dump_path);

        assert_eq!(
            query_root_scalar(
                &resolved,
                target_port,
                "SELECT option_value FROM coffeepos.wp_options WHERE option_name='coffeepos_store_name'"
            ),
            STORE_SENTINEL
        );
        assert_eq!(
            query_root_scalar(
                &resolved,
                target_port,
                "SELECT CONCAT(status, ':', FORMAT(total_amount, 2)) FROM coffeepos.wp_wc_orders WHERE id=72001"
            ),
            "wc-completed:12.34"
        );
        assert_eq!(
            query_root_scalar(
                &resolved,
                target_port,
                "SELECT HEX(payload) FROM coffeepos.wp_binary_fixture WHERE id=1"
            ),
            "000102FEFF"
        );
        assert_eq!(
            query_root_scalar(
                &resolved,
                target_port,
                "SELECT state_value FROM coffeepos.wp_coffeepos_state WHERE state_key='phase72'"
            ),
            "sentinel-ok"
        );
        run_root_sql(
            &resolved,
            target_port,
            "INSERT INTO coffeepos.wp_wc_orders VALUES (72002, 'wc-processing', 4.56);",
        );
        assert_eq!(
            query_root_scalar(
                &resolved,
                target_port,
                "SELECT COUNT(*) FROM coffeepos.wp_trigger_log WHERE order_id IN (72001,72002)"
            ),
            "2"
        );
        let table_count = query_root_scalar(
            &resolved,
            target_port,
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='coffeepos'",
        )
        .parse::<u64>()
        .unwrap();
        assert!(table_count >= 7);
        target_db.shutdown_root(&resolved);

        let after = session.cleanup(&mut manager).unwrap();
        assert_eq!(after.state, crate::runtime::RuntimeState::Stopped);
        assert!(!dump_path.exists());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "provisions a disposable staged WordPress/WooCommerce/CoffeePOS store"]
    fn backup_database_disposable_store_running_and_stopped_smoke() {
        use crate::provisioning::{Provisioner, ProvisioningState};
        use crate::runtime::{resolve_development_manifest, RuntimeManager, RuntimeState};

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap().to_path_buf();
        let runtime_manifest =
            project_root.join("runtime/development/x86_64-pc-windows-msvc/manifest.json");
        let runtime = resolve_development_manifest(&project_root, &runtime_manifest).unwrap();
        let e2e_root = manifest_dir.join("target/phase7-2-e2e");
        fs::create_dir_all(&e2e_root).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("backup-store-")
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
        let mut manager = RuntimeManager::new(runtime.clone(), data_root.clone()).unwrap();
        let running = manager.start().unwrap();
        let installed = provisioner
            .install_wordpress("CoffeePOS Phase 7.2 Backup Smoke", &running)
            .unwrap();
        assert_eq!(installed.state, ProvisioningState::Ready);
        assert!(installed.woocommerce_active);
        assert!(installed.coffeepos_active);
        run_wordpress_test_sql(
            &runtime,
            &data_root,
            running.database_port.unwrap(),
            "INSERT INTO wp_coffeepos_suspended_carts (id, user_id, label, cart_payload, created_at, updated_at) VALUES (72002, 1, 'Phase72 live sentinel', '{\"items\":[{\"product_id\":72,\"quantity\":2}]}', UTC_TIMESTAMP(), UTC_TIMESTAMP());",
        );

        let cancelled = AtomicBool::new(false);
        let running_session = create_database_snapshot(&mut manager, &cancelled).unwrap();
        assert!(running_session.runtime_was_running());
        assert_eq!(running_session.stage(), BackupDatabaseStage::Verified);
        assert!(running_session.artifact().unwrap().size_bytes > 0);
        let running_dump_path = running_session.dump_path().to_path_buf();
        assert_eq!(manager.start().unwrap_err().operation, "start");
        assert_eq!(manager.restart().unwrap_err().operation, "restart");
        assert_eq!(manager.stop().unwrap_err().operation, "stop");
        let second_backup = create_database_snapshot(&mut manager, &cancelled)
            .err()
            .unwrap();
        assert_eq!(second_backup.code, "backup_in_progress");
        assert!(running_dump_path.is_file());
        let resumed = running_session.cleanup(&mut manager).unwrap();
        assert_eq!(resumed.state, RuntimeState::Running);
        assert!(resumed.database_pid.is_some());
        assert!(resumed.php_pid.is_some());
        assert!(resumed.web_server_pid.is_some());

        let stopped = manager.stop().unwrap();
        assert_eq!(stopped.state, RuntimeState::Stopped);
        assert!(stopped.database_pid.is_none());
        assert!(stopped.php_pid.is_none());
        assert!(stopped.web_server_pid.is_none());

        let stopped_session = create_database_snapshot(&mut manager, &cancelled).unwrap();
        assert!(!stopped_session.runtime_was_running());
        assert_eq!(stopped_session.stage(), BackupDatabaseStage::Verified);
        assert!(stopped_session.artifact().unwrap().size_bytes > 0);
        let mut recovery_child_command = Command::new("powershell.exe");
        recovery_child_command
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg("Start-Sleep -Seconds 30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_child_command(&mut recovery_child_command);
        let recovery_child = recovery_child_command.spawn().unwrap();
        manager.contain_backup_child(&recovery_child).unwrap();
        manager.retain_backup_recovery_child(recovery_child);
        drop(stopped_session);
        let stayed_stopped = retry_database_backup_cleanup(&mut manager).unwrap();
        assert_eq!(stayed_stopped.state, RuntimeState::Stopped);
        assert!(stayed_stopped.database_pid.is_none());
        assert!(stayed_stopped.php_pid.is_none());
        assert!(stayed_stopped.web_server_pid.is_none());
        let staging_root = data_root.join(STAGING_ROOT);
        assert_eq!(fs::read_dir(&staging_root).unwrap().count(), 0);

        let cancelled_before = AtomicBool::new(true);
        let cancelled_error = create_database_snapshot(&mut manager, &cancelled_before)
            .err()
            .unwrap();
        assert_eq!(cancelled_error.code, "cancelled");
        assert_eq!(manager.info().state, RuntimeState::Stopped);
        assert_eq!(fs::read_dir(&staging_root).unwrap().count(), 0);

        let wordpress_secret = data_root.join(crate::runtime::DATABASE_WORDPRESS_SECRET);
        let original_password = secret::load(&wordpress_secret).unwrap();
        secret::store_password(&wordpress_secret, "Phase72IntentionallyWrongPassword").unwrap();
        let wrong_credential = create_database_snapshot(&mut manager, &cancelled)
            .err()
            .unwrap();
        assert_eq!(wrong_credential.code, "database_credential_rejected");
        assert_eq!(manager.info().state, RuntimeState::Stopped);
        assert_eq!(fs::read_dir(&staging_root).unwrap().count(), 0);
        secret::store_password(&wordpress_secret, &original_password).unwrap();
    }

    #[cfg(windows)]
    fn initialize_test_datadir(runtime: &ResolvedRuntime, datadir: &Path) {
        fs::create_dir_all(datadir).unwrap();
        let mut command = Command::new(&runtime.mariadb_install_db_executable);
        command
            .arg(format!("--datadir={}", datadir.to_string_lossy()))
            .arg("--silent")
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(&runtime.mariadb_base_dir);
        configure_child_command(&mut command);
        let status = command.status().unwrap();
        assert!(status.success());
        assert!(datadir.join("mysql").is_dir());
    }

    #[cfg(windows)]
    struct TestMariaDb {
        child: Child,
        port: u16,
    }

    #[cfg(windows)]
    impl TestMariaDb {
        fn start(runtime: &ResolvedRuntime, datadir: &Path, port: u16) -> Self {
            let mut command = Command::new(&runtime.mariadb_executable);
            command
                .arg("--no-defaults")
                .arg(format!(
                    "--basedir={}",
                    runtime.mariadb_base_dir.to_string_lossy()
                ))
                .arg(format!("--datadir={}", datadir.to_string_lossy()))
                .arg(format!("--port={port}"))
                .arg(format!("--bind-address={LOOPBACK}"))
                .arg("--skip-name-resolve")
                .arg("--skip-log-bin")
                .arg(format!(
                    "--pid-file={}",
                    datadir.join("phase72-test.pid").to_string_lossy()
                ))
                .arg("--console")
                .env_remove("MYSQL_PWD")
                .env_remove("MYSQL_HOME")
                .env_remove("MARIADB_HOME")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .current_dir(&runtime.mariadb_base_dir);
            configure_child_command(&mut command);
            let child = command.spawn().unwrap();
            Self { child, port }
        }

        fn wait_root_ready(&mut self, runtime: &ResolvedRuntime) {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                assert!(self.child.try_wait().unwrap().is_none());
                let mut command = root_client_command(runtime, self.port);
                command
                    .arg("--execute=SELECT 1")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if command
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
                {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "test MariaDB readiness timed out"
                );
                thread::sleep(Duration::from_millis(100));
            }
        }

        fn shutdown_root(&mut self, runtime: &ResolvedRuntime) {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            let mut command = root_client_command(runtime, self.port);
            command
                .arg("--execute=SHUTDOWN")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let _ = command.status();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(50))
                    }
                    _ => {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        return;
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    impl Drop for TestMariaDb {
        fn drop(&mut self) {
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    #[cfg(windows)]
    fn root_client_command(runtime: &ResolvedRuntime, port: u16) -> Command {
        let mut command = Command::new(&runtime.mariadb_import_executable);
        command
            .arg("--no-defaults")
            .arg("--protocol=tcp")
            .arg(format!("--host={LOOPBACK}"))
            .arg(format!("--port={port}"))
            .arg("--user=root")
            .arg("--connect-timeout=1")
            .arg("--batch")
            .arg("--skip-column-names")
            .env_remove("MYSQL_PWD")
            .env_remove("MYSQL_HOME")
            .env_remove("MARIADB_HOME")
            .current_dir(&runtime.mariadb_base_dir);
        configure_child_command(&mut command);
        command
    }

    #[cfg(windows)]
    fn run_root_sql(runtime: &ResolvedRuntime, port: u16, sql: &str) {
        let mut command = root_client_command(runtime, port);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(sql.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[cfg(windows)]
    fn run_wordpress_test_sql(runtime: &ResolvedRuntime, data_root: &Path, port: u16, sql: &str) {
        let private = tempfile::Builder::new()
            .prefix("phase72-client-")
            .tempdir_in(data_root)
            .unwrap();
        restrict_private_directory(private.path()).unwrap();
        let password = Zeroizing::new(
            secret::load(&data_root.join(crate::runtime::DATABASE_WORDPRESS_SECRET)).unwrap(),
        );
        let option_file = PrivateOptionFile::create(
            private.path(),
            port,
            DATABASE_WORDPRESS_USER,
            password.as_str(),
        )
        .unwrap();
        let mut command = Command::new(&runtime.mariadb_import_executable);
        command
            .arg(format!(
                "--defaults-extra-file={}",
                option_file.path().to_string_lossy()
            ))
            .arg(format!("--database={DATABASE_NAME}"))
            .arg("--batch")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(&runtime.mariadb_base_dir);
        configure_child_command(&mut command);
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(sql.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        drop(option_file);
    }

    #[cfg(windows)]
    fn import_dump_as_root(runtime: &ResolvedRuntime, port: u16, dump_path: &Path) {
        let input = File::open(dump_path).unwrap();
        let mut command = root_client_command(runtime, port);
        command
            .stdin(Stdio::from(input))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        assert!(command.status().unwrap().success());
    }

    #[cfg(windows)]
    fn query_root_scalar(runtime: &ResolvedRuntime, port: u16, sql: &str) -> String {
        let mut command = root_client_command(runtime, port);
        command
            .arg(format!("--execute={sql}"))
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        let output = command.output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
