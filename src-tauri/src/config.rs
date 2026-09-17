use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub schema_version: u32,
    pub store_name: String,
    pub bind_host: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            store_name: "My Coffee".into(),
            bind_host: "127.0.0.1".into(),
        }
    }
}

impl AppConfig {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
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
    fn second_instance_cannot_write_until_first_exits() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().to_owned()).unwrap();
        assert!(Store::open(temp.path().to_owned()).is_err());
        drop(store);
        assert!(Store::open(temp.path().to_owned()).is_ok());
    }
}
