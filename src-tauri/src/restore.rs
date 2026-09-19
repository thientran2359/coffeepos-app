use crate::backup_format::{
    validate_backup, BackupCompatibilityStatus, BackupCompatibilityTarget, BackupInspection,
    BackupValidation,
};
use crate::provisioning::{ProvisioningInfo, ProvisioningState, WORDPRESS_ADMIN_SECRET};
use crate::runtime::{
    DATABASE_RUNTIME_SECRET, DATABASE_WORDPRESS_SECRET, MACHINE_TOKEN_PENDING_SECRET,
    MACHINE_TOKEN_SECRET,
};
use crate::secret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;
use zeroize::Zeroizing;

pub(crate) const RESTORE_JOURNAL_SCHEMA_VERSION: u32 = 1;
const RESTORE_JOURNAL: &str = "config/restore.json";
const RESTORE_OWNERSHIP_SCHEMA_VERSION: u32 = 1;
const RESTORE_OWNERSHIP_MARKER: &str = ".coffeepos-restore-owned.json";
const RESTORE_COMPONENT_MARKER: &str = ".coffeepos-restore-component.json";
const RESTORE_PREVIOUS_COMPONENT_MARKER: &str = ".coffeepos-restore-previous.json";
const MAX_SAFE_ERROR_BYTES: usize = 512;
const FILE_HASH_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct RestoreErrorInfo {
    pub component: String,
    pub action: String,
    pub code: String,
    pub message: String,
    pub recovery: String,
}

impl std::fmt::Display for RestoreErrorInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {}: {} {}",
            self.component, self.action, self.message, self.recovery
        )
    }
}

impl std::error::Error for RestoreErrorInfo {}

