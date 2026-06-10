use crate::error::{LauncherError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub install_dir: Option<PathBuf>,
    pub installed_version: Option<String>,
    #[serde(default)]
    pub bundle_installed: bool,
}

impl Config {
    /// Load config from `path`. Returns `Config::default()` if the file does not exist.
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(|e| LauncherError::Config(e.to_string()))
    }

    /// Save config to `path`, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| LauncherError::Config(e.to_string()))?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// Platform default install directory, offered when no install_dir is configured.
/// The user can always pick a different folder; this is only the pre-filled value.
pub fn default_install_dir(os: &str) -> Option<PathBuf> {
    match os {
        "windows" => Some(PathBuf::from(r"C:\Games\Quetoo")),
        "linux" => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local").join("share").join("quetoo")),
        "macos" => std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.bundle_installed, false);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        let cfg = Config {
            install_dir: Some(PathBuf::from("/games/quetoo")),
            installed_version: Some("v1.0.25".into()),
            bundle_installed: true,
        };
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn default_install_dir_windows() {
        assert_eq!(
            default_install_dir("windows").unwrap(),
            PathBuf::from(r"C:\Games\Quetoo")
        );
    }

    #[test]
    fn default_install_dir_unsupported_is_none() {
        assert!(default_install_dir("freebsd").is_none());
    }
}
