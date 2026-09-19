use crate::runtime::{
    CoffeePosHealthState, RuntimeErrorInfo, RuntimeInfo, RuntimeState, WordPressHealthState,
};
use crate::secret;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

const VIEW_DEFAULT_LINES: usize = 200;
const VIEW_MAX_LINES: usize = 200;
const VIEW_MAX_TEXT_BYTES: usize = 64 * 1024;
const VIEW_MAX_SCAN_BYTES: usize = 256 * 1024;
const VIEW_MAX_LINE_BYTES: usize = 16 * 1024;
const BUNDLE_MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;
const BUNDLE_MAX_UNCOMPRESSED_BYTES: usize = 20 * 1024 * 1024;
const OMITTED_MARKER: &str = "[older log content omitted by support bundle size limit]\n";
const REDACTED: &str = "[redacted]";

const SECRET_PATHS: [&str; 8] = [
    "config/database-bootstrap.secret",
    "config/database-runtime.secret",
    "config/database-wordpress.secret",
    "config/machine-token.secret",
    "config/machine-token.pending.secret",
    "config/wordpress-admin.secret",
    "config/wordpress-admin.pending.secret",
    "config/wordpress-admin.repair.pending.secret",
];

#[derive(Clone, Copy)]
struct LogDefinition {
    id: &'static str,
    label: &'static str,
    file_name: &'static str,
}

