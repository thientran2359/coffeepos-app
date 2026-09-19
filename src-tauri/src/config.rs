use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub const APP_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupView {
    Home,
    Settings,
    Diagnostics,
}

impl Default for StartupView {
    fn default() -> Self {
        Self::Home
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub schema_version: u32,
    pub store_name: String,
    pub bind_host: String,
    #[serde(default)]
    pub startup_view: StartupView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_admin_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_admin_email: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: APP_CONFIG_SCHEMA_VERSION,
            store_name: "My Coffee".into(),
            bind_host: "127.0.0.1".into(),
            startup_view: StartupView::Home,
            setup_admin_username: None,
            setup_admin_email: None,
        }
    }
}

fn valid_admin_username(value: &str) -> bool {
    let value = value.trim();
    (3..=60).contains(&value.chars().count())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_admin_email(value: &str) -> bool {
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

#[cfg(debug_assertions)]
fn canonical_setup_store_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err("Store name must contain 1–80 characters without control characters.".into());
    }
    if value.contains('<') || value.contains('>') || value.contains("  ") {
        return Err("Store name cannot contain HTML brackets or repeated spaces.".into());
    }
    let bytes = value.as_bytes();
    if bytes.windows(3).any(|window| {
        window[0] == b'%' && window[1].is_ascii_hexdigit() && window[2].is_ascii_hexdigit()
    }) {
        return Err("Store name cannot contain percent-encoded byte sequences such as %20.".into());
    }
    Ok(value.to_string())
}

impl AppConfig {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != APP_CONFIG_SCHEMA_VERSION {
            return Err("Unsupported configuration version. Use a compatible CoffeePOS Desktop version; the file has been preserved.".into());
        }
        if self.bind_host != "127.0.0.1" {
            return Err(
                "Phase 1 supports local-only mode. Set bind_host to 127.0.0.1 in config/app.json."
                    .into(),
            );
        }
        if self.store_name.trim().is_empty()
            || self.store_name.chars().count() > 80
            || self.store_name.chars().any(char::is_control)
        {
            return Err(
                "Store name must contain 1–80 characters without control characters.".into(),
            );
        }
        if let Some(username) = &self.setup_admin_username {
            if !valid_admin_username(username) {
                return Err("Administrator username must contain 3–60 ASCII letters, numbers, dots, underscores, or hyphens.".into());
            }
        }
        if let Some(email) = &self.setup_admin_email {
            if !valid_admin_email(email) {
                return Err(
                    "Administrator email must be a valid email address up to 100 characters."
                        .into(),
                );
            }
        }
        if self.setup_admin_username.is_some() != self.setup_admin_email.is_some() {
            return Err("Administrator setup username and email must be saved together.".into());
        }
        Ok(())
    }
}

pub struct Store {
    pub root: PathBuf,
    pub config: AppConfig,
    _lock: File,
}

fn disk_error(operation: &str, error: impl std::fmt::Display) -> String {
    format!("Configuration: cannot {operation}: {error}. Check application-data permissions and available disk space, then retry. Existing files are not reset automatically.")
}

