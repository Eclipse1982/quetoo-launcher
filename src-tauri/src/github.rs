use crate::config::{Channel, Config};
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
    #[serde(default)]
    pub published_at: String,
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

/// Identity string used to detect "is a new build available" for a channel.
/// Stable uses the immutable tag; the snapshot's tag is always `latest`, so it
/// is identified by its publish timestamp instead.
pub fn latest_identity(channel: Channel, release: &Release) -> String {
    match channel {
        Channel::Stable => release.tag_name.clone(),
        Channel::PreRelease => format!("snapshot-{}", release.published_at),
    }
}

/// Human-friendly rendering of an identity for the UI. Stable tags pass
/// through; `snapshot-<ts>` becomes `Snapshot (YYYY-MM-DD HH:MM)`.
pub fn display_version(identity: &str) -> String {
    match identity.strip_prefix("snapshot-") {
        Some(ts) => {
            let pretty = ts
                .get(0..16)
                .map(|s| s.replace('T', " "))
                .unwrap_or_else(|| ts.to_string());
            format!("Snapshot ({pretty})")
        }
        None => identity.to_string(),
    }
}

/// Whether the Snapshot pre-release ships an engine asset for this platform.
/// Upstream only builds the snapshot for Windows and Linux x86_64.
pub fn snapshot_available(os: &str, arch: &str) -> bool {
    matches!((os, arch), ("windows", "x86_64") | ("linux", "x86_64"))
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
const SNAPSHOT_RELEASE_URL: &str =
    "https://api.github.com/repos/jdolan/quetoo/releases/tags/latest";

/// Fetch a release document from the GitHub API.
async fn fetch_release(client: &reqwest::Client, url: &str) -> Result<Release> {
    let resp = client
        .get(url)
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

/// Fetch the latest stable Quetoo release.
pub async fn fetch_latest_release(client: &reqwest::Client) -> Result<Release> {
    fetch_release(client, LATEST_RELEASE_URL).await
}

/// Fetch the rolling "Quetoo Snapshot" pre-release (tag `latest`).
pub async fn fetch_snapshot_release(client: &reqwest::Client) -> Result<Release> {
    fetch_release(client, SNAPSHOT_RELEASE_URL).await
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
            published_at: "2026-05-28T13:53:50Z".into(),
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
            favorites: vec![],
            channel: Channel::Stable,
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
    fn latest_identity_stable_is_tag() {
        let r = sample_release();
        assert_eq!(latest_identity(Channel::Stable, &r), "v1.0.25");
    }

    #[test]
    fn latest_identity_prerelease_is_snapshot_timestamp() {
        let mut r = sample_release();
        r.tag_name = "latest".into();
        r.published_at = "2026-06-11T15:37:15Z".into();
        assert_eq!(
            latest_identity(Channel::PreRelease, &r),
            "snapshot-2026-06-11T15:37:15Z"
        );
    }

    #[test]
    fn display_version_passes_through_stable_tag() {
        assert_eq!(display_version("v1.0.46"), "v1.0.46");
    }

    #[test]
    fn display_version_formats_snapshot() {
        assert_eq!(
            display_version("snapshot-2026-06-11T15:37:15Z"),
            "Snapshot (2026-06-11 15:37)"
        );
    }

    #[test]
    fn display_version_handles_short_snapshot_timestamp() {
        // Defensive: never panic on an unexpected/short timestamp.
        assert_eq!(display_version("snapshot-2026"), "Snapshot (2026)");
    }

    #[test]
    fn snapshot_available_on_win_and_linux_x86_64() {
        assert!(snapshot_available("windows", "x86_64"));
        assert!(snapshot_available("linux", "x86_64"));
    }

    #[test]
    fn snapshot_unavailable_on_macos_and_linux_arm() {
        assert!(!snapshot_available("macos", "aarch64"));
        assert!(!snapshot_available("macos", "x86_64"));
        assert!(!snapshot_available("linux", "aarch64"));
    }

    #[test]
    fn up_to_date_when_installed_matches_snapshot_identity() {
        let cfg = installed_config("snapshot-2026-06-11T15:37:15Z");
        assert_eq!(
            determine_state(&cfg, "snapshot-2026-06-11T15:37:15Z"),
            InstallState::UpToDate
        );
    }

    #[test]
    fn switching_channel_reports_update_available() {
        // Stable v1.0.46 is installed; active channel is now Pre-Release whose
        // identity differs, so the launcher should offer the overlay update.
        let cfg = installed_config("v1.0.46");
        assert_eq!(
            determine_state(&cfg, "snapshot-2026-06-11T15:37:15Z"),
            InstallState::UpdateAvailable {
                from: "v1.0.46".into(),
                to: "snapshot-2026-06-11T15:37:15Z".into(),
            }
        );
    }

    #[test]
    fn newer_snapshot_reports_update_available() {
        let cfg = installed_config("snapshot-2026-06-10T10:00:00Z");
        assert_eq!(
            determine_state(&cfg, "snapshot-2026-06-11T15:37:15Z"),
            InstallState::UpdateAvailable {
                from: "snapshot-2026-06-10T10:00:00Z".into(),
                to: "snapshot-2026-06-11T15:37:15Z".into(),
            }
        );
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
            favorites: vec![],
            channel: Channel::Stable,
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
