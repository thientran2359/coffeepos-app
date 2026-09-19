use crate::config::APP_CONFIG_SCHEMA_VERSION;
use crate::provisioning::{self, PROVISIONING_SCHEMA_VERSION};
use crate::runtime;
use age::secrecy::SecretString;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

pub const BACKUP_SCHEMA_VERSION: u32 = 1;
const INVENTORY_SCHEMA_VERSION: u32 = 1;
const STORE_CONFIG_SCHEMA_VERSION: u32 = 1;
const ADMINISTRATOR_SECRET_SCHEMA_VERSION: u32 = 1;
const BACKUP_KIND: &str = "portable_store_backup";
const INVENTORY_ENTRY: &str = "inventory.json";
const DATABASE_ENTRY: &str = "database/store.sql";
const DATABASE_NAME: &str = "coffeepos";
const STORE_CONFIG_ENTRY: &str = "config/store.json";
const ADMINISTRATOR_SECRET_ENTRY: &str = "secrets/administrator.json";
const UPLOADS_ROOT: &str = "uploads/";

const MAX_PAYLOAD_ENTRIES: u64 = 500_000;
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_ENTRY_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_PATH_BYTES: usize = 1024;
const MAX_MANIFEST_JSON_BYTES: u64 = 1024 * 1024;
const MAX_INVENTORY_JSON_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PORTABLE_CONFIG_JSON_BYTES: u64 = 1024 * 1024;
const MAX_WARNINGS: usize = 128;
const MAX_WARNING_ITEMS: usize = 256;
const MAX_WARNING_ITEM_BYTES: usize = 128;
pub const WARNING_UNMANAGED_EXTENSIONS_EXCLUDED: &str = "unmanaged_extensions_excluded";
pub const WARNING_UNMANAGED_SITE_CODE_NOT_INCLUDED: &str = "unmanaged_site_code_not_included";

