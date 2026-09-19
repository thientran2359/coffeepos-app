use crate::backup_database::{self, BackupDatabaseErrorInfo, DatabaseBackupSession};
use crate::backup_format::{
    self, BackupDestinationIdentity, BackupDestinationSelection, BackupEntrySource,
    BackupErrorInfo, BackupPayloadEntry, BackupSourceVersions, BackupWarning,
};
use crate::provisioning::WORDPRESS_ADMIN_SECRET;
use crate::runtime::{
    RuntimeInfo, RuntimeManager, RuntimeState, DATABASE_RUNTIME_SECRET, DATABASE_WORDPRESS_SECRET,
    MACHINE_TOKEN_PENDING_SECRET,
};
use crate::secret;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use zeroize::Zeroizing;

const BACKUP_JOURNAL: &str = "config/backup.json";
const BACKUP_JOURNAL_SCHEMA_VERSION: u32 = 1;
const INITIAL_ADMIN_PENDING_SECRET: &str = "config/wordpress-admin.pending.secret";
const REPAIR_ADMIN_PENDING_SECRET: &str = "config/wordpress-admin.repair.pending.secret";
const REPAIR_JOURNAL: &str = "config/repair.json";
const RESTORE_JOURNAL: &str = "config/restore.json";
const BACKUP_EXTENSION: &str = "coffeepos-backup";
const DISK_MARGIN_BYTES: u64 = 64 * 1024 * 1024;
const LOCAL_VERIFY_MARGIN_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupStage {
    Idle,
    SelectingDestination,
    Preflight,
    Quiesce,
    Database,
    Uploads,
    Archive,
    Validate,
    Finalize,
    Cleanup,
    Resume,
    Succeeded,
    Cancelled,
    Failed,
}