const LOG_DEFINITIONS: [LogDefinition; 10] = [
    LogDefinition {
        id: "application",
        label: "Ứng dụng",
        file_name: "application.log",
    },
    LogDefinition {
        id: "runtime",
        label: "Runtime",
        file_name: "runtime.log",
    },
    LogDefinition {
        id: "web_server",
        label: "Web server",
        file_name: "web-server.log",
    },
    LogDefinition {
        id: "php",
        label: "PHP",
        file_name: "php.log",
    },
    LogDefinition {
        id: "database",
        label: "Database",
        file_name: "database.log",
    },
    LogDefinition {
        id: "cron",
        label: "Tác vụ nền",
        file_name: "cron.log",
    },
    LogDefinition {
        id: "provisioning",
        label: "Thiết lập",
        file_name: "provisioning.log",
    },
    LogDefinition {
        id: "wordpress",
        label: "WordPress",
        file_name: "wordpress.log",
    },
    LogDefinition {
        id: "woocommerce",
        label: "WooCommerce",
        file_name: "woocommerce.log",
    },
    LogDefinition {
        id: "coffeepos",
        label: "CoffeePOS",
        file_name: "coffeepos.log",
    },
];

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LogErrorInfo {
    pub component: String,
    pub action: String,
    pub code: String,
    pub message: String,
    pub recovery: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LogCatalog {
    pub generated_at: u64,
    pub logs: Vec<LogCatalogItem>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LogCatalogItem {
    pub id: String,
    pub label: String,
    pub exists: bool,
    pub size_bytes: u64,
    pub modified_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LogPage {
    pub log_id: String,
    pub lines: Vec<String>,
    pub older_cursor: Option<String>,
    pub has_older: bool,
    pub truncated: bool,
    pub redaction_count: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupportBundleStatus {
    Exported,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SupportBundleResult {
    pub status: SupportBundleStatus,
    pub destination: Option<String>,
    pub files_included: usize,
    pub redaction_count: u64,
}

impl SupportBundleResult {
    pub fn cancelled() -> Self {
        Self {
            status: SupportBundleStatus::Cancelled,
            destination: None,
            files_included: 0,
            redaction_count: 0,
        }
    }
}

#[derive(Clone, Debug)]
struct Redactor {
    exact_secrets: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct RedactionResult {
    text: String,
    replacements: u64,
}

#[derive(Debug)]
struct ZipEntry {
    name: &'static str,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct PageLine {
    absolute_start: u64,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Cursor {
    size: u64,
    modified: u64,
    end: u64,
}

fn log_error(
    action: &str,
    code: &str,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> LogErrorInfo {
    LogErrorInfo {
        component: "logs".into(),
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

fn modified_stamp(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn definition(log_id: &str) -> Result<LogDefinition, LogErrorInfo> {
    LOG_DEFINITIONS
        .iter()
        .copied()
        .find(|item| item.id == log_id)
        .ok_or_else(|| {
            log_error(
                "read",
                "unknown_log_id",
                "CoffeePOS does not recognize this log source.",
                "Refresh the log catalog and choose one of the listed sources.",
            )
        })
}

fn canonical_source(
    data_root: &Path,
    definition: LogDefinition,
    action: &str,
) -> Result<Option<PathBuf>, LogErrorInfo> {
    let logs = data_root.join("logs");
    let candidate = logs.join(definition.file_name);
    let _source_meta = match fs::symlink_metadata(&candidate) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(log_error(
                action,
                "source_metadata_unavailable",
                "CoffeePOS cannot inspect the selected log file.",
                "Check the CoffeePOS data-folder permissions and retry.",
            ));
        }
    };
    let canonical_logs = fs::canonicalize(&logs).map_err(|_| {
        log_error(
            action,
            "log_directory_unavailable",
            "CoffeePOS cannot resolve the managed log directory.",
            "Check the CoffeePOS data-folder permissions and retry.",
        )
    })?;
    let canonical_candidate = fs::canonicalize(&candidate).map_err(|_| {
        log_error(
            action,
            "source_unavailable",
            "CoffeePOS cannot resolve the selected log file.",
            "Refresh the log catalog and retry.",
        )
    })?;
    if !canonical_candidate.starts_with(&canonical_logs) {
        return Err(log_error(
            action,
            "source_outside_allowlist",
            "The selected log source resolves outside the managed log directory.",
            "Restore the managed log path before reading or exporting logs.",
        ));
    }
    let metadata = fs::metadata(&canonical_candidate).map_err(|_| {
        log_error(
            action,
            "source_unavailable",
            "CoffeePOS cannot inspect the selected log file.",
            "Refresh the log catalog and retry.",
        )
    })?;
    if !metadata.is_file() {
        return Err(log_error(
            action,
            "source_not_file",
            "The selected managed log source is not a regular file.",
            "Restore the managed log file before retrying.",
        ));
    }
    Ok(Some(canonical_candidate))
}

fn load_redactor(data_root: &Path, action: &str) -> Result<Redactor, LogErrorInfo> {
    let mut exact_secrets = Vec::new();
    for relative in SECRET_PATHS {
        let path = data_root.join(relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(log_error(
                    action,
                    "protected_secret_unreadable",
                    "A protected credential exists but CoffeePOS cannot inspect it safely.",
                    "Restore access to the protected CoffeePOS credential, then retry. No log text was returned.",
                ));
            }
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(log_error(
                action,
                "protected_secret_unreadable",
                "A protected credential exists but is not a readable managed credential file.",
                "Restore the protected CoffeePOS credential, then retry. No log text was returned.",
            ));
        }
        let value = secret::load(&path).map_err(|_| {
            log_error(
                action,
                "protected_secret_unreadable",
                "A protected credential exists but CoffeePOS cannot decrypt it for safe log redaction.",
                "Restore the matching protected CoffeePOS credential for this Windows user, then retry. No log text was returned.",
            )
        })?;
        if value.is_empty() {
            return Err(log_error(
                action,
                "protected_secret_unreadable",
                "A protected credential exists but contains no usable value for safe log redaction.",
                "Restore the matching protected CoffeePOS credential, then retry. No log text was returned.",
            ));
        }
        if !exact_secrets.iter().any(|known| known == &value) {
            exact_secrets.push(value);
        }
    }
    exact_secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    Ok(Redactor { exact_secrets })
}

impl Redactor {
    fn redact_exact(&self, input: &str) -> RedactionResult {
        let mut output = input.to_string();
        let mut replacements = 0_u64;
        for secret in &self.exact_secrets {
            let matches = output.matches(secret).count();
            if matches > 0 {
                replacements = replacements.saturating_add(matches as u64);
                output = output.replace(secret, REDACTED);
            }
        }
        RedactionResult {
            text: output,
            replacements,
        }
    }

    fn redact(&self, input: &str) -> RedactionResult {
        let exact = self.redact_exact(input);
        let mut output = exact.text;
        let mut replacements = exact.replacements;

        let mut lines = Vec::new();
        for line in output.lines() {
            if sensitive_line(line) {
                lines.push(REDACTED.to_string());
                replacements = replacements.saturating_add(1);
            } else if line.len() > VIEW_MAX_LINE_BYTES {
                lines.push("[log line omitted: exceeds viewer line limit]".into());
            } else {
                lines.push(line.to_string());
            }
        }
        let trailing_newline = output.ends_with('\n');
        output = lines.join("\n");
        if trailing_newline && !output.is_empty() {
            output.push('\n');
        }
        RedactionResult {
            text: output,
            replacements,
        }
    }

    fn verify_absent(&self, bytes: &[u8]) -> bool {
        self.exact_secrets.iter().all(|secret| {
            let needle = secret.as_bytes();
            needle.is_empty() || !bytes.windows(needle.len()).any(|window| window == needle)
        })
    }
}

fn sensitive_line(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let keyword = [
        "password",
        "passwd",
        "pwd=",
        "pwd:",
        "token",
        "secret",
        "api_key",
        "api-key",
        "apikey",
        "authorization",
        "bearer ",
        "basic ",
        "cookie",
        "set-cookie",
        "wordpress_logged_in_",
        "wordpress_sec_",
    ]
    .iter()
    .any(|needle| lowered.contains(needle));
    keyword || contains_embedded_url_credentials(line)
}

fn contains_embedded_url_credentials(line: &str) -> bool {
    let Some(scheme) = line.find("://") else {
        return false;
    };
    let authority = &line[(scheme + 3)..];
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    let Some(at) = authority.rfind('@') else {
        return false;
    };
    authority[..at].contains(':')
}

pub fn get_log_catalog(data_root: &Path) -> Result<LogCatalog, LogErrorInfo> {
    let mut logs = Vec::with_capacity(LOG_DEFINITIONS.len());
    for item in LOG_DEFINITIONS {
        match canonical_source(data_root, item, "catalog")? {
            Some(path) => {
                let metadata = fs::metadata(path).map_err(|_| {
                    log_error(
                        "catalog",
                        "source_metadata_unavailable",
                        "CoffeePOS cannot inspect a managed log file.",
                        "Check the CoffeePOS data-folder permissions and retry.",
                    )
                })?;
                logs.push(LogCatalogItem {
                    id: item.id.into(),
                    label: item.label.into(),
                    exists: true,
                    size_bytes: metadata.len(),
                    modified_at: Some(modified_stamp(&metadata)),
                });
            }
            None => logs.push(LogCatalogItem {
                id: item.id.into(),
                label: item.label.into(),
                exists: false,
                size_bytes: 0,
                modified_at: None,
            }),
        }
    }
    Ok(LogCatalog {
        generated_at: now_epoch(),
        logs,
    })
}

pub fn read_log_page(
    data_root: &Path,
    log_id: &str,
    cursor: Option<&str>,
    direction: Option<&str>,
    max_lines: Option<usize>,
) -> Result<LogPage, LogErrorInfo> {
    let definition = definition(log_id)?;
    let redactor = load_redactor(data_root, "read")?;
    let Some(path) = canonical_source(data_root, definition, "read")? else {
        return Ok(LogPage {
            log_id: log_id.into(),
            lines: Vec::new(),
            older_cursor: None,
            has_older: false,
            truncated: false,
            redaction_count: 0,
        });
    };
    let metadata = fs::metadata(&path).map_err(|_| {
        log_error(
            "read",
            "source_unavailable",
            "CoffeePOS cannot read the selected log file.",
            "Refresh the log catalog and retry.",
        )
    })?;
    let size = metadata.len();
    let modified = modified_stamp(&metadata);
    let direction = direction.unwrap_or(if cursor.is_some() { "older" } else { "tail" });
    if !matches!(direction, "tail" | "older") {
        return Err(log_error(
            "read",
            "invalid_direction",
            "The requested log paging direction is invalid.",
            "Refresh the log source and retry.",
        ));
    }
    if direction == "tail" && cursor.is_some() {
        return Err(log_error(
            "read",
            "invalid_cursor",
            "A tail refresh cannot reuse an older-page cursor.",
            "Refresh the selected log from its newest lines.",
        ));
    }
    let requested_end = if let Some(token) = cursor {
        if direction != "older" {
            return Err(log_error(
                "read",
                "invalid_cursor",
                "The log cursor does not match this paging request.",
                "Refresh the selected log and retry.",
            ));
        }
        let parsed = decode_cursor(token, log_id)?;
        if parsed.size != size || parsed.modified != modified || parsed.end > size {
            return Err(log_error(
                "read",
                "stale_cursor",
                "The log changed after this page cursor was created.",
                "Refresh the selected log from its newest lines, then load older lines again.",
            ));
        }
        if !cursor_is_line_boundary(&path, parsed.end, size)? {
            return Err(invalid_cursor_error());
        }
        parsed.end
    } else {
        size
    };
    let end = if requested_end == size {
        complete_snapshot_end(&path, requested_end)?
    } else {
        requested_end
    };
    let requested_lines = max_lines
        .unwrap_or(VIEW_DEFAULT_LINES)
        .clamp(1, VIEW_MAX_LINES);
    let (mut page_lines, scan_truncated) = read_page_lines(&path, end, requested_lines)?;
    let mut rendered = Vec::with_capacity(page_lines.len());
    let mut replacement_count = 0_u64;
    for line in &page_lines {
        let text = String::from_utf8_lossy(&line.bytes);
        let result = redactor.redact(&text);
        replacement_count = replacement_count.saturating_add(result.replacements);
        rendered.push(result.text.trim_end_matches('\n').to_string());
    }
    let mut output_bytes = rendered
        .iter()
        .map(|line| line.len().saturating_add(1))
        .sum::<usize>();
    let mut byte_truncated = false;
    while output_bytes > VIEW_MAX_TEXT_BYTES && rendered.len() > 1 {
        output_bytes = output_bytes.saturating_sub(rendered[0].len().saturating_add(1));
        rendered.remove(0);
        page_lines.remove(0);
        byte_truncated = true;
    }
    if let Some(line) = rendered.first_mut() {
        if line.len() > VIEW_MAX_TEXT_BYTES {
            *line = "[log line omitted: exceeds viewer page limit]".into();
            byte_truncated = true;
        }
    }
    let page_start = page_lines
        .first()
        .map(|line| line.absolute_start)
        .unwrap_or(end);
    let has_older = page_start > 0;
    let older_cursor = has_older.then(|| encode_cursor(log_id, size, modified, page_start));
    Ok(LogPage {
        log_id: log_id.into(),
        lines: rendered,
        older_cursor,
        has_older,
        truncated: scan_truncated || byte_truncated,
        redaction_count: replacement_count,
    })
}

fn complete_snapshot_end(path: &Path, end: u64) -> Result<u64, LogErrorInfo> {
    if end == 0 {
        return Ok(0);
    }
    let mut file = File::open(path).map_err(|_| {
        log_error(
            "read",
            "source_unavailable",
            "CoffeePOS cannot open the selected log file.",
            "Refresh the log source and retry.",
        )
    })?;
    file.seek(SeekFrom::Start(end - 1)).map_err(|_| {
        log_error(
            "read",
            "source_changed",
            "The selected log changed while CoffeePOS was reading it.",
            "Refresh the log source and retry.",
        )
    })?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last).map_err(|_| {
        log_error(
            "read",
            "source_changed",
            "The selected log changed while CoffeePOS was reading it.",
            "Refresh the log source and retry.",
        )
    })?;
    if last[0] == b'\n' {
        return Ok(end);
    }

    let scan_start = end.saturating_sub(VIEW_MAX_SCAN_BYTES as u64);
    let scan_len = (end - scan_start) as usize;
    file.seek(SeekFrom::Start(scan_start)).map_err(|_| {
        log_error(
            "read",
            "source_changed",
            "The selected log changed while CoffeePOS was reading it.",
            "Refresh the log source and retry.",
        )
    })?;
    let mut bytes = vec![0_u8; scan_len];
    file.read_exact(&mut bytes).map_err(|_| {
        log_error(
            "read",
            "source_changed",
            "The selected log changed while CoffeePOS was reading it.",
            "Refresh the log source and retry.",
        )
    })?;
    Ok(bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| scan_start + index as u64 + 1)
        .unwrap_or(0))
}

fn cursor_is_line_boundary(path: &Path, end: u64, size: u64) -> Result<bool, LogErrorInfo> {
    if end == 0 || end == size {
        return Ok(true);
    }
    let mut file = File::open(path).map_err(|_| {
        log_error(
            "read",
            "source_unavailable",
            "CoffeePOS cannot open the selected log file.",
            "Refresh the log source and retry.",
        )
    })?;
    file.seek(SeekFrom::Start(end - 1)).map_err(|_| {
        log_error(
            "read",
            "source_changed",
            "The selected log changed while CoffeePOS was validating its page cursor.",
            "Refresh the log source and retry.",
        )
    })?;
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).map_err(|_| {
        log_error(
            "read",
            "source_changed",
            "The selected log changed while CoffeePOS was validating its page cursor.",
            "Refresh the log source and retry.",
        )
    })?;
    Ok(byte[0] == b'\n')
}

fn read_page_lines(
    path: &Path,
    end: u64,
    max_lines: usize,
) -> Result<(Vec<PageLine>, bool), LogErrorInfo> {
    if end == 0 {
        return Ok((Vec::new(), false));
    }
    let mut file = File::open(path).map_err(|_| {
        log_error(
            "read",
            "source_unavailable",
            "CoffeePOS cannot open the selected log file.",
            "Check log permissions and retry.",
        )
    })?;
    let mut base = end;
    let mut buffer = Vec::new();
    let mut enough_lines = false;
    while base > 0 && buffer.len() < VIEW_MAX_SCAN_BYTES && !enough_lines {
        let remaining = VIEW_MAX_SCAN_BYTES - buffer.len();
        let chunk = remaining.min(8192).min(base as usize);
        base -= chunk as u64;
        file.seek(SeekFrom::Start(base)).map_err(|_| {
            log_error(
                "read",
                "source_unavailable",
                "CoffeePOS cannot seek within the selected log file.",
                "Refresh the log source and retry.",
            )
        })?;
        let mut part = vec![0_u8; chunk];
        file.read_exact(&mut part).map_err(|_| {
            log_error(
                "read",
                "source_changed",
                "The selected log changed while CoffeePOS was reading it.",
                "Refresh the log source and retry.",
            )
        })?;
        part.extend_from_slice(&buffer);
        buffer = part;
        enough_lines = buffer.iter().filter(|byte| **byte == b'\n').count() > max_lines;
    }
    let scan_truncated = base > 0 && !enough_lines;
    let mut usable_start = 0usize;
    if base > 0 {
        if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
            usable_start = index + 1;
        } else {
            return Ok((
                vec![PageLine {
                    absolute_start: base,
                    bytes: b"[log line omitted: exceeds viewer scan limit]".to_vec(),
                }],
                true,
            ));
        }
    }
    let mut ranges = Vec::new();
    let mut line_start = usable_start;
    for (index, byte) in buffer.iter().enumerate().skip(usable_start) {
        if *byte == b'\n' {
            let mut line_end = index;
            if line_end > line_start && buffer[line_end - 1] == b'\r' {
                line_end -= 1;
            }
            ranges.push((line_start, line_end));
            line_start = index + 1;
        }
    }
    if line_start < buffer.len() {
        ranges.push((line_start, buffer.len()));
    }
    let keep_from = ranges.len().saturating_sub(max_lines);
    let mut lines = Vec::with_capacity(ranges.len() - keep_from);
    for (start, end_index) in ranges.into_iter().skip(keep_from) {
        let bytes = if end_index.saturating_sub(start) > VIEW_MAX_LINE_BYTES {
            b"[log line omitted: exceeds viewer line limit]".to_vec()
        } else {
            buffer[start..end_index].to_vec()
        };
        lines.push(PageLine {
            absolute_start: base + start as u64,
            bytes,
        });
    }
    Ok((lines, scan_truncated))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn encode_cursor(log_id: &str, size: u64, modified: u64, end: u64) -> String {
    let raw = format!("1|{log_id}|{size}|{modified}|{end}");
    let mut encoded = String::with_capacity(raw.len() * 2 + 17);
    for byte in raw.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    use std::fmt::Write as _;
    let _ = write!(&mut encoded, ".{:016x}", fnv1a(raw.as_bytes()));
    encoded
}

fn decode_cursor(token: &str, expected_log_id: &str) -> Result<Cursor, LogErrorInfo> {
    let Some((hex, checksum)) = token.rsplit_once('.') else {
        return Err(invalid_cursor_error());
    };
    if hex.is_empty() || hex.len() % 2 != 0 || checksum.len() != 16 {
        return Err(invalid_cursor_error());
    }
    let mut raw = Vec::with_capacity(hex.len() / 2);
    for index in (0..hex.len()).step_by(2) {
        let byte =
            u8::from_str_radix(&hex[index..index + 2], 16).map_err(|_| invalid_cursor_error())?;
        raw.push(byte);
    }
    let expected_checksum =
        u64::from_str_radix(checksum, 16).map_err(|_| invalid_cursor_error())?;
    if fnv1a(&raw) != expected_checksum {
        return Err(invalid_cursor_error());
    }
    let raw = String::from_utf8(raw).map_err(|_| invalid_cursor_error())?;
    let mut parts = raw.split('|');
    if parts.next() != Some("1") || parts.next() != Some(expected_log_id) {
        return Err(invalid_cursor_error());
    }
    let size = parts
        .next()
        .ok_or_else(invalid_cursor_error)?
        .parse::<u64>()
        .map_err(|_| invalid_cursor_error())?;
    let modified = parts
        .next()
        .ok_or_else(invalid_cursor_error)?
        .parse::<u64>()
        .map_err(|_| invalid_cursor_error())?;
    let end = parts
        .next()
        .ok_or_else(invalid_cursor_error)?
        .parse::<u64>()
        .map_err(|_| invalid_cursor_error())?;
    if parts.next().is_some() {
        return Err(invalid_cursor_error());
    }
    Ok(Cursor {
        size,
        modified,
        end,
    })
}

fn invalid_cursor_error() -> LogErrorInfo {
    log_error(
        "read",
        "invalid_cursor",
        "The log page cursor is invalid.",
        "Refresh the selected log from its newest lines and retry.",
    )
}

pub fn default_support_bundle_name() -> String {
    let (year, month, day, hour, minute, second) = utc_parts(now_epoch());
    format!("CoffeePOS-support-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.zip")
}

pub fn export_support_bundle(
    data_root: &Path,
    destination: &Path,
    runtime: Option<&RuntimeInfo>,
) -> Result<SupportBundleResult, LogErrorInfo> {
    let redactor = load_redactor(data_root, "export")?;
    export_support_bundle_with_redactor(data_root, destination, runtime, &redactor)
}

fn export_support_bundle_with_redactor(
    data_root: &Path,
    destination: &Path,
    runtime: Option<&RuntimeInfo>,
    redactor: &Redactor,
) -> Result<SupportBundleResult, LogErrorInfo> {
    let destination_parent = destination.parent().ok_or_else(|| {
        log_error(
            "finalize",
            "invalid_destination",
            "The selected support-bundle destination is invalid.",
            "Choose another local destination and retry.",
        )
    })?;
    if !destination_parent.is_dir() {
        return Err(log_error(
            "finalize",
            "invalid_destination",
            "The selected support-bundle folder does not exist.",
            "Choose an existing folder and retry.",
        ));
    }
    let temp_dir = data_root.join("temp");
    fs::create_dir_all(&temp_dir).map_err(|_| {
        log_error(
            "export",
            "temp_unavailable",
            "CoffeePOS cannot prepare temporary support-bundle storage.",
            "Check application-data permissions and free disk space, then retry.",
        )
    })?;

    let mut entries = build_support_entries(data_root, runtime, redactor)?;
    let redaction_count = entries
        .iter()
        .filter_map(|entry| std::str::from_utf8(&entry.bytes).ok())
        .map(|text| text.matches(REDACTED).count() as u64)
        .sum();
    for entry in &entries {
        validate_zip_entry_name(entry.name)?;
        if !redactor.verify_absent(&entry.bytes) {
            return Err(log_error(
                "export",
                "redaction_safety_failed",
                "CoffeePOS stopped support-bundle export because a protected credential remained after redaction.",
                "Do not share a partial bundle. Retry after restoring the protected credential state.",
            ));
        }
    }
    let total_uncompressed = entries.iter().map(|entry| entry.bytes.len()).sum::<usize>();
    if total_uncompressed > BUNDLE_MAX_UNCOMPRESSED_BYTES {
        return Err(log_error(
            "export",
            "bundle_size_limit",
            "The support bundle exceeded CoffeePOS safety limits.",
            "Retry after rotating or trimming unusually large managed logs.",
        ));
    }

    let mut temp_zip = NamedTempFile::new_in(&temp_dir).map_err(|_| {
        log_error(
            "export",
            "temp_unavailable",
            "CoffeePOS cannot create the temporary support bundle.",
            "Check application-data permissions and free disk space, then retry.",
        )
    })?;
    write_zip_store(temp_zip.as_file_mut(), &mut entries)?;
    temp_zip.as_file().sync_all().map_err(|_| {
        log_error(
            "export",
            "zip_finalize_failed",
            "CoffeePOS cannot flush the temporary support bundle.",
            "Check free disk space and retry.",
        )
    })?;

    let mut destination_temp = NamedTempFile::new_in(destination_parent).map_err(|_| {
        log_error(
            "finalize",
            "destination_unwritable",
            "CoffeePOS cannot create a file in the selected destination.",
            "Choose another writable folder and retry.",
        )
    })?;
    temp_zip
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| {
            log_error(
                "finalize",
                "zip_finalize_failed",
                "CoffeePOS cannot reread the completed temporary support bundle.",
                "Retry support-bundle export.",
            )
        })?;
    io::copy(temp_zip.as_file_mut(), destination_temp.as_file_mut()).map_err(|_| {
        log_error(
            "finalize",
            "destination_unwritable",
            "CoffeePOS cannot write the support bundle to the selected destination.",
            "Check free disk space and destination permissions, then retry.",
        )
    })?;
    destination_temp.as_file().sync_all().map_err(|_| {
        log_error(
            "finalize",
            "destination_unwritable",
            "CoffeePOS cannot flush the support bundle at the selected destination.",
            "Check the destination storage and retry.",
        )
    })?;
    destination_temp.persist(destination).map_err(|_| {
        log_error(
            "finalize",
            "destination_unwritable",
            "CoffeePOS cannot finalize the support bundle at the selected destination.",
            "Choose another writable destination and retry.",
        )
    })?;
    Ok(SupportBundleResult {
        status: SupportBundleStatus::Exported,
        destination: Some(destination.to_string_lossy().into_owned()),
        files_included: entries.len(),
        redaction_count,
    })
}

fn build_support_entries(
    data_root: &Path,
    runtime: Option<&RuntimeInfo>,
    redactor: &Redactor,
) -> Result<Vec<ZipEntry>, LogErrorInfo> {
    let created_at = rfc3339_utc(now_epoch());
    let mut entries = Vec::new();
    let runtime_json = runtime_projection(runtime, redactor, data_root)?;
    let health_json = health_projection(runtime, redactor, data_root)?;
    let provisioning_json = projection_from_json_file(
        &data_root.join("config/provisioning.json"),
        &[
            "schema_version",
            "stage",
            "wordpress_version",
            "woocommerce_version",
            "coffeepos_version",
            "recovery_blocker",
        ],
        redactor,
        data_root,
    )?;
    let repair_json = repair_projection(data_root, redactor)?;
    entries.push(json_entry("diagnostics/runtime.json", runtime_json)?);
    entries.push(json_entry("diagnostics/health.json", health_json)?);
    entries.push(json_entry(
        "diagnostics/provisioning.json",
        provisioning_json,
    )?);
    entries.push(json_entry("diagnostics/repair.json", repair_json)?);

    let mut processed = 0_u64;
    let mut replacement_count = 0_u64;
    let mut truncated_sources = Vec::new();
    let diagnostics_bytes = entries.iter().map(|entry| entry.bytes.len()).sum::<usize>();
    let mut remaining = BUNDLE_MAX_UNCOMPRESSED_BYTES.saturating_sub(diagnostics_bytes + 32 * 1024);

    for definition in LOG_DEFINITIONS {
        let Some(path) = canonical_source(data_root, definition, "export")? else {
            continue;
        };
        let (raw, source_truncated) = read_tail_for_bundle(&path)?;
        let input = String::from_utf8_lossy(&raw);
        let redacted = redactor.redact(&input);
        replacement_count = replacement_count.saturating_add(redacted.replacements);
        let normalized = normalize_export_text(&redacted.text, data_root);
        let mut content = normalized.into_bytes();
        let mut truncated = source_truncated;
        if source_truncated {
            let mut prefixed = OMITTED_MARKER.as_bytes().to_vec();
            prefixed.extend_from_slice(&content);
            content = prefixed;
        }
        if content.len() > remaining {
            content = bounded_tail_text(&content, remaining);
            truncated = true;
        }
        if truncated {
            truncated_sources.push(definition.id.to_string());
        }
        if !redactor.verify_absent(&content) {
            return Err(log_error("export", "redaction_safety_failed", "CoffeePOS stopped support-bundle export because a protected credential remained after redaction.", "Restore the protected credential state and retry."));
        }
        remaining = remaining.saturating_sub(content.len());
        processed = processed.saturating_add(1);
        let name = match definition.id {
            "application" => "logs/application.log",
            "runtime" => "logs/runtime.log",
            "web_server" => "logs/web-server.log",
            "php" => "logs/php.log",
            "database" => "logs/database.log",
            "cron" => "logs/cron.log",
            "provisioning" => "logs/provisioning.log",
            "wordpress" => "logs/wordpress.log",
            "woocommerce" => "logs/woocommerce.log",
            "coffeepos" => "logs/coffeepos.log",
            _ => unreachable!("fixed log allowlist"),
        };
        entries.push(ZipEntry {
            name,
            bytes: content,
        });
    }

    let (runtime_state, installation_state, php, mariadb, caddy) = match runtime {
        Some(info) => (
            runtime_state_name(&info.state),
            if matches!(info.state, RuntimeState::NotInstalled) {
                "not_installed"
            } else {
                "installed"
            },
            info.php_version.clone(),
            info.mariadb_version.clone(),
            info.web_server_version.clone(),
        ),
        None => ("uninitialized", "unknown", None, None, None),
    };
    let manifest = json!({
        "schema_version": 1,
        "created_at": created_at,
        "desktop_version": env!("CARGO_PKG_VERSION"),
        "target": target_name(),
        "runtime": { "php": php, "mariadb": mariadb, "caddy": caddy },
        "installation_state": installation_state,
        "runtime_state": runtime_state,
        "redaction": {
            "schema_version": 1,
            "files_processed": processed,
            "replacements": replacement_count,
            "truncated_sources": truncated_sources,
        }
    });
    entries.insert(
        0,
        json_entry(
            "manifest.json",
            normalize_json_value(manifest, redactor, data_root)?,
        )?,
    );
    Ok(entries)
}

fn runtime_projection(
    runtime: Option<&RuntimeInfo>,
    redactor: &Redactor,
    data_root: &Path,
) -> Result<Value, LogErrorInfo> {
    let value = match runtime {
        Some(info) => json!({
            "available": true,
            "state": runtime_state_name(&info.state),
            "versions": {
                "runtime": info.runtime_version,
                "php": info.php_version,
                "web_server": info.web_server_version,
                "mariadb": info.mariadb_version,
            },
            "ports": { "http": info.http_port, "database": info.database_port },
            "processes": {
                "database": info.database_pid.is_some(),
                "php": info.php_pid.is_some(),
                "web_server": info.web_server_pid.is_some(),
            },
            "wordpress_health": wordpress_health_name(&info.wordpress_health),
            "coffeepos_health": coffeepos_health_name(&info.coffeepos_health.state),
            "wordpress_error": safe_error(info.wordpress_error.as_ref(), redactor, data_root),
            "last_error": safe_error(info.last_error.as_ref(), redactor, data_root),
        }),
        None => json!({ "available": false, "reason": "no_runtime_snapshot" }),
    };
    normalize_json_value(value, redactor, data_root)
}

fn health_projection(
    runtime: Option<&RuntimeInfo>,
    redactor: &Redactor,
    data_root: &Path,
) -> Result<Value, LogErrorInfo> {
    let value = match runtime {
        Some(info) => json!({
            "available": true,
            "runtime_state": runtime_state_name(&info.state),
            "database": { "process_present": info.database_pid.is_some() },
            "php": { "process_present": info.php_pid.is_some() },
            "wordpress": { "state": wordpress_health_name(&info.wordpress_health), "error": safe_error(info.wordpress_error.as_ref(), redactor, data_root) },
            "woocommerce": { "state": info.coffeepos_health.payload.as_ref().map(|payload| if payload.woocommerce { "healthy" } else { "unhealthy" }) },
            "coffeepos": { "state": coffeepos_health_name(&info.coffeepos_health.state), "error": safe_error(info.coffeepos_health.error.as_ref(), redactor, data_root) },
        }),
        None => json!({ "available": false, "reason": "no_cached_snapshot" }),
    };
    normalize_json_value(value, redactor, data_root)
}

fn safe_error(error: Option<&RuntimeErrorInfo>, redactor: &Redactor, data_root: &Path) -> Value {
    match error {
        Some(error) => {
            let message = normalize_export_text(&redactor.redact(&error.message).text, data_root);
            let recovery = normalize_export_text(&redactor.redact(&error.recovery).text, data_root);
            json!({ "component": error.component, "action": error.operation, "message": message, "recovery": recovery })
        }
        None => Value::Null,
    }
}

fn projection_from_json_file(
    path: &Path,
    allow: &[&str],
    redactor: &Redactor,
    data_root: &Path,
) -> Result<Value, LogErrorInfo> {
    let bytes = match fs::read(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(json!({ "available": false, "reason": "not_present" }))
        }
        Err(_) => return Ok(json!({ "available": false, "reason": "unreadable" })),
    };
    let source: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Ok(json!({ "available": false, "reason": "invalid" })),
    };
    let Some(object) = source.as_object() else {
        return Ok(json!({ "available": false, "reason": "invalid" }));
    };
    let mut projected = serde_json::Map::new();
    projected.insert("available".into(), Value::Bool(true));
    for key in allow {
        if let Some(value) = object.get(*key) {
            projected.insert((*key).to_string(), value.clone());
        }
    }
    normalize_json_value(Value::Object(projected), redactor, data_root)
}

fn repair_projection(data_root: &Path, redactor: &Redactor) -> Result<Value, LogErrorInfo> {
    projection_from_json_file(
        &data_root.join("config/repair.json"),
        &[
            "schema_version",
            "plan_id",
            "item_ids",
            "completed_item_ids",
            "active_item_id",
            "runtime_was_running",
            "stage",
        ],
        redactor,
        data_root,
    )
}

fn normalize_json_value(
    value: Value,
    redactor: &Redactor,
    data_root: &Path,
) -> Result<Value, LogErrorInfo> {
    Ok(normalize_json_strings(value, redactor, data_root))
}

fn normalize_json_strings(value: Value, redactor: &Redactor, data_root: &Path) -> Value {
    match value {
        Value::String(text) => {
            let exact = redactor.redact_exact(&text);
            Value::String(normalize_export_text(&exact.text, data_root))
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| normalize_json_strings(value, redactor, data_root))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, normalize_json_strings(value, redactor, data_root)))
                .collect(),
        ),
        other => other,
    }
}