const ZIP_LOCAL_FILE_HEADER: u32 = 0x0403_4b50;
const ZIP_CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4b50;
const ZIP64_END_OF_CENTRAL_DIRECTORY: u32 = 0x0606_4b50;
const ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR: u32 = 0x0706_4b50;
const ZIP_END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
const ZIP64_EXTRA_FIELD_ID: u16 = 0x0001;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupErrorInfo {
    pub component: String,
    pub action: String,
    pub code: String,
    pub message: String,
    pub recovery: String,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupSourceVersions {
    pub desktop_version: String,
    pub runtime_version: String,
    pub target: String,
    pub php_version: String,
    pub mariadb_version: String,
    pub wordpress_version: String,
    pub woocommerce_version: String,
    pub coffeepos_version: String,
    pub provisioning_schema: u32,
    pub app_config_schema: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupDestinationIdentity {
    volume_serial_number: u32,
    file_index: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct BackupDestinationSelection {
    pub(crate) path: PathBuf,
    pub(crate) existing_identity: Option<BackupDestinationIdentity>,
}

impl BackupDestinationSelection {
    pub(crate) fn capture(path: PathBuf, action: &str) -> Result<Self, BackupErrorInfo> {
        let existing_identity = capture_destination_identity(&path, action)?;
        Ok(Self {
            path,
            existing_identity,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DatabaseDescriptor {
    format: String,
    database_name: String,
    entry: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct UploadsDescriptor {
    root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct EntryDescriptor {
    entry: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AdministratorSecretDescriptor {
    entry: String,
    present: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompatibilityDescriptor {
    minimum_restore_schema: u32,
    requires_explicit_migration: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupWarning {
    pub code: String,
    pub items: Vec<String>,
}

impl Serialize for BackupWarning {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.code == WARNING_UNMANAGED_EXTENSIONS_EXCLUDED && self.items.is_empty() {
            return serializer.serialize_str(&self.code);
        }
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("BackupWarning", 2)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("items", &self.items)?;
        state.end()
    }
}

impl BackupWarning {
    pub(crate) fn unmanaged_site_code_not_included(
        items: impl IntoIterator<Item = String>,
    ) -> Result<Self, BackupErrorInfo> {
        let mut items = items.into_iter().collect::<Vec<_>>();
        items.sort();
        items.dedup();
        let warning = Self {
            code: WARNING_UNMANAGED_SITE_CODE_NOT_INCLUDED.into(),
            items,
        };
        validate_warning(&warning, "preflight")?;
        Ok(warning)
    }
}

impl<'de> Deserialize<'de> for BackupWarning {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WarningMetadataWire {
            code: String,
            #[serde(default)]
            items: Vec<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WarningWire {
            LegacyCode(String),
            Metadata(WarningMetadataWire),
        }

        match WarningWire::deserialize(deserializer)? {
            WarningWire::LegacyCode(code) => Ok(Self {
                code,
                items: Vec::new(),
            }),
            WarningWire::Metadata(value) => Ok(Self {
                code: value.code,
                items: value.items,
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BackupManifestV1 {
    schema_version: u32,
    backup_id: String,
    created_at: String,
    kind: String,
    source: BackupSourceVersions,
    database: DatabaseDescriptor,
    uploads: UploadsDescriptor,
    store_config: EntryDescriptor,
    administrator_secret: AdministratorSecretDescriptor,
    compatibility: CompatibilityDescriptor,
    inventory_entry: String,
    total_files: u64,
    total_uncompressed_bytes: u64,
    warnings: Vec<BackupWarning>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct InventoryV1 {
    schema_version: u32,
    total_files: u64,
    total_uncompressed_bytes: u64,
    entries: Vec<InventoryEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct InventoryEntry {
    path: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PortableStoreConfigV1 {
    schema_version: u32,
    store_name: String,
    administrator: PortableAdministratorIdentity,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PortableAdministratorIdentity {
    username: String,
    email: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PortableAdministratorSecretV1 {
    schema_version: u32,
    username: String,
    password: Zeroizing<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackupCompatibilityStatus {
    Compatible,
    Blocked,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupCompatibilityResult {
    pub status: BackupCompatibilityStatus,
    pub code: Option<String>,
    pub message: String,
}

impl BackupCompatibilityResult {
    fn compatible() -> Self {
        Self {
            status: BackupCompatibilityStatus::Compatible,
            code: None,
            message: "This backup matches the supported CoffeePOS restore baseline.".into(),
        }
    }

    fn blocked(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: BackupCompatibilityStatus::Blocked,
            code: Some(code.into()),
            message: message.into(),
        }
    }

    fn can_restore(&self) -> bool {
        matches!(self.status, BackupCompatibilityStatus::Compatible)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupInspection {
    pub backup_id: String,
    pub created_at: String,
    pub store_name: String,
    pub source: BackupSourceVersions,
    pub database_bytes: u64,
    pub uploads_bytes: u64,
    pub total_files: u64,
    pub total_uncompressed_bytes: u64,
    pub compatibility: BackupCompatibilityResult,
    /// Stable warning codes kept for the Phase 7.1 inspection IPC contract.
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warning_metadata: Vec<BackupWarning>,
    pub can_restore: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupValidation {
    pub valid: bool,
    pub entries_validated: u64,
    pub inspection: BackupInspection,
}

#[derive(Debug)]
pub(crate) struct RestorePayload {
    pub(crate) store_name: String,
    pub(crate) administrator_username: String,
    pub(crate) administrator_email: String,
    pub(crate) administrator_password: Zeroizing<String>,
    pub(crate) database_dump: PathBuf,
    pub(crate) upload_files: u64,
    pub(crate) upload_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct BackupCompatibilityTarget {
    pub source: BackupSourceVersions,
    pub restore_schema: u32,
    pub available_restore_bytes: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct BackupManifestSeed {
    backup_id: String,
    created_at: String,
    source: BackupSourceVersions,
    database_name: String,
    minimum_restore_schema: u32,
    requires_explicit_migration: bool,
    warnings: Vec<BackupWarning>,
}

impl BackupManifestSeed {
    #[cfg(test)]
    fn fixture(source: BackupSourceVersions) -> Self {
        Self {
            backup_id: "123e4567-e89b-42d3-a456-426614174000".into(),
            created_at: "2026-09-19T06:30:00Z".into(),
            source,
            database_name: DATABASE_NAME.into(),
            minimum_restore_schema: 1,
            requires_explicit_migration: false,
            warnings: Vec::new(),
        }
    }
}

pub(crate) enum BackupEntrySource {
    File(PathBuf),
    Memory(Zeroizing<Vec<u8>>),
}

pub(crate) struct BackupPayloadEntry {
    pub path: String,
    pub source: BackupEntrySource,
}

#[derive(Clone, Debug)]
struct PreparedPayloadEntry {
    source_index: usize,
    path: String,
    size: u64,
    sha256: String,
    crc32: u32,
    source_snapshot: Option<SourceFileSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceFileSnapshot {
    size: u64,
    modified: Option<SystemTime>,
    #[cfg(windows)]
    creation_time: u64,
    #[cfg(windows)]
    last_write_time: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupWriteStage {
    Preparing,
    Archiving,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct BackupWriteProgress {
    pub stage: BackupWriteStage,
    pub entries_completed: u64,
    pub total_entries: u64,
    pub bytes_processed: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug)]
struct RawZipEntry {
    path: String,
    size: u64,
    crc32: u32,
    sha256: Option<String>,
    source: RawZipSource,
}

#[derive(Clone, Debug)]
enum RawZipSource {
    Control(Vec<u8>),
    Payload(usize),
}

#[derive(Clone, Debug)]
struct CentralEntry {
    path: String,
    crc32: u32,
    size: u64,
    local_header_offset: u64,
}

#[derive(Clone, Debug)]
struct LocalEntryMetadata {
    path: String,
    size: u64,
    compressed_size: u64,
    crc32: u32,
    local_header_offset: u64,
}

struct CountingReader<R> {
    inner: R,
    position: u64,
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    overflowed: bool,
}

impl BoundedJsonWriter {
    fn new(max_bytes: u64) -> Result<Self, BackupErrorInfo> {
        let max_bytes = usize::try_from(max_bytes).map_err(|_| {
            size_limit_error(
                "create",
                "The JSON metadata bound cannot be represented on this platform.",
            )
        })?;
        Ok(Self {
            bytes: Vec::with_capacity(max_bytes.min(4 * 1024 * 1024)),
            max_bytes,
            overflowed: false,
        })
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(next_len) = self.bytes.len().checked_add(buffer.len()) else {
            self.overflowed = true;
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "backup JSON metadata exceeds its size bound",
            ));
        };
        if next_len > self.max_bytes {
            self.overflowed = true;
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "backup JSON metadata exceeds its size bound",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<R> CountingReader<R> {
    fn new(inner: R) -> Self {
        Self { inner, position: 0 }
    }

    fn position(&self) -> u64 {
        self.position
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.position = self.position.saturating_add(read as u64);
        Ok(read)
    }
}

#[derive(Default)]
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(0xffff_ffff)
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }

    fn finish(&self) -> u32 {
        !self.0
    }
}

fn current_target_name() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return "x86_64-pc-windows-msvc";
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return "aarch64-apple-darwin";
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return "x86_64-apple-darwin";
    }
    #[allow(unreachable_code)]
    "unknown"
}

pub(crate) fn new_backup_manifest_seed(
    source: BackupSourceVersions,
    warnings: Vec<String>,
) -> Result<BackupManifestSeed, BackupErrorInfo> {
    let warnings = warnings
        .into_iter()
        .map(|code| BackupWarning {
            code,
            items: Vec::new(),
        })
        .collect();
    new_backup_manifest_seed_with_warning_metadata(source, warnings)
}

pub(crate) fn new_backup_manifest_seed_with_warning_metadata(
    source: BackupSourceVersions,
    warnings: Vec<BackupWarning>,
) -> Result<BackupManifestSeed, BackupErrorInfo> {
    for warning in &warnings {
        validate_warning(warning, "preflight")?;
    }
    Ok(BackupManifestSeed {
        backup_id: random_uuid_v4()?,
        created_at: rfc3339_utc(now_epoch()),
        source,
        database_name: DATABASE_NAME.into(),
        minimum_restore_schema: BACKUP_SCHEMA_VERSION,
        requires_explicit_migration: false,
        warnings,
    })
}

pub(crate) fn portable_store_config_entry(
    store_name: &str,
    administrator_username: &str,
    administrator_email: &str,
) -> Result<BackupPayloadEntry, BackupErrorInfo> {
    let config = PortableStoreConfigV1 {
        schema_version: STORE_CONFIG_SCHEMA_VERSION,
        store_name: store_name.to_owned(),
        administrator: PortableAdministratorIdentity {
            username: administrator_username.to_owned(),
            email: administrator_email.to_owned(),
        },
    };
    validate_store_config(&config, "preflight")?;
    let bytes = serialize_json_bounded(
        &config,
        MAX_PORTABLE_CONFIG_JSON_BYTES,
        "store_config_serialize_failed",
        "portable store configuration",
    )?;
    Ok(BackupPayloadEntry {
        path: STORE_CONFIG_ENTRY.into(),
        source: BackupEntrySource::Memory(Zeroizing::new(bytes)),
    })
}

pub(crate) fn portable_administrator_secret_entry(
    administrator_username: &str,
    administrator_password: &str,
) -> Result<BackupPayloadEntry, BackupErrorInfo> {
    if !valid_portable_admin_username(administrator_username) || administrator_password.is_empty() {
        return Err(backup_error(
            "preflight",
            "administrator_secret_unavailable",
            "The portable administrator credential is incomplete.",
            "Repair the administrator account metadata and protected password before creating a backup.",
        ));
    }
    let secret = PortableAdministratorSecretV1 {
        schema_version: ADMINISTRATOR_SECRET_SCHEMA_VERSION,
        username: administrator_username.to_owned(),
        password: Zeroizing::new(administrator_password.to_owned()),
    };
    let bytes = serialize_json_bounded(
        &secret,
        MAX_PORTABLE_CONFIG_JSON_BYTES,
        "administrator_secret_serialize_failed",
        "portable administrator credential",
    )?;
    Ok(BackupPayloadEntry {
        path: ADMINISTRATOR_SECRET_ENTRY.into(),
        source: BackupEntrySource::Memory(Zeroizing::new(bytes)),
    })
}

fn random_uuid_v4() -> Result<String, BackupErrorInfo> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        backup_error(
            "create",
            "randomness_unavailable",
            "CoffeePOS cannot generate a backup identifier safely.",
            "Retry after restarting CoffeePOS Desktop.",
        )
    })?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

pub(crate) fn write_encrypted_backup<W: Write>(
    output: W,
    backup_password: &str,
    seed: BackupManifestSeed,
    entries: &[BackupPayloadEntry],
) -> Result<(), BackupErrorInfo> {
    let cancelled = AtomicBool::new(false);
    write_encrypted_backup_with_control(output, backup_password, seed, entries, &cancelled, |_| {})
}

pub(crate) fn write_encrypted_backup_with_control<W: Write, F: FnMut(BackupWriteProgress)>(
    output: W,
    backup_password: &str,
    seed: BackupManifestSeed,
    entries: &[BackupPayloadEntry],
    cancelled: &AtomicBool,
    mut progress: F,
) -> Result<(), BackupErrorInfo> {
    if backup_password.is_empty() {
        return Err(backup_error(
            "create",
            "empty_password",
            "The backup password cannot be empty.",
            "Enter and confirm a backup password, then retry.",
        ));
    }
    ensure_write_not_cancelled(cancelled, "archive")?;
    progress(BackupWriteProgress {
        stage: BackupWriteStage::Preparing,
        entries_completed: 0,
        total_entries: entries.len() as u64,
        bytes_processed: 0,
        total_bytes: 0,
    });
    let prepared = prepare_payload_entries(entries, Some(cancelled))?;
    let manifest = build_manifest(seed, &prepared)?;
    let inventory = build_inventory(&prepared)?;
    validate_manifest_shape(&manifest, "create")?;
    validate_inventory_shape(&inventory, "create")?;

    let manifest_bytes = serialize_json_bounded(
        &manifest,
        MAX_MANIFEST_JSON_BYTES,
        "manifest_serialize_failed",
        "backup manifest",
    )?;
    let inventory_bytes = serialize_json_bounded(
        &inventory,
        MAX_INVENTORY_JSON_BYTES,
        "inventory_serialize_failed",
        "backup inventory",
    )?;

    let mut raw_entries = Vec::with_capacity(prepared.len() + 2);
    raw_entries.push(raw_control_entry("manifest.json", manifest_bytes));
    raw_entries.push(raw_control_entry(INVENTORY_ENTRY, inventory_bytes));
    for item in &prepared {
        raw_entries.push(RawZipEntry {
            path: item.path.clone(),
            size: item.size,
            crc32: item.crc32,
            sha256: Some(item.sha256.clone()),
            source: RawZipSource::Payload(item.source_index),
        });
    }

    let passphrase = SecretString::from(backup_password.to_owned());
    let encryptor = age::Encryptor::with_user_passphrase(passphrase);
    let mut encrypted = encryptor.wrap_output(output).map_err(|_| {
        backup_error(
            "create",
            "encryption_start_failed",
            "CoffeePOS cannot start encrypted backup output.",
            "Check the destination and retry backup creation.",
        )
    })?;
    progress(BackupWriteProgress {
        stage: BackupWriteStage::Archiving,
        entries_completed: 0,
        total_entries: prepared.len() as u64,
        bytes_processed: 0,
        total_bytes: inventory.total_uncompressed_bytes,
    });
    write_zip64_stream(
        &mut encrypted,
        &raw_entries,
        entries,
        &prepared,
        Some(cancelled),
        &mut progress,
    )?;
    ensure_write_not_cancelled(cancelled, "archive")?;
    encrypted.finish().map_err(|_| {
        backup_error(
            "create",
            "encryption_finalize_failed",
            "CoffeePOS cannot finalize the encrypted backup container.",
            "Discard the incomplete file and retry backup creation.",
        )
    })?;
    Ok(())
}

fn prepare_payload_entries(
    entries: &[BackupPayloadEntry],
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<PreparedPayloadEntry>, BackupErrorInfo> {
    if entries.len() as u64 > MAX_PAYLOAD_ENTRIES {
        return Err(size_limit_error(
            "create",
            "The backup contains too many payload files.",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut prepared = Vec::with_capacity(entries.len());
    let mut total = 0_u64;
    for (source_index, entry) in entries.iter().enumerate() {
        if let Some(cancelled) = cancelled {
            ensure_write_not_cancelled(cancelled, "archive")?;
        }
        let path = validate_archive_path(&entry.path, "create")?;
        if !is_allowed_data_path(&path) {
            return Err(backup_error(
                "create",
                "unsupported_payload_entry",
                "CoffeePOS refused a file outside the portable backup schema.",
                "Create the backup from the managed database, uploads, store config, and administrator secret only.",
            ));
        }
        let key = windows_casefold(&path);
        if !seen.insert(key) {
            return Err(backup_error(
                "create",
                "duplicate_archive_path",
                "CoffeePOS found duplicate backup paths after Windows normalization.",
                "Resolve the duplicate source path before retrying backup creation.",
            ));
        }
        let (size, sha256, crc32, source_snapshot) =
            inspect_entry_source(&entry.source, cancelled)?;
        if size > MAX_ENTRY_UNCOMPRESSED_BYTES {
            return Err(size_limit_error(
                "create",
                "A backup entry exceeds the safety size limit.",
            ));
        }
        total = total.checked_add(size).ok_or_else(|| {
            size_limit_error(
                "create",
                "The backup payload size overflowed the safety counter.",
            )
        })?;
        if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(size_limit_error(
                "create",
                "The backup payload exceeds the safety size limit.",
            ));
        }
        prepared.push(PreparedPayloadEntry {
            source_index,
            path,
            size,
            sha256,
            crc32,
            source_snapshot,
        });
    }
    require_payload_shape(&prepared, "create")?;
    Ok(prepared)
}

fn inspect_entry_source(
    source: &BackupEntrySource,
    cancelled: Option<&AtomicBool>,
) -> Result<(u64, String, u32, Option<SourceFileSnapshot>), BackupErrorInfo> {
    match source {
        BackupEntrySource::File(path) => {
            validate_source_file(path)?;
            let file = File::open(path).map_err(|_| source_read_error())?;
            let before =
                source_snapshot_from_metadata(&file.metadata().map_err(|_| source_read_error())?);
            let (size, sha256, crc32) = hash_reader(BufReader::new(&file), "create", cancelled)?;
            let after =
                source_snapshot_from_metadata(&file.metadata().map_err(|_| source_read_error())?);
            validate_source_file(path)?;
            let path_after = source_snapshot_from_metadata(
                &fs::metadata(path).map_err(|_| source_read_error())?,
            );
            if before != after || after != path_after {
                return Err(source_changed_error());
            }
            Ok((size, sha256, crc32, Some(after)))
        }
        BackupEntrySource::Memory(bytes) => {
            let (size, sha256, crc32) = hash_reader(bytes.as_slice(), "create", cancelled)?;
            Ok((size, sha256, crc32, None))
        }
    }
}

fn validate_source_file(path: &Path) -> Result<(), BackupErrorInfo> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(backup_error(
            "create",
            "unsafe_source_file",
            "CoffeePOS refused an unsafe backup source path.",
            "Use only absolute managed source files without relative traversal and retry.",
        ));
    }

    for (index, ancestor) in path.ancestors().enumerate() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| source_read_error())?;
        if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
            return Err(backup_error(
                "create",
                "unsafe_source_file",
                "CoffeePOS refused a backup source that traverses a symlink, junction, or reparse point.",
                "Remove links or reparse points from the managed backup source path and retry.",
            ));
        }
        if index == 0 && !metadata.file_type().is_file() {
            return Err(backup_error(
                "create",
                "unsafe_source_file",
                "CoffeePOS refused a non-regular backup source file.",
                "Use a regular managed source file and retry.",
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

fn source_read_error() -> BackupErrorInfo {
    backup_error(
        "create",
        "source_read_failed",
        "CoffeePOS cannot read a managed backup source file.",
        "Check the store data permissions and retry backup creation.",
    )
}

fn source_changed_error() -> BackupErrorInfo {
    backup_error(
        "archive",
        "source_changed",
        "A managed backup source changed while CoffeePOS was reading it.",
        "Keep store data idle during backup and retry from a fresh consistent snapshot.",
    )
}

fn source_snapshot_from_metadata(metadata: &fs::Metadata) -> SourceFileSnapshot {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    #[cfg(windows)]
    use std::os::windows::fs::MetadataExt;

    SourceFileSnapshot {
        size: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(windows)]
        creation_time: metadata.creation_time(),
        #[cfg(windows)]
        last_write_time: metadata.last_write_time(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    }
}

fn ensure_write_not_cancelled(cancelled: &AtomicBool, action: &str) -> Result<(), BackupErrorInfo> {
    if cancelled.load(Ordering::Acquire) {
        Err(backup_error(
            action,
            "cancelled",
            "The backup operation was cancelled.",
            "CoffeePOS will remove owned temporary backup files before normal runtime resumes.",
        ))
    } else {
        Ok(())
    }
}

fn hash_reader(
    mut reader: impl Read,
    action: &str,
    cancelled: Option<&AtomicBool>,
) -> Result<(u64, String, u32), BackupErrorInfo> {
    let mut sha = Sha256::new();
    let mut crc = Crc32::new();
    let mut total = 0_u64;
    let mut buffer = Zeroizing::new([0_u8; 64 * 1024]);
    loop {
        if let Some(cancelled) = cancelled {
            ensure_write_not_cancelled(cancelled, "archive")?;
        }
        let read = reader.read(&mut buffer[..]).map_err(|_| {
            backup_error(
                action,
                "entry_read_failed",
                "CoffeePOS cannot read a backup entry.",
                "Retry the backup operation from a healthy source file.",
            )
        })?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            size_limit_error(action, "A backup entry exceeded the safety size counter.")
        })?;
        if total > MAX_ENTRY_UNCOMPRESSED_BYTES {
            return Err(size_limit_error(
                action,
                "A backup entry exceeds the safety size limit.",
            ));
        }
        sha.update(&buffer[..read]);
        crc.update(&buffer[..read]);
    }
    Ok((total, hex_lower(&sha.finalize()), crc.finish()))
}

fn build_manifest(
    seed: BackupManifestSeed,
    entries: &[PreparedPayloadEntry],
) -> Result<BackupManifestV1, BackupErrorInfo> {
    let total_uncompressed_bytes = entries.iter().try_fold(0_u64, |sum, entry| {
        sum.checked_add(entry.size)
            .ok_or_else(|| size_limit_error("create", "The backup payload size overflowed."))
    })?;
    Ok(BackupManifestV1 {
        schema_version: BACKUP_SCHEMA_VERSION,
        backup_id: seed.backup_id,
        created_at: seed.created_at,
        kind: BACKUP_KIND.into(),
        source: seed.source,
        database: DatabaseDescriptor {
            format: "mariadb_logical_sql".into(),
            database_name: seed.database_name,
            entry: DATABASE_ENTRY.into(),
        },
        uploads: UploadsDescriptor {
            root: UPLOADS_ROOT.into(),
        },
        store_config: EntryDescriptor {
            entry: STORE_CONFIG_ENTRY.into(),
        },
        administrator_secret: AdministratorSecretDescriptor {
            entry: ADMINISTRATOR_SECRET_ENTRY.into(),
            present: true,
        },
        compatibility: CompatibilityDescriptor {
            minimum_restore_schema: seed.minimum_restore_schema,
            requires_explicit_migration: seed.requires_explicit_migration,
        },
        inventory_entry: INVENTORY_ENTRY.into(),
        total_files: entries.len() as u64,
        total_uncompressed_bytes,
        warnings: seed.warnings,
    })
}

fn build_inventory(entries: &[PreparedPayloadEntry]) -> Result<InventoryV1, BackupErrorInfo> {
    let total_uncompressed_bytes = entries.iter().try_fold(0_u64, |sum, entry| {
        sum.checked_add(entry.size)
            .ok_or_else(|| size_limit_error("create", "The backup payload size overflowed."))
    })?;
    Ok(InventoryV1 {
        schema_version: INVENTORY_SCHEMA_VERSION,
        total_files: entries.len() as u64,
        total_uncompressed_bytes,
        entries: entries
            .iter()
            .map(|entry| InventoryEntry {
                path: entry.path.clone(),
                size_bytes: entry.size,
                sha256: entry.sha256.clone(),
            })
            .collect(),
    })
}

fn raw_control_entry(path: &str, bytes: Vec<u8>) -> RawZipEntry {
    let mut crc = Crc32::new();
    crc.update(&bytes);
    RawZipEntry {
        path: path.into(),
        size: bytes.len() as u64,
        crc32: crc.finish(),
        sha256: None,
        source: RawZipSource::Control(bytes),
    }
}

fn serialize_json_bounded<T: Serialize>(
    value: &T,
    max_bytes: u64,
    serialize_code: &str,
    label: &str,
) -> Result<Vec<u8>, BackupErrorInfo> {
    let mut writer = BoundedJsonWriter::new(max_bytes)?;
    let result = serde_json::to_writer_pretty(&mut writer, value);
    if writer.overflowed {
        return Err(size_limit_error(
            "create",
            format!("The {label} exceeds its schema-1 metadata size limit."),
        ));
    }
    result.map_err(|_| {
        backup_error(
            "create",
            serialize_code,
            format!("CoffeePOS cannot serialize the {label}."),
            "Retry backup creation.",
        )
    })?;
    Ok(writer.into_inner())
}

fn write_zip64_stream<W: Write, F: FnMut(BackupWriteProgress)>(
    writer: &mut W,
    raw_entries: &[RawZipEntry],
    payload_entries: &[BackupPayloadEntry],
    prepared_entries: &[PreparedPayloadEntry],
    cancelled: Option<&AtomicBool>,
    progress: &mut F,
) -> Result<(), BackupErrorInfo> {
    let mut offset = 0_u64;
    let mut central = Vec::with_capacity(raw_entries.len());
    let total_entries = prepared_entries.len() as u64;
    let total_bytes = prepared_entries.iter().map(|entry| entry.size).sum();
    let mut entries_completed = 0_u64;
    let mut bytes_processed = 0_u64;
    for raw in raw_entries {
        if let Some(cancelled) = cancelled {
            ensure_write_not_cancelled(cancelled, "archive")?;
        }
        let local_header_offset = offset;
        write_u32(writer, ZIP_LOCAL_FILE_HEADER, "create")?;
        write_u16(writer, 45, "create")?;
        write_u16(writer, 1 << 11, "create")?;
        write_u16(writer, 0, "create")?;
        write_u16(writer, 0, "create")?;
        write_u16(writer, 0, "create")?;
        write_u32(writer, raw.crc32, "create")?;
        write_u32(writer, u32::MAX, "create")?;
        write_u32(writer, u32::MAX, "create")?;
        let name = raw.path.as_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| {
            backup_error(
                "create",
                "unsafe_archive_path",
                "A backup path is too long for the ZIP container.",
                "Shorten the managed source path and retry.",
            )
        })?;
        write_u16(writer, name_len, "create")?;
        write_u16(writer, 20, "create")?;
        write_all(writer, name, "create")?;
        write_u16(writer, ZIP64_EXTRA_FIELD_ID, "create")?;
        write_u16(writer, 16, "create")?;
        write_u64(writer, raw.size, "create")?;
        write_u64(writer, raw.size, "create")?;
        offset = offset
            .checked_add(30 + name.len() as u64 + 20)
            .ok_or_else(|| size_limit_error("create", "The ZIP offset overflowed."))?;

        let (written, sha256, crc32) = match &raw.source {
            RawZipSource::Control(bytes) => {
                write_all(writer, bytes, "create")?;
                let (size, sha, crc) = hash_reader(bytes.as_slice(), "create", cancelled)?;
                (size, sha, crc)
            }
            RawZipSource::Payload(index) => {
                let source = payload_entries.get(*index).ok_or_else(|| {
                    backup_error(
                        "create",
                        "source_changed",
                        "A prepared backup source is no longer available.",
                        "Retry backup creation from the current store state.",
                    )
                })?;
                let prepared = prepared_entries.get(*index).ok_or_else(|| {
                    backup_error(
                        "archive",
                        "source_changed",
                        "A prepared backup source is no longer available.",
                        "Retry backup creation from the current store state.",
                    )
                })?;
                let result = copy_source_with_hash(
                    writer,
                    &source.source,
                    prepared.source_snapshot.as_ref(),
                    cancelled,
                    |delta| {
                        bytes_processed = bytes_processed.saturating_add(delta);
                        progress(BackupWriteProgress {
                            stage: BackupWriteStage::Archiving,
                            entries_completed,
                            total_entries,
                            bytes_processed,
                            total_bytes,
                        });
                    },
                )?;
                entries_completed = entries_completed.saturating_add(1);
                progress(BackupWriteProgress {
                    stage: BackupWriteStage::Archiving,
                    entries_completed,
                    total_entries,
                    bytes_processed,
                    total_bytes,
                });
                result
            }
        };
        if written != raw.size || crc32 != raw.crc32 {
            return Err(backup_error(
                "create",
                "source_changed",
                "A backup source changed while CoffeePOS was reading it.",
                "Retry backup creation after store writes are paused by the Phase 7 snapshot flow.",
            ));
        }
        if raw
            .sha256
            .as_deref()
            .is_some_and(|expected| expected != sha256)
        {
            return Err(backup_error(
                "create",
                "source_changed",
                "A backup source changed while CoffeePOS was reading it.",
                "Retry backup creation after store writes are paused by the Phase 7 snapshot flow.",
            ));
        }
        offset = offset
            .checked_add(written)
            .ok_or_else(|| size_limit_error("create", "The ZIP offset overflowed."))?;
        central.push(CentralEntry {
            path: raw.path.clone(),
            crc32: raw.crc32,
            size: raw.size,
            local_header_offset,
        });
    }

    let central_start = offset;
    for item in &central {
        if let Some(cancelled) = cancelled {
            ensure_write_not_cancelled(cancelled, "archive")?;
        }
        let name = item.path.as_bytes();
        write_u32(writer, ZIP_CENTRAL_DIRECTORY_HEADER, "create")?;
        write_u16(writer, (3 << 8) | 45, "create")?;
        write_u16(writer, 45, "create")?;
        write_u16(writer, 1 << 11, "create")?;
        write_u16(writer, 0, "create")?;
        write_u16(writer, 0, "create")?;
        write_u16(writer, 0, "create")?;
        write_u32(writer, item.crc32, "create")?;
        write_u32(writer, u32::MAX, "create")?;
        write_u32(writer, u32::MAX, "create")?;
        write_u16(writer, name.len() as u16, "create")?;
        write_u16(writer, 28, "create")?;
        write_u16(writer, 0, "create")?;
        write_u16(writer, 0, "create")?;
        write_u16(writer, 0, "create")?;
        write_u32(writer, (0o100600_u32) << 16, "create")?;
        write_u32(writer, u32::MAX, "create")?;
        write_all(writer, name, "create")?;
        write_u16(writer, ZIP64_EXTRA_FIELD_ID, "create")?;
        write_u16(writer, 24, "create")?;
        write_u64(writer, item.size, "create")?;
        write_u64(writer, item.size, "create")?;
        write_u64(writer, item.local_header_offset, "create")?;
        offset = offset
            .checked_add(46 + name.len() as u64 + 28)
            .ok_or_else(|| {
                size_limit_error("create", "The ZIP central-directory offset overflowed.")
            })?;
    }
    let central_size = offset
        .checked_sub(central_start)
        .ok_or_else(|| size_limit_error("create", "The ZIP central-directory size overflowed."))?;
    let zip64_eocd_offset = offset;
    write_u32(writer, ZIP64_END_OF_CENTRAL_DIRECTORY, "create")?;
    write_u64(writer, 44, "create")?;
    write_u16(writer, (3 << 8) | 45, "create")?;
    write_u16(writer, 45, "create")?;
    write_u32(writer, 0, "create")?;
    write_u32(writer, 0, "create")?;
    write_u64(writer, central.len() as u64, "create")?;
    write_u64(writer, central.len() as u64, "create")?;
    write_u64(writer, central_size, "create")?;
    write_u64(writer, central_start, "create")?;
    write_u32(writer, ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR, "create")?;
    write_u32(writer, 0, "create")?;
    write_u64(writer, zip64_eocd_offset, "create")?;
    write_u32(writer, 1, "create")?;
    write_u32(writer, ZIP_END_OF_CENTRAL_DIRECTORY, "create")?;
    write_u16(writer, 0, "create")?;
    write_u16(writer, 0, "create")?;
    write_u16(writer, u16::MAX, "create")?;
    write_u16(writer, u16::MAX, "create")?;
    write_u32(writer, u32::MAX, "create")?;
    write_u32(writer, u32::MAX, "create")?;
    write_u16(writer, 0, "create")?;
    Ok(())
}

fn copy_source_with_hash<W: Write>(
    writer: &mut W,
    source: &BackupEntrySource,
    expected_snapshot: Option<&SourceFileSnapshot>,
    cancelled: Option<&AtomicBool>,
    mut on_chunk: impl FnMut(u64),
) -> Result<(u64, String, u32), BackupErrorInfo> {
    match source {
        BackupEntrySource::File(path) => {
            validate_source_file(path)?;
            let file = File::open(path).map_err(|_| source_read_error())?;
            let before =
                source_snapshot_from_metadata(&file.metadata().map_err(|_| source_read_error())?);
            if expected_snapshot.is_some_and(|expected| expected != &before) {
                return Err(source_changed_error());
            }
            let result =
                copy_reader_with_hash(writer, BufReader::new(&file), cancelled, &mut on_chunk)?;
            let after =
                source_snapshot_from_metadata(&file.metadata().map_err(|_| source_read_error())?);
            validate_source_file(path)?;
            let path_after = source_snapshot_from_metadata(
                &fs::metadata(path).map_err(|_| source_read_error())?,
            );
            if before != after
                || after != path_after
                || expected_snapshot.is_some_and(|expected| expected != &after)
            {
                return Err(source_changed_error());
            }
            Ok(result)
        }
        BackupEntrySource::Memory(bytes) => {
            copy_reader_with_hash(writer, bytes.as_slice(), cancelled, &mut on_chunk)
        }
    }
}

fn copy_reader_with_hash<W: Write>(
    writer: &mut W,
    mut reader: impl Read,
    cancelled: Option<&AtomicBool>,
    mut on_chunk: impl FnMut(u64),
) -> Result<(u64, String, u32), BackupErrorInfo> {
    let mut sha = Sha256::new();
    let mut crc = Crc32::new();
    let mut total = 0_u64;
    let mut buffer = Zeroizing::new([0_u8; 64 * 1024]);
    loop {
        if let Some(cancelled) = cancelled {
            ensure_write_not_cancelled(cancelled, "archive")?;
        }
        let read = reader
            .read(&mut buffer[..])
            .map_err(|_| source_read_error())?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            size_limit_error("create", "A backup entry exceeded the safety size counter.")
        })?;
        if total > MAX_ENTRY_UNCOMPRESSED_BYTES {
            return Err(size_limit_error(
                "create",
                "A backup entry exceeds the safety size limit.",
            ));
        }
        write_all(writer, &buffer[..read], "create")?;
        sha.update(&buffer[..read]);
        crc.update(&buffer[..read]);
        on_chunk(read as u64);
    }
    Ok((total, hex_lower(&sha.finalize()), crc.finish()))
}

fn write_all(writer: &mut impl Write, bytes: &[u8], action: &str) -> Result<(), BackupErrorInfo> {
    writer.write_all(bytes).map_err(|_| {
        backup_error(
            action,
            "container_write_failed",
            "CoffeePOS cannot write the backup container.",
            "Check free disk space and destination permissions, then retry.",
        )
    })
}

fn write_u16(writer: &mut impl Write, value: u16, action: &str) -> Result<(), BackupErrorInfo> {
    write_all(writer, &value.to_le_bytes(), action)
}

fn write_u32(writer: &mut impl Write, value: u32, action: &str) -> Result<(), BackupErrorInfo> {
    write_all(writer, &value.to_le_bytes(), action)
}

fn write_u64(writer: &mut impl Write, value: u64, action: &str) -> Result<(), BackupErrorInfo> {
    write_all(writer, &value.to_le_bytes(), action)
}

pub fn inspect_backup(
    path: &Path,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
) -> Result<BackupInspection, BackupErrorInfo> {
    validate_backup_internal(path, backup_password, target, "inspect")
        .map(|result| result.inspection)
}

pub fn validate_backup(
    path: &Path,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
) -> Result<BackupValidation, BackupErrorInfo> {
    validate_backup_internal(path, backup_password, target, "validate")
}

pub(crate) fn extract_restore_payload(
    path: &Path,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
    expected_encrypted_sha256: &str,
    destination_root: &Path,
    cancelled: &AtomicBool,
) -> Result<RestorePayload, BackupErrorInfo> {
    if cancelled.load(Ordering::Acquire) {
        return Err(backup_error(
            "restore_extract",
            "cancelled",
            "Restore was cancelled before backup extraction started.",
            "Choose the backup again when you are ready to retry restore.",
        ));
    }
    let mut file = open_backup_read_stable(path, "restore_extract")?;
    let validation = validate_backup_file(&mut file, backup_password, target, "restore_extract")?;
    verify_bound_restore_source(&mut file, expected_encrypted_sha256)?;
    if !validation.inspection.can_restore {
        return Err(backup_error(
            "restore_extract",
            "incompatible_backup",
            "This backup does not match the supported CoffeePOS restore baseline.",
            validation.inspection.compatibility.message.clone(),
        ));
    }
    prepare_restore_destination(destination_root)?;
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        backup_error(
            "restore_extract",
            "backup_file_unavailable",
            "CoffeePOS cannot rewind the validated backup handle for restore extraction.",
            "Choose the backup again and retry restore.",
        )
    })?;
    let decryptor = age::Decryptor::new(BufReader::new(&mut file)).map_err(|_| {
        backup_error(
            "restore_extract",
            "corrupt_encrypted_container",
            "The selected file is not a valid encrypted CoffeePOS backup container.",
            "Choose an intact CoffeePOS backup and retry.",
        )
    })?;
    let passphrase = SecretString::from(backup_password.to_owned());
    let identity = age::scrypt::Identity::new(passphrase);
    let decrypted = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| {
            backup_error(
                "restore_extract",
                "authentication_failed",
                "The backup password is incorrect or the encrypted backup changed.",
                "Re-inspect the selected backup with its original password before retrying restore.",
            )
        })?;
    let mut reader = CountingReader::new(decrypted);
    let mut store_config: Option<PortableStoreConfigV1> = None;
    let mut administrator_secret: Option<PortableAdministratorSecretV1> = None;
    let mut database_dump = None;
    let mut upload_files = 0_u64;
    let mut upload_bytes = 0_u64;

    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(backup_error(
                "restore_extract",
                "cancelled",
                "Restore was cancelled while decrypting the backup into private staging.",
                "CoffeePOS will discard the owned restore staging before another restore starts.",
            ));
        }
        let next = zip::read::read_zipfile_from_stream(&mut reader).map_err(|_| {
            backup_error(
                "restore_extract",
                "invalid_zip",
                "CoffeePOS could not read the validated backup again during restore extraction.",
                "The backup may have changed after inspection. Re-select and inspect it before retrying.",
            )
        })?;
        let Some(mut entry) = next else {
            break;
        };
        let raw_name = std::str::from_utf8(entry.name_raw()).map_err(|_| {
            backup_error(
                "restore_extract",
                "unsafe_archive_path",
                "The backup contains a non-UTF-8 archive path.",
                "Use an intact backup created by a compatible CoffeePOS version.",
            )
        })?;
        let archive_path = validate_archive_path(raw_name, "restore_extract")?;
        match archive_path.as_str() {
            "manifest.json" | INVENTORY_ENTRY => {
                io::copy(&mut entry, &mut io::sink()).map_err(|_| restore_extract_read_error())?;
            }
            DATABASE_ENTRY => {
                let output = destination_root.join("database/store.sql");
                write_restore_entry(&mut entry, destination_root, &output, cancelled)?;
                database_dump = Some(output);
            }
            STORE_CONFIG_ENTRY => {
                let bytes = read_control_json(
                    &mut entry,
                    MAX_PORTABLE_CONFIG_JSON_BYTES,
                    "restore_extract",
                    "portable store configuration",
                )?;
                let parsed = parse_store_config(&bytes, "restore_extract")?;
                validate_store_config(&parsed, "restore_extract")?;
                store_config = Some(parsed);
            }
            ADMINISTRATOR_SECRET_ENTRY => {
                let bytes = read_control_json(
                    &mut entry,
                    MAX_PORTABLE_CONFIG_JSON_BYTES,
                    "restore_extract",
                    "portable administrator credential",
                )?;
                let parsed = parse_administrator_secret(&bytes, "restore_extract")?;
                if parsed.password.is_empty() {
                    return Err(backup_error(
                        "restore_extract",
                        "invalid_administrator_secret",
                        "The encrypted administrator credential is empty.",
                        "Create a new backup after repairing the source administrator credential.",
                    ));
                }
                administrator_secret = Some(parsed);
            }
            _ if archive_path.starts_with(UPLOADS_ROOT) => {
                let relative = archive_path
                    .strip_prefix(UPLOADS_ROOT)
                    .ok_or_else(restore_extract_path_error)?;
                let output = safe_restore_upload_path(destination_root, relative)?;
                let size = entry.size();
                write_restore_entry(&mut entry, destination_root, &output, cancelled)?;
                upload_files = upload_files.saturating_add(1);
                upload_bytes = upload_bytes.checked_add(size).ok_or_else(|| {
                    size_limit_error("restore_extract", "Restore upload size counter overflowed.")
                })?;
            }
            _ => {
                return Err(backup_error(
                    "restore_extract",
                    "unexpected_restore_entry",
                    "The backup contains a payload entry that restore does not own.",
                    "Use a canonical CoffeePOS backup and retry restore.",
                ));
            }
        }
    }

    let store_config =
        store_config.ok_or_else(|| missing_entry_error("restore_extract", STORE_CONFIG_ENTRY))?;
    let administrator_secret = administrator_secret
        .ok_or_else(|| missing_entry_error("restore_extract", ADMINISTRATOR_SECRET_ENTRY))?;
    if administrator_secret.username != store_config.administrator.username {
        return Err(backup_error(
            "restore_extract",
            "administrator_identity_mismatch",
            "The restored administrator credential does not match the portable store identity.",
            "Use a new backup created from a healthy source store.",
        ));
    }
    let database_dump =
        database_dump.ok_or_else(|| missing_entry_error("restore_extract", DATABASE_ENTRY))?;
    drop(reader);
    verify_bound_restore_source(&mut file, expected_encrypted_sha256)?;
    Ok(RestorePayload {
        store_name: store_config.store_name,
        administrator_username: store_config.administrator.username,
        administrator_email: store_config.administrator.email,
        administrator_password: administrator_secret.password,
        database_dump,
        upload_files,
        upload_bytes,
    })
}

fn verify_bound_restore_source(
    file: &mut File,
    expected_encrypted_sha256: &str,
) -> Result<(), BackupErrorInfo> {
    if expected_encrypted_sha256.len() != 64
        || !expected_encrypted_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(backup_error(
            "restore_extract",
            "invalid_candidate_binding",
            "CoffeePOS restore candidate binding is invalid.",
            "Choose and inspect the backup again before applying restore.",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| restore_extract_read_error())?;
    let mut sha = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| restore_extract_read_error())?;
        if read == 0 {
            break;
        }
        sha.update(&buffer[..read]);
    }
    let actual = hex_lower(&sha.finalize());
    file.seek(SeekFrom::Start(0))
        .map_err(|_| restore_extract_read_error())?;
    if actual != expected_encrypted_sha256 {
        return Err(backup_error(
            "restore_extract",
            "stale_candidate",
            "The selected backup changed after it was inspected.",
            "Choose and inspect the backup again before applying restore.",
        ));
    }
    Ok(())
}

fn prepare_restore_destination(destination_root: &Path) -> Result<(), BackupErrorInfo> {
    if !destination_root.is_absolute() {
        return Err(restore_extract_path_error());
    }
    let metadata =
        fs::symlink_metadata(destination_root).map_err(|_| restore_extract_path_error())?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(restore_extract_path_error());
    }
    for relative in ["database", "uploads"] {
        let directory = destination_root.join(relative);
        match fs::symlink_metadata(&directory) {
            Ok(metadata)
                if metadata.file_type().is_dir()
                    && !metadata.file_type().is_symlink()
                    && !metadata_is_reparse_point(&metadata) => {}
            Ok(_) => return Err(restore_extract_path_error()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&directory).map_err(|_| restore_extract_path_error())?;
            }
            Err(_) => return Err(restore_extract_path_error()),
        }
    }
    Ok(())
}

fn safe_restore_upload_path(
    destination_root: &Path,
    relative: &str,
) -> Result<PathBuf, BackupErrorInfo> {
    if relative.is_empty() {
        return Err(restore_extract_path_error());
    }
    let mut output = destination_root.join("uploads");
    let parts = relative.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || *part == "." || *part == ".." {
            return Err(restore_extract_path_error());
        }
        output.push(part);
        if index + 1 < parts.len() {
            match fs::symlink_metadata(&output) {
                Ok(metadata)
                    if metadata.file_type().is_dir()
                        && !metadata.file_type().is_symlink()
                        && !metadata_is_reparse_point(&metadata) => {}
                Ok(_) => return Err(restore_extract_path_error()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&output).map_err(|_| restore_extract_path_error())?;
                }
                Err(_) => return Err(restore_extract_path_error()),
            }
        }
    }
    Ok(output)
}

fn write_restore_entry(
    reader: &mut impl Read,
    destination_root: &Path,
    output: &Path,
    cancelled: &AtomicBool,
) -> Result<(), BackupErrorInfo> {
    if !output.starts_with(destination_root) {
        return Err(restore_extract_path_error());
    }
    let parent = output.parent().ok_or_else(restore_extract_path_error)?;
    let metadata = fs::symlink_metadata(parent).map_err(|_| restore_extract_path_error())?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
    {
        return Err(restore_extract_path_error());
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|_| {
            backup_error(
                "restore_extract",
                "restore_staging_write_failed",
                "CoffeePOS cannot create a private restore staging file.",
                "Check CoffeePOS data-volume permissions and free space, then retry restore.",
            )
        })?;
    let mut buffer = Zeroizing::new([0_u8; 64 * 1024]);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(backup_error(
                "restore_extract",
                "cancelled",
                "Restore was cancelled while writing private staging.",
                "CoffeePOS will discard owned restore staging before another restore starts.",
            ));
        }
        let read = reader
            .read(&mut buffer[..])
            .map_err(|_| restore_extract_read_error())?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|_| {
            backup_error(
                "restore_extract",
                "restore_staging_write_failed",
                "CoffeePOS cannot write private restore staging.",
                "Check CoffeePOS data-volume free space and permissions, then retry restore.",
            )
        })?;
    }
    file.sync_all().map_err(|_| {
        backup_error(
            "restore_extract",
            "restore_staging_flush_failed",
            "CoffeePOS cannot flush private restore staging to disk.",
            "Check CoffeePOS data-volume health and retry restore.",
        )
    })
}

fn restore_extract_read_error() -> BackupErrorInfo {
    backup_error(
        "restore_extract",
        "restore_extract_read_failed",
        "CoffeePOS could not read the encrypted backup while building private restore staging.",
        "The backup may have changed or become unreadable. Re-select and inspect it before retrying.",
    )
}

fn restore_extract_path_error() -> BackupErrorInfo {
    backup_error(
        "restore_extract",
        "unsafe_restore_staging",
        "CoffeePOS refused an unsafe restore staging path.",
        "Preserve unknown filesystem entries and retry with a normal local CoffeePOS data directory.",
    )
}

fn validate_backup_internal(
    path: &Path,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
    action: &str,
) -> Result<BackupValidation, BackupErrorInfo> {
    let mut file = open_backup_read_stable(path, action)?;
    validate_backup_file(&mut file, backup_password, target, action)
}

fn validate_backup_file(
    file: &mut File,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
    action: &str,
) -> Result<BackupValidation, BackupErrorInfo> {
    if backup_password.is_empty() {
        return Err(backup_error(
            action,
            "authentication_failed",
            "The backup password is incorrect or missing.",
            "Enter the password used when this backup was created and retry.",
        ));
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        backup_error(
            action,
            "backup_file_unavailable",
            "CoffeePOS cannot rewind the selected backup file.",
            "Choose an existing readable .coffeepos-backup file and retry.",
        )
    })?;
    let decryptor = age::Decryptor::new(BufReader::new(file)).map_err(|_| {
        backup_error(
            "decrypt",
            "corrupt_encrypted_container",
            "The selected file is not a valid encrypted CoffeePOS backup container.",
            "Choose an intact CoffeePOS backup and retry.",
        )
    })?;
    let passphrase = SecretString::from(backup_password.to_owned());
    let identity = age::scrypt::Identity::new(passphrase);
    let decrypted = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|error| match error {
            age::DecryptError::NoMatchingKeys
            | age::DecryptError::KeyDecryptionFailed
            | age::DecryptError::DecryptionFailed => backup_error(
                "decrypt",
                "authentication_failed",
                "The backup password is incorrect.",
                "Enter the password used when this backup was created and retry.",
            ),
            _ => backup_error(
                "decrypt",
                "corrupt_encrypted_container",
                "CoffeePOS cannot decrypt this backup container safely.",
                "Use an intact compatible backup file and retry.",
            ),
        })?;
    let mut reader = CountingReader::new(decrypted);
    let validated = validate_decrypted_zip(&mut reader, target, action)?;
    let mut trailing = [0_u8; 1];
    match reader.read(&mut trailing) {
        Ok(0) => Ok(validated),
        Ok(_) => Err(backup_error(
            action,
            "invalid_zip",
            "The backup ZIP contains unexpected trailing data.",
            "Use an intact CoffeePOS backup and retry.",
        )),
        Err(_) => Err(backup_error(
            "decrypt",
            "corrupt_encrypted_container",
            "The encrypted backup failed authentication while being read.",
            "Use an intact backup file or verify the backup password.",
        )),
    }
}

#[cfg(windows)]
fn open_backup_read_stable(path: &Path, action: &str) -> Result<File, BackupErrorInfo> {
    // The restore extractor keeps this exact native file handle alive across validation, seek and
    // extraction. A path replacement therefore cannot swap in a second archive between passes.
    File::open(path).map_err(|_| {
        backup_error(
            action,
            "backup_file_unavailable",
            "CoffeePOS cannot open the selected backup file.",
            "Choose an existing readable .coffeepos-backup file and retry.",
        )
    })
}

#[cfg(not(windows))]
fn open_backup_read_stable(path: &Path, action: &str) -> Result<File, BackupErrorInfo> {
    File::open(path).map_err(|_| {
        backup_error(
            action,
            "backup_file_unavailable",
            "CoffeePOS cannot open the selected backup file.",
            "Choose an existing readable .coffeepos-backup file and retry.",
        )
    })
}

fn validate_decrypted_zip<R: Read>(
    reader: &mut CountingReader<R>,
    target: &BackupCompatibilityTarget,
    action: &str,
) -> Result<BackupValidation, BackupErrorInfo> {
    let mut manifest: Option<BackupManifestV1> = None;
    let mut inventory: Option<InventoryV1> = None;
    let mut inventory_map: BTreeMap<String, InventoryEntry> = BTreeMap::new();
    let mut local_entries = Vec::new();
    let mut local_casefold = BTreeSet::new();
    let mut seen_payload = BTreeSet::new();
    let mut store_config: Option<PortableStoreConfigV1> = None;
    let mut administrator_secret_username: Option<String> = None;
    let mut database_bytes = 0_u64;
    let mut uploads_bytes = 0_u64;
    let mut payload_total = 0_u64;
    let mut payload_count = 0_u64;

    loop {
        let local_header_offset = reader.position();
        let next = zip::read::read_zipfile_from_stream(reader).map_err(|error| {
            let code = if matches!(error, zip::result::ZipError::Io(_)) {
                "corrupt_encrypted_container"
            } else {
                "invalid_zip"
            };
            backup_error(
                action,
                code,
                "CoffeePOS cannot read the encrypted backup ZIP safely.",
                "Use an intact CoffeePOS backup and retry.",
            )
        })?;
        let Some(mut file) = next else {
            break;
        };
        if local_entries.len() as u64 >= MAX_PAYLOAD_ENTRIES + 2 {
            return Err(size_limit_error(
                action,
                "The backup contains too many ZIP entries.",
            ));
        }
        let raw_name = std::str::from_utf8(file.name_raw()).map_err(|_| {
            backup_error(
                action,
                "unsafe_archive_path",
                "The backup contains a non-UTF-8 archive path.",
                "Use a backup created by a compatible CoffeePOS version.",
            )
        })?;
        let path = validate_archive_path(raw_name, action)?;
        let key = windows_casefold(&path);
        if !local_casefold.insert(key) {
            return Err(backup_error(
                action,
                "duplicate_archive_path",
                "The backup contains duplicate paths after Windows normalization.",
                "Do not restore this archive. Create a new backup from the source store.",
            ));
        }
        if file.compressed_size() != file.size() {
            return Err(backup_error(
                action,
                "unsupported_zip_compression",
                "This schema-1 backup uses unsupported ZIP compression.",
                "Use a backup created by the supported CoffeePOS schema-1 writer.",
            ));
        }
        if file.size() > MAX_ENTRY_UNCOMPRESSED_BYTES {
            return Err(size_limit_error(
                action,
                "A backup entry exceeds the safety size limit.",
            ));
        }
        local_entries.push(LocalEntryMetadata {
            path: path.clone(),
            size: file.size(),
            compressed_size: file.compressed_size(),
            crc32: file.crc32(),
            local_header_offset,
        });

        match local_entries.len() {
            1 => {
                if path != "manifest.json" {
                    return Err(backup_error(
                        action,
                        "invalid_zip_layout",
                        "The first backup ZIP entry is not manifest.json.",
                        "Use a canonical CoffeePOS backup and retry.",
                    ));
                }
                let bytes =
                    read_control_json(&mut file, MAX_MANIFEST_JSON_BYTES, action, "manifest")?;
                let parsed = parse_manifest(&bytes, action)?;
                validate_manifest_shape(&parsed, action)?;
                if parsed.total_uncompressed_bytes > target.available_restore_bytes {
                    return Err(backup_error(
                        action,
                        "insufficient_disk_space",
                        "This backup is larger than the currently available restore disk space.",
                        "Free disk space on the CoffeePOS data volume or choose another supported restore target before retrying.",
                    ));
                }
                manifest = Some(parsed);
            }
            2 => {
                if path != INVENTORY_ENTRY {
                    return Err(backup_error(
                        action,
                        "invalid_zip_layout",
                        "The second backup ZIP entry is not inventory.json.",
                        "Use a canonical CoffeePOS backup and retry.",
                    ));
                }
                let bytes =
                    read_control_json(&mut file, MAX_INVENTORY_JSON_BYTES, action, "inventory")?;
                let parsed = parse_inventory(&bytes, action)?;
                validate_inventory_shape(&parsed, action)?;
                for entry in &parsed.entries {
                    inventory_map.insert(entry.path.clone(), entry.clone());
                }
                inventory = Some(parsed);
            }
            _ => {
                let expected = inventory_map.get(&path).ok_or_else(|| {
                    backup_error(
                        action,
                        "extra_archive_entry",
                        "The backup contains a data entry that is not declared in inventory.json.",
                        "Do not restore this archive. Create a new backup from the source store.",
                    )
                })?;
                if !seen_payload.insert(path.clone()) {
                    return Err(backup_error(
                        action,
                        "duplicate_archive_path",
                        "The backup contains a duplicate payload entry.",
                        "Do not restore this archive. Create a new backup from the source store.",
                    ));
                }
                if expected.size_bytes != file.size() {
                    return Err(backup_error(
                        action,
                        "size_mismatch",
                        "A backup entry size does not match inventory.json.",
                        "Do not restore this archive. Create or select an intact backup.",
                    ));
                }
                let mut sha = Sha256::new();
                let mut exact_bytes =
                    if path == STORE_CONFIG_ENTRY || path == ADMINISTRATOR_SECRET_ENTRY {
                        if file.size() > MAX_PORTABLE_CONFIG_JSON_BYTES {
                            return Err(size_limit_error(
                                action,
                                "A backup control entry exceeds its safety size limit.",
                            ));
                        }
                        Some(Zeroizing::new(Vec::with_capacity(file.size() as usize)))
                    } else {
                        None
                    };
                let mut counted = 0_u64;
                let mut buffer = Zeroizing::new([0_u8; 64 * 1024]);
                loop {
                    let read = file.read(&mut buffer[..]).map_err(|_| {
                        backup_error(
                            action,
                            "checksum_mismatch",
                            "A backup entry failed ZIP integrity validation.",
                            "Do not restore this archive. Create or select an intact backup.",
                        )
                    })?;
                    if read == 0 {
                        break;
                    }
                    counted = counted.saturating_add(read as u64);
                    sha.update(&buffer[..read]);
                    if let Some(bytes) = exact_bytes.as_mut() {
                        bytes.extend_from_slice(&buffer[..read]);
                    }
                }
                let actual_sha = hex_lower(&sha.finalize());
                if counted != expected.size_bytes || actual_sha != expected.sha256 {
                    return Err(backup_error(
                        action,
                        "checksum_mismatch",
                        "A backup entry checksum does not match inventory.json.",
                        "Do not restore this archive. Create or select an intact backup.",
                    ));
                }
                payload_count = payload_count.saturating_add(1);
                payload_total = payload_total.checked_add(counted).ok_or_else(|| {
                    size_limit_error(
                        action,
                        "The backup payload size overflowed the safety counter.",
                    )
                })?;
                if payload_total > MAX_TOTAL_UNCOMPRESSED_BYTES {
                    return Err(size_limit_error(
                        action,
                        "The backup payload exceeds the safety size limit.",
                    ));
                }
                if path == DATABASE_ENTRY {
                    database_bytes = counted;
                } else if path.starts_with(UPLOADS_ROOT) {
                    uploads_bytes = uploads_bytes.saturating_add(counted);
                } else if path == STORE_CONFIG_ENTRY {
                    let bytes = exact_bytes
                        .as_ref()
                        .map(|value| value.as_slice())
                        .unwrap_or(&[]);
                    let parsed = parse_store_config(bytes, action)?;
                    validate_store_config(&parsed, action)?;
                    store_config = Some(parsed);
                } else if path == ADMINISTRATOR_SECRET_ENTRY {
                    let bytes = exact_bytes
                        .as_ref()
                        .map(|value| value.as_slice())
                        .unwrap_or(&[]);
                    let parsed = parse_administrator_secret(bytes, action)?;
                    if parsed.password.is_empty() {
                        return Err(backup_error(
                            action,
                            "invalid_administrator_secret",
                            "The encrypted administrator secret is empty.",
                            "Create a new backup after repairing the administrator credential.",
                        ));
                    }
                    administrator_secret_username = Some(parsed.username);
                }
            }
        }
    }

    let central_start = reader.position().checked_sub(4).ok_or_else(|| {
        backup_error(
            action,
            "invalid_zip",
            "The backup ZIP central directory is invalid.",
            "Use an intact CoffeePOS backup and retry.",
        )
    })?;
    validate_central_directory(reader, &local_entries, central_start, action)?;

    let manifest = manifest.ok_or_else(|| missing_entry_error(action, "manifest.json"))?;
    let inventory = inventory.ok_or_else(|| missing_entry_error(action, INVENTORY_ENTRY))?;
    if payload_count != inventory.total_files || seen_payload.len() != inventory.entries.len() {
        return Err(backup_error(
            action,
            "missing_inventory_entry",
            "One or more files declared in inventory.json are missing from the backup.",
            "Do not restore this archive. Create or select an intact backup.",
        ));
    }
    if payload_total != inventory.total_uncompressed_bytes
        || manifest.total_files != inventory.total_files
        || manifest.total_uncompressed_bytes != inventory.total_uncompressed_bytes
    {
        return Err(backup_error(
            action,
            "size_mismatch",
            "Backup totals do not match the validated payload.",
            "Do not restore this archive. Create or select an intact backup.",
        ));
    }
    let store_config =
        store_config.ok_or_else(|| missing_entry_error(action, STORE_CONFIG_ENTRY))?;
    let administrator_secret_username = administrator_secret_username
        .ok_or_else(|| missing_entry_error(action, ADMINISTRATOR_SECRET_ENTRY))?;
    if administrator_secret_username != store_config.administrator.username {
        return Err(backup_error(
            action,
            "administrator_identity_mismatch",
            "The encrypted administrator secret does not match the portable store configuration.",
            "Create a new backup after repairing the administrator account metadata.",
        ));
    }

    let compatibility = evaluate_compatibility(&manifest, target);
    let can_restore = compatibility.can_restore();
    let warning_metadata = manifest.warnings.clone();
    let warning_codes = warning_metadata
        .iter()
        .map(|warning| warning.code.clone())
        .collect();
    let inspection = BackupInspection {
        backup_id: manifest.backup_id,
        created_at: manifest.created_at,
        store_name: store_config.store_name,
        source: manifest.source,
        database_bytes,
        uploads_bytes,
        total_files: inventory.total_files,
        total_uncompressed_bytes: inventory.total_uncompressed_bytes,
        compatibility,
        warnings: warning_codes,
        warning_metadata,
        can_restore,
    };
    Ok(BackupValidation {
        valid: true,
        entries_validated: payload_count,
        inspection,
    })
}

fn read_control_json(
    reader: &mut impl Read,
    max_bytes: u64,
    action: &str,
    label: &str,
) -> Result<Zeroizing<Vec<u8>>, BackupErrorInfo> {
    let mut bytes = Zeroizing::new(Vec::new());
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| {
            backup_error(
                action,
                "invalid_control_json",
                "CoffeePOS cannot read backup control metadata.",
                "Use an intact CoffeePOS backup and retry.",
            )
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(size_limit_error(
            action,
            format!("The backup {label} exceeds its schema-1 metadata size limit."),
        ));
    }
    Ok(bytes)
}

fn parse_manifest(bytes: &[u8], action: &str) -> Result<BackupManifestV1, BackupErrorInfo> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| invalid_manifest_error(action))?;
    let schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| invalid_manifest_error(action))?;
    if schema != u64::from(BACKUP_SCHEMA_VERSION) {
        return Err(backup_error(
            action,
            "unsupported_backup_schema",
            "This backup uses an unsupported backup schema version.",
            "Use a CoffeePOS Desktop version that explicitly supports this backup schema.",
        ));
    }
    serde_json::from_value(value).map_err(|_| invalid_manifest_error(action))
}

fn parse_inventory(bytes: &[u8], action: &str) -> Result<InventoryV1, BackupErrorInfo> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| invalid_inventory_error(action))?;
    let schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| invalid_inventory_error(action))?;
    if schema != u64::from(INVENTORY_SCHEMA_VERSION) {
        return Err(backup_error(
            action,
            "unsupported_inventory_schema",
            "This backup uses an unsupported inventory schema version.",
            "Use a CoffeePOS Desktop version that explicitly supports this inventory schema.",
        ));
    }
    serde_json::from_value(value).map_err(|_| invalid_inventory_error(action))
}

fn parse_store_config(
    bytes: &[u8],
    action: &str,
) -> Result<PortableStoreConfigV1, BackupErrorInfo> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| {
        backup_error(
            action,
            "invalid_store_config",
            "The portable store configuration is invalid.",
            "Create a new backup from a healthy source store.",
        )
    })?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(STORE_CONFIG_SCHEMA_VERSION))
    {
        return Err(backup_error(
            action,
            "unsupported_store_config_schema",
            "The portable store configuration uses an unsupported schema.",
            "Use a CoffeePOS Desktop version that supports this store configuration schema.",
        ));
    }
    serde_json::from_value(value).map_err(|_| {
        backup_error(
            action,
            "invalid_store_config",
            "The portable store configuration is invalid.",
            "Create a new backup from a healthy source store.",
        )
    })
}

