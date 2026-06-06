mod config;
mod error;
mod github;
mod installer;
mod launcher;
mod qconfig;

use config::Config;
use error::Result;
use github::{
    determine_state, fetch_latest_release, fetch_railwarz_release, select_asset, AssetKind,
    InstallState,
};
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
    let client = http_client();
    let official = fetch_latest_release(&client).await?;
    let railwarz = fetch_railwarz_release(&client).await?;
    let state = determine_state(&cfg, &official.tag_name, &railwarz.tag_name);
    Ok(StatusDto {
        install_dir: cfg.install_dir.map(|p| p.to_string_lossy().into_owned()),
        // Headline the RailWarz overlay version; note the official base it sits on.
        latest_version: format!("{} (base {})", railwarz.tag_name, official.tag_name),
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

/// Install/update Quetoo RailWarz: install or update the official base (engine + game
/// data), then overlay our matched RailWarz engine + game/cgame modules on top.
/// Each step records its version only on success, so the operation is resumable.
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

    let official = fetch_latest_release(&client).await?;
    let railwarz = fetch_railwarz_release(&client).await?;

    // 1) Base — official Quetoo. Bundle (engine + game data) on first install, else a
    //    small update when upstream has moved. An official update overwrites our overlaid
    //    modules, so it forces a re-overlay (railwarz_version cleared) below.
    let base_changed = !cfg.bundle_installed
        || cfg.installed_version.as_deref() != Some(official.tag_name.as_str());
    if base_changed {
        let kind = if cfg.bundle_installed {
            AssetKind::Update
        } else {
            AssetKind::Bundle
        };
        let asset = select_asset(&official, os, arch, kind)?.clone();
        installer::download_and_install(&app, &client, &asset, &install_dir).await?;
        if kind == AssetKind::Bundle {
            cfg.bundle_installed = true;
        }
        cfg.installed_version = Some(official.tag_name.clone());
        cfg.railwarz_version = None; // a fresh base wipes any prior overlay
        cfg.save(&path)?;
    }

    // 2) Overlay — RailWarz binaries only (engine + game.dll/cgame.dll); game data comes
    //    from the base. Extracts over the install dir so the matched build wins.
    let overlay_changed = cfg.railwarz_version.as_deref() != Some(railwarz.tag_name.as_str());
    if overlay_changed {
        let asset = select_asset(&railwarz, os, arch, AssetKind::Update)?.clone();
        installer::download_and_install(&app, &client, &asset, &install_dir).await?;
        cfg.railwarz_version = Some(railwarz.tag_name.clone());
        cfg.save(&path)?;
    }

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
            play,
            get_quetoo_settings,
            save_quetoo_settings,
            default_quetoo_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