fn json_entry(name: &'static str, value: Value) -> Result<ZipEntry, LogErrorInfo> {
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|_| {
        log_error(
            "export",
            "diagnostics_serialize_failed",
            "CoffeePOS cannot serialize safe support diagnostics.",
            "Retry support-bundle export.",
        )
    })?;
    bytes.push(b'\n');
    Ok(ZipEntry { name, bytes })
}

fn read_tail_for_bundle(path: &Path) -> Result<(Vec<u8>, bool), LogErrorInfo> {
    let mut file = File::open(path).map_err(|_| {
        log_error(
            "export",
            "source_unavailable",
            "CoffeePOS cannot open a managed log while creating the support bundle.",
            "Check log permissions and retry export.",
        )
    })?;
    let size = file
        .metadata()
        .map_err(|_| {
            log_error(
                "export",
                "source_unavailable",
                "CoffeePOS cannot inspect a managed log while creating the support bundle.",
                "Check log permissions and retry export.",
            )
        })?
        .len();
    let truncated = size > BUNDLE_MAX_SOURCE_BYTES;
    let start = size.saturating_sub(BUNDLE_MAX_SOURCE_BYTES);
    file.seek(SeekFrom::Start(start)).map_err(|_| {
        log_error(
            "export",
            "source_unavailable",
            "CoffeePOS cannot seek within a managed log while creating the support bundle.",
            "Retry support-bundle export.",
        )
    })?;
    let mut bytes = Vec::with_capacity((size - start) as usize);
    file.take(BUNDLE_MAX_SOURCE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            log_error(
                "export",
                "source_changed",
                "A managed log changed unexpectedly while CoffeePOS was exporting it.",
                "Retry support-bundle export.",
            )
        })?;
    if truncated {
        if let Some(index) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=index);
        } else {
            bytes.clear();
        }
    }
    if !bytes.ends_with(b"\n") {
        if let Some(index) = bytes.iter().rposition(|byte| *byte == b'\n') {
            bytes.truncate(index + 1);
        } else {
            bytes.clear();
        }
    }
    Ok((bytes, truncated))
}