fn parse_administrator_secret(
    bytes: &[u8],
    action: &str,
) -> Result<PortableAdministratorSecretV1, BackupErrorInfo> {
    let parsed: PortableAdministratorSecretV1 = serde_json::from_slice(bytes).map_err(|_| {
        backup_error(
            action,
            "invalid_administrator_secret",
            "The encrypted administrator credential record is invalid.",
            "Create a new backup after repairing the administrator credential.",
        )
    })?;
    if parsed.schema_version != ADMINISTRATOR_SECRET_SCHEMA_VERSION {
        return Err(backup_error(
            action,
            "unsupported_administrator_secret_schema",
            "The encrypted administrator credential uses an unsupported schema.",
            "Use a CoffeePOS Desktop version that supports this credential schema.",
        ));
    }
    Ok(parsed)
}

fn invalid_manifest_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "invalid_manifest",
        "The backup manifest is missing required schema-1 fields or contains unsupported fields.",
        "Use a backup created by a compatible CoffeePOS Desktop version.",
    )
}

fn invalid_inventory_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "invalid_inventory",
        "The backup inventory is missing required schema-1 fields or contains unsupported fields.",
        "Use a backup created by a compatible CoffeePOS Desktop version.",
    )
}

fn validate_manifest_shape(
    manifest: &BackupManifestV1,
    action: &str,
) -> Result<(), BackupErrorInfo> {
    if manifest.schema_version != BACKUP_SCHEMA_VERSION
        || manifest.kind != BACKUP_KIND
        || manifest.inventory_entry != INVENTORY_ENTRY
        || manifest.database.format != "mariadb_logical_sql"
        || manifest.database.entry != DATABASE_ENTRY
        || manifest.database.database_name != DATABASE_NAME
        || manifest.uploads.root != UPLOADS_ROOT
        || manifest.store_config.entry != STORE_CONFIG_ENTRY
        || manifest.administrator_secret.entry != ADMINISTRATOR_SECRET_ENTRY
        || !manifest.administrator_secret.present
        || manifest.total_files > MAX_PAYLOAD_ENTRIES
        || manifest.total_uncompressed_bytes > MAX_TOTAL_UNCOMPRESSED_BYTES
        || !valid_uuid(&manifest.backup_id)
        || !valid_rfc3339_utc(&manifest.created_at)
        || !valid_source_versions(&manifest.source)
    {
        return Err(invalid_manifest_error(action));
    }
    let mut warning_codes = BTreeSet::new();
    if manifest.warnings.len() > MAX_WARNINGS
        || manifest.warnings.iter().any(|warning| {
            validate_warning(warning, action).is_err()
                || !warning_codes.insert(warning.code.as_str())
        })
    {
        return Err(invalid_manifest_error(action));
    }
    Ok(())
}

