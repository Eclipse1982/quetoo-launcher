use crate::error::{LauncherError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub install_dir: Option<PathBuf>,
    pub installed_version: Option<String>,
    #[serde(default)]
    pub bundle_installed: bool,
    #[serde(default)]
    pub favorites: Vec<String>,
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

/// Pure path builder for the platform default install directory.
/// `env` returns the value of an environment variable, if set.
fn default_install_dir_from_env(os: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    match os {
        "windows" => Some(PathBuf::from(r"C:\Games\Quetoo")),
        "linux" => env("HOME")
            .map(|h| PathBuf::from(h).join(".local").join("share").join("quetoo")),
        "macos" => env("HOME").map(|h| PathBuf::from(h).join("Applications")),
        _ => None,
    }
}

/// Platform default install directory, offered when no install_dir is configured.
/// The user can always pick a different folder; this is only the pre-filled value.
pub fn default_install_dir(os: &str) -> Option<PathBuf> {
    default_install_dir_from_env(os, &|k| std::env::var(k).ok())
}

/// Canonicalize a user-entered favorite server address: trim, append the
/// default Quetoo port (:1998) when none is given, and require a parseable
/// IPv4 socket address (hostnames are rejected for now).
pub fn normalize_favorite(input: &str) -> Result<String> {
    let s = input.trim();
    let candidate = if s.contains(':') { s.to_string() } else { format!("{s}:1998") };
    candidate
        .parse::<std::net::SocketAddrV4>()
        .map(|a| a.to_string())
        .map_err(|_| LauncherError::Config(format!("not a valid server address: {input}")))
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
            favorites: vec![],
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

    #[test]
    fn default_install_dir_linux_from_home() {
        let env = |k: &str| (k == "HOME").then(|| "/home/j".to_string());
        assert_eq!(
            default_install_dir_from_env("linux", &env).unwrap(),
            PathBuf::from("/home/j").join(".local").join("share").join("quetoo")
        );
    }

    #[test]
    fn default_install_dir_macos_from_home() {
        let env = |k: &str| (k == "HOME").then(|| "/Users/j".to_string());
        assert_eq!(
            default_install_dir_from_env("macos", &env).unwrap(),
            PathBuf::from("/Users/j").join("Applications")
        );
    }

    #[test]
    fn default_install_dir_linux_no_home_is_none() {
        let env = |_k: &str| None;
        assert!(default_install_dir_from_env("linux", &env).is_none());
    }

    #[test]
    fn default_install_dir_macos_no_home_is_none() {
        let env = |_k: &str| None;
        assert!(default_install_dir_from_env("macos", &env).is_none());
    }

    #[test]
    fn default_install_dir_windows_ignores_env() {
        // Windows branch is a constant — env is never consulted.
        let env = |_k: &str| None;
        assert_eq!(
            default_install_dir_from_env("windows", &env).unwrap(),
            PathBuf::from(r"C:\Games\Quetoo")
        );
    }

    #[test]
    fn default_install_dir_freebsd_is_none_from_env() {
        let env = |_k: &str| None;
        assert!(default_install_dir_from_env("freebsd", &env).is_none());
    }

    // --- favorites field tests ---

    #[test]
    fn load_old_config_without_favorites_defaults_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // JSON that predates the favorites field — must still load cleanly.
        std::fs::write(
            &path,
            r#"{"install_dir":null,"installed_version":null,"bundle_installed":false}"#,
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.favorites, Vec::<String>::new());
    }

    #[test]
    fn roundtrip_with_favorites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config {
            install_dir: None,
            installed_version: None,
            bundle_installed: false,
            favorites: vec!["1.2.3.4:1998".to_string()],
        };
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.favorites, vec!["1.2.3.4:1998"]);
        assert_eq!(loaded, cfg);
    }

    // --- normalize_favorite tests ---

    #[test]
    fn normalize_bare_ip_appends_default_port() {
        assert_eq!(
            normalize_favorite("144.202.77.138").unwrap(),
            "144.202.77.138:1998"
        );
    }

    #[test]
    fn normalize_ip_with_port_trims_whitespace() {
        assert_eq!(
            normalize_favorite(" 1.2.3.4:27910 ").unwrap(),
            "1.2.3.4:27910"
        );
    }

    #[test]
    fn normalize_garbage_returns_err() {
        assert!(normalize_favorite("garbage").is_err());
    }

    #[test]
    fn normalize_hostname_returns_err() {
        assert!(normalize_favorite("play.example.com").is_err());
    }

    #[test]
    fn normalize_non_numeric_port_returns_err() {
        assert!(normalize_favorite("1.2.3.4:notaport").is_err());
    }
}