fn bounded_tail_text(bytes: &[u8], limit: usize) -> Vec<u8> {
    if limit == 0 {
        return Vec::new();
    }
    if bytes.len() <= limit {
        return bytes.to_vec();
    }
    if limit <= OMITTED_MARKER.len() {
        return OMITTED_MARKER.as_bytes()[..limit].to_vec();
    }
    let keep = limit - OMITTED_MARKER.len();
    let mut start = bytes.len() - keep;
    while start < bytes.len() && !std::str::from_utf8(&bytes[start..]).is_ok() {
        start += 1;
    }
    let mut output = OMITTED_MARKER.as_bytes().to_vec();
    output.extend_from_slice(&bytes[start..]);
    output
}

fn normalize_export_text(input: &str, data_root: &Path) -> String {
    let profile = std::env::var_os("USERPROFILE")
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty());
    normalize_export_text_with_profile(input, data_root, profile.as_deref())
}

fn normalize_export_text_with_profile(
    input: &str,
    data_root: &Path,
    user_profile: Option<&str>,
) -> String {
    let root = data_root.to_string_lossy();
    let root_forward = root.replace('\\', "/");
    let mut output = replace_export_path(input, root.as_ref(), "%COFFEEPOS_DATA%");
    output = replace_export_path(&output, &root_forward, "%COFFEEPOS_DATA%");
    if let Some(profile) = user_profile.filter(|value| !value.is_empty()) {
        let profile_forward = profile.replace('\\', "/");
        output = replace_export_path(&output, profile, "%USERPROFILE%");
        output = replace_export_path(&output, &profile_forward, "%USERPROFILE%");
    }
    output
}