fn validate_warning(warning: &BackupWarning, action: &str) -> Result<(), BackupErrorInfo> {
    let valid = match warning.code.as_str() {
        WARNING_UNMANAGED_EXTENSIONS_EXCLUDED => warning.items.is_empty(),
        WARNING_UNMANAGED_SITE_CODE_NOT_INCLUDED => {
            !warning.items.is_empty()
                && warning.items.len() <= MAX_WARNING_ITEMS
                && warning.items.iter().all(|item| {
                    !item.is_empty()
                        && item.len() <= MAX_WARNING_ITEM_BYTES
                        && item.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                        })
                })
                && warning.items.iter().collect::<BTreeSet<_>>().len() == warning.items.len()
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(backup_error(
            action,
            "invalid_manifest",
            "The backup manifest contains unsupported or unsafe warning metadata.",
            "Create a new backup from a healthy source store without exposing source paths in warning metadata.",
        ))
    }
}

fn valid_source_versions(source: &BackupSourceVersions) -> bool {
    [
        &source.desktop_version,
        &source.runtime_version,
        &source.target,
        &source.php_version,
        &source.mariadb_version,
        &source.wordpress_version,
        &source.woocommerce_version,
        &source.coffeepos_version,
    ]
    .iter()
    .all(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
}

fn validate_inventory_shape(inventory: &InventoryV1, action: &str) -> Result<(), BackupErrorInfo> {
    if inventory.schema_version != INVENTORY_SCHEMA_VERSION
        || inventory.entries.len() as u64 != inventory.total_files
    {
        return Err(invalid_inventory_error(action));
    }
    if inventory.total_files > MAX_PAYLOAD_ENTRIES {
        return Err(size_limit_error(
            action,
            "The inventory contains too many payload entries.",
        ));
    }
    if inventory.total_uncompressed_bytes > MAX_TOTAL_UNCOMPRESSED_BYTES {
        return Err(size_limit_error(
            action,
            "The inventory exceeds the backup safety size limit.",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for entry in &inventory.entries {
        let canonical = validate_archive_path(&entry.path, action)?;
        if canonical != entry.path || !is_allowed_data_path(&entry.path) {
            return Err(backup_error(
                action,
                "unsupported_payload_entry",
                "inventory.json declares a file outside the portable backup schema.",
                "Do not restore this archive. Create a new backup from the source store.",
            ));
        }
        if !seen.insert(windows_casefold(&entry.path)) {
            return Err(backup_error(
                action,
                "duplicate_archive_path",
                "inventory.json contains duplicate paths after Windows normalization.",
                "Do not restore this archive. Create a new backup from the source store.",
            ));
        }
        if entry.size_bytes > MAX_ENTRY_UNCOMPRESSED_BYTES {
            return Err(size_limit_error(
                action,
                "An inventory entry exceeds the backup safety size limit.",
            ));
        }
        if !valid_sha256(&entry.sha256) {
            return Err(invalid_inventory_error(action));
        }
        total = total.checked_add(entry.size_bytes).ok_or_else(|| {
            size_limit_error(action, "The inventory total overflowed the safety counter.")
        })?;
        if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(size_limit_error(
                action,
                "The inventory exceeds the backup safety size limit.",
            ));
        }
    }
    if total != inventory.total_uncompressed_bytes {
        return Err(backup_error(
            action,
            "size_mismatch",
            "inventory.json totals do not match its entries.",
            "Do not restore this archive. Create a new backup from the source store.",
        ));
    }
    require_inventory_shape(inventory, action)
}

fn require_payload_shape(
    entries: &[PreparedPayloadEntry],
    action: &str,
) -> Result<(), BackupErrorInfo> {
    let paths = entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<BTreeSet<_>>();
    for required in [
        DATABASE_ENTRY,
        STORE_CONFIG_ENTRY,
        ADMINISTRATOR_SECRET_ENTRY,
    ] {
        if !paths.contains(required) {
            return Err(missing_entry_error(action, required));
        }
    }
    Ok(())
}

fn require_inventory_shape(inventory: &InventoryV1, action: &str) -> Result<(), BackupErrorInfo> {
    let paths = inventory
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<BTreeSet<_>>();
    for required in [
        DATABASE_ENTRY,
        STORE_CONFIG_ENTRY,
        ADMINISTRATOR_SECRET_ENTRY,
    ] {
        if !paths.contains(required) {
            return Err(missing_entry_error(action, required));
        }
    }
    Ok(())
}

fn missing_entry_error(action: &str, path: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "missing_required_entry",
        format!("The backup is missing required schema entry {path}."),
        "Do not restore this archive. Create or select an intact CoffeePOS backup.",
    )
}

fn validate_store_config(
    config: &PortableStoreConfigV1,
    action: &str,
) -> Result<(), BackupErrorInfo> {
    let valid = config.schema_version == STORE_CONFIG_SCHEMA_VERSION
        && !config.store_name.trim().is_empty()
        && config.store_name.chars().count() <= 80
        && !config.store_name.chars().any(char::is_control)
        && valid_portable_admin_username(&config.administrator.username)
        && valid_portable_admin_email(&config.administrator.email);
    if valid {
        Ok(())
    } else {
        Err(backup_error(
            action,
            "invalid_store_config",
            "The portable store configuration contains invalid store or administrator metadata.",
            "Create a new backup from a healthy source store.",
        ))
    }
}

fn valid_portable_admin_username(value: &str) -> bool {
    let value = value.trim();
    (3..=60).contains(&value.chars().count())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_portable_admin_email(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 100
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return false;
    }
    let Some((local, domain)) = value.rsplit_once('@') else {
        return false;
    };
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'_' | b'-'))
    {
        return false;
    }
    let labels = domain.split('.').collect::<Vec<_>>();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn evaluate_compatibility(
    manifest: &BackupManifestV1,
    target: &BackupCompatibilityTarget,
) -> BackupCompatibilityResult {
    if target.restore_schema < manifest.compatibility.minimum_restore_schema {
        return BackupCompatibilityResult::blocked(
            "restore_schema_too_old",
            "This backup requires a newer restore schema than this CoffeePOS Desktop supports.",
        );
    }
    if manifest.compatibility.requires_explicit_migration {
        return BackupCompatibilityResult::blocked(
            "explicit_migration_required",
            "This backup requires an explicit migration that is not registered in the schema-1 restore baseline.",
        );
    }
    let source = &manifest.source;
    let expected = &target.source;
    if source.target != expected.target {
        return BackupCompatibilityResult::blocked(
            "unqualified_source_target",
            "This backup comes from a source target that is not qualified for restore on this CoffeePOS Desktop target.",
        );
    }
    let version_matches = source.desktop_version == expected.desktop_version
        && source.runtime_version == expected.runtime_version
        && source.php_version == expected.php_version
        && source.mariadb_version == expected.mariadb_version
        && source.wordpress_version == expected.wordpress_version
        && source.woocommerce_version == expected.woocommerce_version
        && source.coffeepos_version == expected.coffeepos_version
        && source.provisioning_schema == expected.provisioning_schema
        && source.app_config_schema == expected.app_config_schema;
    if !version_matches {
        return BackupCompatibilityResult::blocked(
            "incompatible_component_version",
            "This schema-1 backup does not match the exact component/schema baseline supported by this CoffeePOS Desktop build.",
        );
    }
    BackupCompatibilityResult::compatible()
}

fn validate_archive_path(raw: &str, action: &str) -> Result<String, BackupErrorInfo> {
    let invalid = raw.is_empty()
        || raw.len() > MAX_ARCHIVE_PATH_BYTES
        || raw.contains('\0')
        || raw.contains('\\')
        || raw.starts_with('/')
        || raw.starts_with("//")
        || (raw.len() >= 2 && raw.as_bytes()[0].is_ascii_alphabetic() && raw.as_bytes()[1] == b':');
    if invalid {
        return Err(unsafe_path_error(action));
    }
    for segment in raw.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.ends_with('.')
            || segment.ends_with(' ')
            || segment.chars().any(char::is_control)
            || segment
                .chars()
                .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
            || is_windows_reserved_name(segment)
        {
            return Err(unsafe_path_error(action));
        }
    }
    Ok(raw.to_string())
}