fn restore_error(
    action: &str,
    code: &str,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> RestoreErrorInfo {
    RestoreErrorInfo {
        component: "restore".into(),
        action: action.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RestoreOriginalState {
    ExistingStore,
    NoPreviousStore,
}

impl RestoreOriginalState {
    pub(crate) fn from_provisioning(info: &ProvisioningInfo) -> Result<Self, RestoreErrorInfo> {
        match info.state {
            ProvisioningState::Ready => Ok(Self::ExistingStore),
            ProvisioningState::NotInstalled => Ok(Self::NoPreviousStore),
            ProvisioningState::Installing | ProvisioningState::NeedsRepair => Err(restore_error(
                "admission",
                "target_state_not_restorable",
                "CoffeePOS cannot start restore while the current store is installing or needs repair.",
                "Finish or recover the current provisioning/repair transaction before retrying restore.",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RestoreStage {
    Planned,
    Validated,
    RuntimeStopped,
    RecoveryBackupReady,
    StagingPrepared,
    DatabaseImported,
    UploadsRestored,
    TargetSecretsBound,
    StagingVerified,
    CutoverStarted,
    ActiveSwapped,
    ActiveVerified,
    Committed,
    Cleanup,
    AbortStarted,
    Aborted,
    RollbackStarted,
    RolledBack,
}

impl RestoreStage {
    pub(crate) fn terminal(self) -> bool {
        matches!(
            self,
            Self::Committed | Self::Cleanup | Self::Aborted | Self::RolledBack
        )
    }

    fn may_transition_to(self, next: Self, original: &RestoreOriginalState) -> bool {
        use RestoreStage::*;
        matches!(
            (self, next),
            (Planned, Validated)
                | (Validated, RuntimeStopped)
                | (Validated, AbortStarted)
                | (RuntimeStopped, RecoveryBackupReady)
                | (RuntimeStopped, StagingPrepared)
                | (RuntimeStopped, AbortStarted)
                | (RecoveryBackupReady, StagingPrepared)
                | (RecoveryBackupReady, AbortStarted)
                | (StagingPrepared, DatabaseImported)
                | (StagingPrepared, AbortStarted)
                | (DatabaseImported, UploadsRestored)
                | (DatabaseImported, AbortStarted)
                | (UploadsRestored, TargetSecretsBound)
                | (UploadsRestored, AbortStarted)
                | (TargetSecretsBound, StagingVerified)
                | (TargetSecretsBound, AbortStarted)
                | (StagingVerified, CutoverStarted)
                | (StagingVerified, AbortStarted)
                | (CutoverStarted, ActiveSwapped)
                | (CutoverStarted, RollbackStarted)
                | (ActiveSwapped, ActiveVerified)
                | (ActiveSwapped, RollbackStarted)
                | (ActiveVerified, Committed)
                | (Committed, Cleanup)
                | (AbortStarted, Aborted)
                | (RollbackStarted, RolledBack)
        ) && !matches!(
            (self, next, original),
            (
                RuntimeStopped,
                RecoveryBackupReady,
                RestoreOriginalState::NoPreviousStore
            )
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreSourceFingerprint {
    pub canonical_path_sha256: String,
    pub volume_serial_number: Option<u32>,
    pub file_index: Option<u64>,
    pub size_bytes: u64,
    pub modified_ticks: u64,
    pub encrypted_sha256: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RestoreCandidate {
    candidate_id: String,
    source_path: PathBuf,
    source_fingerprint: RestoreSourceFingerprint,
    validated_projection_sha256: String,
    inspection: BackupInspection,
}

impl RestoreCandidate {
    pub(crate) fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub(crate) fn inspection(&self) -> &BackupInspection {
        &self.inspection
    }

    pub(crate) fn source_fingerprint(&self) -> &RestoreSourceFingerprint {
        &self.source_fingerprint
    }

    pub(crate) fn validated_projection_sha256(&self) -> &str {
        &self.validated_projection_sha256
    }

    /// Native orchestration only. The selected backup path is deliberately absent from
    /// `RestoreInspection`, so the WebView only receives the opaque candidate id.
    pub(crate) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct RestoreInspection {
    pub candidate_id: String,
    pub backup: BackupInspection,
    pub validated_projection_sha256: String,
}

#[derive(Default)]
pub(crate) struct RestoreCandidateRegistry {
    candidates: BTreeMap<String, RestoreCandidate>,
}

impl RestoreCandidateRegistry {
    pub(crate) fn insert(&mut self, candidate: RestoreCandidate) -> RestoreInspection {
        let inspection = RestoreInspection {
            candidate_id: candidate.candidate_id.clone(),
            backup: candidate.inspection.clone(),
            validated_projection_sha256: candidate.validated_projection_sha256.clone(),
        };
        self.candidates
            .insert(candidate.candidate_id.clone(), candidate);
        inspection
    }

    pub(crate) fn get(&self, candidate_id: &str) -> Option<&RestoreCandidate> {
        self.candidates.get(candidate_id)
    }

    pub(crate) fn remove(&mut self, candidate_id: &str) {
        self.candidates.remove(candidate_id);
    }

    pub(crate) fn clear(&mut self) {
        self.candidates.clear();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreOwnedPaths {
    pub staging: String,
    pub rollback: String,
    pub quarantine: String,
    pub protected_state: String,
    pub recovery_backup: String,
}

impl RestoreOwnedPaths {
    fn for_transaction(transaction_id: &str) -> Self {
        Self {
            staging: format!("backups/restore-staging/{transaction_id}"),
            rollback: format!("backups/restore-rollback/{transaction_id}"),
            quarantine: format!("backups/restore-quarantine/{transaction_id}"),
            protected_state: format!("config/restore-protected/{transaction_id}"),
            recovery_backup: format!("backups/restore-recovery/{transaction_id}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RestoreComponent {
    Site,
    Database,
    Uploads,
}

impl RestoreComponent {
    fn directory_name(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Database => "database",
            Self::Uploads => "uploads",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreComponentEvidence {
    pub active_moved_to_rollback: bool,
    pub fresh_placeholder_removed: bool,
    pub staging_moved_to_active: bool,
    pub failed_target_quarantined: bool,
    pub previous_restored: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreCutoverEvidence {
    pub site: RestoreComponentEvidence,
    pub database: RestoreComponentEvidence,
    pub uploads: RestoreComponentEvidence,
    #[serde(default)]
    pub config_files: Vec<RestoreConfigFileEvidence>,
    pub config_snapshot_ready: bool,
    pub config_applied: bool,
    pub config_rolled_back: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreConfigFileEvidence {
    pub relative_path: String,
    pub previous_existed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_sha256: Option<String>,
    pub target_sha256: String,
    pub snapshot_ready: bool,
    pub installed: bool,
    pub rollback_restored: bool,
}

impl RestoreCutoverEvidence {
    fn component(&self, component: RestoreComponent) -> &RestoreComponentEvidence {
        match component {
            RestoreComponent::Site => &self.site,
            RestoreComponent::Database => &self.database,
            RestoreComponent::Uploads => &self.uploads,
        }
    }

    fn component_mut(&mut self, component: RestoreComponent) -> &mut RestoreComponentEvidence {
        match component {
            RestoreComponent::Site => &mut self.site,
            RestoreComponent::Database => &mut self.database,
            RestoreComponent::Uploads => &mut self.uploads,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreSafeError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreJournal {
    pub schema_version: u32,
    pub transaction_id: String,
    pub backup_id: String,
    pub validated_projection_sha256: String,
    pub source_file: RestoreSourceFingerprint,
    pub migration_ids: Vec<String>,
    pub original_state: RestoreOriginalState,
    pub runtime_was_running: bool,
    pub owned_paths: RestoreOwnedPaths,
    pub cutover: RestoreCutoverEvidence,
    pub stage: RestoreStage,
    pub created_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_error: Option<RestoreSafeError>,
}

impl RestoreJournal {
    pub(crate) fn new(
        candidate: &RestoreCandidate,
        original_state: RestoreOriginalState,
        runtime_was_running: bool,
        migration_ids: Vec<String>,
    ) -> Result<Self, RestoreErrorInfo> {
        let transaction_id = random_hex_id(16, "create restore transaction")?;
        let now = unix_seconds();
        Ok(Self {
            schema_version: RESTORE_JOURNAL_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            backup_id: candidate.inspection.backup_id.clone(),
            validated_projection_sha256: candidate.validated_projection_sha256.clone(),
            source_file: candidate.source_fingerprint.clone(),
            migration_ids,
            original_state,
            runtime_was_running,
            owned_paths: RestoreOwnedPaths::for_transaction(&transaction_id),
            cutover: RestoreCutoverEvidence::default(),
            stage: RestoreStage::Planned,
            created_at_unix_seconds: now,
            updated_at_unix_seconds: now,
            safe_error: None,
        })
    }

    pub(crate) fn advance(&mut self, next: RestoreStage) -> Result<(), RestoreErrorInfo> {
        if !self.stage.may_transition_to(next, &self.original_state) {
            return Err(restore_error(
                "advance journal",
                "invalid_restore_transition",
                format!(
                    "Restore cannot transition from {:?} to {:?}.",
                    self.stage, next
                ),
                "Preserve config/restore.json and recover from the recorded stage instead of skipping restore boundaries.",
            ));
        }
        self.stage = next;
        self.updated_at_unix_seconds = unix_seconds();
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct RestoreAdmissionStatus {
    pub blocked: bool,
    pub operation_id: Option<String>,
    pub recovery_required: bool,
}

#[derive(Default)]
pub(crate) struct RestoreAdmissionGate {
    owner: Option<String>,
    recovery_required: bool,
}

impl RestoreAdmissionGate {
    pub(crate) fn status(&self) -> RestoreAdmissionStatus {
        RestoreAdmissionStatus {
            blocked: self.owner.is_some(),
            operation_id: self.owner.clone(),
            recovery_required: self.recovery_required,
        }
    }

    pub(crate) fn acquire(&mut self, transaction_id: &str) -> Result<(), RestoreErrorInfo> {
        match self.owner.as_deref() {
            None => {
                self.owner = Some(transaction_id.into());
                self.recovery_required = false;
                Ok(())
            }
            Some(current) if current == transaction_id => Ok(()),
            Some(_) => Err(restore_error(
                "admission",
                "restore_already_active",
                "Another restore transaction already owns the external restore admission gate.",
                "Finish or recover the current restore before starting another managed operation.",
            )),
        }
    }

    pub(crate) fn recover_from_journal(&mut self, journal: &RestoreJournal) {
        if journal.stage.terminal() {
            self.owner = Some(journal.transaction_id.clone());
            self.recovery_required = true;
        } else {
            self.owner = Some(journal.transaction_id.clone());
            self.recovery_required = true;
        }
    }

    pub(crate) fn release_reconciled(
        &mut self,
        journal: &RestoreJournal,
    ) -> Result<(), RestoreErrorInfo> {
        if self.owner.as_deref() != Some(journal.transaction_id.as_str()) {
            return Err(restore_error(
                "admission",
                "restore_gate_owner_mismatch",
                "The restore admission gate is owned by a different transaction.",
                "Keep runtime admission blocked and recover the transaction recorded in config/restore.json.",
            ));
        }
        if !journal.stage.terminal() {
            return Err(restore_error(
                "admission",
                "restore_not_reconciled",
                "Restore admission cannot be reopened before the transaction reaches a verified terminal stage.",
                "Complete restore, rollback, or pre-cutover abort reconciliation first.",
            ));
        }
        self.owner = None;
        self.recovery_required = false;
        Ok(())
    }

    pub(crate) fn blocks_managed_operations(&self) -> bool {
        self.owner.is_some()
    }
}

pub(crate) fn inspect_restore_candidate(
    path: &Path,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
) -> Result<RestoreCandidate, RestoreErrorInfo> {
    if backup_password.is_empty() {
        return Err(restore_error(
            "inspect",
            "authentication_failed",
            "The backup password is incorrect or missing.",
            "Enter the password used when this backup was created and retry.",
        ));
    }
    let canonical = canonical_restore_source(path)?;
    let before = capture_source_fingerprint(&canonical, "inspect")?;
    let validation = validate_backup(&canonical, backup_password, target)
        .map_err(|error| restore_error_from_backup("inspect", error))?;
    if !validation.valid
        || !validation.inspection.can_restore
        || validation.inspection.compatibility.status != BackupCompatibilityStatus::Compatible
    {
        return Err(restore_error(
            "inspect",
            "backup_not_restorable",
            "The selected backup did not pass the restore compatibility gate.",
            "Choose a backup that matches the currently staged CoffeePOS restore baseline.",
        ));
    }
    let after = capture_source_fingerprint(&canonical, "inspect")?;
    if before != after {
        return Err(stale_candidate_error("inspect"));
    }
    let validated_projection_sha256 = validation_projection_sha256(&validation)?;
    Ok(RestoreCandidate {
        candidate_id: random_hex_id(16, "create restore candidate")?,
        source_path: canonical,
        source_fingerprint: after,
        validated_projection_sha256,
        inspection: validation.inspection,
    })
}

pub(crate) fn revalidate_restore_candidate(
    candidate: &RestoreCandidate,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
) -> Result<BackupValidation, RestoreErrorInfo> {
    let current = capture_source_fingerprint(&candidate.source_path, "apply")?;
    if current != candidate.source_fingerprint {
        return Err(stale_candidate_error("apply"));
    }
    let validation = validate_backup(&candidate.source_path, backup_password, target)
        .map_err(|error| restore_error_from_backup("apply", error))?;
    if !validation.valid
        || !validation.inspection.can_restore
        || validation.inspection.compatibility.status != BackupCompatibilityStatus::Compatible
    {
        return Err(restore_error(
            "apply",
            "backup_not_restorable",
            "The backup no longer passes the restore compatibility gate.",
            "Inspect the backup again before retrying restore.",
        ));
    }
    let projection_hash = validation_projection_sha256(&validation)?;
    if validation.inspection.backup_id != candidate.inspection.backup_id
        || projection_hash != candidate.validated_projection_sha256
    {
        return Err(stale_candidate_error("apply"));
    }
    let after_validation = capture_source_fingerprint(&candidate.source_path, "apply")?;
    if after_validation != current {
        return Err(stale_candidate_error("apply"));
    }
    Ok(validation)
}

fn restore_error_from_backup(
    action: &str,
    error: crate::backup_format::BackupErrorInfo,
) -> RestoreErrorInfo {
    RestoreErrorInfo {
        component: "restore".into(),
        action: action.into(),
        code: error.code,
        message: error.message,
        recovery: error.recovery,
    }
}

fn stale_candidate_error(action: &str) -> RestoreErrorInfo {
    restore_error(
        action,
        "stale_candidate",
        "The selected backup changed after it was inspected.",
        "Choose and inspect the backup again before applying restore.",
    )
}

fn validation_projection_sha256(validation: &BackupValidation) -> Result<String, RestoreErrorInfo> {
    let bytes = serde_json::to_vec(validation).map_err(|_| {
        restore_error(
            "inspect",
            "validation_binding_failed",
            "CoffeePOS could not bind the validated backup projection to a restore candidate.",
            "Retry inspection. If this repeats, preserve the backup and inspect application logs.",
        )
    })?;
    Ok(sha256_bytes(&bytes))
}

fn canonical_restore_source(path: &Path) -> Result<PathBuf, RestoreErrorInfo> {
    if !path.is_absolute() {
        return Err(restore_error(
            "inspect",
            "backup_path_not_absolute",
            "The native backup selection did not resolve to an absolute path.",
            "Choose the backup again using the native Open dialog.",
        ));
    }
    reject_reparse_existing_ancestors(path, "inspect")?;
    let canonical = fs::canonicalize(path).map_err(|_| {
        restore_error(
            "inspect",
            "backup_file_unavailable",
            "CoffeePOS cannot resolve the selected backup file.",
            "Choose an existing readable backup file and retry.",
        )
    })?;
    reject_reparse_existing_ancestors(&canonical, "inspect")?;
    Ok(canonical)
}

fn capture_source_fingerprint(
    path: &Path,
    action: &str,
) -> Result<RestoreSourceFingerprint, RestoreErrorInfo> {
    let mut file = File::open(path).map_err(|_| {
        restore_error(
            action,
            "backup_file_unavailable",
            "CoffeePOS cannot open the selected backup file.",
            "Choose an existing readable backup file and retry.",
        )
    })?;
    let before = file.metadata().map_err(|_| source_identity_error(action))?;
    if !before.file_type().is_file() || metadata_is_reparse_point(&before) {
        return Err(restore_error(
            action,
            "unsafe_backup_source",
            "CoffeePOS accepts only regular, non-reparse backup files.",
            "Choose a regular local backup file and retry.",
        ));
    }
    let (volume_serial_number, file_index) = native_file_identity(&file, action)?;
    let mut sha = Sha256::new();
    let mut buffer = vec![0_u8; FILE_HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            restore_error(
                action,
                "backup_file_read_failed",
                "CoffeePOS cannot read the selected backup while binding its identity.",
                "Check the backup file and storage device, then retry inspection.",
            )
        })?;
        if read == 0 {
            break;
        }
        sha.update(&buffer[..read]);
    }
    let after = file.metadata().map_err(|_| source_identity_error(action))?;
    if before.len() != after.len()
        || metadata_modified_ticks(&before) != metadata_modified_ticks(&after)
    {
        return Err(stale_candidate_error(action));
    }
    let canonical_path_sha256 = sha256_bytes(path.to_string_lossy().as_bytes());
    Ok(RestoreSourceFingerprint {
        canonical_path_sha256,
        volume_serial_number,
        file_index,
        size_bytes: after.len(),
        modified_ticks: metadata_modified_ticks(&after),
        encrypted_sha256: hex_lower(&sha.finalize()),
    })
}

fn source_identity_error(action: &str) -> RestoreErrorInfo {
    restore_error(
        action,
        "backup_identity_unavailable",
        "CoffeePOS cannot read the selected backup file identity safely.",
        "Choose a regular local backup file and retry.",
    )
}

#[cfg(windows)]
fn native_file_identity(
    file: &File,
    action: &str,
) -> Result<(Option<u32>, Option<u64>), RestoreErrorInfo> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let result = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if result == 0 {
        return Err(source_identity_error(action));
    }
    Ok((
        Some(info.dwVolumeSerialNumber),
        Some((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)),
    ))
}

#[cfg(not(windows))]
fn native_file_identity(
    _file: &File,
    _action: &str,
) -> Result<(Option<u32>, Option<u64>), RestoreErrorInfo> {
    Ok((None, None))
}

#[cfg(windows)]
fn metadata_modified_ticks(metadata: &fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    metadata.last_write_time()
}

#[cfg(not(windows))]
fn metadata_modified_ticks(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

pub(crate) fn persist_restore_journal(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    validate_restore_journal(journal)?;
    let path = data_root.join(RESTORE_JOURNAL);
    let parent = path
        .parent()
        .ok_or_else(|| journal_storage_error("persist"))?;
    fs::create_dir_all(parent).map_err(|_| journal_storage_error("persist"))?;
    reject_reparse_existing_ancestors(parent, "persist journal")?;
    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|_| journal_storage_error("persist"))?;
    serde_json::to_writer_pretty(&mut temporary, journal)
        .map_err(|_| journal_storage_error("persist"))?;
    temporary
        .write_all(b"\n")
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| journal_storage_error("persist"))?;
    temporary
        .persist(&path)
        .map_err(|_| journal_storage_error("persist"))?;
    Ok(())
}

pub(crate) fn load_restore_journal(
    data_root: &Path,
) -> Result<Option<RestoreJournal>, RestoreErrorInfo> {
    let path = data_root.join(RESTORE_JOURNAL);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(journal_storage_error("load")),
    };
    let journal: RestoreJournal = serde_json::from_slice(&bytes).map_err(|_| {
        restore_error(
            "load journal",
            "restore_journal_invalid",
            "config/restore.json is unreadable or has an unsupported structure.",
            "Keep runtime admission blocked and preserve the journal plus restore recovery evidence for explicit recovery.",
        )
    })?;
    validate_restore_journal(&journal)?;
    Ok(Some(journal))
}

/// Retire the durable restore marker only after terminal cleanup has removed every transient
/// transaction root. The caller still owns admission-gate release/startup-policy sequencing; this
/// helper makes journal deletion itself fail closed against stale or replaced on-disk state.
pub(crate) fn retire_terminal_restore_journal(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if !journal.stage.terminal() {
        return Err(restore_error(
            "retire journal",
            "restore_not_terminal",
            "CoffeePOS cannot retire a non-terminal restore journal.",
            "Complete restore, rollback, or abort reconciliation before retiring config/restore.json.",
        ));
    }
    validate_restore_journal(journal)?;

    for kind in [
        RestoreOwnedRootKind::Staging,
        RestoreOwnedRootKind::Rollback,
        RestoreOwnedRootKind::Quarantine,
    ] {
        if inspect_owned_root_state(data_root, journal, kind) != RestoreOwnedEvidenceState::Missing
        {
            return Err(restore_journal_retire_error());
        }
    }
    if journal.original_state == RestoreOriginalState::NoPreviousStore {
        for kind in [
            RestoreOwnedRootKind::ProtectedState,
            RestoreOwnedRootKind::RecoveryBackup,
        ] {
            if inspect_owned_root_state(data_root, journal, kind)
                != RestoreOwnedEvidenceState::Missing
            {
                return Err(restore_journal_retire_error());
            }
        }
    } else {
        for kind in [
            RestoreOwnedRootKind::ProtectedState,
            RestoreOwnedRootKind::RecoveryBackup,
        ] {
            if matches!(
                inspect_owned_root_state(data_root, journal, kind),
                RestoreOwnedEvidenceState::Mismatch | RestoreOwnedEvidenceState::Unsafe
            ) {
                return Err(restore_journal_retire_error());
            }
        }
    }

    let path = data_root.join(RESTORE_JOURNAL);
    let metadata = fs::symlink_metadata(&path).map_err(|_| restore_journal_retire_error())?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(restore_journal_retire_error());
    }
    reject_reparse_existing_ancestors(&path, "retire journal")?;
    let bytes = fs::read(&path).map_err(|_| restore_journal_retire_error())?;
    let on_disk: RestoreJournal =
        serde_json::from_slice(&bytes).map_err(|_| restore_journal_retire_error())?;
    validate_restore_journal(&on_disk)?;
    if on_disk.transaction_id != journal.transaction_id
        || on_disk.stage != journal.stage
        || on_disk != *journal
    {
        return Err(restore_journal_retire_error());
    }
    fs::remove_file(&path).map_err(|_| restore_journal_retire_error())?;
    if path.exists() {
        return Err(restore_journal_retire_error());
    }
    Ok(())
}

pub(crate) fn advance_restore_journal(
    data_root: &Path,
    journal: &mut RestoreJournal,
    next: RestoreStage,
) -> Result<(), RestoreErrorInfo> {
    journal.advance(next)?;
    persist_restore_journal(data_root, journal)
}

pub(crate) fn record_restore_error(
    data_root: &Path,
    journal: &mut RestoreJournal,
    code: &str,
    message: &str,
) -> Result<(), RestoreErrorInfo> {
    let safe_code = sanitize_safe_text(code, 96);
    let safe_message = sanitize_safe_text(message, MAX_SAFE_ERROR_BYTES);
    journal.safe_error = Some(RestoreSafeError {
        code: safe_code,
        message: safe_message,
    });
    journal.updated_at_unix_seconds = unix_seconds();
    persist_restore_journal(data_root, journal)
}

fn validate_restore_journal(journal: &RestoreJournal) -> Result<(), RestoreErrorInfo> {
    if journal.schema_version != RESTORE_JOURNAL_SCHEMA_VERSION
        || !valid_hex_id(&journal.transaction_id, 32)
        || journal.backup_id.trim().is_empty()
        || !valid_sha256(&journal.validated_projection_sha256)
        || !valid_sha256(&journal.source_file.canonical_path_sha256)
        || !valid_sha256(&journal.source_file.encrypted_sha256)
    {
        return Err(restore_error(
            "validate journal",
            "restore_journal_invalid",
            "The restore journal failed its schema or transaction binding checks.",
            "Keep runtime admission blocked and preserve config/restore.json plus owned recovery evidence for explicit recovery.",
        ));
    }
    let expected = RestoreOwnedPaths::for_transaction(&journal.transaction_id);
    if journal.owned_paths != expected {
        return Err(restore_error(
            "validate journal",
            "restore_journal_path_mismatch",
            "The restore journal contains owned paths that do not match its transaction id.",
            "Do not follow the recorded paths. Keep runtime blocked and recover from trusted transaction evidence.",
        ));
    }
    Ok(())
}

fn journal_storage_error(action: &str) -> RestoreErrorInfo {
    restore_error(
        "restore journal",
        "restore_journal_io_failed",
        format!("CoffeePOS could not {action} the durable restore journal."),
        "Keep runtime admission blocked, check application-data permissions/free space, and retry recovery without deleting restore evidence.",
    )
}

fn restore_journal_retire_error() -> RestoreErrorInfo {
    restore_error(
        "retire journal",
        "restore_journal_retire_unconfirmed",
        "CoffeePOS could not prove that the terminal restore journal is safe to retire.",
        "Keep restore admission fenced and preserve config/restore.json until terminal cleanup and journal identity can be reconciled.",
    )
}

fn sanitize_safe_text(value: &str, max_bytes: usize) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_control() {
            continue;
        }
        if output.len() + ch.len_utf8() > max_bytes {
            break;
        }
        output.push(ch);
    }
    output
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RestoreOwnedRootKind {
    Staging,
    Rollback,
    Quarantine,
    ProtectedState,
    RecoveryBackup,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RestoreOwnershipMarker {
    schema_version: u32,
    transaction_id: String,
    backup_id: String,
    kind: RestoreOwnedRootKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RestoreComponentMarker {
    schema_version: u32,
    transaction_id: String,
    backup_id: String,
    component: RestoreComponent,
}

pub(crate) fn prepare_restore_owned_roots(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    validate_restore_journal(journal)?;
    ensure_owned_root(data_root, journal, RestoreOwnedRootKind::Staging)?;
    ensure_owned_root(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    ensure_owned_root(data_root, journal, RestoreOwnedRootKind::Quarantine)?;
    ensure_owned_root(data_root, journal, RestoreOwnedRootKind::ProtectedState)?;
    if journal.original_state == RestoreOriginalState::ExistingStore {
        ensure_owned_root(data_root, journal, RestoreOwnedRootKind::RecoveryBackup)?;
    }
    Ok(())
}

pub(crate) fn staging_store_root(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<PathBuf, RestoreErrorInfo> {
    let staging = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&staging, journal, RestoreOwnedRootKind::Staging)?;
    let store = staging.join("store");
    match fs::symlink_metadata(&store) {
        Ok(metadata)
            if metadata.file_type().is_dir()
                && !metadata.file_type().is_symlink()
                && !metadata_is_reparse_point(&metadata) => {}
        Ok(_) => return Err(owned_path_error("prepare staging store")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&store).map_err(|_| owned_path_error("prepare staging store"))?;
        }
        Err(_) => return Err(owned_path_error("prepare staging store")),
    }
    reject_reparse_existing_ancestors(&store, "prepare staging store")?;
    Ok(store)
}

pub(crate) fn recovery_backup_root(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<PathBuf, RestoreErrorInfo> {
    if journal.original_state != RestoreOriginalState::ExistingStore {
        return Err(restore_error(
            "prepare recovery backup",
            "recovery_backup_not_required",
            "A fresh restore does not have a previous store to snapshot.",
            "Continue the fresh restore without creating a placeholder recovery archive.",
        ));
    }
    let root = owned_root_path(data_root, journal, RestoreOwnedRootKind::RecoveryBackup)?;
    verify_owned_root(&root, journal, RestoreOwnedRootKind::RecoveryBackup)?;
    Ok(root)
}

pub(crate) struct RestoreTargetSecrets {
    pub runtime_database_password: Zeroizing<String>,
    pub wordpress_database_password: Zeroizing<String>,
    pub pending_machine_token: Zeroizing<String>,
}

pub(crate) fn prepare_target_secrets(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<RestoreTargetSecrets, RestoreErrorInfo> {
    let staging = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&staging, journal, RestoreOwnedRootKind::Staging)?;
    let config = staging.join("store/config");
    fs::create_dir_all(&config).map_err(|_| secret_prepare_error())?;
    reject_reparse_existing_ancestors(&config, "prepare target secrets")?;

    let runtime_path = staging.join("store").join(DATABASE_RUNTIME_SECRET);
    let wordpress_path = staging.join("store").join(DATABASE_WORDPRESS_SECRET);
    let pending_machine_path = staging.join("store").join(MACHINE_TOKEN_PENDING_SECRET);
    let active_machine_path = staging.join("store").join(MACHINE_TOKEN_SECRET);

    if active_machine_path.exists() {
        return Err(restore_error(
            "prepare target secrets",
            "machine_token_promoted_too_early",
            "The staging store already contains an active machine token before server-side binding is verified.",
            "Keep the restore fenced and recover the staging transaction from its journaled stage.",
        ));
    }

    let runtime_database_password = secret::create(&runtime_path)
        .map(Zeroizing::new)
        .map_err(|_| secret_prepare_error())?;
    let wordpress_database_password = secret::create(&wordpress_path)
        .map(Zeroizing::new)
        .map_err(|_| secret_prepare_error())?;

    let pending_machine_token = secret::create_machine_token(&pending_machine_path)
        .map(Zeroizing::new)
        .map_err(|_| secret_prepare_error())?;
    Ok(RestoreTargetSecrets {
        runtime_database_password,
        wordpress_database_password,
        pending_machine_token,
    })
}

/// Protect the portable administrator password only after the caller has verified it against the
/// restored WordPress account/hash in isolated staging. This keeps an unverified portable secret
/// from ever becoming the target profile's authoritative Copy-password credential.
pub(crate) fn protect_verified_administrator_password(
    data_root: &Path,
    journal: &RestoreJournal,
    administrator_password: &str,
) -> Result<(), RestoreErrorInfo> {
    if administrator_password.is_empty() {
        return Err(restore_error(
            "protect restored administrator",
            "administrator_secret_missing",
            "The verified portable administrator password is empty.",
            "Re-extract and verify the portable administrator credential in staging before retrying.",
        ));
    }
    let staging = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&staging, journal, RestoreOwnedRootKind::Staging)?;
    let admin_path = staging.join("store").join(WORDPRESS_ADMIN_SECRET);
    match secret::load(&admin_path) {
        Ok(existing) => {
            if existing != administrator_password {
                return Err(restore_error(
                    "protect restored administrator",
                    "administrator_secret_binding_mismatch",
                    "The protected staging administrator credential does not match the verified portable credential.",
                    "Keep restore fenced and recover from a clean transaction-owned staging state.",
                ));
            }
        }
        Err(_) if !admin_path.exists() => {
            secret::store_password(&admin_path, administrator_password)
                .map_err(|_| secret_prepare_error())?;
        }
        Err(_) => return Err(secret_prepare_error()),
    }
    let verified = Zeroizing::new(secret::load(&admin_path).map_err(|_| secret_prepare_error())?);
    if verified.as_str() != administrator_password {
        return Err(restore_error(
            "protect restored administrator",
            "administrator_secret_binding_mismatch",
            "The target-protected administrator credential failed verification after persistence.",
            "Keep restore fenced and preserve staging evidence before retrying.",
        ));
    }
    Ok(())
}

pub(crate) fn promote_target_machine_token(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    let staging = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&staging, journal, RestoreOwnedRootKind::Staging)?;
    let pending = staging.join("store").join(MACHINE_TOKEN_PENDING_SECRET);
    let active = staging.join("store").join(MACHINE_TOKEN_SECRET);
    let token = Zeroizing::new(secret::load(&pending).map_err(|_| secret_prepare_error())?);
    secret::store_machine_token(&active, &token).map_err(|_| secret_prepare_error())?;
    let active_token = Zeroizing::new(secret::load(&active).map_err(|_| secret_prepare_error())?);
    if *active_token != *token {
        return Err(restore_error(
            "promote target machine token",
            "machine_token_promotion_failed",
            "The target-protected active machine token did not verify after promotion.",
            "Keep restore fenced and preserve the staging credential files for recovery.",
        ));
    }
    fs::remove_file(&pending).map_err(|_| {
        restore_error(
            "promote target machine token",
            "machine_token_pending_cleanup_failed",
            "CoffeePOS could not remove the protected pending machine token after promotion.",
            "Keep restore fenced and retry cleanup before staging verification.",
        )
    })?;
    Ok(())
}

pub(crate) fn prepare_recovery_snapshot_password(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<Zeroizing<String>, RestoreErrorInfo> {
    if journal.original_state != RestoreOriginalState::ExistingStore {
        return Err(restore_error(
            "prepare recovery backup",
            "recovery_backup_not_required",
            "A fresh restore does not have a previous store to snapshot.",
            "Continue without creating a recovery password for a nonexistent previous store.",
        ));
    }
    let protected = owned_root_path(data_root, journal, RestoreOwnedRootKind::ProtectedState)?;
    verify_owned_root(&protected, journal, RestoreOwnedRootKind::ProtectedState)?;
    let path = protected.join("recovery-password.secret");
    secret::create(&path).map(Zeroizing::new).map_err(|_| {
        restore_error(
            "prepare recovery backup",
            "recovery_password_protection_failed",
            "CoffeePOS could not create the target-protected recovery snapshot password.",
            "Keep restore fenced, check the Windows user profile credential store, and retry.",
        )
    })
}

fn secret_prepare_error() -> RestoreErrorInfo {
    restore_error(
        "prepare target secrets",
        "target_secret_protection_failed",
        "CoffeePOS could not create or verify target-protected restore credentials.",
        "Keep restore fenced, preserve staging evidence, and check the current Windows user profile before retrying.",
    )
}

fn ensure_owned_root(
    data_root: &Path,
    journal: &RestoreJournal,
    kind: RestoreOwnedRootKind,
) -> Result<PathBuf, RestoreErrorInfo> {
    let path = owned_root_path(data_root, journal, kind)?;
    if path.exists() {
        verify_owned_root(&path, journal, kind)?;
        return Ok(path);
    }
    let parent = path.parent().ok_or_else(|| owned_path_error("prepare"))?;
    fs::create_dir_all(parent).map_err(|_| owned_path_error("prepare"))?;
    reject_reparse_existing_ancestors(parent, "prepare restore staging")?;
    fs::create_dir(&path).map_err(|_| owned_path_error("prepare"))?;
    let marker = RestoreOwnershipMarker {
        schema_version: RESTORE_OWNERSHIP_SCHEMA_VERSION,
        transaction_id: journal.transaction_id.clone(),
        backup_id: journal.backup_id.clone(),
        kind,
    };
    atomic_json_write(&path.join(RESTORE_OWNERSHIP_MARKER), &marker, "prepare")?;
    verify_owned_root(&path, journal, kind)?;
    Ok(path)
}

fn verify_owned_root(
    path: &Path,
    journal: &RestoreJournal,
    kind: RestoreOwnedRootKind,
) -> Result<(), RestoreErrorInfo> {
    let metadata = fs::symlink_metadata(path).map_err(|_| owned_path_error("verify"))?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(owned_path_error("verify"));
    }
    reject_reparse_existing_ancestors(path, "verify restore ownership")?;
    let marker_path = path.join(RESTORE_OWNERSHIP_MARKER);
    let marker_metadata =
        fs::symlink_metadata(&marker_path).map_err(|_| owned_path_error("verify"))?;
    if !marker_metadata.file_type().is_file()
        || marker_metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&marker_metadata)
    {
        return Err(owned_path_error("verify"));
    }
    reject_reparse_existing_ancestors(&marker_path, "verify restore ownership marker")?;
    let mut marker_file =
        open_restore_marker_nofollow(&marker_path).map_err(|_| owned_path_error("verify"))?;
    let opened_metadata = marker_file
        .metadata()
        .map_err(|_| owned_path_error("verify"))?;
    if !opened_metadata.file_type().is_file()
        || opened_metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&opened_metadata)
    {
        return Err(owned_path_error("verify"));
    }
    let marker: RestoreOwnershipMarker =
        serde_json::from_reader(&mut marker_file).map_err(|_| owned_path_error("verify"))?;
    let expected = RestoreOwnershipMarker {
        schema_version: RESTORE_OWNERSHIP_SCHEMA_VERSION,
        transaction_id: journal.transaction_id.clone(),
        backup_id: journal.backup_id.clone(),
        kind,
    };
    if marker != expected {
        return Err(owned_path_error("verify"));
    }
    Ok(())
}

fn owned_root_path(
    data_root: &Path,
    journal: &RestoreJournal,
    kind: RestoreOwnedRootKind,
) -> Result<PathBuf, RestoreErrorInfo> {
    validate_restore_journal(journal)?;
    let relative = match kind {
        RestoreOwnedRootKind::Staging => &journal.owned_paths.staging,
        RestoreOwnedRootKind::Rollback => &journal.owned_paths.rollback,
        RestoreOwnedRootKind::Quarantine => &journal.owned_paths.quarantine,
        RestoreOwnedRootKind::ProtectedState => &journal.owned_paths.protected_state,
        RestoreOwnedRootKind::RecoveryBackup => &journal.owned_paths.recovery_backup,
    };
    resolve_owned_relative(data_root, relative)
}

fn resolve_owned_relative(data_root: &Path, relative: &str) -> Result<PathBuf, RestoreErrorInfo> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(owned_path_error("resolve"));
    }
    Ok(data_root.join(relative_path))
}

fn owned_path_error(action: &str) -> RestoreErrorInfo {
    restore_error(
        "restore ownership",
        "restore_ownership_unconfirmed",
        format!("CoffeePOS could not {action} a transaction-owned restore path safely."),
        "Keep runtime admission blocked and preserve restore staging/rollback evidence for explicit recovery.",
    )
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RestoreOwnedEvidenceState {
    Missing,
    Owned,
    Mismatch,
    Unsafe,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct RestoreRecoveryEvidence {
    pub staging: RestoreOwnedEvidenceState,
    pub rollback: RestoreOwnedEvidenceState,
    pub quarantine: RestoreOwnedEvidenceState,
    pub protected_state: RestoreOwnedEvidenceState,
    pub recovery_backup: RestoreOwnedEvidenceState,
    pub managed_children_stopped: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RestoreRecoveryAction {
    CleanupPreMutation,
    ResumeOrAbortPreCutover,
    CompleteCutoverOrRollback,
    VerifyActiveOrRollback,
    CommitVerifiedActive,
    CompleteRollback,
    ReconcileRolledBack,
    CompleteAbort,
    ReconcileAborted,
    ReconcileCommitted,
    Blocked,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct RestoreRecoveryClassification {
    pub action: RestoreRecoveryAction,
    pub reason_code: String,
}

pub(crate) fn inspect_restore_recovery_evidence(
    data_root: &Path,
    journal: &RestoreJournal,
    managed_children_stopped: bool,
) -> RestoreRecoveryEvidence {
    RestoreRecoveryEvidence {
        staging: inspect_owned_root_state(data_root, journal, RestoreOwnedRootKind::Staging),
        rollback: inspect_owned_root_state(data_root, journal, RestoreOwnedRootKind::Rollback),
        quarantine: inspect_owned_root_state(data_root, journal, RestoreOwnedRootKind::Quarantine),
        protected_state: inspect_owned_root_state(
            data_root,
            journal,
            RestoreOwnedRootKind::ProtectedState,
        ),
        recovery_backup: inspect_owned_root_state(
            data_root,
            journal,
            RestoreOwnedRootKind::RecoveryBackup,
        ),
        managed_children_stopped,
    }
}

pub(crate) fn classify_restore_recovery(
    journal: &RestoreJournal,
    evidence: &RestoreRecoveryEvidence,
) -> RestoreRecoveryClassification {
    use RestoreRecoveryAction::*;
    use RestoreStage::*;

    let unsafe_evidence = [
        evidence.staging,
        evidence.rollback,
        evidence.quarantine,
        evidence.protected_state,
        evidence.recovery_backup,
    ]
    .iter()
    .any(|state| {
        matches!(
            state,
            RestoreOwnedEvidenceState::Mismatch | RestoreOwnedEvidenceState::Unsafe
        )
    });
    if unsafe_evidence {
        return recovery_classification(Blocked, "owned_evidence_untrusted");
    }

    match journal.stage {
        Planned | Validated => recovery_classification(CleanupPreMutation, "pre_mutation"),
        RuntimeStopped | RecoveryBackupReady | StagingPrepared | DatabaseImported
        | UploadsRestored | TargetSecretsBound | StagingVerified => {
            if journal.original_state == RestoreOriginalState::ExistingStore
                && journal.stage >= RecoveryBackupReady
                && evidence.recovery_backup != RestoreOwnedEvidenceState::Owned
            {
                return recovery_classification(Blocked, "recovery_backup_evidence_missing");
            }
            if journal.stage >= StagingPrepared
                && evidence.staging != RestoreOwnedEvidenceState::Owned
            {
                return recovery_classification(Blocked, "staging_evidence_missing");
            }
            recovery_classification(ResumeOrAbortPreCutover, "pre_cutover")
        }
        CutoverStarted => {
            if evidence.rollback != RestoreOwnedEvidenceState::Owned
                || evidence.quarantine != RestoreOwnedEvidenceState::Owned
            {
                return recovery_classification(Blocked, "cutover_evidence_missing");
            }
            recovery_classification(CompleteCutoverOrRollback, "cutover_in_progress")
        }
        ActiveSwapped => {
            if evidence.rollback != RestoreOwnedEvidenceState::Owned
                || evidence.quarantine != RestoreOwnedEvidenceState::Owned
            {
                return recovery_classification(Blocked, "rollback_evidence_missing");
            }
            recovery_classification(VerifyActiveOrRollback, "active_swapped")
        }
        ActiveVerified => {
            if !evidence.managed_children_stopped {
                return recovery_classification(Blocked, "verification_runtime_still_active");
            }
            if evidence.rollback != RestoreOwnedEvidenceState::Owned
                || evidence.quarantine != RestoreOwnedEvidenceState::Owned
            {
                return recovery_classification(Blocked, "verified_ownership_evidence_missing");
            }
            recovery_classification(CommitVerifiedActive, "active_verified")
        }
        RollbackStarted => recovery_classification(CompleteRollback, "rollback_in_progress"),
        RolledBack => recovery_classification(ReconcileRolledBack, "rolled_back"),
        AbortStarted => recovery_classification(CompleteAbort, "abort_in_progress"),
        Aborted => recovery_classification(ReconcileAborted, "aborted"),
        Committed | Cleanup => recovery_classification(ReconcileCommitted, "committed"),
    }
}

fn recovery_classification(
    action: RestoreRecoveryAction,
    reason_code: &str,
) -> RestoreRecoveryClassification {
    RestoreRecoveryClassification {
        action,
        reason_code: reason_code.into(),
    }
}

fn inspect_owned_root_state(
    data_root: &Path,
    journal: &RestoreJournal,
    kind: RestoreOwnedRootKind,
) -> RestoreOwnedEvidenceState {
    let path = match owned_root_path(data_root, journal, kind) {
        Ok(path) => path,
        Err(_) => return RestoreOwnedEvidenceState::Unsafe,
    };
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return RestoreOwnedEvidenceState::Missing;
        }
        Err(_) => return RestoreOwnedEvidenceState::Unsafe,
    };
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return RestoreOwnedEvidenceState::Unsafe;
    }
    match verify_owned_root(&path, journal, kind) {
        Ok(()) => RestoreOwnedEvidenceState::Owned,
        Err(error) if error.code == "restore_ownership_unconfirmed" => {
            RestoreOwnedEvidenceState::Mismatch
        }
        Err(_) => RestoreOwnedEvidenceState::Unsafe,
    }
}

pub(crate) fn begin_restore_cutover(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if journal.stage != RestoreStage::StagingVerified {
        return Err(restore_error(
            "begin cutover",
            "staging_not_verified",
            "Restore cutover requires a fully verified staging store.",
            "Keep the active store unchanged and finish staging verification first.",
        ));
    }
    let staging = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    let rollback = owned_root_path(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    let quarantine = owned_root_path(data_root, journal, RestoreOwnedRootKind::Quarantine)?;
    verify_owned_root(&staging, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&rollback, journal, RestoreOwnedRootKind::Rollback)?;
    verify_owned_root(&quarantine, journal, RestoreOwnedRootKind::Quarantine)?;

    for component in [
        RestoreComponent::Site,
        RestoreComponent::Database,
        RestoreComponent::Uploads,
    ] {
        prepare_staged_component_marker(data_root, journal, component)?;
    }
    advance_restore_journal(data_root, journal, RestoreStage::CutoverStarted)
}

pub(crate) fn cutover_component(
    data_root: &Path,
    journal: &mut RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    if journal.stage != RestoreStage::CutoverStarted {
        return Err(restore_error(
            "cutover component",
            "cutover_not_started",
            "Restore component swap was requested before the durable cutover boundary.",
            "Persist cutover_started before moving active store directories.",
        ));
    }
    let staging_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    let rollback_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    verify_owned_root(&staging_root, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&rollback_root, journal, RestoreOwnedRootKind::Rollback)?;

    let name = component.directory_name();
    let staged = staging_root.join("store").join(name);
    let active = data_root.join(name);
    let previous = rollback_root.join(name);

    reconcile_component_cutover_evidence(data_root, journal, component)?;
    if journal.cutover.component(component).staging_moved_to_active {
        verify_active_component_marker(&active, journal, component)?;
        return Ok(());
    }

    match journal.original_state {
        RestoreOriginalState::ExistingStore => {
            if !journal
                .cutover
                .component(component)
                .active_moved_to_rollback
            {
                verify_regular_directory(&active, "cutover active component")?;
                ensure_path_absent(&previous, "cutover rollback destination")?;
                prepare_previous_component_marker(&active, journal, component)?;
                fs::rename(&active, &previous).map_err(|_| cutover_io_error(component))?;
                journal
                    .cutover
                    .component_mut(component)
                    .active_moved_to_rollback = true;
                journal.updated_at_unix_seconds = unix_seconds();
                persist_restore_journal(data_root, journal)?;
            }
        }
        RestoreOriginalState::NoPreviousStore => {
            if !journal
                .cutover
                .component(component)
                .fresh_placeholder_removed
            {
                if active.exists() {
                    verify_empty_directory(&active, "cutover fresh placeholder")?;
                    fs::remove_dir(&active).map_err(|_| cutover_io_error(component))?;
                }
                journal
                    .cutover
                    .component_mut(component)
                    .fresh_placeholder_removed = true;
                journal.updated_at_unix_seconds = unix_seconds();
                persist_restore_journal(data_root, journal)?;
            }
            ensure_path_absent(&previous, "fresh rollback destination")?;
        }
    }

    verify_staged_component_marker(&staged, journal, component)?;
    ensure_path_absent(&active, "cutover active destination")?;
    fs::rename(&staged, &active).map_err(|_| cutover_io_error(component))?;
    verify_active_component_marker(&active, journal, component)?;
    journal
        .cutover
        .component_mut(component)
        .staging_moved_to_active = true;
    journal.updated_at_unix_seconds = unix_seconds();
    persist_restore_journal(data_root, journal)
}

pub(crate) fn mark_active_swapped(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    for component in [
        RestoreComponent::Site,
        RestoreComponent::Database,
        RestoreComponent::Uploads,
    ] {
        let evidence = journal.cutover.component(component);
        if !evidence.staging_moved_to_active {
            return Err(restore_error(
                "finish cutover",
                "cutover_incomplete",
                format!("Restore component {:?} has not been swapped into the active store.", component),
                "Keep admission blocked and complete or roll back the journaled cutover transaction.",
            ));
        }
    }
    if !journal.cutover.config_applied {
        return Err(restore_error(
            "finish cutover",
            "config_cutover_incomplete",
            "Target configuration has not been durably applied.",
            "Apply target-local config and protected credentials before marking the active store swapped.",
        ));
    }
    advance_restore_journal(data_root, journal, RestoreStage::ActiveSwapped)
}

const RESTORE_CONFIG_FILES: &[&str] = &[
    "config/app.json",
    "config/provisioning.json",
    "config/wordpress-router.php",
    DATABASE_RUNTIME_SECRET,
    DATABASE_WORDPRESS_SECRET,
    WORDPRESS_ADMIN_SECRET,
    MACHINE_TOKEN_SECRET,
];

pub(crate) fn apply_target_config(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if journal.stage != RestoreStage::CutoverStarted {
        return Err(restore_error(
            "apply target config",
            "cutover_not_started",
            "Target configuration cannot be applied before cutover_started is durable.",
            "Persist the cutover boundary and keep runtime stopped before applying target config.",
        ));
    }
    let staging_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    let rollback_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    verify_owned_root(&staging_root, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&rollback_root, journal, RestoreOwnedRootKind::Rollback)?;
    let rollback_config = rollback_root.join("config");
    fs::create_dir_all(&rollback_config).map_err(|_| config_cutover_error())?;
    reject_reparse_existing_ancestors(&rollback_config, "snapshot config")?;

    for relative in RESTORE_CONFIG_FILES {
        reconcile_or_initialize_config_evidence(data_root, &staging_root, journal, relative)?;
        let index = journal
            .cutover
            .config_files
            .iter()
            .position(|item| item.relative_path == *relative)
            .ok_or_else(config_cutover_error)?;

        if !journal.cutover.config_files[index].snapshot_ready {
            let evidence = journal.cutover.config_files[index].clone();
            if evidence.previous_existed {
                let source = data_root.join(relative);
                let snapshot = rollback_root.join(relative);
                let parent = snapshot.parent().ok_or_else(config_cutover_error)?;
                fs::create_dir_all(parent).map_err(|_| config_cutover_error())?;
                copy_file_atomic(&source, &snapshot, "snapshot config")?;
                let hash = sha256_file(&snapshot, "snapshot config")?;
                if Some(hash) != evidence.previous_sha256 {
                    return Err(config_cutover_error());
                }
            }
            journal.cutover.config_files[index].snapshot_ready = true;
            journal.updated_at_unix_seconds = unix_seconds();
            persist_restore_journal(data_root, journal)?;
        }

        if !journal.cutover.config_files[index].installed {
            let evidence = journal.cutover.config_files[index].clone();
            let target = staging_root.join("store").join(relative);
            let active = data_root.join(relative);
            if active.is_file()
                && sha256_file(&active, "apply target config")? == evidence.target_sha256
            {
                journal.cutover.config_files[index].installed = true;
                journal.updated_at_unix_seconds = unix_seconds();
                persist_restore_journal(data_root, journal)?;
                continue;
            }
            copy_file_atomic(&target, &active, "apply target config")?;
            if sha256_file(&active, "apply target config")? != evidence.target_sha256 {
                return Err(config_cutover_error());
            }
            journal.cutover.config_files[index].installed = true;
            journal.updated_at_unix_seconds = unix_seconds();
            persist_restore_journal(data_root, journal)?;
        }
    }

    journal.cutover.config_snapshot_ready = journal
        .cutover
        .config_files
        .iter()
        .all(|item| item.snapshot_ready);
    journal.cutover.config_applied = journal
        .cutover
        .config_files
        .iter()
        .all(|item| item.installed);
    journal.updated_at_unix_seconds = unix_seconds();
    persist_restore_journal(data_root, journal)
}

fn reconcile_or_initialize_config_evidence(
    data_root: &Path,
    staging_root: &Path,
    journal: &mut RestoreJournal,
    relative: &str,
) -> Result<(), RestoreErrorInfo> {
    if let Some(index) = journal
        .cutover
        .config_files
        .iter()
        .position(|item| item.relative_path == relative)
    {
        let evidence = journal.cutover.config_files[index].clone();
        if !valid_sha256(&evidence.target_sha256)
            || evidence
                .previous_sha256
                .as_deref()
                .is_some_and(|hash| !valid_sha256(hash))
        {
            return Err(config_cutover_error());
        }
        let target = staging_root.join("store").join(relative);
        if sha256_file(&target, "verify target config")? != evidence.target_sha256 {
            return Err(config_cutover_error());
        }
        if evidence.installed {
            let active = data_root.join(relative);
            if sha256_file(&active, "verify active config")? != evidence.target_sha256 {
                return Err(config_cutover_error());
            }
        }
        return Ok(());
    }

    let target = staging_root.join("store").join(relative);
    verify_regular_file(&target, "prepare target config")?;
    let target_sha256 = sha256_file(&target, "prepare target config")?;
    let active = data_root.join(relative);
    let (previous_existed, previous_sha256) = match fs::symlink_metadata(&active) {
        Ok(metadata) => {
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata_is_reparse_point(&metadata)
            {
                return Err(config_cutover_error());
            }
            (true, Some(sha256_file(&active, "snapshot config")?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (false, None),
        Err(_) => return Err(config_cutover_error()),
    };
    journal
        .cutover
        .config_files
        .push(RestoreConfigFileEvidence {
            relative_path: relative.into(),
            previous_existed,
            previous_sha256,
            target_sha256,
            snapshot_ready: false,
            installed: false,
            rollback_restored: false,
        });
    journal.updated_at_unix_seconds = unix_seconds();
    persist_restore_journal(data_root, journal)
}

pub(crate) fn rollback_restore_cutover(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if !matches!(
        journal.stage,
        RestoreStage::CutoverStarted | RestoreStage::ActiveSwapped | RestoreStage::RollbackStarted
    ) {
        return Err(restore_error(
            "rollback",
            "rollback_not_applicable",
            "Restore rollback was requested outside the cutover rollback window.",
            "Recover from the exact stage recorded in config/restore.json.",
        ));
    }
    if journal.stage != RestoreStage::RollbackStarted {
        advance_restore_journal(data_root, journal, RestoreStage::RollbackStarted)?;
    }
    let rollback_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    let quarantine_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Quarantine)?;
    verify_owned_root(&rollback_root, journal, RestoreOwnedRootKind::Rollback)?;
    verify_owned_root(&quarantine_root, journal, RestoreOwnedRootKind::Quarantine)?;

    rollback_target_config(data_root, journal, &rollback_root)?;
    for component in [
        RestoreComponent::Uploads,
        RestoreComponent::Database,
        RestoreComponent::Site,
    ] {
        rollback_component(data_root, journal, component)?;
    }
    journal.cutover.config_rolled_back = true;
    journal.updated_at_unix_seconds = unix_seconds();
    persist_restore_journal(data_root, journal)
}

fn rollback_target_config(
    data_root: &Path,
    journal: &mut RestoreJournal,
    rollback_root: &Path,
) -> Result<(), RestoreErrorInfo> {
    for index in 0..journal.cutover.config_files.len() {
        if journal.cutover.config_files[index].rollback_restored {
            continue;
        }
        let evidence = journal.cutover.config_files[index].clone();
        if !RESTORE_CONFIG_FILES.contains(&evidence.relative_path.as_str()) {
            return Err(config_cutover_error());
        }
        let active = data_root.join(&evidence.relative_path);
        if evidence.installed && active.exists() {
            let active_hash = sha256_file(&active, "rollback config")?;
            if active_hash != evidence.target_sha256 {
                return Err(restore_error(
                    "rollback config",
                    "active_config_identity_mismatch",
                    "An active restore-owned configuration file no longer matches its journaled target identity.",
                    "Keep restore fenced and preserve rollback evidence for explicit recovery.",
                ));
            }
        }
        if evidence.previous_existed {
            let snapshot = rollback_root.join(&evidence.relative_path);
            let expected = evidence
                .previous_sha256
                .as_deref()
                .ok_or_else(config_cutover_error)?;
            if sha256_file(&snapshot, "rollback config")? != expected {
                return Err(config_cutover_error());
            }
            copy_file_atomic(&snapshot, &active, "rollback config")?;
            if sha256_file(&active, "rollback config")? != expected {
                return Err(config_cutover_error());
            }
        } else if active.exists() {
            let metadata = fs::symlink_metadata(&active).map_err(|_| config_cutover_error())?;
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata_is_reparse_point(&metadata)
            {
                return Err(config_cutover_error());
            }
            fs::remove_file(&active).map_err(|_| config_cutover_error())?;
        }
        journal.cutover.config_files[index].rollback_restored = true;
        journal.updated_at_unix_seconds = unix_seconds();
        persist_restore_journal(data_root, journal)?;
    }
    Ok(())
}

fn rollback_component(
    data_root: &Path,
    journal: &mut RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    reconcile_component_cutover_evidence(data_root, journal, component)?;
    let evidence = journal.cutover.component(component).clone();
    if evidence.previous_restored {
        return Ok(());
    }
    let rollback_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    let quarantine_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Quarantine)?;
    let active = data_root.join(component.directory_name());
    let previous = rollback_root.join(component.directory_name());
    let failed = quarantine_root.join(component.directory_name());

    if evidence.staging_moved_to_active && !evidence.failed_target_quarantined {
        verify_active_component_marker(&active, journal, component)?;
        ensure_path_absent(&failed, "rollback quarantine destination")?;
        fs::rename(&active, &failed).map_err(|_| rollback_io_error(component))?;
        journal
            .cutover
            .component_mut(component)
            .failed_target_quarantined = true;
        journal.updated_at_unix_seconds = unix_seconds();
        persist_restore_journal(data_root, journal)?;
    }

    match journal.original_state {
        RestoreOriginalState::ExistingStore => {
            if !journal
                .cutover
                .component(component)
                .active_moved_to_rollback
            {
                return Err(rollback_io_error(component));
            }
            ensure_path_absent(&active, "rollback active destination")?;
            verify_regular_directory(&previous, "rollback previous component")?;
            fs::rename(&previous, &active).map_err(|_| rollback_io_error(component))?;
        }
        RestoreOriginalState::NoPreviousStore => {
            if previous.exists() {
                return Err(restore_error(
                    "rollback",
                    "unexpected_fresh_rollback_tree",
                    "A no_previous_store transaction unexpectedly contains a previous-store rollback tree.",
                    "Keep restore fenced and inspect transaction ownership before deleting or moving data.",
                ));
            }
            if active.exists() {
                verify_empty_directory(&active, "rollback fresh target")?;
            } else {
                fs::create_dir(&active).map_err(|_| rollback_io_error(component))?;
            }
        }
    }
    journal.cutover.component_mut(component).previous_restored = true;
    journal.updated_at_unix_seconds = unix_seconds();
    persist_restore_journal(data_root, journal)
}

fn config_cutover_error() -> RestoreErrorInfo {
    restore_error(
        "config cutover",
        "config_cutover_unconfirmed",
        "CoffeePOS could not prove a target configuration cutover or rollback step safely.",
        "Keep restore fenced and preserve config/restore.json plus rollback config snapshots for recovery.",
    )
}

fn cutover_io_error(component: RestoreComponent) -> RestoreErrorInfo {
    restore_error(
        "cutover",
        "cutover_move_failed",
        format!("CoffeePOS could not complete the {:?} directory cutover move.", component),
        "Keep runtime stopped and recover using the journal plus transaction-owned staging/rollback evidence.",
    )
}

fn rollback_io_error(component: RestoreComponent) -> RestoreErrorInfo {
    restore_error(
        "rollback",
        "rollback_move_failed",
        format!("CoffeePOS could not restore the previous {:?} directory safely.", component),
        "Keep runtime admission blocked and preserve all rollback/quarantine evidence for explicit recovery.",
    )
}

fn prepare_staged_component_marker(
    data_root: &Path,
    journal: &RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    let staging_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    verify_owned_root(&staging_root, journal, RestoreOwnedRootKind::Staging)?;
    let component_root = staging_root.join("store").join(component.directory_name());
    verify_regular_directory(&component_root, "prepare staged component")?;
    write_component_marker(
        &component_root,
        RESTORE_COMPONENT_MARKER,
        journal,
        component,
    )
}

fn prepare_previous_component_marker(
    component_root: &Path,
    journal: &RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    verify_regular_directory(component_root, "prepare previous component")?;
    write_component_marker(
        component_root,
        RESTORE_PREVIOUS_COMPONENT_MARKER,
        journal,
        component,
    )
}

fn write_component_marker(
    component_root: &Path,
    marker_name: &str,
    journal: &RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    let marker_path = component_root.join(marker_name);
    if marker_path.exists() {
        if component_marker_matches(component_root, marker_name, journal, component)? {
            return Ok(());
        }
        return Err(component_identity_error(component));
    }
    let marker = RestoreComponentMarker {
        schema_version: RESTORE_OWNERSHIP_SCHEMA_VERSION,
        transaction_id: journal.transaction_id.clone(),
        backup_id: journal.backup_id.clone(),
        component,
    };
    atomic_json_write(&marker_path, &marker, "write component marker")?;
    if !component_marker_matches(component_root, marker_name, journal, component)? {
        return Err(component_identity_error(component));
    }
    Ok(())
}

fn verify_staged_component_marker(
    component_root: &Path,
    journal: &RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    verify_regular_directory(component_root, "verify staged component")?;
    if component_marker_matches(component_root, RESTORE_COMPONENT_MARKER, journal, component)? {
        Ok(())
    } else {
        Err(component_identity_error(component))
    }
}

fn verify_active_component_marker(
    component_root: &Path,
    journal: &RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    verify_staged_component_marker(component_root, journal, component)
}

fn component_marker_matches(
    component_root: &Path,
    marker_name: &str,
    journal: &RestoreJournal,
    component: RestoreComponent,
) -> Result<bool, RestoreErrorInfo> {
    let marker_path = component_root.join(marker_name);
    let metadata = match fs::symlink_metadata(&marker_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(component_identity_error(component)),
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(component_identity_error(component));
    }
    reject_reparse_existing_ancestors(&marker_path, "verify restore component marker")
        .map_err(|_| component_identity_error(component))?;
    let mut marker_file = open_restore_marker_nofollow(&marker_path)
        .map_err(|_| component_identity_error(component))?;
    let opened_metadata = marker_file
        .metadata()
        .map_err(|_| component_identity_error(component))?;
    if !opened_metadata.file_type().is_file()
        || opened_metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&opened_metadata)
    {
        return Err(component_identity_error(component));
    }
    let marker: RestoreComponentMarker = serde_json::from_reader(&mut marker_file)
        .map_err(|_| component_identity_error(component))?;
    Ok(marker
        == (RestoreComponentMarker {
            schema_version: RESTORE_OWNERSHIP_SCHEMA_VERSION,
            transaction_id: journal.transaction_id.clone(),
            backup_id: journal.backup_id.clone(),
            component,
        }))
}

#[cfg(windows)]
fn open_restore_marker_nofollow(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
fn open_restore_marker_nofollow(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn reconcile_component_cutover_evidence(
    data_root: &Path,
    journal: &mut RestoreJournal,
    component: RestoreComponent,
) -> Result<(), RestoreErrorInfo> {
    let staging_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Staging)?;
    let rollback_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    let quarantine_root = owned_root_path(data_root, journal, RestoreOwnedRootKind::Quarantine)?;
    let staged = staging_root.join("store").join(component.directory_name());
    let active = data_root.join(component.directory_name());
    let previous = rollback_root.join(component.directory_name());
    let failed = quarantine_root.join(component.directory_name());

    let staged_target = if staged.exists() {
        verify_staged_component_marker(&staged, journal, component)?;
        true
    } else {
        false
    };
    let active_target = if active.exists() {
        component_marker_matches(&active, RESTORE_COMPONENT_MARKER, journal, component)?
    } else {
        false
    };
    let active_previous = if active.exists() {
        component_marker_matches(
            &active,
            RESTORE_PREVIOUS_COMPONENT_MARKER,
            journal,
            component,
        )?
    } else {
        false
    };
    let rollback_previous = if previous.exists() {
        verify_regular_directory(&previous, "reconcile rollback component")?;
        component_marker_matches(
            &previous,
            RESTORE_PREVIOUS_COMPONENT_MARKER,
            journal,
            component,
        )?
    } else {
        false
    };
    let failed_target = if failed.exists() {
        verify_regular_directory(&failed, "reconcile quarantine component")?;
        component_marker_matches(&failed, RESTORE_COMPONENT_MARKER, journal, component)?
    } else {
        false
    };

    if active_target && staged_target {
        return Err(component_identity_error(component));
    }

    let mut next = journal.cutover.component(component).clone();
    let before = next.clone();
    match journal.original_state {
        RestoreOriginalState::ExistingStore => {
            if rollback_previous {
                next.active_moved_to_rollback = true;
            }
            if active_target && !staged_target && rollback_previous {
                next.active_moved_to_rollback = true;
                next.staging_moved_to_active = true;
            }
            if failed_target {
                next.failed_target_quarantined = true;
            }
            if active_previous && !previous.exists() && next.active_moved_to_rollback {
                next.previous_restored = true;
            }
            if next.active_moved_to_rollback && previous.exists() && !rollback_previous {
                return Err(component_identity_error(component));
            }
            if next.staging_moved_to_active && active.exists() && !active_target && !active_previous
            {
                return Err(component_identity_error(component));
            }
        }
        RestoreOriginalState::NoPreviousStore => {
            if previous.exists() {
                return Err(restore_error(
                    "reconcile cutover",
                    "unexpected_fresh_rollback_tree",
                    "A fresh restore has an unexpected previous-store rollback component.",
                    "Keep restore fenced and inspect transaction ownership before modifying the rollback tree.",
                ));
            }
            if !active.exists() || active_target {
                next.fresh_placeholder_removed = true;
            }
            if active_target && !staged_target {
                next.staging_moved_to_active = true;
            }
            if failed_target {
                next.failed_target_quarantined = true;
            }
            if next.failed_target_quarantined && active.exists() && !active_target {
                verify_empty_directory(&active, "reconcile fresh rollback")?;
                next.previous_restored = true;
            }
        }
    }

    if next != before {
        *journal.cutover.component_mut(component) = next;
        journal.updated_at_unix_seconds = unix_seconds();
        persist_restore_journal(data_root, journal)?;
    }
    Ok(())
}

pub(crate) fn mark_restore_active_verified(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if journal.stage != RestoreStage::ActiveSwapped {
        return Err(restore_error(
            "verify active",
            "active_not_swapped",
            "The active restore cannot be marked verified before active_swapped.",
            "Run final maintenance verification against the journaled active target first.",
        ));
    }
    advance_restore_journal(data_root, journal, RestoreStage::ActiveVerified)
}

pub(crate) fn commit_verified_restore(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if journal.stage != RestoreStage::ActiveVerified {
        return Err(restore_error(
            "commit restore",
            "active_not_verified",
            "Restore commit requires a durably recorded active_verified stage.",
            "Keep admission blocked and complete final maintenance verification before commit.",
        ));
    }
    verify_active_verified_commit_evidence(data_root, journal)?;
    advance_restore_journal(data_root, journal, RestoreStage::Committed)
}

fn verify_active_verified_commit_evidence(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    validate_restore_journal(journal)?;
    if journal.stage != RestoreStage::ActiveVerified {
        return Err(active_verified_evidence_error());
    }

    for kind in [
        RestoreOwnedRootKind::Staging,
        RestoreOwnedRootKind::Rollback,
        RestoreOwnedRootKind::Quarantine,
        RestoreOwnedRootKind::ProtectedState,
    ] {
        let path = owned_root_path(data_root, journal, kind)?;
        verify_owned_root(&path, journal, kind).map_err(|_| active_verified_evidence_error())?;
    }
    if journal.original_state == RestoreOriginalState::ExistingStore {
        let path = owned_root_path(data_root, journal, RestoreOwnedRootKind::RecoveryBackup)?;
        verify_owned_root(&path, journal, RestoreOwnedRootKind::RecoveryBackup)
            .map_err(|_| active_verified_evidence_error())?;
    }

    for component in [
        RestoreComponent::Site,
        RestoreComponent::Database,
        RestoreComponent::Uploads,
    ] {
        let evidence = journal.cutover.component(component);
        if !evidence.staging_moved_to_active || evidence.previous_restored {
            return Err(active_verified_evidence_error());
        }
        let active = data_root.join(component.directory_name());
        verify_active_component_marker(&active, journal, component)
            .map_err(|_| active_verified_evidence_error())?;
    }

    if !journal.cutover.config_snapshot_ready
        || !journal.cutover.config_applied
        || journal.cutover.config_files.len() != RESTORE_CONFIG_FILES.len()
    {
        return Err(active_verified_evidence_error());
    }
    for relative in RESTORE_CONFIG_FILES {
        let mut matching = journal
            .cutover
            .config_files
            .iter()
            .filter(|item| item.relative_path == *relative);
        let evidence = matching.next().ok_or_else(active_verified_evidence_error)?;
        if matching.next().is_some()
            || !evidence.snapshot_ready
            || !evidence.installed
            || evidence.rollback_restored
            || !valid_sha256(&evidence.target_sha256)
            || (evidence.previous_existed
                && !evidence
                    .previous_sha256
                    .as_deref()
                    .map(valid_sha256)
                    .unwrap_or(false))
            || (!evidence.previous_existed && evidence.previous_sha256.is_some())
        {
            return Err(active_verified_evidence_error());
        }
        let active = data_root.join(relative);
        let active_hash = sha256_file(&active, "verify active commit evidence")
            .map_err(|_| active_verified_evidence_error())?;
        if active_hash != evidence.target_sha256 {
            return Err(active_verified_evidence_error());
        }
    }
    Ok(())
}

fn active_verified_evidence_error() -> RestoreErrorInfo {
    restore_error(
        "commit restore",
        "active_verified_evidence_mismatch",
        "CoffeePOS could not re-prove the transaction-owned active restore evidence before commit.",
        "Keep restore admission blocked and preserve config/restore.json plus rollback evidence for explicit recovery.",
    )
}

pub(crate) fn begin_pre_cutover_abort(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if !matches!(
        journal.stage,
        RestoreStage::Validated
            | RestoreStage::RuntimeStopped
            | RestoreStage::RecoveryBackupReady
            | RestoreStage::StagingPrepared
            | RestoreStage::DatabaseImported
            | RestoreStage::UploadsRestored
            | RestoreStage::TargetSecretsBound
            | RestoreStage::StagingVerified
    ) {
        return Err(restore_error(
            "abort restore",
            "abort_not_applicable",
            "Pre-cutover abort is not valid at the current restore stage.",
            "After cutover_started, use the explicit rollback transaction instead of abandoning restore.",
        ));
    }
    advance_restore_journal(data_root, journal, RestoreStage::AbortStarted)
}

pub(crate) fn mark_restore_aborted(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if journal.stage != RestoreStage::AbortStarted {
        return Err(restore_error(
            "abort restore",
            "abort_cleanup_not_started",
            "Restore cannot be marked aborted before abort_started cleanup/reconciliation.",
            "Keep the admission gate and complete owned staging cleanup plus previous-state verification first.",
        ));
    }
    advance_restore_journal(data_root, journal, RestoreStage::Aborted)
}

pub(crate) fn mark_restore_rolled_back(
    data_root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    if journal.stage != RestoreStage::RollbackStarted
        || !journal.cutover.config_rolled_back
        || [
            RestoreComponent::Site,
            RestoreComponent::Database,
            RestoreComponent::Uploads,
        ]
        .iter()
        .any(|component| !journal.cutover.component(*component).previous_restored)
    {
        return Err(restore_error(
            "rollback",
            "rollback_not_verified",
            "Restore rollback evidence is incomplete and cannot be marked rolled_back.",
            "Keep admission blocked and complete directory/config rollback plus previous-store verification.",
        ));
    }
    advance_restore_journal(data_root, journal, RestoreStage::RolledBack)
}

pub(crate) fn reconcile_terminal_component_markers(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<(), RestoreErrorInfo> {
    match journal.stage {
        RestoreStage::Committed | RestoreStage::Cleanup => {
            for component in [
                RestoreComponent::Site,
                RestoreComponent::Database,
                RestoreComponent::Uploads,
            ] {
                let active = data_root.join(component.directory_name());
                let marker = active.join(RESTORE_COMPONENT_MARKER);
                match fs::symlink_metadata(&marker) {
                    Ok(_) => {
                        if !component_marker_matches(
                            &active,
                            RESTORE_COMPONENT_MARKER,
                            journal,
                            component,
                        )? {
                            return Err(component_identity_error(component));
                        }
                        remove_regular_marker(&marker)?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => return Err(component_identity_error(component)),
                }
            }
        }
        RestoreStage::RolledBack => {
            for component in [
                RestoreComponent::Site,
                RestoreComponent::Database,
                RestoreComponent::Uploads,
            ] {
                let active = data_root.join(component.directory_name());
                let marker = active.join(RESTORE_PREVIOUS_COMPONENT_MARKER);
                match fs::symlink_metadata(&marker) {
                    Ok(_) => {
                        if !component_marker_matches(
                            &active,
                            RESTORE_PREVIOUS_COMPONENT_MARKER,
                            journal,
                            component,
                        )? {
                            return Err(component_identity_error(component));
                        }
                        remove_regular_marker(&marker)?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => return Err(component_identity_error(component)),
                }
            }
        }
        RestoreStage::Aborted => {}
        _ => {
            return Err(restore_error(
                "reconcile restore",
                "restore_not_terminal",
                "Restore ownership markers cannot be reconciled before a terminal stage.",
                "Finish restore, rollback, or abort verification before releasing the admission gate.",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct RestoreTerminalCleanupResult {
    pub staging_removed: bool,
    pub rollback_removed: bool,
    pub quarantine_removed: bool,
    pub protected_state_preserved: bool,
    pub recovery_backup_preserved: bool,
}

/// Reconcile transaction markers and remove only roots whose ownership marker still binds them to
/// this terminal transaction. Existing-store recovery archives and their protected recovery state
/// are retained together so the retained recovery archive never outlives its decrypt credential.
pub(crate) fn cleanup_terminal_restore_owned_roots(
    data_root: &Path,
    journal: &RestoreJournal,
) -> Result<RestoreTerminalCleanupResult, RestoreErrorInfo> {
    if !journal.stage.terminal() {
        return Err(restore_error(
            "cleanup restore",
            "restore_not_terminal",
            "Restore-owned roots cannot be removed before the transaction reaches a terminal stage.",
            "Keep the restore admission gate active and finish restore, rollback, or abort reconciliation first.",
        ));
    }
    validate_restore_journal(journal)?;
    reconcile_terminal_component_markers(data_root, journal)?;

    // Preflight every transaction-owned root before deleting any evidence. If one root has been
    // replaced, tampered with, or turned into a reparse tree, cleanup fails with all other roots
    // still intact for recovery.
    preflight_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::Staging)?;
    preflight_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    preflight_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::Quarantine)?;
    preflight_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::ProtectedState)?;
    preflight_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::RecoveryBackup)?;

    let staging_removed =
        cleanup_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::Staging)?;
    let rollback_removed =
        cleanup_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::Rollback)?;
    let quarantine_removed =
        cleanup_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::Quarantine)?;

    let (protected_state_preserved, recovery_backup_preserved) = if journal.original_state
        == RestoreOriginalState::ExistingStore
    {
        let protected = owned_root_path(data_root, journal, RestoreOwnedRootKind::ProtectedState)?;
        let recovery = owned_root_path(data_root, journal, RestoreOwnedRootKind::RecoveryBackup)?;
        let protected_state_preserved = verify_preserved_owned_root_if_present(
            &protected,
            journal,
            RestoreOwnedRootKind::ProtectedState,
        )?;
        let recovery_backup_preserved = verify_preserved_owned_root_if_present(
            &recovery,
            journal,
            RestoreOwnedRootKind::RecoveryBackup,
        )?;
        (protected_state_preserved, recovery_backup_preserved)
    } else {
        cleanup_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::ProtectedState)?;
        cleanup_owned_root_if_present(data_root, journal, RestoreOwnedRootKind::RecoveryBackup)?;
        (false, false)
    };

    Ok(RestoreTerminalCleanupResult {
        staging_removed,
        rollback_removed,
        quarantine_removed,
        protected_state_preserved,
        recovery_backup_preserved,
    })
}

fn preflight_owned_root_if_present(
    data_root: &Path,
    journal: &RestoreJournal,
    kind: RestoreOwnedRootKind,
) -> Result<bool, RestoreErrorInfo> {
    let path = owned_root_path(data_root, journal, kind)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(owned_cleanup_error()),
        Ok(_) => {
            verify_owned_root(&path, journal, kind)?;
            verify_owned_tree_no_reparse(&path)?;
            Ok(true)
        }
    }
}

fn cleanup_owned_root_if_present(
    data_root: &Path,
    journal: &RestoreJournal,
    kind: RestoreOwnedRootKind,
) -> Result<bool, RestoreErrorInfo> {
    let path = owned_root_path(data_root, journal, kind)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(owned_cleanup_error()),
        Ok(_) => {}
    }
    verify_owned_root(&path, journal, kind)?;
    verify_owned_tree_no_reparse(&path)?;
    fs::remove_dir_all(&path).map_err(|_| owned_cleanup_error())?;
    if path.exists() {
        return Err(owned_cleanup_error());
    }
    Ok(true)
}

fn verify_preserved_owned_root_if_present(
    path: &Path,
    journal: &RestoreJournal,
    kind: RestoreOwnedRootKind,
) -> Result<bool, RestoreErrorInfo> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(owned_cleanup_error()),
        Ok(_) => {
            verify_owned_root(path, journal, kind)?;
            verify_owned_tree_no_reparse(path)?;
            Ok(true)
        }
    }
}

fn verify_owned_tree_no_reparse(root: &Path) -> Result<(), RestoreErrorInfo> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        verify_regular_directory(&directory, "cleanup restore")?;
        let entries = fs::read_dir(&directory).map_err(|_| owned_cleanup_error())?;
        for entry in entries {
            let entry = entry.map_err(|_| owned_cleanup_error())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| owned_cleanup_error())?;
            if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
                return Err(owned_cleanup_error());
            }
            if metadata.file_type().is_dir() {
                pending.push(path);
            } else if !metadata.file_type().is_file() {
                return Err(owned_cleanup_error());
            }
        }
    }
    Ok(())
}

fn owned_cleanup_error() -> RestoreErrorInfo {
    restore_error(
        "cleanup restore",
        "restore_cleanup_ownership_unconfirmed",
        "CoffeePOS could not prove that a restore-owned tree is safe to remove.",
        "Keep the restore admission gate active and preserve the tree plus config/restore.json for explicit recovery.",
    )
}

fn component_identity_error(component: RestoreComponent) -> RestoreErrorInfo {
    restore_error(
        "restore ownership",
        "component_identity_unconfirmed",
        format!("CoffeePOS cannot prove ownership of the {:?} restore component.", component),
        "Keep runtime admission blocked and preserve active/staging/rollback/quarantine evidence for explicit recovery.",
    )
}

fn verify_regular_directory(path: &Path, action: &str) -> Result<(), RestoreErrorInfo> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        restore_error(
            action,
            "restore_directory_unavailable",
            "A required restore directory is missing or unreadable.",
            "Keep restore fenced and preserve transaction evidence before retrying recovery.",
        )
    })?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(restore_error(
            action,
            "unsafe_restore_directory",
            "CoffeePOS refused a non-directory, symlink, junction, or reparse point in restore data.",
            "Use only regular transaction-owned local directories for restore.",
        ));
    }
    reject_reparse_existing_ancestors(path, action)
}

fn verify_regular_file(path: &Path, action: &str) -> Result<(), RestoreErrorInfo> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        restore_error(
            action,
            "restore_file_unavailable",
            "A required restore file is missing or unreadable.",
            "Preserve restore staging and retry only after the required target artifact is present.",
        )
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(restore_error(
            action,
            "unsafe_restore_file",
            "CoffeePOS refused a non-regular or reparse restore file.",
            "Use only regular transaction-owned local files for restore.",
        ));
    }
    reject_reparse_existing_ancestors(path, action)
}

fn verify_empty_directory(path: &Path, action: &str) -> Result<(), RestoreErrorInfo> {
    verify_regular_directory(path, action)?;
    let mut entries = fs::read_dir(path).map_err(|_| owned_path_error(action))?;
    if entries.next().is_some() {
        return Err(restore_error(
            action,
            "fresh_placeholder_not_empty",
            "A fresh-profile managed directory contains data and cannot be treated as an empty placeholder.",
            "Keep restore fenced and inspect the unexpected data before retrying.",
        ));
    }
    Ok(())
}

fn ensure_path_absent(path: &Path, action: &str) -> Result<(), RestoreErrorInfo> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(owned_path_error(action)),
        Ok(_) => Err(restore_error(
            action,
            "restore_destination_occupied",
            "A restore move destination is already occupied.",
            "Keep restore fenced and reconcile the journaled ownership evidence before retrying the move.",
        )),
    }
}

fn copy_file_atomic(
    source: &Path,
    destination: &Path,
    action: &str,
) -> Result<(), RestoreErrorInfo> {
    verify_regular_file(source, action)?;
    let parent = destination.parent().ok_or_else(config_cutover_error)?;
    fs::create_dir_all(parent).map_err(|_| config_cutover_error())?;
    reject_reparse_existing_ancestors(parent, action)?;
    match fs::symlink_metadata(destination) {
        Ok(metadata)
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata_is_reparse_point(&metadata) =>
        {
            return Err(config_cutover_error());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(config_cutover_error()),
    }
    let mut input = File::open(source).map_err(|_| config_cutover_error())?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|_| config_cutover_error())?;
    io::copy(&mut input, &mut temporary).map_err(|_| config_cutover_error())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| config_cutover_error())?;
    temporary
        .persist(destination)
        .map_err(|_| config_cutover_error())?;
    Ok(())
}

fn atomic_json_write<T: Serialize>(
    path: &Path,
    value: &T,
    action: &str,
) -> Result<(), RestoreErrorInfo> {
    let parent = path.parent().ok_or_else(|| owned_path_error(action))?;
    fs::create_dir_all(parent).map_err(|_| owned_path_error(action))?;
    reject_reparse_existing_ancestors(parent, action)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|_| owned_path_error(action))?;
    serde_json::to_writer_pretty(&mut temporary, value).map_err(|_| owned_path_error(action))?;
    temporary
        .write_all(b"\n")
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| owned_path_error(action))?;
    temporary
        .persist(path)
        .map_err(|_| owned_path_error(action))?;
    Ok(())
}

fn sha256_file(path: &Path, action: &str) -> Result<String, RestoreErrorInfo> {
    verify_regular_file(path, action)?;
    let mut file = File::open(path).map_err(|_| config_cutover_error())?;
    let mut sha = Sha256::new();
    let mut buffer = vec![0_u8; FILE_HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|_| config_cutover_error())?;
        if read == 0 {
            break;
        }
        sha.update(&buffer[..read]);
    }
    Ok(hex_lower(&sha.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut sha = Sha256::new();
    sha.update(bytes);
    hex_lower(&sha.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn valid_sha256(value: &str) -> bool {
    valid_hex_id(value, 64)
}

fn valid_hex_id(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn random_hex_id(byte_len: usize, action: &str) -> Result<String, RestoreErrorInfo> {
    let mut random = vec![0_u8; byte_len];
    getrandom::fill(&mut random).map_err(|_| {
        restore_error(
            action,
            "restore_random_failed",
            "CoffeePOS could not generate a cryptographically random restore identifier.",
            "Retry after checking the operating-system random source.",
        )
    })?;
    Ok(hex_lower(&random))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn remove_regular_marker(path: &Path) -> Result<(), RestoreErrorInfo> {
    let metadata = fs::symlink_metadata(path).map_err(|_| owned_path_error("cleanup marker"))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(owned_path_error("cleanup marker"));
    }
    fs::remove_file(path).map_err(|_| owned_path_error("cleanup marker"))
}

fn reject_reparse_existing_ancestors(path: &Path, action: &str) -> Result<(), RestoreErrorInfo> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
                    return Err(restore_error(
                        action,
                        "unsafe_reparse_point",
                        "CoffeePOS refused a symlink, junction, or reparse point in a restore path.",
                        "Use regular local CoffeePOS data and backup paths before retrying restore.",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(owned_path_error(action)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint() -> RestoreSourceFingerprint {
        RestoreSourceFingerprint {
            canonical_path_sha256: "1".repeat(64),
            volume_serial_number: Some(1),
            file_index: Some(2),
            size_bytes: 123,
            modified_ticks: 456,
            encrypted_sha256: "2".repeat(64),
        }
    }

    fn journal(original_state: RestoreOriginalState, stage: RestoreStage) -> RestoreJournal {
        let transaction_id = "0123456789abcdef0123456789abcdef".to_string();
        RestoreJournal {
            schema_version: RESTORE_JOURNAL_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            backup_id: "backup-fixture".into(),
            validated_projection_sha256: "3".repeat(64),
            source_file: fingerprint(),
            migration_ids: Vec::new(),
            original_state,
            runtime_was_running: false,
            owned_paths: RestoreOwnedPaths::for_transaction(&transaction_id),
            cutover: RestoreCutoverEvidence::default(),
            stage,
            created_at_unix_seconds: 1,
            updated_at_unix_seconds: 1,
            safe_error: None,
        }
    }

    fn active_verified_fixture(data_root: &Path) -> RestoreJournal {
        let mut journal = journal(
            RestoreOriginalState::ExistingStore,
            RestoreStage::StagingVerified,
        );
        prepare_restore_owned_roots(data_root, &journal).unwrap();
        let staging = staging_store_root(data_root, &journal).unwrap();
        for component in ["site", "database", "uploads"] {
            let active = data_root.join(component);
            let staged = staging.join(component);
            fs::create_dir_all(&active).unwrap();
            fs::create_dir_all(&staged).unwrap();
            fs::write(active.join("old.txt"), b"old").unwrap();
            fs::write(staged.join("new.txt"), b"new").unwrap();
        }
        persist_restore_journal(data_root, &journal).unwrap();
        begin_restore_cutover(data_root, &mut journal).unwrap();
        for component in [
            RestoreComponent::Site,
            RestoreComponent::Database,
            RestoreComponent::Uploads,
        ] {
            cutover_component(data_root, &mut journal, component).unwrap();
        }
        for relative in RESTORE_CONFIG_FILES {
            let active = data_root.join(relative);
            fs::create_dir_all(active.parent().unwrap()).unwrap();
            fs::write(&active, format!("target:{relative}")).unwrap();
            journal
                .cutover
                .config_files
                .push(RestoreConfigFileEvidence {
                    relative_path: (*relative).into(),
                    previous_existed: false,
                    previous_sha256: None,
                    target_sha256: sha256_file(&active, "test target config").unwrap(),
                    snapshot_ready: true,
                    installed: true,
                    rollback_restored: false,
                });
        }
        journal.cutover.config_snapshot_ready = true;
        journal.cutover.config_applied = true;
        persist_restore_journal(data_root, &journal).unwrap();
        mark_active_swapped(data_root, &mut journal).unwrap();
        mark_restore_active_verified(data_root, &mut journal).unwrap();
        journal
    }

    #[test]
    fn fresh_restore_cannot_claim_recovery_backup_stage() {
        let mut journal = journal(
            RestoreOriginalState::NoPreviousStore,
            RestoreStage::RuntimeStopped,
        );
        let error = journal
            .advance(RestoreStage::RecoveryBackupReady)
            .unwrap_err();
        assert_eq!(error.code, "invalid_restore_transition");
        journal.advance(RestoreStage::StagingPrepared).unwrap();
    }

    #[test]
    fn journal_rejects_owned_path_not_bound_to_transaction() {
        let mut journal = journal(RestoreOriginalState::ExistingStore, RestoreStage::Planned);
        journal.owned_paths.staging = "backups/restore-staging/other".into();
        let error = validate_restore_journal(&journal).unwrap_err();
        assert_eq!(error.code, "restore_journal_path_mismatch");
    }

    #[test]
    fn admission_gate_stays_closed_until_terminal_reconciliation() {
        let mut gate = RestoreAdmissionGate::default();
        let mut journal = journal(RestoreOriginalState::ExistingStore, RestoreStage::Validated);
        gate.acquire(&journal.transaction_id).unwrap();
        assert!(gate.blocks_managed_operations());
        assert_eq!(
            gate.release_reconciled(&journal).unwrap_err().code,
            "restore_not_reconciled"
        );
        journal.stage = RestoreStage::Committed;
        gate.release_reconciled(&journal).unwrap();
        assert!(!gate.blocks_managed_operations());
    }

    #[test]
    fn fresh_recovery_does_not_require_previous_store_evidence() {
        let journal = journal(
            RestoreOriginalState::NoPreviousStore,
            RestoreStage::StagingPrepared,
        );
        let evidence = RestoreRecoveryEvidence {
            staging: RestoreOwnedEvidenceState::Owned,
            rollback: RestoreOwnedEvidenceState::Owned,
            quarantine: RestoreOwnedEvidenceState::Owned,
            protected_state: RestoreOwnedEvidenceState::Owned,
            recovery_backup: RestoreOwnedEvidenceState::Missing,
            managed_children_stopped: true,
        };
        assert_eq!(
            classify_restore_recovery(&journal, &evidence).action,
            RestoreRecoveryAction::ResumeOrAbortPreCutover
        );
    }

    #[test]
    fn active_verified_recovery_blocks_while_managed_child_is_alive() {
        let journal = journal(
            RestoreOriginalState::ExistingStore,
            RestoreStage::ActiveVerified,
        );
        let evidence = RestoreRecoveryEvidence {
            staging: RestoreOwnedEvidenceState::Owned,
            rollback: RestoreOwnedEvidenceState::Owned,
            quarantine: RestoreOwnedEvidenceState::Owned,
            protected_state: RestoreOwnedEvidenceState::Owned,
            recovery_backup: RestoreOwnedEvidenceState::Owned,
            managed_children_stopped: false,
        };
        let classified = classify_restore_recovery(&journal, &evidence);
        assert_eq!(classified.action, RestoreRecoveryAction::Blocked);
        assert_eq!(classified.reason_code, "verification_runtime_still_active");
    }

    #[test]
    fn active_verified_commit_rechecks_transaction_owned_target_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let mut journal = active_verified_fixture(data_root);

        commit_verified_restore(data_root, &mut journal).unwrap();
        assert_eq!(journal.stage, RestoreStage::Committed);
    }

    #[test]
    fn active_verified_commit_blocks_missing_component_marker() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let mut journal = active_verified_fixture(data_root);
        fs::remove_file(data_root.join("site").join(RESTORE_COMPONENT_MARKER)).unwrap();

        let error = commit_verified_restore(data_root, &mut journal).unwrap_err();
        assert_eq!(error.code, "active_verified_evidence_mismatch");
        assert_eq!(journal.stage, RestoreStage::ActiveVerified);
    }

    #[test]
    fn active_verified_commit_blocks_changed_target_config() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let mut journal = active_verified_fixture(data_root);
        fs::write(data_root.join("config/app.json"), b"unexpected replacement").unwrap();

        let error = commit_verified_restore(data_root, &mut journal).unwrap_err();
        assert_eq!(error.code, "active_verified_evidence_mismatch");
        assert_eq!(journal.stage, RestoreStage::ActiveVerified);
    }

    #[test]
    fn existing_store_directory_cutover_and_rollback_are_transaction_owned() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let mut journal = journal(
            RestoreOriginalState::ExistingStore,
            RestoreStage::StagingVerified,
        );
        prepare_restore_owned_roots(data_root, &journal).unwrap();
        let staging = staging_store_root(data_root, &journal).unwrap();
        for component in ["site", "database", "uploads"] {
            let active = data_root.join(component);
            let staged = staging.join(component);
            fs::create_dir_all(&active).unwrap();
            fs::create_dir_all(&staged).unwrap();
            fs::write(active.join("old.txt"), b"old").unwrap();
            fs::write(staged.join("new.txt"), b"new").unwrap();
        }
        persist_restore_journal(data_root, &journal).unwrap();
        begin_restore_cutover(data_root, &mut journal).unwrap();
        for component in [
            RestoreComponent::Site,
            RestoreComponent::Database,
            RestoreComponent::Uploads,
        ] {
            cutover_component(data_root, &mut journal, component).unwrap();
        }
        journal.cutover.config_applied = true;
        persist_restore_journal(data_root, &journal).unwrap();
        mark_active_swapped(data_root, &mut journal).unwrap();
        assert!(data_root.join("site/new.txt").is_file());

        rollback_restore_cutover(data_root, &mut journal).unwrap();
        mark_restore_rolled_back(data_root, &mut journal).unwrap();
        assert!(data_root.join("site/old.txt").is_file());
        assert!(!data_root.join("site/new.txt").exists());
        let quarantine =
            owned_root_path(data_root, &journal, RestoreOwnedRootKind::Quarantine).unwrap();
        assert!(quarantine.join("site/new.txt").is_file());
    }

    #[test]
    fn fresh_store_rollback_recreates_empty_managed_placeholders() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let mut journal = journal(
            RestoreOriginalState::NoPreviousStore,
            RestoreStage::StagingVerified,
        );
        prepare_restore_owned_roots(data_root, &journal).unwrap();
        let staging = staging_store_root(data_root, &journal).unwrap();
        for component in ["site", "database", "uploads"] {
            fs::create_dir_all(data_root.join(component)).unwrap();
            fs::create_dir_all(staging.join(component)).unwrap();
            fs::write(staging.join(component).join("new.txt"), b"new").unwrap();
        }
        persist_restore_journal(data_root, &journal).unwrap();
        begin_restore_cutover(data_root, &mut journal).unwrap();
        for component in [
            RestoreComponent::Site,
            RestoreComponent::Database,
            RestoreComponent::Uploads,
        ] {
            cutover_component(data_root, &mut journal, component).unwrap();
        }
        journal.cutover.config_applied = true;
        persist_restore_journal(data_root, &journal).unwrap();
        mark_active_swapped(data_root, &mut journal).unwrap();
        rollback_restore_cutover(data_root, &mut journal).unwrap();
        mark_restore_rolled_back(data_root, &mut journal).unwrap();
        for component in ["site", "database", "uploads"] {
            assert!(data_root.join(component).is_dir());
            assert!(fs::read_dir(data_root.join(component))
                .unwrap()
                .next()
                .is_none());
        }
    }

    #[test]
    fn terminal_cleanup_removes_only_marker_bound_owned_roots() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let mut journal = journal(
            RestoreOriginalState::NoPreviousStore,
            RestoreStage::Committed,
        );
        prepare_restore_owned_roots(data_root, &journal).unwrap();
        let staging = owned_root_path(data_root, &journal, RestoreOwnedRootKind::Staging).unwrap();
        fs::write(staging.join("owned.txt"), b"owned").unwrap();

        let result = cleanup_terminal_restore_owned_roots(data_root, &journal).unwrap();
        assert!(result.staging_removed);
        assert!(result.rollback_removed);
        assert!(result.quarantine_removed);
        assert!(!result.protected_state_preserved);
        assert!(!result.recovery_backup_preserved);
        assert!(!staging.exists());

        journal.stage = RestoreStage::Committed;
        prepare_restore_owned_roots(data_root, &journal).unwrap();
        let rollback =
            owned_root_path(data_root, &journal, RestoreOwnedRootKind::Rollback).unwrap();
        let staging = owned_root_path(data_root, &journal, RestoreOwnedRootKind::Staging).unwrap();
        fs::write(rollback.join(RESTORE_OWNERSHIP_MARKER), b"not-owned").unwrap();
        let error = cleanup_terminal_restore_owned_roots(data_root, &journal).unwrap_err();
        assert!(matches!(
            error.code.as_str(),
            "restore_ownership_unconfirmed" | "restore_cleanup_ownership_unconfirmed"
        ));
        assert!(rollback.exists());
        assert!(staging.exists());
    }

    #[test]
    fn existing_store_terminal_cleanup_preserves_recovery_archive_and_password_state() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let journal = journal(RestoreOriginalState::ExistingStore, RestoreStage::Committed);
        prepare_restore_owned_roots(data_root, &journal).unwrap();
        let protected =
            owned_root_path(data_root, &journal, RestoreOwnedRootKind::ProtectedState).unwrap();
        let recovery =
            owned_root_path(data_root, &journal, RestoreOwnedRootKind::RecoveryBackup).unwrap();
        fs::write(protected.join("recovery-password.secret"), b"protected").unwrap();
        fs::write(recovery.join("recovery.coffeepos-backup"), b"archive").unwrap();

        let result = cleanup_terminal_restore_owned_roots(data_root, &journal).unwrap();
        assert!(result.protected_state_preserved);
        assert!(result.recovery_backup_preserved);
        assert!(protected.is_dir());
        assert!(recovery.is_dir());
    }

    #[test]
    fn terminal_journal_retirement_requires_cleanup_and_exact_disk_state() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path();
        let journal = journal(
            RestoreOriginalState::NoPreviousStore,
            RestoreStage::Committed,
        );
        prepare_restore_owned_roots(data_root, &journal).unwrap();
        persist_restore_journal(data_root, &journal).unwrap();

        let error = retire_terminal_restore_journal(data_root, &journal).unwrap_err();
        assert_eq!(error.code, "restore_journal_retire_unconfirmed");
        assert!(data_root.join(RESTORE_JOURNAL).is_file());

        cleanup_terminal_restore_owned_roots(data_root, &journal).unwrap();
        let mut stale = journal.clone();
        stale.updated_at_unix_seconds += 1;
        let error = retire_terminal_restore_journal(data_root, &stale).unwrap_err();
        assert_eq!(error.code, "restore_journal_retire_unconfirmed");
        assert!(data_root.join(RESTORE_JOURNAL).is_file());

        retire_terminal_restore_journal(data_root, &journal).unwrap();
        assert!(load_restore_journal(data_root).unwrap().is_none());
    }
}