fn replace_export_path(input: &str, path: &str, replacement: &str) -> String {
    if path.is_empty() {
        return input.to_string();
    }
    #[cfg(windows)]
    {
        replace_ascii_case_insensitive(input, path, replacement)
    }
    #[cfg(not(windows))]
    {
        input.replace(path, replacement)
    }
}

#[cfg(windows)]
fn replace_ascii_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut index = 0usize;
    while index < input.len() {
        let end = index.saturating_add(needle.len());
        if end <= input.len()
            && input
                .get(index..end)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
        {
            output.push_str(replacement);
            index = end;
            continue;
        }

        let ch = input[index..]
            .chars()
            .next()
            .expect("index always points at a UTF-8 character boundary");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn validate_zip_entry_name(name: &str) -> Result<(), LogErrorInfo> {
    let path = Path::new(name);
    if name.is_empty()
        || name.contains('\\')
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(log_error(
            "export",
            "unsafe_archive_path",
            "CoffeePOS refused an unsafe support-bundle archive path.",
            "Retry with the built-in support-bundle schema.",
        ));
    }
    Ok(())
}

fn write_zip_store(writer: &mut File, entries: &mut [ZipEntry]) -> Result<(), LogErrorInfo> {
    struct Central {
        name: &'static str,
        crc: u32,
        size: u32,
        offset: u32,
    }
    let mut central = Vec::with_capacity(entries.len());
    let mut offset = 0_u64;
    for entry in entries.iter() {
        let name = entry.name.as_bytes();
        let size = u32::try_from(entry.bytes.len()).map_err(|_| {
            log_error(
                "export",
                "bundle_size_limit",
                "A support-bundle entry is too large.",
                "Trim managed logs and retry.",
            )
        })?;
        let offset32 = u32::try_from(offset).map_err(|_| {
            log_error(
                "export",
                "bundle_size_limit",
                "The support bundle is too large.",
                "Trim managed logs and retry.",
            )
        })?;
        let crc = crc32(&entry.bytes);
        write_u32(writer, 0x04034b50)?;
        write_u16(writer, 20)?;
        write_u16(writer, 0x0800)?;
        write_u16(writer, 0)?;
        write_u16(writer, 0)?;
        write_u16(writer, 33)?;
        write_u32(writer, crc)?;
        write_u32(writer, size)?;
        write_u32(writer, size)?;
        write_u16(writer, name.len() as u16)?;
        write_u16(writer, 0)?;
        writer.write_all(name).map_err(zip_write_error)?;
        writer.write_all(&entry.bytes).map_err(zip_write_error)?;
        offset += 30 + name.len() as u64 + entry.bytes.len() as u64;
        central.push(Central {
            name: entry.name,
            crc,
            size,
            offset: offset32,
        });
    }
    let central_start = offset;
    for item in &central {
        let name = item.name.as_bytes();
        write_u32(writer, 0x02014b50)?;
        write_u16(writer, 20)?;
        write_u16(writer, 20)?;
        write_u16(writer, 0x0800)?;
        write_u16(writer, 0)?;
        write_u16(writer, 0)?;
        write_u16(writer, 33)?;
        write_u32(writer, item.crc)?;
        write_u32(writer, item.size)?;
        write_u32(writer, item.size)?;
        write_u16(writer, name.len() as u16)?;
        write_u16(writer, 0)?;
        write_u16(writer, 0)?;
        write_u16(writer, 0)?;
        write_u16(writer, 0)?;
        write_u32(writer, 0)?;
        write_u32(writer, item.offset)?;
        writer.write_all(name).map_err(zip_write_error)?;
        offset += 46 + name.len() as u64;
    }
    let central_size = offset - central_start;
    let entry_count = u16::try_from(central.len()).map_err(|_| {
        log_error(
            "export",
            "bundle_size_limit",
            "The support bundle contains too many entries.",
            "Retry with the built-in support-bundle schema.",
        )
    })?;
    write_u32(writer, 0x06054b50)?;
    write_u16(writer, 0)?;
    write_u16(writer, 0)?;
    write_u16(writer, entry_count)?;
    write_u16(writer, entry_count)?;
    write_u32(writer, central_size as u32)?;
    write_u32(writer, central_start as u32)?;
    write_u16(writer, 0)?;
    Ok(())
}

