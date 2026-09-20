use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::{is_owned_temp_profile_name, USER_WINDOW_PROFILE_PREFIX};

pub const ENV_CHROME_USER_DATA_DIR: &str = "OVERLEAF_SWITCHER_CHROME_USER_DATA_DIR";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChromeProfileDiscovery {
    pub user_data_dir: Option<String>,
    pub local_state_path: Option<String>,
    pub local_state_present: bool,
    pub profiles: Vec<ChromeProfileSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChromeProfileSummary {
    pub name: String,
    pub display_name: String,
    pub email: Option<String>,
    pub profile_dir: String,
    pub cookie_store_present: bool,
}

pub fn chrome_user_data_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = env::var(ENV_CHROME_USER_DATA_DIR) {
        if !path.trim().is_empty() {
            candidates.push(PathBuf::from(path));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Google")
                    .join("Chrome")
                    .join("User Data"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = env::var("HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("Google")
                    .join("Chrome"),
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(home) = env::var("HOME") {
            let config = PathBuf::from(home).join(".config");
            candidates.push(config.join("google-chrome"));
            candidates.push(config.join("chromium"));
        }
    }

    dedupe_paths(candidates)
}

pub fn default_chrome_user_data_dir() -> Option<PathBuf> {
    let candidates = chrome_user_data_dir_candidates();
    candidates
        .iter()
        .find(|path| path.join("Local State").is_file())
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

pub fn discover_chrome_profiles_from_environment() -> ChromeProfileDiscovery {
    let Some(user_data_dir) = default_chrome_user_data_dir() else {
        return ChromeProfileDiscovery {
            user_data_dir: None,
            local_state_path: None,
            local_state_present: false,
            profiles: Vec::new(),
            error: Some("Chrome user data directory was not found from environment".to_string()),
        };
    };

    match discover_chrome_profiles(&user_data_dir) {
        Ok(discovery) => discovery,
        Err(error) => ChromeProfileDiscovery {
            user_data_dir: Some(path_to_string(&user_data_dir)),
            local_state_path: Some(path_to_string(&user_data_dir.join("Local State"))),
            local_state_present: user_data_dir.join("Local State").is_file(),
            profiles: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

pub fn discover_chrome_profiles(
    user_data_dir: impl AsRef<Path>,
) -> io::Result<ChromeProfileDiscovery> {
    let user_data_dir = user_data_dir.as_ref();
    let local_state_path = user_data_dir.join("Local State");
    if !local_state_path.is_file() {
        return Ok(ChromeProfileDiscovery {
            user_data_dir: Some(path_to_string(user_data_dir)),
            local_state_path: Some(path_to_string(&local_state_path)),
            local_state_present: false,
            profiles: Vec::new(),
            error: None,
        });
    }

    let local_state = fs::read_to_string(&local_state_path)?;
    let value: Value = serde_json::from_str(&local_state)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let info_cache = value
        .get("profile")
        .and_then(|profile| profile.get("info_cache"))
        .and_then(Value::as_object);

    let mut profiles = Vec::new();
    if let Some(info_cache) = info_cache {
        for (name, info) in info_cache {
            let profile_dir = user_data_dir.join(name);
            if !profile_dir.is_dir() || is_non_switchable_profile_name(name) {
                continue;
            }
            let display_name = info
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(name)
                .to_string();
            let email = info
                .get("user_name")
                .or_else(|| info.get("gaia_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let is_default_named = info
                .get("is_using_default_name")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if name != "Default"
                && email.is_none()
                && (is_default_named || is_generic_chrome_profile_name(&display_name))
            {
                continue;
            }
            let cookie_store_present = profile_dir.join("Network").join("Cookies").is_file()
                || profile_dir.join("Cookies").is_file();

            profiles.push(ChromeProfileSummary {
                name: name.to_string(),
                display_name,
                email,
                profile_dir: path_to_string(&profile_dir),
                cookie_store_present,
            });
        }
    }
    profiles.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(ChromeProfileDiscovery {
        user_data_dir: Some(path_to_string(user_data_dir)),
        local_state_path: Some(path_to_string(&local_state_path)),
        local_state_present: true,
        profiles,
        error: None,
    })
}

fn is_generic_chrome_profile_name(display_name: &str) -> bool {
    matches!(
        display_name.trim().to_ascii_lowercase().as_str(),
        "您的 chrome" | "您的 chrome profile" | "your chrome" | "your chrome profile"
    )
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }
    unique
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn is_non_switchable_profile_name(name: &str) -> bool {
    is_owned_temp_profile_name(name) || name.starts_with(USER_WINDOW_PROFILE_PREFIX)
}
