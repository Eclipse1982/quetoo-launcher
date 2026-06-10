mod config;
mod error;
mod github;
mod installer;
mod launcher;
mod qconfig;
mod snapshot;

use config::Config;
use error::Result;
use github::{determine_state, fetch_latest_release, select_asset, AssetKind, InstallState};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const CONFIG_FILE: &str = "config.json";

fn config_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| error::LauncherError::Config(e.to_string()))?;
    Ok(dir.join(CONFIG_FILE))
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("failed to build reqwest client")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusDto {
    install_dir: Option<String>,
    default_install_dir: Option<String>,
    latest_version: String,
    state: InstallState,
    can_rollback: bool,
}

/// Check GitHub and return the current launcher status.
#[tauri::command]
async fn get_status(app: AppHandle) -> std::result::Result<StatusDto, error::LauncherError> {
    let cfg = Config::load(&config_path(&app)?)?;
    let release = fetch_latest_release(&http_client()).await?;
    let state = determine_state(&cfg, &release.tag_name);
    let can_rollback = match &cfg.install_dir {
        // Degrade to false on a bad manifest — status must always render.
        Some(dir) => snapshot::load_manifest(dir).ok().flatten().is_some(),
        None => false,
    };
    Ok(StatusDto {
        install_dir: cfg.install_dir.map(|p| p.to_string_lossy().into_owned()),
        default_install_dir: config::default_install_dir(std::env::consts::OS)
            .map(|p| p.to_string_lossy().into_owned()),
        latest_version: release.tag_name,
        state,
        can_rollback,
    })
}

/// Persist the user-chosen install directory.
#[tauri::command]
async fn set_install_dir(
    app: AppHandle,
    dir: String,
) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    cfg.install_dir = Some(PathBuf::from(dir));
    cfg.save(&path)?;
    Ok(())
}

/// Pre-flight guard: on non-macOS, check that the game executable is not
/// currently held open (which would cause cryptic os error 32 mid-extract).
/// Returns `Launch` error if the game appears to be running.
fn ensure_game_not_running(install_dir: &std::path::Path, os: &str) -> Result<()> {
    if os == "macos" {
        return Ok(());
    }
    let target = launcher::executable_path(install_dir, os)?;
    if target.exists()
        && std::fs::OpenOptions::new().write(true).open(&target).is_err()
    {
        return Err(error::LauncherError::Launch(
            "Quetoo appears to be running — close the game and try again".into(),
        ));
    }
    Ok(())
}

/// Install or update official Quetoo. First install downloads the full
/// bundle (engine + game data); afterwards the small update asset.
/// Updates snapshot the files they overwrite so they can be rolled back.
#[tauri::command]
async fn install_or_update(app: AppHandle) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    let install_dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| error::LauncherError::Config("no install directory set".into()))?;

    let client = http_client();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let release = fetch_latest_release(&client).await?;

    // Bundle if we've never installed one; otherwise a small update.
    let kind = if cfg.bundle_installed {
        AssetKind::Update
    } else {
        AssetKind::Bundle
    };
    let asset = select_asset(&release, os, arch, kind)?.clone();

    // Pre-flight: on non-macOS, refuse to update if the executable is locked
    // (Windows holds running executables open). Checked before any download so
    // nothing is touched on failure.
    if kind == AssetKind::Update {
        ensure_game_not_running(&install_dir, os)?;
    }

    std::fs::create_dir_all(&install_dir)?;
    let tmp = install_dir.join(format!(".{}", asset.name));
    installer::download_asset(&app, &client, &asset, &tmp).await?;
    let format = installer::detect_format(&asset.name)?;

    // Snapshot before updates only — a failed snapshot aborts the update.
    if kind == AssetKind::Update {
        if snapshot::has_snapshot_for(&install_dir, &release.tag_name) {
            // A prior attempt already captured the pre-update state for this
            // version transition. Reuse it — re-snapshotting after a failed
            // mid-extract would capture a mixed/new tree as "old".
            installer::emit_progress(&app, "snapshot", 100, "Reusing existing backup".into());
        } else {
            installer::emit_progress(&app, "snapshot", 0, "Backing up current version".into());
            let from = cfg.installed_version.clone().unwrap_or_else(|| "unknown".into());
            let snap_result = if os == "macos" {
                snapshot::create_snapshot_macos(&install_dir, &from, &release.tag_name).map(|_| ())
            } else {
                installer::list_entries(&tmp, format).and_then(|entries| {
                    snapshot::create_snapshot(&install_dir, &entries, &from, &release.tag_name)
                        .map(|_| ())
                })
            };
            if let Err(e) = snap_result {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            installer::emit_progress(&app, "snapshot", 100, "Backup complete".into());
        }
    }

    let app2 = app.clone();
    let extract_result =
        installer::extract_archive(&tmp, format, &install_dir, &mut |done, total| {
            let percent = installer::percent(done as u64, total as u64);
            let detail = if total == 0 {
                "Preparing\u{2026}".to_string()
            } else {
                format!("{done}/{total} files")
            };
            installer::emit_progress(&app2, "extract", percent, detail);
        });
    let _ = std::fs::remove_file(&tmp);
    extract_result?;

    // Verify the launch target exists before declaring success.
    installer::emit_progress(&app, "verify", 50, "Verifying installation".into());
    let target = launcher::executable_path(&install_dir, os)?;
    if !target.exists() {
        return Err(error::LauncherError::Extract(format!(
            "install incomplete: {} missing",
            target.display()
        )));
    }
    installer::emit_progress(&app, "verify", 100, "Done".into());

    if kind == AssetKind::Bundle {
        cfg.bundle_installed = true;
    }
    cfg.installed_version = Some(release.tag_name);
    cfg.save(&path)?;
    Ok(())
}