impl BackupStage {
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Succeeded | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupStatus {
    pub(crate) operation_id: Option<String>,
    pub(crate) stage: BackupStage,
    pub(crate) processed_files: u64,
    pub(crate) estimated_files: u64,
    pub(crate) processed_bytes: u64,
    pub(crate) estimated_bytes: u64,
    pub(crate) started_at: Option<u64>,
    pub(crate) finished_at: Option<u64>,
    pub(crate) cancelled: bool,
    pub(crate) succeeded: bool,
    pub(crate) failed: bool,
    pub(crate) warnings: Vec<String>,
    pub(crate) last_error: Option<BackupErrorInfo>,
}

impl Default for BackupStatus {
    fn default() -> Self {
        Self {
            operation_id: None,
            stage: BackupStage::Idle,
            processed_files: 0,
            estimated_files: 0,
            processed_bytes: 0,
            estimated_bytes: 0,
            started_at: None,
            finished_at: None,
            cancelled: false,
            succeeded: false,
            failed: false,
            warnings: Vec::new(),
            last_error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupResult {
    pub(crate) operation_id: String,
    pub(crate) status: String,
}

#[derive(Default)]
pub(crate) struct BackupOperationState {
    pub(crate) status: BackupStatus,
    pub(crate) cancel: Option<std::sync::Arc<AtomicBool>>,
    pub(crate) last_destination: Option<(String, PathBuf)>,
}

impl BackupOperationState {
    pub(crate) fn active(&self) -> bool {
        !self.status.stage.is_terminal()
    }

    pub(crate) fn begin(
        &mut self,
        operation_id: String,
    ) -> Result<std::sync::Arc<AtomicBool>, BackupErrorInfo> {
        if self.active() {
            return Err(backup_error(
                "preflight",
                "backup_in_progress",
                "A CoffeePOS backup operation is already active.",
                "Wait for the current backup to finish or cancel it before starting another backup.",
            ));
        }
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        self.status = BackupStatus {
            operation_id: Some(operation_id),
            stage: BackupStage::SelectingDestination,
            started_at: Some(now_epoch()),
            ..BackupStatus::default()
        };
        self.cancel = Some(cancel.clone());
        Ok(cancel)
    }

    pub(crate) fn update(&mut self, update: BackupProgress) {
        if self.status.operation_id.as_deref() != Some(update.operation_id.as_str()) {
            return;
        }
        self.status.stage = update.stage;
        self.status.processed_files = update.processed_files;
        self.status.estimated_files = update.estimated_files;
        self.status.processed_bytes = update.processed_bytes;
        self.status.estimated_bytes = update.estimated_bytes;
        self.status.warnings = update.warnings;
    }

    pub(crate) fn finish_success(
        &mut self,
        operation_id: &str,
        destination: PathBuf,
        warnings: Vec<String>,
    ) {
        if self.status.operation_id.as_deref() != Some(operation_id) {
            return;
        }
        self.status.stage = BackupStage::Succeeded;
        self.status.finished_at = Some(now_epoch());
        self.status.succeeded = true;
        self.status.failed = false;
        self.status.cancelled = false;
        self.status.warnings = warnings;
        self.status.last_error = None;
        self.cancel = None;
        self.last_destination = Some((operation_id.to_string(), destination));
    }

    pub(crate) fn finish_cancelled(&mut self, operation_id: &str) {
        if self.status.operation_id.as_deref() != Some(operation_id) {
            return;
        }
        self.status.stage = BackupStage::Cancelled;
        self.status.finished_at = Some(now_epoch());
        self.status.cancelled = true;
        self.status.succeeded = false;
        self.status.failed = false;
        self.status.last_error = None;
        self.cancel = None;
    }

    pub(crate) fn finish_failed(&mut self, operation_id: &str, error: BackupErrorInfo) {
        if self.status.operation_id.as_deref() != Some(operation_id) {
            return;
        }
        self.status.stage = BackupStage::Failed;
        self.status.finished_at = Some(now_epoch());
        self.status.failed = true;
        self.status.succeeded = false;
        self.status.cancelled = error.code == "cancelled";
        self.status.last_error = Some(error);
        self.cancel = None;
    }

    pub(crate) fn request_cancel(&self, operation_id: &str) -> Result<(), BackupErrorInfo> {
        if !self.active() || self.status.operation_id.as_deref() != Some(operation_id) {
            return Err(backup_error(
                "cleanup",
                "backup_not_active",
                "The requested backup operation is no longer active.",
                "Refresh backup status before trying another action.",
            ));
        }
        let cancel = self.cancel.as_ref().ok_or_else(|| {
            backup_error(
                "cleanup",
                "cancel_unavailable",
                "CoffeePOS cannot signal cancellation for this backup stage.",
                "Keep CoffeePOS open while the current bounded cleanup finishes.",
            )
        })?;
        cancel.store(true, Ordering::Release);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackupProgress {
    pub(crate) operation_id: String,
    pub(crate) stage: BackupStage,
    pub(crate) processed_files: u64,
    pub(crate) estimated_files: u64,
    pub(crate) processed_bytes: u64,
    pub(crate) estimated_bytes: u64,
    pub(crate) warnings: Vec<String>,
}

impl BackupProgress {
    fn stage(operation_id: &str, stage: BackupStage) -> Self {
        Self {
            operation_id: operation_id.to_string(),
            stage,
            processed_files: 0,
            estimated_files: 0,
            processed_bytes: 0,
            estimated_bytes: 0,
            warnings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackupCreateContext {
    pub(crate) data_root: PathBuf,
    pub(crate) admin_username: String,
    pub(crate) source: BackupSourceVersions,
}

#[derive(Clone, Debug)]
pub(crate) struct BackupDestinationPlan {
    pub(crate) path: PathBuf,
    expected_existing_identity: Option<BackupDestinationIdentity>,
}

pub(crate) struct BackupRunRequest<'a> {
    pub(crate) context: &'a BackupCreateContext,
    pub(crate) destination: &'a BackupDestinationPlan,
    pub(crate) operation_id: &'a str,
    pub(crate) backup_password: &'a str,
    pub(crate) cancelled: &'a AtomicBool,
    pub(crate) warnings: Vec<BackupWarning>,
}

#[derive(Clone, Debug)]
struct UploadSource {
    archive_path: String,
    source_path: PathBuf,
    size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalStage {
    Planned,
    Database,
    SnapshotSealed,
    ArchiveWriting,
    ArchiveValidated,
    Finalized,
    Cleanup,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupJournal {
    schema_version: u32,
    operation_id: String,
    stage: JournalStage,
    destination_temp: String,
    runtime_was_running: Option<bool>,
}

pub(crate) fn new_operation_id() -> Result<String, BackupErrorInfo> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        backup_error(
            "preflight",
            "operation_id_failed",
            "CoffeePOS could not generate a secure backup operation id.",
            "Restart CoffeePOS Desktop and retry backup.",
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn preflight_before_quiesce(
    context: &BackupCreateContext,
    selection: &BackupDestinationSelection,
) -> Result<(BackupDestinationPlan, u64, u64, Vec<BackupWarning>), BackupErrorInfo> {
    validate_operation_id_component(&context.admin_username, "administrator identity")?;
    validate_pending_transactions(&context.data_root)?;
    require_secret(
        &context.data_root.join(WORDPRESS_ADMIN_SECRET),
        "administrator",
    )?;
    require_secret(
        &context.data_root.join(DATABASE_RUNTIME_SECRET),
        "runtime database",
    )?;
    require_secret(
        &context.data_root.join(DATABASE_WORDPRESS_SECRET),
        "WordPress database",
    )?;

    let destination = validate_destination(&context.data_root, &selection.path)?;
    let current_identity = backup_format::capture_destination_identity(&destination, "preflight")?;
    if current_identity != selection.existing_identity {
        return Err(destination_changed_error("preflight"));
    }
    let uploads = enumerate_uploads(&context.data_root.join("uploads"))?;
    let upload_bytes = uploads.iter().try_fold(0_u64, |sum, item| {
        sum.checked_add(item.size)
            .ok_or_else(|| size_error("preflight"))
    })?;
    let database_bytes = estimate_directory_bytes(&context.data_root.join("database"))?;
    preflight_capacity(
        &context.data_root,
        &destination,
        database_bytes,
        upload_bytes,
    )?;
    probe_destination_writable(&destination)?;

    let warnings = unmanaged_extension_warnings(&context.data_root)?;
    Ok((
        BackupDestinationPlan {
            path: destination,
            expected_existing_identity: selection.existing_identity.clone(),
        },
        uploads.len() as u64,
        upload_bytes,
        warnings,
    ))
}

pub(crate) fn run_backup<F>(
    runtime: &mut RuntimeManager,
    request: BackupRunRequest<'_>,
    mut report: F,
) -> Result<RuntimeInfo, BackupErrorInfo>
where
    F: FnMut(BackupProgress),
{
    let BackupRunRequest {
        context,
        destination,
        operation_id,
        backup_password,
        cancelled,
        warnings,
    } = request;
    if backup_password.is_empty() {
        return Err(backup_error(
            "preflight",
            "empty_password",
            "The backup password cannot be empty.",
            "Enter and confirm a backup password before creating the backup.",
        ));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(cancelled_error("preflight"));
    }
    let temp_path = destination_temp_path(&destination.path, operation_id)?;
    let mut journal = BackupJournal {
        schema_version: BACKUP_JOURNAL_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        stage: JournalStage::Planned,
        destination_temp: temp_path.to_string_lossy().into_owned(),
        runtime_was_running: None,
    };
    let before = runtime.refresh();
    journal.runtime_was_running = Some(before.state == RuntimeState::Running);
    persist_journal(&context.data_root, &journal)?;
    report(BackupProgress::stage(operation_id, BackupStage::Quiesce));

    let mut session = match DatabaseBackupSession::begin_with_operation_id(
        runtime,
        cancelled,
        operation_id.to_string(),
    ) {
        Ok(session) => session,
        Err(error) => {
            let mapped = database_error(error);
            cleanup_pre_session_failure(&context.data_root, operation_id, &temp_path);
            return Err(mapped);
        }
    };
    journal.runtime_was_running = Some(session.runtime_was_running());
    journal.stage = JournalStage::Database;
    persist_journal(&context.data_root, &journal)?;
    report(BackupProgress::stage(operation_id, BackupStage::Database));

    let primary = (|| -> Result<(Vec<BackupPayloadEntry>, u64, u64), BackupErrorInfo> {
        session
            .create_dump(runtime, cancelled)
            .map_err(database_error)?;
        session
            .verify_dump_import(runtime, cancelled)
            .map_err(database_error)?;
        let store_name = session
            .verified_store_name()
            .ok_or_else(|| {
                backup_error(
                    "database",
                    "store_identity_unavailable",
                    "The verified database snapshot did not expose the CoffeePOS store identity.",
                    "Retry backup after checking CoffeePOS database health.",
                )
            })?
            .to_string();
        let admin_email = session
            .capture_administrator_email(runtime, cancelled, &context.admin_username)
            .map_err(database_error)?;
        session.seal_snapshot(runtime).map_err(database_error)?;
        journal.stage = JournalStage::SnapshotSealed;
        persist_journal(&context.data_root, &journal)?;
        report(BackupProgress::stage(operation_id, BackupStage::Uploads));

        if cancelled.load(Ordering::Acquire) {
            return Err(cancelled_error("uploads"));
        }
        let uploads = enumerate_uploads(&context.data_root.join("uploads"))?;
        let upload_bytes = uploads.iter().try_fold(0_u64, |sum, item| {
            sum.checked_add(item.size)
                .ok_or_else(|| size_error("uploads"))
        })?;
        let artifact = session.artifact().ok_or_else(|| {
            backup_error(
                "database",
                "database_snapshot_missing",
                "The verified database snapshot artifact is unavailable.",
                "Retry backup from the beginning.",
            )
        })?;
        let estimated_files = uploads.len() as u64 + 3;
        let estimated_bytes = upload_bytes
            .checked_add(artifact.size_bytes)
            .ok_or_else(|| size_error("archive"))?;

        let mut entries = Vec::with_capacity(uploads.len() + 3);
        entries.push(BackupPayloadEntry {
            path: "database/store.sql".into(),
            source: BackupEntrySource::File(session.dump_path().to_path_buf()),
        });
        for item in uploads {
            entries.push(BackupPayloadEntry {
                path: item.archive_path,
                source: BackupEntrySource::File(item.source_path),
            });
        }
        entries.push(backup_format::portable_store_config_entry(
            &store_name,
            &context.admin_username,
            &admin_email,
        )?);
        let admin_password = Zeroizing::new(
            secret::load(&context.data_root.join(WORDPRESS_ADMIN_SECRET)).map_err(|_| {
                backup_error(
                    "archive",
                    "administrator_secret_unavailable",
                    "The protected administrator credential became unavailable during backup.",
                    "Keep the source store unchanged, repair the protected credential, and retry backup.",
                )
            })?,
        );
        entries.push(backup_format::portable_administrator_secret_entry(
            &context.admin_username,
            admin_password.as_str(),
        )?);
        Ok((entries, estimated_files, estimated_bytes))
    })();

    let (entries, estimated_files, estimated_bytes) = match primary {
        Ok(value) => value,
        Err(error) => {
            return Err(fail_with_session(
                session, runtime, context, &temp_path, error,
            ))
        }
    };
    if cancelled.load(Ordering::Acquire) {
        return Err(fail_with_session(
            session,
            runtime,
            context,
            &temp_path,
            cancelled_error("archive"),
        ));
    }

    journal.stage = JournalStage::ArchiveWriting;
    if let Err(error) = persist_journal(&context.data_root, &journal) {
        return Err(fail_with_session(
            session, runtime, context, &temp_path, error,
        ));
    }
    report(BackupProgress {
        operation_id: operation_id.to_string(),
        stage: BackupStage::Archive,
        processed_files: 0,
        estimated_files,
        processed_bytes: 0,
        estimated_bytes,
        warnings: warning_codes(&warnings),
    });

    let write_result = (|| -> Result<(), BackupErrorInfo> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|_| {
                backup_error(
                    "archive",
                    "destination_temp_unavailable",
                    "CoffeePOS cannot create its private incomplete backup file beside the selected destination.",
                    "Choose a writable local destination and retry backup.",
                )
            })?;
        let seed = backup_format::new_backup_manifest_seed_with_warning_metadata(
            context.source.clone(),
            warnings.clone(),
        )?;
        backup_format::write_encrypted_backup_with_control(
            &mut output,
            backup_password,
            seed,
            &entries,
            cancelled,
            |writer_progress| {
                report(BackupProgress {
                    operation_id: operation_id.to_string(),
                    stage: BackupStage::Archive,
                    processed_files: writer_progress.entries_completed,
                    estimated_files: if writer_progress.total_entries == 0 {
                        estimated_files
                    } else {
                        writer_progress.total_entries
                    },
                    processed_bytes: writer_progress.bytes_processed,
                    estimated_bytes: if writer_progress.total_bytes == 0 {
                        estimated_bytes
                    } else {
                        writer_progress.total_bytes
                    },
                    warnings: warning_codes(&warnings),
                });
            },
        )?;
        output.sync_all().map_err(|_| {
            backup_error(
                "archive",
                "archive_flush_failed",
                "CoffeePOS could not flush the encrypted backup to disk.",
                "Check the destination disk and retry; the incomplete temp file will be cleaned.",
            )
        })?;
        Ok(())
    })();
    if let Err(error) = write_result {
        return Err(fail_with_session(
            session, runtime, context, &temp_path, error,
        ));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(fail_with_session(
            session,
            runtime,
            context,
            &temp_path,
            cancelled_error("archive"),
        ));
    }

    report(BackupProgress {
        operation_id: operation_id.to_string(),
        stage: BackupStage::Validate,
        processed_files: estimated_files,
        estimated_files,
        processed_bytes: estimated_bytes,
        estimated_bytes,
        warnings: warning_codes(&warnings),
    });
    let target = backup_format::BackupCompatibilityTarget {
        source: context.source.clone(),
        restore_schema: backup_format::BACKUP_SCHEMA_VERSION,
        available_restore_bytes: u64::MAX,
    };
    if let Err(error) = backup_format::validate_backup(&temp_path, backup_password, &target) {
        return Err(fail_with_session(
            session, runtime, context, &temp_path, error,
        ));
    }
    journal.stage = JournalStage::ArchiveValidated;
    if let Err(error) = persist_journal(&context.data_root, &journal) {
        return Err(fail_with_session(
            session, runtime, context, &temp_path, error,
        ));
    }

    report(BackupProgress {
        operation_id: operation_id.to_string(),
        stage: BackupStage::Finalize,
        processed_files: estimated_files,
        estimated_files,
        processed_bytes: estimated_bytes,
        estimated_bytes,
        warnings: warning_codes(&warnings),
    });
    let final_destination = match validate_destination(&context.data_root, &destination.path) {
        Ok(path) => path,
        Err(error) => {
            return Err(fail_with_session(
                session, runtime, context, &temp_path, error,
            ))
        }
    };
    if !same_windows_path(&final_destination, &destination.path) {
        return Err(fail_with_session(
            session,
            runtime,
            context,
            &temp_path,
            destination_changed_error("finalize"),
        ));
    }
    let current_identity =
        match backup_format::capture_destination_identity(&final_destination, "finalize") {
            Ok(identity) => identity,
            Err(error) => {
                return Err(fail_with_session(
                    session, runtime, context, &temp_path, error,
                ))
            }
        };
    if current_identity != destination.expected_existing_identity {
        return Err(fail_with_session(
            session,
            runtime,
            context,
            &temp_path,
            destination_changed_error("finalize"),
        ));
    }
    if let Err(error) = atomic_finalize(&temp_path, destination) {
        return Err(fail_with_session(
            session, runtime, context, &temp_path, error,
        ));
    }
    journal.stage = JournalStage::Finalized;
    if let Err(error) = persist_journal(&context.data_root, &journal) {
        return Err(fail_after_finalize(session, runtime, context, error));
    }

    report(BackupProgress {
        operation_id: operation_id.to_string(),
        stage: BackupStage::Cleanup,
        processed_files: estimated_files,
        estimated_files,
        processed_bytes: estimated_bytes,
        estimated_bytes,
        warnings: warning_codes(&warnings),
    });
    journal.stage = JournalStage::Cleanup;
    let _ = persist_journal(&context.data_root, &journal);
    let runtime_info = session.cleanup(runtime).map_err(|error| {
        backup_error(
            "resume",
            "runtime_resume_failed",
            "The encrypted backup was finalized, but CoffeePOS could not fully restore the previous runtime state.",
            format!(
                "The backup file is preserved. Keep CoffeePOS open and repair runtime state before another managed operation. {}",
                error.message
            ),
        )
    })?;
    remove_journal(&context.data_root)?;
    report(BackupProgress {
        operation_id: operation_id.to_string(),
        stage: BackupStage::Resume,
        processed_files: estimated_files,
        estimated_files,
        processed_bytes: estimated_bytes,
        estimated_bytes,
        warnings: warning_codes(&warnings),
    });
    Ok(runtime_info)
}

pub(crate) fn recover_interrupted_backup(data_root: &Path) -> Result<(), BackupErrorInfo> {
    let path = data_root.join(BACKUP_JOURNAL);
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(&path).map_err(|_| recovery_error())?;
    if !metadata.file_type().is_file() || metadata_is_reparse_point(&metadata) {
        return Err(backup_error(
            "cleanup",
            "unsafe_recovery_marker",
            "CoffeePOS refused an unsafe backup recovery marker.",
            "Inspect config/backup.json manually before starting the managed runtime.",
        ));
    }
    let bytes = fs::read(&path).map_err(|_| recovery_error())?;
    let journal: BackupJournal = serde_json::from_slice(&bytes).map_err(|_| recovery_error())?;
    if journal.schema_version != BACKUP_JOURNAL_SCHEMA_VERSION {
        return Err(recovery_error());
    }
    validate_operation_id(&journal.operation_id)?;

    // Resolve every durably recorded backup child before removing its owned staging. This also
    // refuses incomplete evidence, so normal startup cannot silently proceed past an orphan whose
    // containment/termination was never confirmed before the previous process died.
    backup_database::recover_interrupted_backup_children(data_root, &journal.operation_id)
        .map_err(database_error)?;

    let temp_path = PathBuf::from(&journal.destination_temp);
    let temp_result = if temp_path.exists() {
        remove_owned_destination_temp(&temp_path, &journal.operation_id)
    } else {
        Ok(())
    };
    let staging_result =
        backup_database::cleanup_interrupted_database_staging(data_root, &journal.operation_id)
            .map_err(database_error);
    match (temp_result, staging_result) {
        (Ok(()), Ok(())) => remove_journal(data_root),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(temp_error), Err(staging_error)) => Err(backup_error(
            "cleanup",
            "interrupted_backup_cleanup_failed",
            "CoffeePOS could not fully clean interrupted backup staging.",
            format!(
                "Encrypted temp cleanup: {} Database staging cleanup: {} Managed startup remains blocked until recovery succeeds.",
                temp_error.message, staging_error.message
            ),
        )),
    }
}

pub(crate) fn open_backup_folder(path: &Path) -> Result<(), BackupErrorInfo> {
    let parent = path.parent().ok_or_else(|| {
        backup_error(
            "finalize",
            "destination_parent_missing",
            "CoffeePOS cannot resolve the backup destination folder.",
            "Create another backup to a valid local folder.",
        )
    })?;
    open_folder_native(parent)
}

fn validate_pending_transactions(data_root: &Path) -> Result<(), BackupErrorInfo> {
    for path in [
        BACKUP_JOURNAL,
        INITIAL_ADMIN_PENDING_SECRET,
        REPAIR_ADMIN_PENDING_SECRET,
        MACHINE_TOKEN_PENDING_SECRET,
        REPAIR_JOURNAL,
        RESTORE_JOURNAL,
    ] {
        if data_root.join(path).exists() {
            return Err(backup_error(
                "preflight",
                "pending_transaction",
                "CoffeePOS found an unfinished managed transaction and will not start a backup from an ambiguous state.",
                "Finish or recover the pending setup/repair/restore transaction, then retry backup.",
            ));
        }
    }
    Ok(())
}

fn require_secret(path: &Path, label: &str) -> Result<(), BackupErrorInfo> {
    let value = Zeroizing::new(secret::load(path).map_err(|_| {
        backup_error(
            "preflight",
            "credential_unavailable",
            format!("The protected {label} credential cannot be read for backup."),
            "Repair the protected credential under the same Windows profile before retrying backup.",
        )
    })?);
    if value.is_empty() {
        return Err(backup_error(
            "preflight",
            "credential_unavailable",
            format!("The protected {label} credential is empty."),
            "Repair the protected credential before retrying backup.",
        ));
    }
    Ok(())
}

fn validate_operation_id_component(value: &str, label: &str) -> Result<(), BackupErrorInfo> {
    if value.is_empty() || value.chars().count() > 100 || value.chars().any(char::is_control) {
        return Err(backup_error(
            "preflight",
            "invalid_store_identity",
            format!("The managed {label} is invalid for portable backup metadata."),
            "Repair the managed store identity before retrying backup.",
        ));
    }
    Ok(())
}

fn validate_operation_id(operation_id: &str) -> Result<(), BackupErrorInfo> {
    if operation_id.len() == 32
        && operation_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(recovery_error())
    }
}

fn validate_destination(data_root: &Path, destination: &Path) -> Result<PathBuf, BackupErrorInfo> {
    if destination
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case(BACKUP_EXTENSION))
        != Some(true)
    {
        return Err(backup_error(
            "preflight",
            "invalid_destination_extension",
            "CoffeePOS backups must use the .coffeepos-backup extension.",
            "Choose a destination ending in .coffeepos-backup.",
        ));
    }
    let parent = destination.parent().ok_or_else(invalid_destination_error)?;
    let parent = fs::canonicalize(parent).map_err(|_| invalid_destination_error())?;
    reject_reparse_ancestors(&parent, "preflight")?;
    let file_name = destination
        .file_name()
        .ok_or_else(invalid_destination_error)?;
    let normalized = parent.join(file_name);
    match fs::symlink_metadata(&normalized) {
        Ok(metadata) => {
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata_is_reparse_point(&metadata)
            {
                return Err(backup_error(
                    "preflight",
                    "unsafe_destination",
                    "CoffeePOS refused an existing backup destination that is not a regular local file.",
                    "Choose a regular local .coffeepos-backup file path and retry.",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(invalid_destination_error()),
    }
    let root = fs::canonicalize(data_root).map_err(|_| invalid_destination_error())?;
    for relative in ["uploads", "site", "database", "config", "logs"] {
        let protected = root.join(relative);
        if protected.exists() {
            let protected =
                fs::canonicalize(&protected).map_err(|_| invalid_destination_error())?;
            if path_is_within(&normalized, &protected) {
                return Err(backup_error(
                    "preflight",
                    "unsafe_destination",
                    "CoffeePOS refused a backup destination inside managed mutable store data.",
                    "Choose the managed backups folder or a folder outside the CoffeePOS data root.",
                ));
            }
        }
    }
    Ok(normalized)
}

fn probe_destination_writable(destination: &Path) -> Result<(), BackupErrorInfo> {
    let parent = destination.parent().ok_or_else(invalid_destination_error)?;
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| invalid_destination_error())?;
    let probe = parent.join(format!(
        ".coffeepos-backup-write-probe-{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ));
    let result = OpenOptions::new().write(true).create_new(true).open(&probe);
    match result {
        Ok(file) => {
            drop(file);
            fs::remove_file(&probe).map_err(|_| invalid_destination_error())?;
            Ok(())
        }
        Err(_) => Err(backup_error(
            "preflight",
            "destination_not_writable",
            "CoffeePOS cannot write to the selected backup destination folder.",
            "Choose a writable local folder and retry backup.",
        )),
    }
}

fn destination_temp_path(
    destination: &Path,
    operation_id: &str,
) -> Result<PathBuf, BackupErrorInfo> {
    validate_operation_id(operation_id)?;
    let parent = destination.parent().ok_or_else(invalid_destination_error)?;
    Ok(parent.join(format!(".coffeepos-backup-{operation_id}.partial")))
}

fn enumerate_uploads(root: &Path) -> Result<Vec<UploadSource>, BackupErrorInfo> {
    if !root.is_absolute() || !root.exists() {
        return Err(backup_error(
            "uploads",
            "uploads_root_unavailable",
            "CoffeePOS cannot resolve the managed uploads directory.",
            "Repair the managed store directory layout and retry backup.",
        ));
    }
    reject_reparse_ancestors(root, "uploads")?;
    let root_metadata = fs::symlink_metadata(root).map_err(|_| uploads_read_error())?;
    if !root_metadata.file_type().is_dir() || metadata_is_reparse_point(&root_metadata) {
        return Err(unsafe_upload_error());
    }

    let mut pending = vec![root.to_path_buf()];
    let mut uploads = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|_| uploads_read_error())?;
        for entry in entries {
            let entry = entry.map_err(|_| uploads_read_error())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| uploads_read_error())?;
            if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
                return Err(unsafe_upload_error());
            }
            if metadata.file_type().is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.file_type().is_file() {
                return Err(unsafe_upload_error());
            }
            let relative = path.strip_prefix(root).map_err(|_| unsafe_upload_error())?;
            let archive_relative = archive_relative_path(relative)?;
            uploads.push(UploadSource {
                archive_path: format!("uploads/{archive_relative}"),
                source_path: path,
                size: metadata.len(),
            });
        }
    }
    uploads.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
    Ok(uploads)
}

fn archive_relative_path(path: &Path) -> Result<String, BackupErrorInfo> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(unsafe_upload_error());
        };
        let value = value.to_str().ok_or_else(|| {
            backup_error(
                "uploads",
                "unsupported_upload_path",
                "A managed upload filename cannot be represented safely in the portable backup format.",
                "Rename that upload to a valid Unicode filename and retry backup.",
            )
        })?;
        if value.is_empty() || value == "." || value == ".." {
            return Err(unsafe_upload_error());
        }
        parts.push(value);
    }
    if parts.is_empty() {
        return Err(unsafe_upload_error());
    }
    Ok(parts.join("/"))
}

fn estimate_directory_bytes(root: &Path) -> Result<u64, BackupErrorInfo> {
    if !root.exists() {
        return Ok(0);
    }
    reject_reparse_ancestors(root, "preflight")?;
    let mut pending = vec![root.to_path_buf()];
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        let metadata = fs::symlink_metadata(&directory).map_err(|_| capacity_error())?;
        if metadata_is_reparse_point(&metadata) || metadata.file_type().is_symlink() {
            return Err(backup_error(
                "preflight",
                "unsafe_database_path",
                "CoffeePOS refused a reparse point while estimating managed database backup capacity.",
                "Move the managed data root back to regular local directories before retrying backup.",
            ));
        }
        for entry in fs::read_dir(&directory).map_err(|_| capacity_error())? {
            let entry = entry.map_err(|_| capacity_error())?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| capacity_error())?;
            if metadata_is_reparse_point(&metadata) || metadata.file_type().is_symlink() {
                return Err(backup_error(
                    "preflight",
                    "unsafe_database_path",
                    "CoffeePOS refused a reparse point inside managed database data.",
                    "Repair the managed data layout before retrying backup.",
                ));
            }
            if metadata.file_type().is_dir() {
                pending.push(entry.path());
            } else if metadata.file_type().is_file() {
                total = total
                    .checked_add(metadata.len())
                    .ok_or_else(capacity_error)?;
            }
        }
    }
    Ok(total)
}

fn preflight_capacity(
    data_root: &Path,
    destination: &Path,
    database_bytes: u64,
    upload_bytes: u64,
) -> Result<(), BackupErrorInfo> {
    let final_estimate = database_bytes
        .saturating_mul(2)
        .saturating_add(upload_bytes)
        .saturating_add(DISK_MARGIN_BYTES);
    let local_estimate = database_bytes
        .saturating_mul(3)
        .saturating_add(LOCAL_VERIFY_MARGIN_BYTES);
    if let Some(parent) = destination.parent() {
        if let Ok(available) = fs2::available_space(parent) {
            if available < final_estimate {
                return Err(backup_error(
                    "preflight",
                    "destination_disk_full",
                    "The selected backup destination does not have enough free space for the estimated encrypted backup.",
                    "Free disk space or choose another destination before retrying backup.",
                ));
            }
        }
    }
    let local_root = data_root.join("backups");
    if let Ok(available) = fs2::available_space(&local_root) {
        if available < local_estimate {
            return Err(backup_error(
                "preflight",
                "local_disk_full",
                "The CoffeePOS data volume does not have enough free space for the verified database snapshot staging.",
                "Free space on the CoffeePOS data volume before retrying backup.",
            ));
        }
    }
    Ok(())
}

fn unmanaged_extension_warnings(data_root: &Path) -> Result<Vec<BackupWarning>, BackupErrorInfo> {
    let plugins = data_root.join("site/wp-content/plugins");
    let themes = data_root.join("site/wp-content/themes");
    let mut unmanaged_plugins = 0_u64;
    let mut unmanaged_themes = 0_u64;
    if plugins.is_dir() {
        reject_reparse_ancestors(&plugins, "preflight")?;
        for entry in fs::read_dir(&plugins).map_err(|_| uploads_read_error())? {
            let entry = entry.map_err(|_| uploads_read_error())?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.eq_ignore_ascii_case("woocommerce") && !name.eq_ignore_ascii_case("coffeepos")
            {
                unmanaged_plugins = unmanaged_plugins.saturating_add(1);
            }
        }
    }
    if themes.is_dir() {
        reject_reparse_ancestors(&themes, "preflight")?;
        for entry in fs::read_dir(&themes).map_err(|_| uploads_read_error())? {
            entry.map_err(|_| uploads_read_error())?;
            unmanaged_themes = unmanaged_themes.saturating_add(1);
        }
    }
    let mut items = Vec::new();
    if unmanaged_plugins > 0 {
        items.push(format!("plugins.{unmanaged_plugins}"));
    }
    if unmanaged_themes > 0 {
        items.push(format!("themes.{unmanaged_themes}"));
    }
    if items.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(vec![BackupWarning::unmanaged_site_code_not_included(
            items,
        )?])
    }
}

pub(crate) fn warning_codes(warnings: &[BackupWarning]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| warning.code.clone())
        .collect()
}