fn unsafe_path_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "unsafe_archive_path",
        "The backup contains an unsafe or ambiguous archive path.",
        "Do not extract or restore this archive. Create a new backup from the source store.",
    )
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

fn windows_casefold(path: &str) -> String {
    path.to_lowercase()
}

fn is_allowed_data_path(path: &str) -> bool {
    path == DATABASE_ENTRY
        || path == STORE_CONFIG_ENTRY
        || path == ADMINISTRATOR_SECRET_ENTRY
        || (path.starts_with(UPLOADS_ROOT) && path.len() > UPLOADS_ROOT.len())
}

fn validate_central_directory<R: Read>(
    reader: &mut CountingReader<R>,
    local_entries: &[LocalEntryMetadata],
    central_start: u64,
    action: &str,
) -> Result<(), BackupErrorInfo> {
    let mut central = Vec::new();
    let mut signature = ZIP_CENTRAL_DIRECTORY_HEADER;
    while signature == ZIP_CENTRAL_DIRECTORY_HEADER {
        if central.len() >= local_entries.len().saturating_add(1) {
            return Err(backup_error(
                action,
                "invalid_zip_central_directory",
                "The ZIP central directory contains unexpected extra entries.",
                "Use an intact CoffeePOS backup and retry.",
            ));
        }
        central.push(read_central_entry_after_signature(reader, action)?);
        signature = read_u32(reader, action)?;
    }
    let central_end = reader
        .position()
        .checked_sub(4)
        .ok_or_else(|| invalid_zip_error(action))?;
    if central.len() != local_entries.len() {
        return Err(backup_error(
            action,
            "invalid_zip_central_directory",
            "The ZIP central directory does not match the local file entries.",
            "Use an intact CoffeePOS backup and retry.",
        ));
    }
    let mut central_casefold = BTreeSet::new();
    for (local, central_entry) in local_entries.iter().zip(central.iter()) {
        if !central_casefold.insert(windows_casefold(&central_entry.path)) {
            return Err(backup_error(
                action,
                "duplicate_archive_path",
                "The ZIP central directory contains duplicate paths after Windows normalization.",
                "Do not restore this archive. Create a new backup from the source store.",
            ));
        }
        if local.path != central_entry.path
            || local.size != central_entry.size
            || local.compressed_size != central_entry.compressed_size
            || local.crc32 != central_entry.crc32
            || local.local_header_offset != central_entry.local_header_offset
        {
            return Err(backup_error(
                action,
                "invalid_zip_central_directory",
                "The ZIP central directory metadata does not match the validated entries.",
                "Use an intact CoffeePOS backup and retry.",
            ));
        }
    }
    let central_size = central_end
        .checked_sub(central_start)
        .ok_or_else(|| invalid_zip_error(action))?;
    match signature {
        ZIP64_END_OF_CENTRAL_DIRECTORY => {
            let zip64_eocd_offset = reader
                .position()
                .checked_sub(4)
                .ok_or_else(|| invalid_zip_error(action))?;
            validate_zip64_end_records(
                reader,
                central.len() as u64,
                central_start,
                central_size,
                zip64_eocd_offset,
                action,
            )?;
        }
        ZIP_END_OF_CENTRAL_DIRECTORY => {
            validate_classic_end_record(
                reader,
                central.len() as u64,
                central_start,
                central_size,
                false,
                action,
            )?;
        }
        _ => return Err(invalid_zip_error(action)),
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ParsedCentralEntry {
    path: String,
    size: u64,
    compressed_size: u64,
    crc32: u32,
    local_header_offset: u64,
}

fn read_central_entry_after_signature<R: Read>(
    reader: &mut CountingReader<R>,
    action: &str,
) -> Result<ParsedCentralEntry, BackupErrorInfo> {
    let version_made_by = read_u16(reader, action)?;
    let _version_needed = read_u16(reader, action)?;
    let flags = read_u16(reader, action)?;
    let compression = read_u16(reader, action)?;
    let _time = read_u16(reader, action)?;
    let _date = read_u16(reader, action)?;
    let crc32 = read_u32(reader, action)?;
    let compressed32 = read_u32(reader, action)?;
    let uncompressed32 = read_u32(reader, action)?;
    let name_len = read_u16(reader, action)? as usize;
    let extra_len = read_u16(reader, action)? as usize;
    let comment_len = read_u16(reader, action)? as usize;
    let disk_start = read_u16(reader, action)?;
    let _internal_attributes = read_u16(reader, action)?;
    let external_attributes = read_u32(reader, action)?;
    let local_offset32 = read_u32(reader, action)?;
    if flags & 1 != 0 || flags & (1 << 3) != 0 || compression != 0 {
        return Err(invalid_zip_error(action));
    }
    let mut name = vec![0_u8; name_len];
    read_exact(reader, &mut name, action)?;
    let name = std::str::from_utf8(&name).map_err(|_| unsafe_path_error(action))?;
    let path = validate_archive_path(name, action)?;
    let mut extra = vec![0_u8; extra_len];
    read_exact(reader, &mut extra, action)?;
    let mut comment = vec![0_u8; comment_len];
    read_exact(reader, &mut comment, action)?;
    if !comment.is_empty() {
        return Err(invalid_zip_error(action));
    }
    let (uncompressed, compressed, local_offset, zip64_disk) = parse_zip64_extra(
        &extra,
        uncompressed32,
        compressed32,
        local_offset32,
        disk_start,
        action,
    )?;
    if zip64_disk != 0 {
        return Err(invalid_zip_error(action));
    }
    let system = (version_made_by >> 8) as u8;
    if system == 3 {
        let mode = external_attributes >> 16;
        let file_type = mode & 0o170000;
        if file_type != 0 && file_type != 0o100000 {
            return Err(backup_error(
                action,
                "special_archive_entry",
                "The backup contains a symlink or special-file ZIP entry.",
                "Do not restore this archive. Create a new backup without links or special files.",
            ));
        }
    }
    Ok(ParsedCentralEntry {
        path,
        size: uncompressed,
        compressed_size: compressed,
        crc32,
        local_header_offset: local_offset,
    })
}

fn parse_zip64_extra(
    extra: &[u8],
    uncompressed32: u32,
    compressed32: u32,
    local_offset32: u32,
    disk_start16: u16,
    action: &str,
) -> Result<(u64, u64, u64, u32), BackupErrorInfo> {
    let needs_zip64 = uncompressed32 == u32::MAX
        || compressed32 == u32::MAX
        || local_offset32 == u32::MAX
        || disk_start16 == u16::MAX;
    if !needs_zip64 {
        return Ok((
            u64::from(uncompressed32),
            u64::from(compressed32),
            u64::from(local_offset32),
            u32::from(disk_start16),
        ));
    }
    let mut cursor = 0usize;
    while cursor + 4 <= extra.len() {
        let id = u16::from_le_bytes([extra[cursor], extra[cursor + 1]]);
        let len = u16::from_le_bytes([extra[cursor + 2], extra[cursor + 3]]) as usize;
        cursor += 4;
        let end = cursor
            .checked_add(len)
            .ok_or_else(|| invalid_zip_error(action))?;
        if end > extra.len() {
            return Err(invalid_zip_error(action));
        }
        if id == ZIP64_EXTRA_FIELD_ID {
            let field = &extra[cursor..end];
            let mut index = 0usize;
            let mut take_u64 = || -> Result<u64, BackupErrorInfo> {
                let end = index
                    .checked_add(8)
                    .ok_or_else(|| invalid_zip_error(action))?;
                let bytes = field
                    .get(index..end)
                    .ok_or_else(|| invalid_zip_error(action))?;
                index = end;
                Ok(u64::from_le_bytes(
                    bytes.try_into().map_err(|_| invalid_zip_error(action))?,
                ))
            };
            let uncompressed = if uncompressed32 == u32::MAX {
                take_u64()?
            } else {
                u64::from(uncompressed32)
            };
            let compressed = if compressed32 == u32::MAX {
                take_u64()?
            } else {
                u64::from(compressed32)
            };
            let local_offset = if local_offset32 == u32::MAX {
                take_u64()?
            } else {
                u64::from(local_offset32)
            };
            let disk = if disk_start16 == u16::MAX {
                let end = index
                    .checked_add(4)
                    .ok_or_else(|| invalid_zip_error(action))?;
                let bytes = field
                    .get(index..end)
                    .ok_or_else(|| invalid_zip_error(action))?;
                u32::from_le_bytes(bytes.try_into().map_err(|_| invalid_zip_error(action))?)
            } else {
                u32::from(disk_start16)
            };
            return Ok((uncompressed, compressed, local_offset, disk));
        }
        cursor = end;
    }
    Err(invalid_zip_error(action))
}

fn validate_zip64_end_records<R: Read>(
    reader: &mut CountingReader<R>,
    entry_count: u64,
    central_start: u64,
    central_size: u64,
    zip64_eocd_offset: u64,
    action: &str,
) -> Result<(), BackupErrorInfo> {
    let record_size = read_u64(reader, action)?;
    if !(44..=1024 * 1024).contains(&record_size) {
        return Err(invalid_zip_error(action));
    }
    let _version_made_by = read_u16(reader, action)?;
    let _version_needed = read_u16(reader, action)?;
    let disk = read_u32(reader, action)?;
    let central_disk = read_u32(reader, action)?;
    let entries_disk = read_u64(reader, action)?;
    let entries_total = read_u64(reader, action)?;
    let recorded_central_size = read_u64(reader, action)?;
    let recorded_central_start = read_u64(reader, action)?;
    if disk != 0
        || central_disk != 0
        || entries_disk != entry_count
        || entries_total != entry_count
        || recorded_central_size != central_size
        || recorded_central_start != central_start
    {
        return Err(invalid_zip_error(action));
    }
    let extensible_len = record_size - 44;
    skip_exact(reader, extensible_len, action)?;
    if read_u32(reader, action)? != ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR {
        return Err(invalid_zip_error(action));
    }
    let locator_disk = read_u32(reader, action)?;
    let locator_zip64_offset = read_u64(reader, action)?;
    let total_disks = read_u32(reader, action)?;
    if locator_disk != 0 || locator_zip64_offset != zip64_eocd_offset || total_disks != 1 {
        return Err(invalid_zip_error(action));
    }
    if read_u32(reader, action)? != ZIP_END_OF_CENTRAL_DIRECTORY {
        return Err(invalid_zip_error(action));
    }
    validate_classic_end_record(
        reader,
        entry_count,
        central_start,
        central_size,
        true,
        action,
    )
}

fn validate_classic_end_record<R: Read>(
    reader: &mut CountingReader<R>,
    entry_count: u64,
    central_start: u64,
    central_size: u64,
    zip64: bool,
    action: &str,
) -> Result<(), BackupErrorInfo> {
    let disk = read_u16(reader, action)?;
    let central_disk = read_u16(reader, action)?;
    let entries_disk = read_u16(reader, action)?;
    let entries_total = read_u16(reader, action)?;
    let recorded_central_size = read_u32(reader, action)?;
    let recorded_central_start = read_u32(reader, action)?;
    let comment_len = read_u16(reader, action)? as u64;
    if disk != 0 || central_disk != 0 {
        return Err(invalid_zip_error(action));
    }
    if zip64 {
        if entries_disk != u16::MAX
            || entries_total != u16::MAX
            || recorded_central_size != u32::MAX
            || recorded_central_start != u32::MAX
        {
            return Err(invalid_zip_error(action));
        }
    } else if u64::from(entries_disk) != entry_count
        || u64::from(entries_total) != entry_count
        || u64::from(recorded_central_size) != central_size
        || u64::from(recorded_central_start) != central_start
    {
        return Err(invalid_zip_error(action));
    }
    skip_exact(reader, comment_len, action)?;
    Ok(())
}

fn invalid_zip_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "invalid_zip",
        "The decrypted backup ZIP structure is invalid or inconsistent.",
        "Use an intact CoffeePOS backup and retry.",
    )
}

