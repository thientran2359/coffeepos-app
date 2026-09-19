use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub const APP_CONFIG_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupView {
    #[default]
    Home,
    Settings,
    Diagnostics,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppLanguage {
    #[default]
    Vi,
    En,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    LocalOnly,
    Lan,
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
    pub app_language: Option<AppLanguage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_admin_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_admin_email: Option<String>,
    #[serde(default)]
    pub network_mode: NetworkMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_adapter_id: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: APP_CONFIG_SCHEMA_VERSION,
            store_name: "My Coffee".into(),
            bind_host: "127.0.0.1".into(),
            startup_view: StartupView::Home,
            app_language: None,
            setup_admin_username: None,
            setup_admin_email: None,
            network_mode: NetworkMode::LocalOnly,
            lan_adapter_id: None,
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
                "bind_host is an internal compatibility field and must remain 127.0.0.1. Use network_mode for LAN access."
                    .into(),
            );
        }
        match self.network_mode {
            NetworkMode::LocalOnly if self.lan_adapter_id.is_some() => {
                return Err("lan_adapter_id must be empty while network_mode is local_only.".into());
            }
            NetworkMode::Lan => {
                if self
                    .lan_adapter_id
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty() || value.len() > 128)
                {
                    return Err("lan_adapter_id must be a stable non-empty adapter identity.".into());
                }
            }
            NetworkMode::LocalOnly => {}
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

fn invalid_config_error() -> String {
    "Cannot read config/app.json: invalid configuration. The file has been preserved. Correct it or restore a known-good copy, then retry."
        .to_string()
}

struct DecodedConfig {
    config: AppConfig,
    migrated: bool,
}

fn decode_config(bytes: &[u8]) -> Result<DecodedConfig, String> {
    let mut value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| invalid_config_error())?;
    if let Some(language) = value.get("app_language") {
        if !language.is_null() && !matches!(language.as_str(), Some("vi" | "en")) {
            return Err(
                "Cannot read config/app.json: unsupported app_language. Use \"vi\" or \"en\". The file has been preserved."
                    .to_string(),
            );
        }
    }
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(invalid_config_error)?;
    let migrated = match schema_version {
        1 => {
            if value.get("bind_host").and_then(serde_json::Value::as_str) != Some("127.0.0.1") {
                return Err(invalid_config_error());
            }
            let object = value.as_object_mut().ok_or_else(invalid_config_error)?;
            object.insert(
                "schema_version".into(),
                serde_json::Value::from(APP_CONFIG_SCHEMA_VERSION),
            );
            object.insert(
                "network_mode".into(),
                serde_json::Value::String("local_only".into()),
            );
            object.remove("lan_adapter_id");
            true
        }
        version if version == APP_CONFIG_SCHEMA_VERSION as u64 => false,
        _ => return Err(invalid_config_error()),
    };
    let config: AppConfig = serde_json::from_value(value).map_err(|_| invalid_config_error())?;
    config.validate()?;
    Ok(DecodedConfig { config, migrated })
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