fn persist_journal(data_root: &Path, journal: &BackupJournal) -> Result<(), BackupErrorInfo> {
    let path = data_root.join(BACKUP_JOURNAL);
    let parent = path.parent().ok_or_else(recovery_error)?;
    fs::create_dir_all(parent).map_err(|_| recovery_error())?;
    reject_reparse_ancestors(parent, "cleanup")?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|_| recovery_error())?;
    serde_json::to_writer(&mut temporary, journal).map_err(|_| recovery_error())?;
    temporary.write_all(b"\n").map_err(|_| recovery_error())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| recovery_error())?;
    temporary.persist(&path).map_err(|_| recovery_error())?;
    Ok(())
}

fn remove_journal(data_root: &Path) -> Result<(), BackupErrorInfo> {
    let path = data_root.join(BACKUP_JOURNAL);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(backup_error(
            "cleanup",
            "recovery_marker_cleanup_failed",
            "CoffeePOS could not remove the completed backup recovery marker.",
            "Keep CoffeePOS open and retry recovery before starting another backup.",
        )),
    }
}

fn fail_with_session(
    session: DatabaseBackupSession,
    runtime: &mut RuntimeManager,
    context: &BackupCreateContext,
    temp_path: &Path,
    primary: BackupErrorInfo,
) -> BackupErrorInfo {
    let _ = remove_owned_destination_temp(temp_path, session.operation_id());
    if primary.code == "child_cleanup_unconfirmed" {
        // Phase 7.2 deliberately keeps the maintenance fence, retained Child handle and owned
        // staging alive on this code. The staging now also contains durable process evidence for
        // crash/relaunch recovery, so deleting it here would destroy the only safe recovery path.
        return primary;
    }
    match session.cleanup(runtime) {
        Ok(_) => {
            let _ = remove_journal(&context.data_root);
            primary
        }
        Err(cleanup) => backup_error(
            "resume",
            "cleanup_failed",
            "The backup did not complete and CoffeePOS could not fully restore the previous runtime state.",
            format!(
                "Backup error: {} Cleanup error: {} Keep CoffeePOS open and recover runtime state before retrying.",
                primary.message, cleanup.message
            ),
        ),
    }
}