fn persist(path: &Path, config: &AppConfig) -> Result<(), String> {
    config.validate()?;
    let parent = path.parent().ok_or("Configuration directory is missing.")?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|e| disk_error("create temporary configuration", e))?;
    serde_json::to_writer_pretty(&mut temporary, config)
        .map_err(|e| disk_error("serialize configuration", e))?;
    temporary
        .write_all(b"\n")
        .map_err(|e| disk_error("write configuration", e))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| disk_error("flush configuration", e))?;
    temporary
        .persist(path)
        .map_err(|e| disk_error("replace configuration", e))?;
    Ok(())
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|e| disk_error("create data directory", e))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("desktop.lock"))
            .map_err(|e| disk_error("open application lock", e))?;
        lock.try_lock_exclusive().map_err(|_| "Cannot lock CoffeePOS data. Close any other CoffeePOS Desktop window using this data directory, then retry.".to_string())?;
        for directory in ["site", "database", "uploads", "config", "logs", "backups"] {
            fs::create_dir_all(root.join(directory))
                .map_err(|e| disk_error("create store directories", e))?;
        }
        let path = root.join("config/app.json");
        let config: AppConfig = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "Cannot read config/app.json: invalid configuration. The file has been preserved. Correct it or restore a known-good copy, then retry.".to_string())?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let config = AppConfig::default();
                persist(&path, &config)?;
                config
            }
            Err(e) => return Err(disk_error("read configuration", e)),
        };
        config.validate()?;
        let store = Self {
            root,
            config,
            _lock: lock,
        };
        store.log("desktop configuration ready");
        Ok(store)
    }

    #[cfg(test)]
    pub fn save_name(&mut self, name: &str) -> Result<(), String> {
        let next = AppConfig {
            store_name: name.trim().into(),
            ..self.config.clone()
        };
        persist(&self.root.join("config/app.json"), &next)?;
        self.config = next;
        self.log("desktop configuration saved");
        Ok(())
    }

    #[cfg(debug_assertions)]
    pub fn save_setup_profile(
        &mut self,
        store_name: &str,
        admin_username: &str,
        admin_email: &str,
    ) -> Result<(), String> {
        let next = AppConfig {
            store_name: canonical_setup_store_name(store_name)?,
            setup_admin_username: Some(admin_username.trim().into()),
            setup_admin_email: Some(admin_email.trim().into()),
            ..self.config.clone()
        };
        persist(&self.root.join("config/app.json"), &next)?;
        self.config = next;
        self.log("desktop onboarding profile saved");
        Ok(())
    }

    pub fn save_startup_view(&mut self, startup_view: StartupView) -> Result<(), String> {
        let next = AppConfig {
            startup_view,
            ..self.config.clone()
        };
        persist(&self.root.join("config/app.json"), &next)?;
        self.config = next;
        self.log("desktop application settings saved");
        Ok(())
    }

    fn log(&self, event: &str) {
        // Diagnostic logging never includes configuration values or credentials.
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_secs())
            .unwrap_or_default();
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("logs/application.log"))
        {
            let _ = writeln!(file, "{timestamp} {event}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_and_restart_preserve_configuration_and_store_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("CoffeePOS có dấu");
        let mut store = Store::open(root.clone()).unwrap();
        store.save_name("  Cà phê Việt  ").unwrap();
        fs::write(root.join("database/sentinel"), b"existing store").unwrap();
        drop(store);
        let reopened = Store::open(root.clone()).unwrap();
        assert_eq!(reopened.config.store_name, "Cà phê Việt");
        assert_eq!(
            fs::read(root.join("database/sentinel")).unwrap(),
            b"existing store"
        );
    }

    #[test]
    fn malformed_or_future_config_is_preserved() {
        let temp = tempfile::tempdir().unwrap();
        drop(Store::open(temp.path().to_owned()).unwrap());
        let path = temp.path().join("config/app.json");
        for bytes in [
            "{broken",
            r#"{"schema_version":2,"store_name":"Shop","bind_host":"127.0.0.1"}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(Store::open(temp.path().to_owned()).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn phase_4_schema_one_config_without_onboarding_fields_remains_compatible() {
        let temp = tempfile::tempdir().unwrap();
        drop(Store::open(temp.path().to_owned()).unwrap());
        let path = temp.path().join("config/app.json");
        fs::write(
            &path,
            r#"{"schema_version":1,"store_name":"Legacy Coffee","bind_host":"127.0.0.1"}"#,
        )
        .unwrap();
        let reopened = Store::open(temp.path().to_owned()).unwrap();
        assert_eq!(reopened.config.store_name, "Legacy Coffee");
        assert_eq!(reopened.config.startup_view, StartupView::Home);
        assert!(reopened.config.setup_admin_username.is_none());
        assert!(reopened.config.setup_admin_email.is_none());
    }

    #[test]
    fn invalid_save_leaves_memory_and_disk_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_owned()).unwrap();
        let previous = store.config.clone();
        let path = temp.path().join("config/app.json");
        let bytes = fs::read(&path).unwrap();
        assert!(store.save_name("  ").is_err());
        assert!(store.save_name(&"a".repeat(81)).is_err());
        assert!(store.save_name("A\nB").is_err());
        assert_eq!(store.config, previous);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn onboarding_profile_is_validated_and_persists_without_a_password() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mut store = Store::open(root.clone()).unwrap();
        store
            .save_setup_profile("  Cà phê 5.2  ", "owner_52", "owner52@example.com")
            .unwrap();
        assert_eq!(store.config.store_name, "Cà phê 5.2");
        assert_eq!(
            store.config.setup_admin_username.as_deref(),
            Some("owner_52")
        );
        assert_eq!(
            store.config.setup_admin_email.as_deref(),
            Some("owner52@example.com")
        );
        drop(store);

        let reopened = Store::open(root).unwrap();
        assert_eq!(reopened.config.store_name, "Cà phê 5.2");
        assert_eq!(
            reopened.config.setup_admin_username.as_deref(),
            Some("owner_52")
        );
        assert_eq!(
            reopened.config.setup_admin_email.as_deref(),
            Some("owner52@example.com")
        );
        let serialized = serde_json::to_string(&reopened.config).unwrap();
        assert!(!serialized.to_ascii_lowercase().contains("password"));

        let mut reopened = reopened;
        reopened
            .save_setup_profile("Coffee & \"Co\" Café", "owner_52", "owner52@example.com")
            .unwrap();
        assert_eq!(reopened.config.store_name, "Coffee & \"Co\" Café");
    }

    #[test]
    fn onboarding_profile_rejects_invalid_identity_without_replacing_config() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_owned()).unwrap();
        let previous = store.config.clone();
        assert!(store
            .save_setup_profile("Store", "x", "owner@example.com")
            .is_err());
        assert!(store
            .save_setup_profile("Store", "owner_52", "not-an-email")
            .is_err());
        assert!(store
            .save_setup_profile("Store", "owner_52", "owner!tag@example.com")
            .is_err());
        assert!(store
            .save_setup_profile("My  Coffee", "owner_52", "owner@example.com")
            .is_err());
        assert!(store
            .save_setup_profile("<b>Cafe</b>", "owner_52", "owner@example.com")
            .is_err());
        assert!(store
            .save_setup_profile("Cafe%20One", "owner_52", "owner@example.com")
            .is_err());
        assert_eq!(store.config, previous);
    }

    #[test]
    fn onboarding_profile_accepts_wordpress_preserving_plus_email() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_owned()).unwrap();
        store
            .save_setup_profile("Store", "owner_52", "owner+tag@example.com")
            .unwrap();
        assert_eq!(
            store.config.setup_admin_email.as_deref(),
            Some("owner+tag@example.com")
        );
    }

    #[test]
    fn startup_view_setting_persists_without_changing_store_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mut store = Store::open(root.clone()).unwrap();
        let original_name = store.config.store_name.clone();
        let original_bind_host = store.config.bind_host.clone();
        store.save_startup_view(StartupView::Diagnostics).unwrap();
        assert_eq!(store.config.startup_view, StartupView::Diagnostics);
        assert_eq!(store.config.store_name, original_name);
        assert_eq!(store.config.bind_host, original_bind_host);
        drop(store);

        let reopened = Store::open(root).unwrap();
        assert_eq!(reopened.config.startup_view, StartupView::Diagnostics);
        assert_eq!(reopened.config.store_name, original_name);
        assert_eq!(reopened.config.bind_host, original_bind_host);
    }

    #[test]
    fn second_instance_cannot_write_until_first_exits() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().to_owned()).unwrap();
        assert!(Store::open(temp.path().to_owned()).is_err());
        drop(store);
        assert!(Store::open(temp.path().to_owned()).is_ok());
    }
}