fn read_exact(
    reader: &mut impl Read,
    buffer: &mut [u8],
    action: &str,
) -> Result<(), BackupErrorInfo> {
    reader
        .read_exact(buffer)
        .map_err(|_| invalid_zip_error(action))
}

fn read_u16(reader: &mut impl Read, action: &str) -> Result<u16, BackupErrorInfo> {
    let mut bytes = [0_u8; 2];
    read_exact(reader, &mut bytes, action)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(reader: &mut impl Read, action: &str) -> Result<u32, BackupErrorInfo> {
    let mut bytes = [0_u8; 4];
    read_exact(reader, &mut bytes, action)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read, action: &str) -> Result<u64, BackupErrorInfo> {
    let mut bytes = [0_u8; 8];
    read_exact(reader, &mut bytes, action)?;
    Ok(u64::from_le_bytes(bytes))
}

fn skip_exact(reader: &mut impl Read, mut bytes: u64, action: &str) -> Result<(), BackupErrorInfo> {
    let mut buffer = [0_u8; 4096];
    while bytes > 0 {
        let wanted = usize::try_from(bytes.min(buffer.len() as u64)).unwrap_or(buffer.len());
        read_exact(reader, &mut buffer[..wanted], action)?;
        bytes -= wanted as u64;
    }
    Ok(())
}

fn size_limit_error(action: &str, message: impl Into<String>) -> BackupErrorInfo {
    backup_error(
        action,
        "backup_size_limit",
        message,
        "Use an intact CoffeePOS backup within the documented schema-1 safety bounds.",
    )
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn valid_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

fn valid_rfc3339_utc(value: &str) -> bool {
    if value.len() != 20 {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return false;
    }
    for index in [0usize, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return false;
        }
    }
    let month = parse_two(bytes[5], bytes[6]);
    let day = parse_two(bytes[8], bytes[9]);
    let hour = parse_two(bytes[11], bytes[12]);
    let minute = parse_two(bytes[14], bytes[15]);
    let second = parse_two(bytes[17], bytes[18]);
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour <= 23
        && minute <= 59
        && second <= 60
}

fn parse_two(a: u8, b: u8) -> u8 {
    (a - b'0') * 10 + (b - b'0')
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn rfc3339_utc(timestamp: u64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(timestamp);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn utc_parts(timestamp: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (
        year as i32,
        month as u32,
        day as u32,
        (seconds / 3_600) as u32,
        ((seconds % 3_600) / 60) as u32,
        (seconds % 60) as u32,
    )
}

pub fn load_development_compatibility_target(
    project_root: &Path,
    runtime_manifest: &Path,
    restore_root: &Path,
    action: &str,
) -> Result<BackupCompatibilityTarget, BackupErrorInfo> {
    let resolved_runtime = runtime::resolve_development_manifest(project_root, runtime_manifest)
        .map_err(|_| target_manifest_error(action))?;
    let wordpress = provisioning::resolve_development_wordpress(project_root, runtime_manifest)
        .map_err(|_| target_manifest_error(action))?;
    let woocommerce = provisioning::resolve_development_woocommerce(project_root, runtime_manifest)
        .map_err(|_| target_manifest_error(action))?;
    let coffeepos = provisioning::resolve_development_coffeepos(project_root, runtime_manifest)
        .map_err(|_| target_manifest_error(action))?;
    if coffeepos.required_wordpress_version != wordpress.version
        || coffeepos.required_php_version != resolved_runtime.php_version
        || coffeepos.required_mariadb_version != resolved_runtime.mariadb_version
        || coffeepos.required_woocommerce_version != woocommerce.version
    {
        return Err(target_manifest_error(action));
    }
    let source = BackupSourceVersions {
        desktop_version: env!("CARGO_PKG_VERSION").into(),
        runtime_version: resolved_runtime.runtime_version,
        target: current_target_name().into(),
        php_version: resolved_runtime.php_version,
        mariadb_version: resolved_runtime.mariadb_version,
        wordpress_version: wordpress.version,
        woocommerce_version: woocommerce.version,
        coffeepos_version: coffeepos.version,
        provisioning_schema: PROVISIONING_SCHEMA_VERSION,
        app_config_schema: APP_CONFIG_SCHEMA_VERSION,
    };
    let available_restore_bytes = available_space_for_restore_root(restore_root, action)?;
    Ok(BackupCompatibilityTarget {
        source,
        restore_schema: BACKUP_SCHEMA_VERSION,
        available_restore_bytes,
    })
}

fn available_space_for_restore_root(
    restore_root: &Path,
    action: &str,
) -> Result<u64, BackupErrorInfo> {
    let mut candidate = restore_root;
    while !candidate.exists() {
        candidate = candidate.parent().ok_or_else(|| {
            backup_error(
                action,
                "disk_capacity_unavailable",
                "CoffeePOS cannot resolve the restore volume capacity.",
                "Repair the CoffeePOS data path or select a valid local data volume before retrying.",
            )
        })?;
    }
    fs2::available_space(candidate).map_err(|_| {
        backup_error(
            action,
            "disk_capacity_unavailable",
            "CoffeePOS cannot read the currently available restore disk space.",
            "Check the CoffeePOS data volume and retry backup inspection.",
        )
    })
}

fn target_manifest_error(action: &str) -> BackupErrorInfo {
    backup_error(
        action,
        "target_compatibility_unavailable",
        "CoffeePOS cannot resolve the pinned target versions needed to validate this backup.",
        "Repair the managed runtime artifacts before inspecting or restoring backups.",
    )
}

#[cfg(windows)]
pub fn choose_backup_file(action: &str) -> Result<Option<PathBuf>, BackupErrorInfo> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let mut file_buffer = vec![0_u16; 32_768];
    let filter: Vec<u16> =
        "CoffeePOS backup (*.coffeepos-backup)\0*.coffeepos-backup\0All files (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file_buffer.as_mut_ptr();
    dialog.nMaxFile = file_buffer.len() as u32;
    dialog.Flags = OFN_NOCHANGEDIR | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
    let selected = unsafe { GetOpenFileNameW(&mut dialog) };
    if selected == 0 {
        let code = unsafe { CommDlgExtendedError() };
        if code == 0 {
            return Ok(None);
        }
        return Err(backup_error(
            action,
            "open_dialog_failed",
            "Windows could not open the CoffeePOS backup file picker.",
            "Retry or restart CoffeePOS Desktop.",
        ));
    }
    let len = file_buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(file_buffer.len());
    Ok(Some(PathBuf::from(String::from_utf16_lossy(
        &file_buffer[..len],
    ))))
}

#[cfg(not(windows))]
pub fn choose_backup_file(action: &str) -> Result<Option<PathBuf>, BackupErrorInfo> {
    Err(backup_error(
        action,
        "unsupported_platform",
        "Backup inspection is currently qualified for Windows.",
        "Run this Phase 7.1 flow on the supported Windows build.",
    ))
}

pub(crate) fn default_backup_file_name(store_name: &str) -> String {
    let mut slug = String::with_capacity(store_name.len().min(64));
    let mut previous_separator = false;
    for ch in store_name.trim().chars() {
        if slug.chars().count() >= 48 {
            break;
        }
        if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
            slug.push(ch);
            previous_separator = false;
        } else if !slug.is_empty() && !previous_separator {
            slug.push('-');
            previous_separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("Store");
    }
    let (year, month, day, hour, minute, second) = utc_parts(now_epoch());
    format!(
        "CoffeePOS-{slug}-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.coffeepos-backup"
    )
}

#[cfg(windows)]
pub(crate) fn choose_backup_destination(
    suggested_file_name: &str,
) -> Result<Option<BackupDestinationSelection>, BackupErrorInfo> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let suggested = if suggested_file_name
        .to_ascii_lowercase()
        .ends_with(".coffeepos-backup")
    {
        suggested_file_name.to_owned()
    } else {
        format!("{suggested_file_name}.coffeepos-backup")
    };
    let mut file_buffer = vec![0_u16; 32_768];
    let suggested_utf16 = suggested.encode_utf16().collect::<Vec<_>>();
    if suggested_utf16.len() >= file_buffer.len() {
        return Err(backup_error(
            "preflight",
            "invalid_destination",
            "The suggested backup file name is too long for the Windows file picker.",
            "Shorten the store name and retry backup creation.",
        ));
    }
    file_buffer[..suggested_utf16.len()].copy_from_slice(&suggested_utf16);
    let filter: Vec<u16> = "CoffeePOS backup (*.coffeepos-backup)\0*.coffeepos-backup\0\0"
        .encode_utf16()
        .collect();
    let default_extension: Vec<u16> = "coffeepos-backup\0".encode_utf16().collect();
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file_buffer.as_mut_ptr();
    dialog.nMaxFile = file_buffer.len() as u32;
    dialog.lpstrDefExt = default_extension.as_ptr();
    dialog.Flags = OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT;
    let selected = unsafe { GetSaveFileNameW(&mut dialog) };
    if selected == 0 {
        let code = unsafe { CommDlgExtendedError() };
        if code == 0 {
            return Ok(None);
        }
        return Err(backup_error(
            "preflight",
            "save_dialog_failed",
            "Windows could not open the CoffeePOS backup destination picker.",
            "Retry backup creation or restart CoffeePOS Desktop.",
        ));
    }
    let len = file_buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(file_buffer.len());
    let path = PathBuf::from(String::from_utf16_lossy(&file_buffer[..len]));
    let extension_is_valid = path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.to_ascii_lowercase().ends_with(".coffeepos-backup"));
    if !extension_is_valid {
        return Err(backup_error(
            "preflight",
            "invalid_destination_extension",
            "CoffeePOS backups must use the .coffeepos-backup extension.",
            "Choose a destination ending in .coffeepos-backup and retry.",
        ));
    }
    BackupDestinationSelection::capture(path, "preflight").map(Some)
}