fn fail_after_finalize(
    session: DatabaseBackupSession,
    runtime: &mut RuntimeManager,
    context: &BackupCreateContext,
    primary: BackupErrorInfo,
) -> BackupErrorInfo {
    match session.cleanup(runtime) {
        Ok(_) => {
            let _ = remove_journal(&context.data_root);
            primary
        }
        Err(cleanup) => backup_error(
            "resume",
            "cleanup_failed",
            "The encrypted backup was finalized, but CoffeePOS could not fully restore the previous runtime state.",
            format!(
                "The backup file is preserved. Marker error: {} Cleanup error: {}",
                primary.message, cleanup.message
            ),
        ),
    }
}

fn cleanup_pre_session_failure(data_root: &Path, operation_id: &str, temp_path: &Path) {
    let _ = remove_owned_destination_temp(temp_path, operation_id);
    let _ = backup_database::cleanup_interrupted_database_staging(data_root, operation_id);
    let _ = remove_journal(data_root);
}

fn remove_owned_destination_temp(path: &Path, operation_id: &str) -> Result<(), BackupErrorInfo> {
    let expected = format!(".coffeepos-backup-{operation_id}.partial");
    if path.file_name().and_then(|value| value.to_str()) != Some(expected.as_str()) {
        return Err(backup_error(
            "cleanup",
            "unsafe_destination_temp",
            "CoffeePOS refused to delete an unowned backup temp path.",
            "Preserve the file and inspect backup recovery state manually.",
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata_is_reparse_point(&metadata) {
                return Err(backup_error(
                    "cleanup",
                    "unsafe_destination_temp",
                    "CoffeePOS refused to delete a backup temp path whose type changed.",
                    "Preserve the path and inspect it manually before retrying backup.",
                ));
            }
            fs::remove_file(path).map_err(|_| {
                backup_error(
                    "cleanup",
                    "destination_temp_cleanup_failed",
                    "CoffeePOS could not remove the incomplete encrypted backup temp file.",
                    "Close processes using the destination and retry recovery.",
                )
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(recovery_error()),
    }
}

#[cfg(windows)]
fn atomic_finalize(
    temp: &Path,
    destination: &BackupDestinationPlan,
) -> Result<(), BackupErrorInfo> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<u16> = temp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = destination
        .path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let flags = if destination.expected_existing_identity.is_some() {
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    } else {
        MOVEFILE_WRITE_THROUGH
    };
    let result = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) };
    if result == 0 {
        return Err(backup_error(
            "finalize",
            "atomic_finalize_failed",
            "Windows could not atomically finalize the validated CoffeePOS backup.",
            "The incomplete temp file will be cleaned; check destination permissions and retry.",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_finalize(
    temp: &Path,
    destination: &BackupDestinationPlan,
) -> Result<(), BackupErrorInfo> {
    fs::rename(temp, &destination.path).map_err(|_| {
        backup_error(
            "finalize",
            "atomic_finalize_failed",
            "CoffeePOS could not finalize the validated backup.",
            "Check destination permissions and retry.",
        )
    })
}

#[cfg(windows)]
fn open_folder_native(folder: &Path) -> Result<(), BackupErrorInfo> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let operation: Vec<u16> = OsStr::new("open")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = folder
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
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
        Err(backup_error(
            "finalize",
            "open_destination_failed",
            "Windows could not open the backup destination folder.",
            "Open the folder manually or create another backup to a known location.",
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn open_folder_native(_folder: &Path) -> Result<(), BackupErrorInfo> {
    Err(backup_error(
        "finalize",
        "unsupported_platform",
        "Opening the backup destination is currently qualified for Windows.",
        "Open the destination folder manually.",
    ))
}

fn reject_reparse_ancestors(path: &Path, action: &str) -> Result<(), BackupErrorInfo> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| {
            backup_error(
                action,
                "path_unavailable",
                "CoffeePOS cannot inspect a managed backup path safely.",
                "Repair the local path and retry backup.",
            )
        })?;
        if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
            return Err(backup_error(
                action,
                "unsafe_reparse_point",
                "CoffeePOS refused a symlink, junction, or reparse point in backup source/destination paths.",
                "Use regular local directories for CoffeePOS managed data and the backup destination.",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let root = root
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    path.len() >= root.len()
        && path
            .iter()
            .zip(root.iter())
            .all(|(left, right)| left == right)
}

fn same_windows_path(left: &Path, right: &Path) -> bool {
    let left = left
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let right = right
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    left == right
}

fn destination_changed_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "destination_changed",
        "The selected backup destination changed after the native Save As confirmation.",
        "CoffeePOS did not replace the changed destination. Choose the destination again and retry backup.",
    )
}

fn unsafe_upload_error() -> BackupErrorInfo {
    backup_error(
        "uploads",
        "unsafe_upload_path",
        "CoffeePOS refused a non-regular upload or a symlink, junction, or reparse point.",
        "Keep uploads inside regular managed files/directories and retry backup.",
    )
}

fn uploads_read_error() -> BackupErrorInfo {
    backup_error(
        "uploads",
        "uploads_read_failed",
        "CoffeePOS cannot enumerate managed uploads safely.",
        "Check uploads permissions and retry backup without changing the source store.",
    )
}

fn invalid_destination_error() -> BackupErrorInfo {
    backup_error(
        "preflight",
        "invalid_destination",
        "CoffeePOS cannot resolve the selected backup destination safely.",
        "Choose an existing writable local folder and retry backup.",
    )
}

fn capacity_error() -> BackupErrorInfo {
    backup_error(
        "preflight",
        "capacity_estimate_failed",
        "CoffeePOS cannot estimate managed backup staging requirements safely.",
        "Check the managed data directory and retry backup.",
    )
}

fn size_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "size_overflow",
        "CoffeePOS backup size accounting overflowed its safety counter.",
        "Inspect the managed store data size before retrying backup.",
    )
}

fn recovery_error() -> BackupErrorInfo {
    backup_error(
        "cleanup",
        "recovery_marker_invalid",
        "CoffeePOS cannot safely interpret the interrupted backup recovery marker.",
        "Keep the managed store stopped and inspect config/backup.json before retrying.",
    )
}

fn cancelled_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "cancelled",
        "The CoffeePOS backup was cancelled.",
        "CoffeePOS will finish bounded cleanup and restore the previous runtime state before another backup starts.",
    )
}

fn database_error(error: BackupDatabaseErrorInfo) -> BackupErrorInfo {
    BackupErrorInfo {
        component: "backup".into(),
        action: error.action,
        code: error.code,
        message: error.message,
        recovery: error.recovery,
    }
}

fn backup_error(
    action: &str,
    code: &str,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> BackupErrorInfo {
    BackupErrorInfo {
        component: "backup".into(),
        action: action.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_temp_cleanup_requires_exact_owned_name() {
        let temp = tempfile::tempdir().unwrap();
        let unrelated = temp.path().join("unrelated.partial");
        fs::write(&unrelated, b"keep").unwrap();
        let error = remove_owned_destination_temp(&unrelated, "00112233445566778899aabbccddeeff")
            .unwrap_err();
        assert_eq!(error.code, "unsafe_destination_temp");
        assert!(unrelated.exists());
    }

    #[test]
    fn upload_enumeration_preserves_relative_portable_paths() {
        let temp = tempfile::tempdir().unwrap();
        let uploads = temp.path().join("uploads");
        fs::create_dir_all(uploads.join("2026/09")).unwrap();
        fs::write(uploads.join("2026/09/menu-café.txt"), b"fixture").unwrap();
        let entries = enumerate_uploads(&uploads).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_path, "uploads/2026/09/menu-café.txt");
        assert_eq!(entries[0].size, 7);
    }

    #[test]
    fn portable_projection_does_not_include_desktop_only_fields() {
        let entry =
            backup_format::portable_store_config_entry("Cafe", "owner", "owner@example.com")
                .unwrap();
        let BackupEntrySource::Memory(bytes) = entry.source else {
            panic!("portable config must be in-memory JSON")
        };
        let value: serde_json::Value = serde_json::from_slice(bytes.as_slice()).unwrap();
        assert_eq!(value["store_name"], "Cafe");
        assert_eq!(value["administrator"]["username"], "owner");
        let object = value.as_object().unwrap();
        assert!(!object.contains_key("startup_view"));
        assert!(!object.contains_key("bind_host"));
        assert!(!object.contains_key("data_root"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "provisions a disposable staged store and creates a complete encrypted Phase 7.3 backup"]
    fn backup_complete_disposable_store_smoke() {
        use crate::provisioning::{Provisioner, ProvisioningState};
        use crate::runtime::{resolve_development_manifest, RuntimeState};

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let project_root = manifest_dir.parent().unwrap().to_path_buf();
        let runtime_manifest =
            project_root.join("runtime/development/x86_64-pc-windows-msvc/manifest.json");
        let resolved = resolve_development_manifest(&project_root, &runtime_manifest).unwrap();
        let e2e_root = manifest_dir.join("target/phase7-3-e2e");
        fs::create_dir_all(&e2e_root).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("portable-backup-store-")
            .tempdir_in(&e2e_root)
            .unwrap();
        let data_root = temp.path().join("store");
        fs::create_dir_all(&data_root).unwrap();

        let mut provisioner = Provisioner::from_development(
            &project_root,
            &runtime_manifest,
            resolved.clone(),
            data_root.clone(),
        )
        .unwrap();
        provisioner.prepare().unwrap();
        let mut manager = RuntimeManager::new(resolved, data_root.clone()).unwrap();
        let running = manager.start().unwrap();
        let installed = provisioner
            .install_wordpress("CoffeePOS Phase 7.3 Portable Backup Smoke", &running)
            .unwrap();
        assert_eq!(installed.state, ProvisioningState::Ready);
        let admin_username = installed.admin_username.unwrap();

        let upload = data_root.join("uploads/2026/09/phase73-sentinel.bin");
        fs::create_dir_all(upload.parent().unwrap()).unwrap();
        fs::write(&upload, b"phase-7.3-upload-sentinel\x00\xff").unwrap();

        let compatibility = backup_format::load_development_compatibility_target(
            &project_root,
            &runtime_manifest,
            &data_root,
            "create",
        )
        .unwrap();
        let context = BackupCreateContext {
            data_root: data_root.clone(),
            admin_username,
            source: compatibility.source.clone(),
        };
        let destination = temp.path().join("portable-smoke.coffeepos-backup");
        let selection = BackupDestinationSelection::capture(destination, "preflight").unwrap();
        let (destination, _, _, warnings) = preflight_before_quiesce(&context, &selection).unwrap();
        let operation_id = new_operation_id().unwrap();
        let cancelled = AtomicBool::new(false);
        let after = run_backup(
            &mut manager,
            BackupRunRequest {
                context: &context,
                destination: &destination,
                operation_id: &operation_id,
                backup_password: "Phase73-Smoke-Backup-Password",
                cancelled: &cancelled,
                warnings,
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(after.state, RuntimeState::Running);
        assert!(destination.path.is_file());

        let validation_target = backup_format::BackupCompatibilityTarget {
            source: compatibility.source,
            restore_schema: backup_format::BACKUP_SCHEMA_VERSION,
            available_restore_bytes: u64::MAX,
        };
        let validation = backup_format::validate_backup(
            &destination.path,
            "Phase73-Smoke-Backup-Password",
            &validation_target,
        )
        .unwrap();
        assert!(validation.valid);
        assert!(backup_format::validate_backup(
            &destination.path,
            "wrong-password",
            &validation_target,
        )
        .is_err());
        assert!(!data_root.join(BACKUP_JOURNAL).exists());
        assert_eq!(manager.stop().unwrap().state, RuntimeState::Stopped);
    }
}
