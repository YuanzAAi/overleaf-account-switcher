use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use overleaf_browser::ChromeProxyPolicy;
use serde::{Deserialize, Serialize};

pub mod batch;
pub mod dashboard;
pub mod profiles;
pub mod server;
pub mod tasks;
pub mod updates;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    pub data_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub workspace_dir: PathBuf,
    pub default_export_dir_override: Option<PathBuf>,
    pub browser_proxy_policy: ChromeProxyPolicy,
}

impl ServiceConfig {
    pub fn from_environment() -> Self {
        let layout = overleaf_storage::StorageLayout::from_environment();
        let data_dir = layout.data_dir;
        let tmp_dir = layout.tmp_dir;
        let workspace_dir = std::env::var("OVERLEAF_SWITCHER_WORKSPACE_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let runtime_settings =
            load_runtime_settings(&runtime_settings_path_in(&data_dir)).unwrap_or_default();
        let default_export_dir_override = runtime_settings
            .default_export_dir
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty());

        Self {
            data_dir,
            tmp_dir,
            workspace_dir,
            default_export_dir_override,
            browser_proxy_policy: runtime_settings.browser_proxy_policy,
        }
    }

    pub fn storage_layout(&self) -> overleaf_storage::StorageLayout {
        overleaf_storage::StorageLayout::new(self.data_dir.clone(), self.tmp_dir.clone())
    }

    pub fn accounts_path(&self) -> PathBuf {
        self.storage_layout().accounts_path()
    }

    pub fn cards_path(&self) -> PathBuf {
        self.storage_layout().cards_path()
    }

    pub fn addresses_path(&self) -> PathBuf {
        self.storage_layout().addresses_path()
    }

    pub fn sqlite_dir(&self) -> PathBuf {
        self.storage_layout().sqlite_dir()
    }

    pub fn default_export_dir(&self) -> PathBuf {
        self.default_export_dir_override
            .clone()
            .unwrap_or_else(|| self.storage_layout().exports_dir())
    }

    pub fn extension_dir(&self) -> PathBuf {
        self.workspace_dir.join("chrome_extension")
    }

    pub fn runtime_settings_path(&self) -> PathBuf {
        runtime_settings_path_in(&self.data_dir)
    }

    pub fn set_default_export_dir(&mut self, path: PathBuf) -> io::Result<()> {
        fs::create_dir_all(&path)?;
        fs::create_dir_all(&self.data_dir)?;
        let mut settings = load_runtime_settings(&self.runtime_settings_path()).unwrap_or_default();
        settings.default_export_dir = Some(path.to_string_lossy().to_string());
        save_runtime_settings(&self.runtime_settings_path(), &settings)?;
        self.default_export_dir_override = Some(path);
        Ok(())
    }

    pub fn set_browser_proxy_policy(&mut self, policy: ChromeProxyPolicy) -> io::Result<()> {
        fs::create_dir_all(&self.data_dir)?;
        let mut settings = load_runtime_settings(&self.runtime_settings_path()).unwrap_or_default();
        settings.browser_proxy_policy = policy.clone();
        save_runtime_settings(&self.runtime_settings_path(), &settings)?;
        self.browser_proxy_policy = policy;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct RuntimeSettings {
    default_export_dir: Option<String>,
    #[serde(default)]
    browser_proxy_policy: ChromeProxyPolicy,
}

fn runtime_settings_path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

fn load_runtime_settings(path: &Path) -> io::Result<RuntimeSettings> {
    if !path.exists() {
        return Ok(RuntimeSettings::default());
    }
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn save_runtime_settings(path: &Path, settings: &RuntimeSettings) -> io::Result<()> {
    overleaf_storage::save_json_atomic(path, settings)
}

pub fn health() -> &'static str {
    "ok"
}