fn write_u16(writer: &mut File, value: u16) -> Result<(), LogErrorInfo> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(zip_write_error)
}

fn write_u32(writer: &mut File, value: u32) -> Result<(), LogErrorInfo> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(zip_write_error)
}

fn zip_write_error(_: io::Error) -> LogErrorInfo {
    log_error(
        "export",
        "zip_write_failed",
        "CoffeePOS cannot write the temporary support bundle.",
        "Check free disk space and retry.",
    )
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn runtime_state_name(state: &RuntimeState) -> &'static str {
    match state {
        RuntimeState::NotInstalled => "not_installed",
        RuntimeState::Installing => "installing",
        RuntimeState::Stopped => "stopped",
        RuntimeState::Starting => "starting",
        RuntimeState::Running => "running",
        RuntimeState::Stopping => "stopping",
    }
}

fn wordpress_health_name(state: &WordPressHealthState) -> &'static str {
    match state {
        WordPressHealthState::Unavailable => "unavailable",
        WordPressHealthState::Checking => "checking",
        WordPressHealthState::Healthy => "healthy",
        WordPressHealthState::Unhealthy => "unhealthy",
    }
}

fn coffeepos_health_name(state: &CoffeePosHealthState) -> &'static str {
    match state {
        CoffeePosHealthState::Unavailable => "unavailable",
        CoffeePosHealthState::Checking => "checking",
        CoffeePosHealthState::Healthy => "healthy",
        CoffeePosHealthState::Degraded => "degraded",
        CoffeePosHealthState::Failed => "failed",
    }
}