#[cfg(not(windows))]
pub(crate) fn choose_backup_destination(
    _suggested_file_name: &str,
) -> Result<Option<BackupDestinationSelection>, BackupErrorInfo> {
    Err(backup_error(
        "preflight",
        "unsupported_platform",
        "Backup creation is currently qualified for Windows.",
        "Run the Phase 7 backup flow on the supported Windows build.",
    ))
}

#[cfg(windows)]
pub(crate) fn capture_destination_identity(
    path: &Path,
    action: &str,
) -> Result<Option<BackupDestinationIdentity>, BackupErrorInfo> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let file =
        match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(backup_error(
                action,
                "destination_identity_unavailable",
                "CoffeePOS cannot inspect the selected backup destination safely.",
                "Choose a regular local destination that CoffeePOS can inspect, then retry backup.",
            )),
        };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let result = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if result == 0 {
        return Err(backup_error(
            action,
            "destination_identity_unavailable",
            "CoffeePOS cannot read the selected backup destination identity.",
            "Choose another local destination and retry backup.",
        ));
    }
    Ok(Some(BackupDestinationIdentity {
        volume_serial_number: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    }))
}

#[cfg(not(windows))]
pub(crate) fn capture_destination_identity(
    _path: &Path,
    action: &str,
) -> Result<Option<BackupDestinationIdentity>, BackupErrorInfo> {
    Err(backup_error(
        action,
        "unsupported_platform",
        "Backup destination identity checks are currently qualified for Windows.",
        "Run the Phase 7 backup flow on the supported Windows build.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const PASSWORD: &str = "CoffeePOS Phase 7 Fixture Password";
    const ADMIN_CANARY: &str = "Phase71-Admin-Plaintext-Canary";
    const DPAPI_CANARY: &str = "Phase71-DPAPI-Blob-Canary";

    fn source_versions() -> BackupSourceVersions {
        BackupSourceVersions {
            desktop_version: "0.1.0".into(),
            runtime_version: "2026-09-19.windows-dev.2".into(),
            target: "x86_64-pc-windows-msvc".into(),
            php_version: "8.4.25".into(),
            mariadb_version: "11.4.13".into(),
            wordpress_version: "7.1".into(),
            woocommerce_version: "11.1.0".into(),
            coffeepos_version: "1.0.1".into(),
            provisioning_schema: 1,
            app_config_schema: 1,
        }
    }

    fn target() -> BackupCompatibilityTarget {
        BackupCompatibilityTarget {
            source: source_versions(),
            restore_schema: 1,
            available_restore_bytes: MAX_TOTAL_UNCOMPRESSED_BYTES,
        }
    }

    fn memory_entry(path: &str, bytes: Vec<u8>) -> BackupPayloadEntry {
        BackupPayloadEntry {
            path: path.into(),
            source: BackupEntrySource::Memory(Zeroizing::new(bytes)),
        }
    }

    fn valid_entries() -> Vec<BackupPayloadEntry> {
        vec![
            memory_entry(
                DATABASE_ENTRY,
                b"-- logical SQL fixture\nSELECT 1;\n".to_vec(),
            ),
            memory_entry("uploads/2026/09/receipt.txt", b"fixture-upload".to_vec()),
            memory_entry(
                STORE_CONFIG_ENTRY,
                serde_json::to_vec(&PortableStoreConfigV1 {
                    schema_version: 1,
                    store_name: "Coffee & Co Café".into(),
                    administrator: PortableAdministratorIdentity {
                        username: "owner".into(),
                        email: "owner@example.com".into(),
                    },
                })
                .unwrap(),
            ),
            memory_entry(
                ADMINISTRATOR_SECRET_ENTRY,
                serde_json::to_vec(&PortableAdministratorSecretV1 {
                    schema_version: 1,
                    username: "owner".into(),
                    password: Zeroizing::new(ADMIN_CANARY.into()),
                })
                .unwrap(),
            ),
        ]
    }

    fn write_fixture(source: BackupSourceVersions, entries: &[BackupPayloadEntry]) -> Vec<u8> {
        write_fixture_with_seed(BackupManifestSeed::fixture(source), entries)
    }

    fn write_fixture_with_seed(
        seed: BackupManifestSeed,
        entries: &[BackupPayloadEntry],
    ) -> Vec<u8> {
        let mut encrypted = Vec::new();
        write_encrypted_backup(&mut encrypted, PASSWORD, seed, entries).unwrap();
        encrypted
    }

    fn validate_bytes(bytes: &[u8], password: &str) -> Result<BackupValidation, BackupErrorInfo> {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), bytes).unwrap();
        validate_backup(temp.path(), password, &target())
    }

    fn decrypt_fixture(bytes: &[u8], password: &str) -> Vec<u8> {
        let decryptor = age::Decryptor::new(bytes).unwrap();
        let identity = age::scrypt::Identity::new(SecretString::from(password.to_owned()));
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .unwrap();
        let mut plain = Vec::new();
        reader.read_to_end(&mut plain).unwrap();
        plain
    }

    fn encrypt_plain_zip(bytes: &[u8], password: &str) -> Vec<u8> {
        let encryptor =
            age::Encryptor::with_user_passphrase(SecretString::from(password.to_owned()));
        let mut output = Vec::new();
        let mut writer = encryptor.wrap_output(&mut output).unwrap();
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap();
        output
    }

    #[test]
    fn backup_format_portable_projection_entries_are_minimal_and_validated() {
        let store =
            portable_store_config_entry("Coffee & Co Café", "owner_73", "owner73@example.com")
                .unwrap();
        assert_eq!(store.path, STORE_CONFIG_ENTRY);
        let BackupEntrySource::Memory(store_bytes) = store.source else {
            panic!("portable store config must stay in memory until encryption");
        };
        let store_json: serde_json::Value = serde_json::from_slice(&store_bytes).unwrap();
        assert_eq!(store_json["store_name"], "Coffee & Co Café");
        assert_eq!(store_json["administrator"]["username"], "owner_73");
        assert_eq!(store_json["administrator"]["email"], "owner73@example.com");
        assert!(store_json.get("bind_host").is_none());
        assert!(store_json.get("startup_view").is_none());

        let administrator = portable_administrator_secret_entry("owner_73", ADMIN_CANARY).unwrap();
        assert_eq!(administrator.path, ADMINISTRATOR_SECRET_ENTRY);
        let BackupEntrySource::Memory(secret_bytes) = administrator.source else {
            panic!("portable administrator secret must stay in zeroizing memory");
        };
        let secret_json: serde_json::Value = serde_json::from_slice(&secret_bytes).unwrap();
        assert_eq!(secret_json["username"], "owner_73");
        assert_eq!(secret_json["password"], ADMIN_CANARY);

        assert!(portable_store_config_entry("Store", "x", "owner@example.com").is_err());
        assert!(portable_store_config_entry("Store", "owner_73", "bad-email").is_err());
        assert!(portable_administrator_secret_entry("owner_73", "").is_err());
    }

    #[test]
    fn backup_format_structured_warning_roundtrips_without_paths() {
        let warning = BackupWarning::unmanaged_site_code_not_included(vec![
            "custom-theme".into(),
            "custom-plugin".into(),
            "custom-plugin".into(),
        ])
        .unwrap();
        assert_eq!(warning.items, vec!["custom-plugin", "custom-theme"]);
        assert!(BackupWarning::unmanaged_site_code_not_included(vec![
            "C:\\Users\\Alice\\plugin".into()
        ])
        .is_err());

        let mut seed = BackupManifestSeed::fixture(source_versions());
        seed.warnings = vec![warning.clone()];
        let encrypted = write_fixture_with_seed(seed, &valid_entries());
        let validation = validate_bytes(&encrypted, PASSWORD).unwrap();
        assert_eq!(
            validation.inspection.warnings,
            vec![WARNING_UNMANAGED_SITE_CODE_NOT_INCLUDED]
        );
        assert_eq!(validation.inspection.warning_metadata, vec![warning]);
    }

    #[test]
    fn backup_format_controlled_writer_reports_progress_and_cancels() {
        let entries = valid_entries();
        let mut encrypted = Vec::new();
        let cancelled = AtomicBool::new(false);
        let mut progress = Vec::new();
        write_encrypted_backup_with_control(
            &mut encrypted,
            PASSWORD,
            BackupManifestSeed::fixture(source_versions()),
            &entries,
            &cancelled,
            |update| progress.push(update),
        )
        .unwrap();
        let last = progress.last().expect("archive progress must be reported");
        assert_eq!(last.stage, BackupWriteStage::Archiving);
        assert_eq!(last.entries_completed, entries.len() as u64);
        assert_eq!(last.bytes_processed, last.total_bytes);
        assert!(validate_bytes(&encrypted, PASSWORD).unwrap().valid);

        let cancelled = AtomicBool::new(true);
        let error = write_encrypted_backup_with_control(
            Vec::new(),
            PASSWORD,
            BackupManifestSeed::fixture(source_versions()),
            &entries,
            &cancelled,
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error.code, "cancelled");
        assert_eq!(error.action, "archive");
    }

    #[test]
    fn backup_format_file_snapshot_detects_change_before_streaming() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("upload.bin");
        fs::write(&path, b"before").unwrap();
        let file = File::open(&path).unwrap();
        let expected = source_snapshot_from_metadata(&file.metadata().unwrap());
        drop(file);
        fs::write(&path, b"after-with-a-different-size").unwrap();

        let mut output = Vec::new();
        let error = copy_source_with_hash(
            &mut output,
            &BackupEntrySource::File(path),
            Some(&expected),
            None,
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error.code, "source_changed");
    }

    #[test]
    fn backup_format_default_file_name_is_safe_and_has_required_extension() {
        let name = default_backup_file_name(" Café / Main:Store? ");
        assert!(name.starts_with("CoffeePOS-Café-Main-Store-"));
        assert!(name.ends_with(".coffeepos-backup"));
        assert!(!name.contains('/'));
        assert!(!name.contains(':'));
        assert!(!name.contains('?'));
    }

    fn build_seek_zip(entries: Vec<(&str, Vec<u8>, Option<u32>)>) -> Vec<u8> {
        use zip::write::FileOptions;
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut output);
            for (path, bytes, mode) in entries {
                if let Some(mode) = mode {
                    if mode & 0o170000 == 0o120000 {
                        writer
                            .add_symlink(
                                path,
                                String::from_utf8(bytes).unwrap(),
                                FileOptions::default(),
                            )
                            .unwrap();
                        continue;
                    }
                }
                writer
                    .start_file(
                        path,
                        FileOptions::default()
                            .compression_method(zip::CompressionMethod::Stored)
                            .large_file(true)
                            .unix_permissions(mode.unwrap_or(0o600)),
                    )
                    .unwrap();
                writer.write_all(&bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn control_bytes_for(
        entries: &[(&str, Vec<u8>)],
        source: BackupSourceVersions,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut inventory_entries = Vec::new();
        let mut total = 0_u64;
        for (path, bytes) in entries {
            let mut sha = Sha256::new();
            sha.update(bytes);
            inventory_entries.push(InventoryEntry {
                path: (*path).into(),
                size_bytes: bytes.len() as u64,
                sha256: hex_lower(&sha.finalize()),
            });
            total += bytes.len() as u64;
        }
        let inventory = InventoryV1 {
            schema_version: 1,
            total_files: inventory_entries.len() as u64,
            total_uncompressed_bytes: total,
            entries: inventory_entries,
        };
        let manifest = BackupManifestV1 {
            schema_version: 1,
            backup_id: "123e4567-e89b-42d3-a456-426614174000".into(),
            created_at: "2026-09-19T06:30:00Z".into(),
            kind: BACKUP_KIND.into(),
            source,
            database: DatabaseDescriptor {
                format: "mariadb_logical_sql".into(),
                database_name: DATABASE_NAME.into(),
                entry: DATABASE_ENTRY.into(),
            },
            uploads: UploadsDescriptor {
                root: UPLOADS_ROOT.into(),
            },
            store_config: EntryDescriptor {
                entry: STORE_CONFIG_ENTRY.into(),
            },
            administrator_secret: AdministratorSecretDescriptor {
                entry: ADMINISTRATOR_SECRET_ENTRY.into(),
                present: true,
            },
            compatibility: CompatibilityDescriptor {
                minimum_restore_schema: 1,
                requires_explicit_migration: false,
            },
            inventory_entry: INVENTORY_ENTRY.into(),
            total_files: inventory.total_files,
            total_uncompressed_bytes: inventory.total_uncompressed_bytes,
            warnings: Vec::new(),
        };
        (
            serde_json::to_vec(&manifest).unwrap(),
            serde_json::to_vec(&inventory).unwrap(),
        )
    }

    fn raw_fixture_with_data(
        data_entries: Vec<(&str, Vec<u8>, Option<u32>)>,
        source: BackupSourceVersions,
    ) -> Vec<u8> {
        let inventory_source = data_entries
            .iter()
            .map(|(path, bytes, _)| (*path, bytes.clone()))
            .collect::<Vec<_>>();
        let (manifest, inventory) = control_bytes_for(&inventory_source, source);
        let mut all = vec![
            ("manifest.json", manifest, None),
            (INVENTORY_ENTRY, inventory, None),
        ];
        all.extend(data_entries);
        encrypt_plain_zip(&build_seek_zip(all), PASSWORD)
    }

    #[test]
    fn backup_format_valid_schema1_fixture_inspects_and_validates() {
        let entries = valid_entries();
        let encrypted = write_fixture(source_versions(), &entries);
        let validation = validate_bytes(&encrypted, PASSWORD).unwrap();
        assert!(validation.valid);
        assert!(validation.inspection.can_restore);
        assert_eq!(validation.entries_validated, 4);
        assert_eq!(validation.inspection.store_name, "Coffee & Co Café");
        assert_eq!(validation.inspection.database_bytes, 33);
        assert_eq!(validation.inspection.uploads_bytes, 14);

        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), &encrypted).unwrap();
        let inspection = inspect_backup(temp.path(), PASSWORD, &target()).unwrap();
        assert_eq!(inspection.backup_id, "123e4567-e89b-42d3-a456-426614174000");
        assert_eq!(
            inspection.compatibility.status,
            BackupCompatibilityStatus::Compatible
        );
    }

    #[test]
    fn restore_extraction_is_bound_to_the_inspected_ciphertext() {
        let encrypted = write_fixture(source_versions(), &valid_entries());
        let source = tempfile::NamedTempFile::new().unwrap();
        fs::write(source.path(), &encrypted).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let cancelled = AtomicBool::new(false);

        let stale = extract_restore_payload(
            source.path(),
            PASSWORD,
            &target(),
            &"0".repeat(64),
            staging.path(),
            &cancelled,
        )
        .unwrap_err();
        assert_eq!(stale.code, "stale_candidate");

        let staging = tempfile::tempdir().unwrap();
        let expected = hex_lower(&Sha256::digest(&encrypted));
        let payload = extract_restore_payload(
            source.path(),
            PASSWORD,
            &target(),
            &expected,
            staging.path(),
            &cancelled,
        )
        .unwrap();
        assert_eq!(payload.store_name, "Coffee & Co Café");
        assert_eq!(payload.administrator_username, "owner");
        assert_eq!(payload.administrator_email, "owner@example.com");
        assert_eq!(payload.administrator_password.as_str(), ADMIN_CANARY);
        assert!(payload.database_dump.is_file());
        assert_eq!(payload.upload_files, 1);
        assert_eq!(payload.upload_bytes, 14);
    }

    #[test]
    fn backup_format_wrong_password_and_encrypted_tamper_fail_closed() {
        let encrypted = write_fixture(source_versions(), &valid_entries());
        let wrong = validate_bytes(&encrypted, "wrong password").unwrap_err();
        assert_eq!(wrong.code, "authentication_failed");

        let mut tampered = encrypted.clone();
        let index = tampered.len().saturating_sub(24);
        tampered[index] ^= 0x40;
        let error = validate_bytes(&tampered, PASSWORD).unwrap_err();
        assert!(matches!(
            error.code.as_str(),
            "corrupt_encrypted_container" | "invalid_zip" | "checksum_mismatch"
        ));
    }

    #[test]
    fn backup_format_checksum_tamper_and_missing_database_are_rejected() {
        let entries = valid_entries();
        let encrypted = write_fixture(source_versions(), &entries);
        let plain = decrypt_fixture(&encrypted, PASSWORD);
        let mut archive = zip::ZipArchive::new(Cursor::new(&plain)).unwrap();
        let mut rebuilt = Vec::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            if file.name() == DATABASE_ENTRY {
                bytes[0] ^= 1;
            }
            rebuilt.push((file.name().to_string(), bytes));
        }
        let zipped = build_seek_zip(
            rebuilt
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.clone(), None))
                .collect(),
        );
        let error = validate_bytes(&encrypt_plain_zip(&zipped, PASSWORD), PASSWORD).unwrap_err();
        assert_eq!(error.code, "checksum_mismatch");

        let store = serde_json::to_vec(&PortableStoreConfigV1 {
            schema_version: 1,
            store_name: "Missing DB".into(),
            administrator: PortableAdministratorIdentity {
                username: "owner".into(),
                email: "owner@example.com".into(),
            },
        })
        .unwrap();
        let admin = serde_json::to_vec(&PortableAdministratorSecretV1 {
            schema_version: 1,
            username: "owner".into(),
            password: Zeroizing::new("secret".into()),
        })
        .unwrap();
        let missing = raw_fixture_with_data(
            vec![
                (STORE_CONFIG_ENTRY, store, None),
                (ADMINISTRATOR_SECRET_ENTRY, admin, None),
            ],
            source_versions(),
        );
        let error = validate_bytes(&missing, PASSWORD).unwrap_err();
        assert_eq!(error.code, "missing_required_entry");
    }

    #[test]
    fn backup_format_unsafe_duplicate_and_special_paths_are_rejected() {
        for unsafe_path in [
            "../escape.txt",
            "/absolute.txt",
            "C:/escape.txt",
            "uploads/NUL.txt",
        ] {
            let mut entries = valid_entries();
            entries.push(memory_entry(unsafe_path, b"x".to_vec()));
            let error = write_encrypted_backup(
                Vec::new(),
                PASSWORD,
                BackupManifestSeed::fixture(source_versions()),
                &entries,
            )
            .unwrap_err();
            assert_eq!(error.code, "unsafe_archive_path");
        }

        let mut entries = valid_entries();
        entries.push(memory_entry("uploads/A.txt", b"a".to_vec()));
        entries.push(memory_entry("uploads/a.txt", b"b".to_vec()));
        let duplicate = write_encrypted_backup(
            Vec::new(),
            PASSWORD,
            BackupManifestSeed::fixture(source_versions()),
            &entries,
        )
        .unwrap_err();
        assert_eq!(duplicate.code, "duplicate_archive_path");

        let mut data = Vec::new();
        for entry in valid_entries() {
            let BackupEntrySource::Memory(bytes) = entry.source else {
                unreachable!()
            };
            data.push((entry.path, bytes.to_vec(), None));
        }
        data.push((
            "uploads/link".into(),
            b"target.txt".to_vec(),
            Some(0o120777),
        ));
        let refs = data
            .iter()
            .map(|(path, bytes, mode)| (path.as_str(), bytes.clone(), *mode))
            .collect();
        let special = raw_fixture_with_data(refs, source_versions());
        let error = validate_bytes(&special, PASSWORD).unwrap_err();
        assert_eq!(error.code, "special_archive_entry");
    }

    #[test]
    fn backup_format_source_ancestor_link_is_rejected_when_platform_can_create_one() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let managed = temp.path().join("managed");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&managed).unwrap();
        fs::write(outside.join("photo.jpg"), b"linked-upload").unwrap();
        let linked = managed.join("linked");

        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, &linked).is_err() {
            return;
        }
        #[cfg(unix)]
        if std::os::unix::fs::symlink(&outside, &linked).is_err() {
            return;
        }
        #[cfg(not(any(windows, unix)))]
        return;

        let mut entries = valid_entries();
        entries.push(BackupPayloadEntry {
            path: "uploads/linked/photo.jpg".into(),
            source: BackupEntrySource::File(linked.join("photo.jpg")),
        });
        let error = write_encrypted_backup(
            Vec::new(),
            PASSWORD,
            BackupManifestSeed::fixture(source_versions()),
            &entries,
        )
        .unwrap_err();
        assert_eq!(error.code, "unsafe_source_file");
    }

    #[test]
    fn backup_format_incompatible_version_is_valid_but_blocked() {
        let mut source = source_versions();
        source.coffeepos_version = "9.0.0".into();
        let encrypted = write_fixture(source, &valid_entries());
        let validation = validate_bytes(&encrypted, PASSWORD).unwrap();
        assert!(validation.valid);
        assert!(!validation.inspection.can_restore);
        assert_eq!(
            validation.inspection.compatibility.code.as_deref(),
            Some("incompatible_component_version")
        );
    }

    #[test]
    fn backup_format_unqualified_cross_target_restore_is_blocked() {
        let mut source = source_versions();
        source.target = "aarch64-apple-darwin".into();
        let encrypted = write_fixture(source, &valid_entries());
        let validation = validate_bytes(&encrypted, PASSWORD).unwrap();
        assert!(validation.valid);
        assert!(!validation.inspection.can_restore);
        assert_eq!(
            validation.inspection.compatibility.code.as_deref(),
            Some("unqualified_source_target")
        );
    }

    #[test]
    fn backup_format_secret_canaries_respect_encrypted_boundary() {
        let encrypted = write_fixture(source_versions(), &valid_entries());
        assert!(!encrypted
            .windows(ADMIN_CANARY.len())
            .any(|window| window == ADMIN_CANARY.as_bytes()));
        assert!(!encrypted
            .windows(DPAPI_CANARY.len())
            .any(|window| window == DPAPI_CANARY.as_bytes()));
        let plain = decrypt_fixture(&encrypted, PASSWORD);
        assert!(plain
            .windows(ADMIN_CANARY.len())
            .any(|window| window == ADMIN_CANARY.as_bytes()));
        assert!(!plain
            .windows(DPAPI_CANARY.len())
            .any(|window| window == DPAPI_CANARY.as_bytes()));

        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), &encrypted).unwrap();
        let inspection = inspect_backup(temp.path(), PASSWORD, &target()).unwrap();
        let ipc = serde_json::to_vec(&inspection).unwrap();
        assert!(!ipc
            .windows(ADMIN_CANARY.len())
            .any(|window| window == ADMIN_CANARY.as_bytes()));
        assert!(!ipc
            .windows(PASSWORD.len())
            .any(|window| window == PASSWORD.as_bytes()));
    }

    #[test]
    fn backup_format_schema_and_inventory_bounds_fail_closed() {
        let data_entries = valid_entries()
            .into_iter()
            .map(|entry| {
                let BackupEntrySource::Memory(bytes) = entry.source else {
                    unreachable!()
                };
                (entry.path, bytes.to_vec())
            })
            .collect::<Vec<_>>();
        let refs = data_entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.clone()))
            .collect::<Vec<_>>();
        let (manifest_bytes, inventory_bytes) = control_bytes_for(&refs, source_versions());
        let mut manifest_value: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).unwrap();
        manifest_value["schema_version"] = serde_json::json!(2);
        let mut all = vec![
            (
                "manifest.json",
                serde_json::to_vec(&manifest_value).unwrap(),
                None,
            ),
            (INVENTORY_ENTRY, inventory_bytes.clone(), None),
        ];
        all.extend(
            data_entries
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.clone(), None)),
        );
        let unsupported = encrypt_plain_zip(&build_seek_zip(all), PASSWORD);
        let error = validate_bytes(&unsupported, PASSWORD).unwrap_err();
        assert_eq!(error.code, "unsupported_backup_schema");

        let mut inventory_value: serde_json::Value =
            serde_json::from_slice(&inventory_bytes).unwrap();
        inventory_value["entries"][0]["size_bytes"] =
            serde_json::json!(MAX_ENTRY_UNCOMPRESSED_BYTES + 1);
        inventory_value["total_uncompressed_bytes"] =
            serde_json::json!(MAX_ENTRY_UNCOMPRESSED_BYTES + 1);
        let mut all = vec![
            ("manifest.json", manifest_bytes, None),
            (
                INVENTORY_ENTRY,
                serde_json::to_vec(&inventory_value).unwrap(),
                None,
            ),
        ];
        all.extend(
            data_entries
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.clone(), None)),
        );
        let oversized = encrypt_plain_zip(&build_seek_zip(all), PASSWORD);
        let error = validate_bytes(&oversized, PASSWORD).unwrap_err();
        assert_eq!(error.code, "backup_size_limit");
    }

    #[test]
    fn backup_format_rejects_warning_text_that_could_expose_source_paths() {
        let data_entries = valid_entries()
            .into_iter()
            .map(|entry| {
                let BackupEntrySource::Memory(bytes) = entry.source else {
                    unreachable!()
                };
                (entry.path, bytes.to_vec())
            })
            .collect::<Vec<_>>();
        let refs = data_entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.clone()))
            .collect::<Vec<_>>();
        let (manifest_bytes, inventory_bytes) = control_bytes_for(&refs, source_versions());
        let mut manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        manifest["warnings"] = serde_json::json!(["C:\\Users\\Alice\\CoffeePOS"]);
        let mut all = vec![
            (
                "manifest.json",
                serde_json::to_vec(&manifest).unwrap(),
                None,
            ),
            (INVENTORY_ENTRY, inventory_bytes, None),
        ];
        all.extend(
            data_entries
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.clone(), None)),
        );
        let encrypted = encrypt_plain_zip(&build_seek_zip(all), PASSWORD);
        let error = validate_bytes(&encrypted, PASSWORD).unwrap_err();
        assert_eq!(error.code, "invalid_manifest");
    }

    #[test]
    fn backup_format_rejects_noncanonical_database_name() {
        let data_entries = valid_entries()
            .into_iter()
            .map(|entry| {
                let BackupEntrySource::Memory(bytes) = entry.source else {
                    unreachable!()
                };
                (entry.path, bytes.to_vec())
            })
            .collect::<Vec<_>>();
        let refs = data_entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.clone()))
            .collect::<Vec<_>>();
        let (manifest_bytes, inventory_bytes) = control_bytes_for(&refs, source_versions());
        let mut manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        manifest["database"]["database_name"] = serde_json::json!("otherdb");
        let mut all = vec![
            (
                "manifest.json",
                serde_json::to_vec(&manifest).unwrap(),
                None,
            ),
            (INVENTORY_ENTRY, inventory_bytes, None),
        ];
        all.extend(
            data_entries
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.clone(), None)),
        );
        let encrypted = encrypt_plain_zip(&build_seek_zip(all), PASSWORD);
        let error = validate_bytes(&encrypted, PASSWORD).unwrap_err();
        assert_eq!(error.code, "invalid_manifest");
    }

    #[test]
    fn backup_format_writer_and_reader_share_control_metadata_size_policy() {
        let inventory = InventoryV1 {
            schema_version: INVENTORY_SCHEMA_VERSION,
            total_files: 0,
            total_uncompressed_bytes: 0,
            entries: Vec::new(),
        };
        let expected = serde_json::to_vec_pretty(&inventory).unwrap();
        let exact_bound = expected.len() as u64;
        let written = serialize_json_bounded(
            &inventory,
            exact_bound,
            "inventory_serialize_failed",
            "backup inventory",
        )
        .unwrap();
        assert_eq!(written, expected);

        let mut readable = Cursor::new(written.clone());
        let read = read_control_json(&mut readable, exact_bound, "validate", "inventory").unwrap();
        assert_eq!(read.as_slice(), written.as_slice());

        let too_small = exact_bound.saturating_sub(1);
        let write_error = serialize_json_bounded(
            &inventory,
            too_small,
            "inventory_serialize_failed",
            "backup inventory",
        )
        .unwrap_err();
        assert_eq!(write_error.code, "backup_size_limit");

        let mut unreadable = Cursor::new(written);
        let read_error =
            read_control_json(&mut unreadable, too_small, "validate", "inventory").unwrap_err();
        assert_eq!(read_error.code, "backup_size_limit");
    }

    #[test]
    fn backup_format_rejects_inconsistent_zip64_locator_offset() {
        let encrypted = write_fixture(source_versions(), &valid_entries());
        let mut plain = decrypt_fixture(&encrypted, PASSWORD);
        let signature = ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR.to_le_bytes();
        let locator = plain
            .windows(signature.len())
            .rposition(|window| window == signature)
            .expect("fixture must contain ZIP64 locator");
        let offset_start = locator + 8;
        let offset_end = offset_start + 8;
        let mut raw_offset = [0_u8; 8];
        raw_offset.copy_from_slice(&plain[offset_start..offset_end]);
        let original = u64::from_le_bytes(raw_offset);
        plain[offset_start..offset_end]
            .copy_from_slice(&original.checked_add(1).unwrap().to_le_bytes());

        let tampered = encrypt_plain_zip(&plain, PASSWORD);
        let error = validate_bytes(&tampered, PASSWORD).unwrap_err();
        assert_eq!(error.code, "invalid_zip");
    }

    #[test]
    fn backup_format_rejects_backup_larger_than_available_restore_space() {
        let encrypted = write_fixture(source_versions(), &valid_entries());
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), &encrypted).unwrap();
        let mut constrained = target();
        constrained.available_restore_bytes = 1;
        let error = validate_backup(temp.path(), PASSWORD, &constrained).unwrap_err();
        assert_eq!(error.code, "insufficient_disk_space");
    }

    #[test]
    fn backup_format_target_resolution_rejects_manifests_without_staged_artifacts() {
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let source_root = project_root
            .join("runtime/development")
            .join(current_target_name());
        let temp = tempfile::tempdir().unwrap();
        let target_root = temp
            .path()
            .join("runtime/development")
            .join(current_target_name());
        fs::create_dir_all(&target_root).unwrap();
        for name in [
            "manifest.json",
            "wordpress-manifest.json",
            "woocommerce-manifest.json",
            "coffeepos-manifest.json",
        ] {
            fs::copy(source_root.join(name), target_root.join(name)).unwrap();
        }
        let error = load_development_compatibility_target(
            temp.path(),
            &target_root.join("manifest.json"),
            temp.path(),
            "validate",
        )
        .unwrap_err();
        assert_eq!(error.code, "target_compatibility_unavailable");
    }
}
