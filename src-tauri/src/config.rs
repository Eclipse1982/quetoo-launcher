use crate::error::{LauncherError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub install_dir: Option<PathBuf>,
    /// Tag of the official jdolan/quetoo base install currently on disk.
    pub installed_version: Option<String>,
    #[serde(default)]
    pub bundle_installed: bool,
    /// Tag of the Eclipse1982/quetoo RailWarz overlay applied on top of the base
    /// install. `None` means no overlay has been applied yet.
    #[serde(default)]
    pub railwarz_version: Option<String>,
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
            railwarz_version: Some("railwarz-v1".into()),
        };
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, cfg);
    }
}