/// Restore the previous version from the pre-update snapshot.
#[tauri::command]
async fn rollback_update(app: AppHandle) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    let install_dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| error::LauncherError::Config("no install directory set".into()))?;
    let from_version = snapshot::rollback(&install_dir)?;
    cfg.installed_version = Some(from_version);
    cfg.save(&path)?;
    Ok(())
}

/// Wipe the install directory and re-download the full bundle.
#[tauri::command]
async fn reinstall(app: AppHandle) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    let install_dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| error::LauncherError::Config("no install directory set".into()))?;

    let os = std::env::consts::OS;
    ensure_game_not_running(&install_dir, os)?;

    if !installer::looks_like_quetoo_install(&install_dir)? {
        return Err(error::LauncherError::Config(format!(
            "{} doesn't look like a Quetoo install; refusing to delete it",
            install_dir.display()
        )));
    }
    if install_dir.exists() {
        for entry in std::fs::read_dir(&install_dir)? {
            let p = entry?.path();
            if p.is_dir() {
                std::fs::remove_dir_all(&p)?;
            } else {
                std::fs::remove_file(&p)?;
            }
        }
    }
    cfg.installed_version = None;
    cfg.bundle_installed = false;
    cfg.save(&path)?;
    install_or_update(app).await
}

/// Launch the installed game.
#[tauri::command]
async fn play(app: AppHandle) -> std::result::Result<(), error::LauncherError> {
    let cfg = Config::load(&config_path(&app)?)?;
    let install_dir = cfg
        .install_dir
        .ok_or_else(|| error::LauncherError::Launch("no install directory set".into()))?;
    launcher::launch(&install_dir)
}

/// Read the curated Quetoo settings from autoexec.cfg.
#[tauri::command]
async fn get_quetoo_settings() -> std::result::Result<qconfig::Settings, error::LauncherError> {
    qconfig::load_settings()
}

/// Write the curated Quetoo settings to autoexec.cfg (preserving other lines).
#[tauri::command]
async fn save_quetoo_settings(
    settings: qconfig::Settings,
) -> std::result::Result<(), error::LauncherError> {
    qconfig::save_settings(&settings)
}

/// Return the documented default settings (for the UI's "reset").
#[tauri::command]
fn default_quetoo_settings() -> qconfig::Settings {
    qconfig::Settings::defaults()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            get_status,
            set_install_dir,
            install_or_update,
            rollback_update,
            reinstall,
            play,
            get_quetoo_settings,
            save_quetoo_settings,
            default_quetoo_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