fn target_name() -> &'static str {
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

#[cfg(windows)]
pub fn choose_support_bundle_destination(
    default_name: &str,
) -> Result<Option<PathBuf>, LogErrorInfo> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let mut file_buffer = vec![0_u16; 32_768];
    let default: Vec<u16> = default_name.encode_utf16().collect();
    if default.len() + 1 >= file_buffer.len() {
        return Err(log_error(
            "export",
            "invalid_destination",
            "The default support-bundle filename is too long.",
            "Retry support-bundle export.",
        ));
    }
    file_buffer[..default.len()].copy_from_slice(&default);
    let filter: Vec<u16> = "ZIP archive (*.zip)\0*.zip\0All files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let extension: Vec<u16> = "zip\0".encode_utf16().collect();
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = file_buffer.as_mut_ptr();
    dialog.nMaxFile = file_buffer.len() as u32;
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.Flags = OFN_NOCHANGEDIR | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST;
    let selected = unsafe { GetSaveFileNameW(&mut dialog) };
    if selected == 0 {
        let code = unsafe { CommDlgExtendedError() };
        if code == 0 {
            return Ok(None);
        }
        return Err(log_error(
            "export",
            "save_dialog_failed",
            "Windows could not open the support-bundle Save As dialog.",
            "Retry export or restart CoffeePOS Desktop.",
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
pub fn choose_support_bundle_destination(
    _default_name: &str,
) -> Result<Option<PathBuf>, LogErrorInfo> {
    Err(log_error(
        "export",
        "unsupported_platform",
        "Support-bundle Save As is currently qualified for Windows.",
        "Run this Phase 6.4 flow on the supported Windows build.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_redaction_removes_generic_and_exact_secret_canaries() {
        let canary = "Phase64-Exact-Secret-Canary";
        let redactor = Redactor {
            exact_secrets: vec![canary.into()],
        };
        let input = format!(
            "safe line\npassword=hunter2\nAuthorization: Bearer abc\nCookie: wordpress_logged_in_x=y\npostgres://alice:p4ss@localhost/db\nvalue={canary}\n"
        );
        let result = redactor.redact(&input);
        assert!(result.text.contains("safe line"));
        assert!(!result.text.contains("hunter2"));
        assert!(!result.text.contains("Bearer abc"));
        assert!(!result.text.contains("wordpress_logged_in_x"));
        assert!(!result.text.contains("p4ss"));
        assert!(!result.text.contains(canary));
        assert!(result.replacements >= 5);
    }

    #[test]
    fn log_reader_rejects_arbitrary_path_shaped_log_id() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("logs")).unwrap();
        let error = read_log_page(temp.path(), "../config/app.json", None, None, None).unwrap_err();
        assert_eq!(error.code, "unknown_log_id");
    }

    #[test]
    fn log_reader_fails_closed_when_existing_secret_is_unreadable() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("logs")).unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(temp.path().join("logs/runtime.log"), b"safe line\n").unwrap();
        fs::write(
            temp.path().join("config/database-runtime.secret"),
            b"not-a-valid-protected-secret",
        )
        .unwrap();
        let error = read_log_page(temp.path(), "runtime", None, None, None).unwrap_err();
        assert_eq!(error.code, "protected_secret_unreadable");
    }

    #[test]
    fn log_paging_is_bounded_and_does_not_duplicate_stable_lines() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("logs")).unwrap();
        let body = (0..9)
            .map(|index| format!("line-{index}\n"))
            .collect::<String>();
        fs::write(temp.path().join("logs/runtime.log"), body).unwrap();
        let first = read_log_page(temp.path(), "runtime", None, Some("tail"), Some(3)).unwrap();
        assert_eq!(first.lines, vec!["line-6", "line-7", "line-8"]);
        let second = read_log_page(
            temp.path(),
            "runtime",
            first.older_cursor.as_deref(),
            Some("older"),
            Some(3),
        )
        .unwrap();
        assert_eq!(second.lines, vec!["line-3", "line-4", "line-5"]);
        let third = read_log_page(
            temp.path(),
            "runtime",
            second.older_cursor.as_deref(),
            Some("older"),
            Some(3),
        )
        .unwrap();
        assert_eq!(third.lines, vec!["line-0", "line-1", "line-2"]);
        assert!(!third.has_older);
    }

    #[test]
    fn log_reader_rejects_cursor_that_points_inside_a_line() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("logs")).unwrap();
        let path = temp.path().join("logs/runtime.log");
        fs::write(&path, b"line-0\nline-1\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let cursor = encode_cursor("runtime", metadata.len(), modified_stamp(&metadata), 3);
        let error = read_log_page(
            temp.path(),
            "runtime",
            Some(&cursor),
            Some("older"),
            Some(1),
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid_cursor");
    }

    #[cfg(windows)]
    #[test]
    fn incomplete_trailing_secret_prefix_is_never_returned_or_exported() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        let output = temp.path().join("support.zip");
        fs::create_dir_all(data_root.join("logs")).unwrap();
        fs::create_dir_all(data_root.join("config")).unwrap();
        let secret_value = "Phase64SecretCanaryForIncompleteLine";
        let leaked_prefix = "Phase64SecretCanary";
        secret::store_password(
            &data_root.join("config/wordpress-admin.secret"),
            secret_value,
        )
        .unwrap();
        fs::write(
            data_root.join("logs/runtime.log"),
            format!("safe complete line\n{leaked_prefix}"),
        )
        .unwrap();

        let page = read_log_page(&data_root, "runtime", None, Some("tail"), Some(20)).unwrap();
        assert_eq!(page.lines, vec!["safe complete line"]);
        assert!(!page.lines.join("\n").contains(leaked_prefix));

        export_support_bundle(&data_root, &output, None).unwrap();
        let archive = fs::read(output).unwrap();
        assert!(!archive
            .windows(leaked_prefix.len())
            .any(|window| window == leaked_prefix.as_bytes()));
    }

    #[test]
    fn support_bundle_redacts_secret_canary_and_ignores_unlisted_log() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        let out = temp.path().join("support.zip");
        fs::create_dir_all(data_root.join("logs")).unwrap();
        fs::create_dir_all(data_root.join("config")).unwrap();
        let canary = "Phase64-Bundle-Secret-Canary";
        fs::write(
            data_root.join("logs/runtime.log"),
            format!("safe\nvalue={canary}\nAuthorization: Bearer leak\n"),
        )
        .unwrap();
        fs::write(
            data_root.join("logs/unlisted.log"),
            b"must not be exported\n",
        )
        .unwrap();
        let redactor = Redactor {
            exact_secrets: vec![canary.into()],
        };
        let result =
            export_support_bundle_with_redactor(&data_root, &out, None, &redactor).unwrap();
        assert_eq!(result.status, SupportBundleStatus::Exported);
        let archive = fs::read(out).unwrap();
        assert!(!archive
            .windows(canary.len())
            .any(|window| window == canary.as_bytes()));
        assert!(!archive
            .windows(b"must not be exported".len())
            .any(|window| window == b"must not be exported"));
        assert!(archive
            .windows(b"manifest.json".len())
            .any(|window| window == b"manifest.json"));
        assert!(archive
            .windows(b"logs/runtime.log".len())
            .any(|window| window == b"logs/runtime.log"));
        assert!(archive
            .windows(REDACTED.len())
            .any(|window| window == REDACTED.as_bytes()));
    }

    #[test]
    fn support_bundle_paths_are_relative_and_normalized() {
        for name in [
            "manifest.json",
            "diagnostics/runtime.json",
            "logs/runtime.log",
        ] {
            validate_zip_entry_name(name).unwrap();
        }
        for name in ["../secret", "/absolute", "logs\\runtime.log"] {
            assert!(validate_zip_entry_name(name).is_err());
        }
        let root = Path::new(r"C:\Users\Alice\AppData\Local\CoffeePOS");
        let normalized = normalize_export_text(
            r"error at C:\Users\Alice\AppData\Local\CoffeePOS\logs\runtime.log",
            root,
        );
        assert!(normalized.contains("%COFFEEPOS_DATA%"));
        assert!(!normalized.contains("Alice"));
    }

    #[cfg(windows)]
    #[test]
    fn exported_windows_paths_are_normalized_ascii_case_insensitively() {
        let root = Path::new(r"C:\Users\Alice\AppData\Local\CoffeePOS");
        let profile = r"C:\Users\Alice";
        let input = concat!(
            r"runtime=c:\USERS\alice\APPDATA\local\coffeepos\logs\runtime.log; ",
            r"profile=C:/USERS/ALICE/Documents/support.txt; ",
            "unicode=Đường/Dẫn"
        );
        let normalized = normalize_export_text_with_profile(input, root, Some(profile));
        assert!(normalized.contains(r"runtime=%COFFEEPOS_DATA%\logs\runtime.log"));
        assert!(normalized.contains("profile=%USERPROFILE%/Documents/support.txt"));
        assert!(normalized.contains("unicode=Đường/Dẫn"));
        assert!(!normalized.to_ascii_lowercase().contains(r"c:\users\alice"));
        assert!(!normalized.to_ascii_lowercase().contains("c:/users/alice"));
    }

    #[test]
    fn structured_diagnostics_preserve_machine_token_like_keys_and_ids() {
        let redactor = Redactor {
            exact_secrets: vec!["real-secret-canary".into()],
        };
        let value = json!({
            "active_item_id": "machine_token_pending",
            "machine_token_status": "blocked",
            "message": "secret=real-secret-canary"
        });
        let normalized =
            normalize_json_value(value, &redactor, Path::new(r"C:\CoffeePOS")).unwrap();
        assert_eq!(normalized["active_item_id"], "machine_token_pending");
        assert_eq!(normalized["machine_token_status"], "blocked");
        assert_eq!(normalized["message"], "secret=[redacted]");
    }

    #[test]
    fn utc_timestamp_format_matches_support_bundle_contract() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_parts(1_758_278_700), (2025, 9, 19, 10, 45, 0));
    }
}