pub fn read_app_language(root: &Path) -> Result<Option<AppLanguage>, String> {
    let path = root.join("config/app.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(disk_error("read configuration", error)),
    };
    let decoded = decode_config(&bytes)?;
    Ok(decoded.config.app_language)
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
        let (config, migrated) = match fs::read(&path) {
            Ok(bytes) => {
                let decoded = decode_config(&bytes)?;
                (decoded.config, decoded.migrated)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let config = AppConfig::default();
                persist(&path, &config)?;
                (config, false)
            }
            Err(e) => return Err(disk_error("read configuration", e)),
        };
        config.validate()?;
        if migrated {
            persist(&path, &config)?;
        }
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
        self.save_desktop_preferences(startup_view, self.config.app_language)
    }

    pub fn save_desktop_preferences(
        &mut self,
        startup_view: StartupView,
        app_language: Option<AppLanguage>,
    ) -> Result<(), String> {
        let next = AppConfig {
            startup_view,
            app_language,
            ..self.config.clone()
        };
        persist(&self.root.join("config/app.json"), &next)?;
        self.config = next;
        self.log("desktop application settings saved");
        Ok(())
    }

    pub fn save_app_language(&mut self, app_language: AppLanguage) -> Result<(), String> {
        self.save_desktop_preferences(self.config.startup_view.clone(), Some(app_language))
    }

    pub fn save_network_preference(
        &mut self,
        network_mode: NetworkMode,
        lan_adapter_id: Option<String>,
    ) -> Result<(), String> {
        let next = AppConfig {
            network_mode,
            lan_adapter_id,
            bind_host: "127.0.0.1".into(),
            ..self.config.clone()
        };
        persist(&self.root.join("config/app.json"), &next)?;
        self.config = next;
        self.log("desktop network preference saved");
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
            r#"{"schema_version":3,"store_name":"Shop","bind_host":"127.0.0.1"}"#,
            r#"{"schema_version":1,"store_name":"Shop","bind_host":"192.168.1.20"}"#,
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
        assert!(reopened.config.app_language.is_none());
        assert!(reopened.config.setup_admin_username.is_none());
        assert!(reopened.config.setup_admin_email.is_none());
        assert_eq!(reopened.config.network_mode, NetworkMode::LocalOnly);
        assert!(reopened.config.lan_adapter_id.is_none());
        let migrated = fs::read_to_string(path).unwrap();
        assert!(migrated.contains("\"schema_version\": 2"));
        assert!(migrated.contains("\"network_mode\": \"local_only\""));
    }

    #[test]
    fn app_language_persists_without_changing_existing_profile_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mut store = Store::open(root.clone()).unwrap();
        store
            .save_setup_profile("Coffee & \"Co\" Café", "owner_52", "owner52@example.com")
            .unwrap();
        store.save_startup_view(StartupView::Diagnostics).unwrap();
        store.save_app_language(AppLanguage::En).unwrap();

        assert_eq!(store.config.app_language, Some(AppLanguage::En));
        assert_eq!(store.config.startup_view, StartupView::Diagnostics);
        assert_eq!(store.config.store_name, "Coffee & \"Co\" Café");
        assert_eq!(
            store.config.setup_admin_username.as_deref(),
            Some("owner_52")
        );
        drop(store);

        let reopened = Store::open(root.clone()).unwrap();
        assert_eq!(reopened.config.app_language, Some(AppLanguage::En));
        assert_eq!(read_app_language(&root).unwrap(), Some(AppLanguage::En));
        let json = fs::read_to_string(root.join("config/app.json")).unwrap();
        assert!(json.contains("\"schema_version\": 2"));
        assert!(json.contains("\"app_language\": \"en\""));
    }

    #[test]
    fn network_preference_is_atomic_and_keeps_bind_host_internal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mut store = Store::open(root.clone()).unwrap();
        store
            .save_network_preference(NetworkMode::Lan, Some("adapter-001".into()))
            .unwrap();
        assert_eq!(store.config.network_mode, NetworkMode::Lan);
        assert_eq!(store.config.lan_adapter_id.as_deref(), Some("adapter-001"));
        assert_eq!(store.config.bind_host, "127.0.0.1");
        drop(store);

        let reopened = Store::open(root).unwrap();
        assert_eq!(reopened.config.network_mode, NetworkMode::Lan);
        assert_eq!(reopened.config.lan_adapter_id.as_deref(), Some("adapter-001"));
        assert_eq!(reopened.config.bind_host, "127.0.0.1");
    }

    #[test]
    fn unsupported_app_language_is_rejected_without_rewriting_legacy_config() {
        let temp = tempfile::tempdir().unwrap();
        drop(Store::open(temp.path().to_owned()).unwrap());
        let path = temp.path().join("config/app.json");
        let bytes = r#"{"schema_version":1,"store_name":"Legacy Coffee","bind_host":"127.0.0.1","app_language":"fr"}"#;
        fs::write(&path, bytes).unwrap();

        let error = match Store::open(temp.path().to_owned()) {
            Ok(_) => panic!("unsupported locale unexpectedly opened"),
            Err(error) => error,
        };
        assert!(error.contains("app_language"));
        let language_error = read_app_language(temp.path()).unwrap_err();
        assert!(language_error.contains("app_language"));
        assert_eq!(fs::read_to_string(path).unwrap(), bytes);
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
