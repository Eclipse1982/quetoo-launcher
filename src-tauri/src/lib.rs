mod config;
mod error;
mod github;
mod installer;
mod launcher;
mod qconfig;

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
    latest_version: String,
    state: InstallState,
}

/// Check GitHub and return the current launcher status.
#[tauri::command]
async fn get_status(app: AppHandle) -> std::result::Result<StatusDto, error::LauncherError> {
    let cfg = Config::load(&config_path(&app)?)?;
    let release = fetch_latest_release(&http_client()).await?;
    let state = determine_state(&cfg, &release.tag_name);
    Ok(StatusDto {
        install_dir: cfg.install_dir.map(|p| p.to_string_lossy().into_owned()),
        latest_version: release.tag_name,
        state,
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

/// Install (bundle) or update (small) Quetoo, then record the new version.
#[tauri::command]
async fn install_or_update(app: AppHandle) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    let install_dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| error::LauncherError::Config("no install directory set".into()))?;

    let client = http_client();
    let release = fetch_latest_release(&client).await?;

    // Bundle if we've never installed one; otherwise a small update.
    let kind = if cfg.bundle_installed {
        AssetKind::Update
    } else {
        AssetKind::Bundle
    };
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let asset = select_asset(&release, os, arch, kind)?.clone();

    installer::download_and_install(&app, &client, &asset, &install_dir).await?;

    // Commit version only on success.
    if kind == AssetKind::Bundle {
        cfg.bundle_installed = true;
    }
    cfg.installed_version = Some(release.tag_name);
    cfg.save(&path)?;
    Ok(())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_status,
            set_install_dir,
            install_or_update,
            play
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
