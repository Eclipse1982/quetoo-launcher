use crate::config::Config;
use crate::error::{LauncherError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Bundle,
    Update,
}

/// Build the exact asset file name for the given platform and kind.
pub fn expected_asset_name(os: &str, arch: &str, kind: AssetKind) -> Result<String> {
    let (token, ext) = match (os, arch) {
        ("windows", "x86_64") => ("x86_64-pc-windows", "zip"),
        ("linux", "x86_64") => ("x86_64-pc-linux", "tar.gz"),
        ("linux", "aarch64") => ("aarch64-pc-linux", "tar.gz"),
        ("macos", _) => ("universal-apple-darwin", "dmg"),
        _ => return Err(LauncherError::UnsupportedPlatform(format!("{os}/{arch}"))),
    };
    let prefix = match kind {
        AssetKind::Bundle => "quetoo-bundle-",
        AssetKind::Update => "quetoo-",
    };
    Ok(format!("{prefix}{token}.{ext}"))
}

/// Find the asset matching the current platform and kind in a release.
pub fn select_asset<'a>(
    release: &'a Release,
    os: &str,
    arch: &str,
    kind: AssetKind,
) -> Result<&'a Asset> {
    let wanted = expected_asset_name(os, arch, kind)?;
    release
        .assets
        .iter()
        .find(|a| a.name == wanted)
        .ok_or(LauncherError::AssetNotFound(wanted))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum InstallState {
    NotInstalled,
    UpdateAvailable { from: String, to: String },
    UpToDate,
}

/// Decide what the launcher should do given current config and the latest tag.
pub fn determine_state(cfg: &Config, latest_tag: &str) -> InstallState {
    match (&cfg.install_dir, cfg.bundle_installed, &cfg.installed_version) {
        (Some(_), true, Some(installed)) if installed == latest_tag => InstallState::UpToDate,
        (Some(_), true, Some(installed)) => InstallState::UpdateAvailable {
            from: installed.clone(),
            to: latest_tag.to_string(),
        },
        _ => InstallState::NotInstalled,
    }
}

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/jdolan/quetoo/releases/latest";

/// Fetch the latest Quetoo release from the GitHub API.
pub async fn fetch_latest_release(client: &reqwest::Client) -> Result<Release> {
    let resp = client
        .get(LATEST_RELEASE_URL)
        .header("User-Agent", "quetoo-launcher")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| LauncherError::Network(e.to_string()))?;

    if resp.status().as_u16() == 403 {
        return Err(LauncherError::RateLimit);
    }
    if !resp.status().is_success() {
        return Err(LauncherError::Network(format!("HTTP {}", resp.status())));
    }
    resp.json::<Release>()
        .await
        .map_err(|e| LauncherError::Network(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;

    fn sample_release() -> Release {
        let names = [
            "quetoo-bundle-x86_64-pc-windows.zip",
            "quetoo-x86_64-pc-windows.zip",
            "quetoo-bundle-x86_64-pc-linux.tar.gz",
            "quetoo-x86_64-pc-linux.tar.gz",
            "quetoo-x86_64-pc-linux.deb",
            "quetoo-bundle-universal-apple-darwin.dmg",
            "quetoo-universal-apple-darwin.dmg",
        ];
        Release {
            tag_name: "v1.0.25".into(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: n.to_string(),
                    browser_download_url: format!("https://example/{n}"),
                    size: 100,
                })
                .collect(),
        }
    }

    fn installed_config(version: &str) -> Config {
        Config {
            install_dir: Some(PathBuf::from("/games/quetoo")),
            installed_version: Some(version.into()),
            bundle_installed: true,
        }
    }

    #[test]
    fn windows_bundle_and_update_names() {
        assert_eq!(
            expected_asset_name("windows", "x86_64", AssetKind::Bundle).unwrap(),
            "quetoo-bundle-x86_64-pc-windows.zip"
        );
        assert_eq!(
            expected_asset_name("windows", "x86_64", AssetKind::Update).unwrap(),
            "quetoo-x86_64-pc-windows.zip"
        );
    }

    #[test]
    fn linux_update_picks_targz_not_deb() {
        let r = sample_release();
        let a = select_asset(&r, "linux", "x86_64", AssetKind::Update).unwrap();
        assert_eq!(a.name, "quetoo-x86_64-pc-linux.tar.gz");
    }

    #[test]
    fn macos_uses_universal_dmg() {
        let r = sample_release();
        let a = select_asset(&r, "macos", "aarch64", AssetKind::Bundle).unwrap();
        assert_eq!(a.name, "quetoo-bundle-universal-apple-darwin.dmg");
    }

    #[test]
    fn unsupported_platform_errors() {
        let err = expected_asset_name("freebsd", "x86_64", AssetKind::Bundle);
        assert!(matches!(err, Err(LauncherError::UnsupportedPlatform(_))));
    }

    #[test]
    fn missing_asset_errors() {
        let mut r = sample_release();
        r.assets.clear();
        let err = select_asset(&r, "windows", "x86_64", AssetKind::Bundle);
        assert!(matches!(err, Err(LauncherError::AssetNotFound(_))));
    }

    #[test]
    fn not_installed_when_no_dir() {
        let cfg = Config::default();
        assert_eq!(determine_state(&cfg, "v1.0.25"), InstallState::NotInstalled);
    }

    #[test]
    fn not_installed_when_bundle_flag_false() {
        let cfg = Config {
            install_dir: Some(PathBuf::from("/games/quetoo")),
            installed_version: Some("v1.0.25".into()),
            bundle_installed: false,
        };
        assert_eq!(determine_state(&cfg, "v1.0.25"), InstallState::NotInstalled);
    }

    #[test]
    fn up_to_date_when_versions_match() {
        let cfg = installed_config("v1.0.25");
        assert_eq!(determine_state(&cfg, "v1.0.25"), InstallState::UpToDate);
    }

    #[test]
    fn update_available_when_versions_differ() {
        let cfg = installed_config("v1.0.24");
        assert_eq!(
            determine_state(&cfg, "v1.0.25"),
            InstallState::UpdateAvailable { from: "v1.0.24".into(), to: "v1.0.25".into() }
        );
    }
}
