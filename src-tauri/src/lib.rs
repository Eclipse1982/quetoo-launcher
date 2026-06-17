mod browser;
mod config;
mod data;
mod error;
mod github;
mod installer;
mod launcher;
mod qconfig;
mod snapshot;

use config::{Channel, Config};
use error::Result;
use github::{
    determine_state, display_version, fetch_latest_release, fetch_snapshot_release,
    latest_identity, select_asset, snapshot_available, AssetKind, InstallState, Release,
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
    default_install_dir: Option<String>,
    latest_version: String,
    state: InstallState,
    can_rollback: bool,
    channel: Channel,
    pre_release_available: bool,
}

/// Render a state's version identities for display (snapshot timestamps become
/// "Snapshot (date time)"; stable tags pass through). Comparison upstream uses
/// raw identities — only the user-facing copy is rewritten here.
fn display_state(state: InstallState) -> InstallState {
    match state {
        InstallState::UpdateAvailable { from, to } => InstallState::UpdateAvailable {
            from: display_version(&from),
            to: display_version(&to),
        },
        other => other,
    }
}

/// Fetch the release for the active channel.
async fn fetch_for_channel(client: &reqwest::Client, channel: Channel) -> Result<Release> {
    match channel {
        Channel::Stable => fetch_latest_release(client).await,
        Channel::PreRelease => fetch_snapshot_release(client).await,
    }
}

/// Check GitHub and return the current launcher status.
#[tauri::command]
async fn get_status(app: AppHandle) -> std::result::Result<StatusDto, error::LauncherError> {
    let cfg = Config::load(&config_path(&app)?)?;
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let channel = cfg.channel;

    let release = fetch_for_channel(&http_client(), channel).await?;
    let identity = latest_identity(channel, &release);
    let state = display_state(determine_state(&cfg, &identity));

    let can_rollback = match &cfg.install_dir {
        // Degrade to false on a bad manifest — status must always render.
        Some(dir) => snapshot::load_manifest(dir).ok().flatten().is_some(),
        None => false,
    };
    Ok(StatusDto {
        install_dir: cfg.install_dir.map(|p| p.to_string_lossy().into_owned()),
        default_install_dir: config::default_install_dir(os)
            .map(|p| p.to_string_lossy().into_owned()),
        latest_version: display_version(&identity),
        state,
        can_rollback,
        channel,
        pre_release_available: snapshot_available(os, arch),
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

/// Switch the active release channel and return refreshed status.
#[tauri::command]
async fn set_channel(
    app: AppHandle,
    channel: Channel,
) -> std::result::Result<StatusDto, error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    cfg.channel = channel;
    cfg.save(&path)?;
    get_status(app).await
}

/// Pre-flight guard: on non-macOS, check that the game executable is not
/// currently held open (which would cause cryptic os error 32 mid-extract).
/// Returns `Busy` error if the game appears to be running.
fn ensure_game_not_running(install_dir: &std::path::Path, os: &str) -> Result<()> {
    if os == "macos" {
        return Ok(());
    }
    let target = launcher::executable_path(install_dir, os)?;
    if target.exists()
        && std::fs::OpenOptions::new().write(true).open(&target).is_err()
    {
        return Err(error::LauncherError::Busy(
            "Quetoo appears to be running — close the game and try again".into(),
        ));
    }
    Ok(())
}

/// Download + extract the full game-data bundle for `release` into the install
/// dir and verify the launch target. Used for the initial install (no backup —
/// there is nothing to roll back to yet). Sets no config; the caller persists.
async fn install_bundle(
    app: &AppHandle,
    client: &reqwest::Client,
    install_dir: &std::path::Path,
    os: &str,
    arch: &str,
    release: &Release,
) -> Result<()> {
    let asset = select_asset(release, os, arch, AssetKind::Bundle)?.clone();
    std::fs::create_dir_all(install_dir)?;
    let tmp = install_dir.join(format!(".{}", asset.name));
    if let Err(e) = installer::download_asset(app, client, &asset, &tmp).await {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let format = installer::detect_format(&asset.name)?;

    let app2 = app.clone();
    let extract_result =
        installer::extract_archive(&tmp, format, install_dir, &mut |done, total| {
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

    installer::emit_progress(app, "verify", 50, "Verifying installation".into());
    let target = launcher::executable_path(install_dir, os)?;
    if !target.exists() {
        return Err(error::LauncherError::Extract(format!(
            "install incomplete: {} missing",
            target.display()
        )));
    }
    installer::emit_progress(app, "verify", 100, "Done".into());
    Ok(())
}

/// Download + extract the small engine update asset for `release` over an
/// existing install, snapshotting overwritten files first so the change can be
/// rolled back. `identity` is recorded as the installed version on success.
#[allow(clippy::too_many_arguments)]
async fn apply_engine_update(
    app: &AppHandle,
    client: &reqwest::Client,
    cfg: &mut Config,
    cfg_path: &std::path::Path,
    install_dir: &std::path::Path,
    os: &str,
    arch: &str,
    release: &Release,
    identity: &str,
) -> Result<()> {
    let asset = select_asset(release, os, arch, AssetKind::Update)?.clone();

    // Pre-flight: refuse if the executable is locked (Windows holds running
    // executables open). Checked before any download so nothing is touched.
    ensure_game_not_running(install_dir, os)?;

    std::fs::create_dir_all(install_dir)?;
    let tmp = install_dir.join(format!(".{}", asset.name));
    if let Err(e) = installer::download_asset(app, client, &asset, &tmp).await {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let format = installer::detect_format(&asset.name)?;

    // Snapshot before overwriting — a failed snapshot aborts the update.
    if snapshot::has_snapshot_for(install_dir, identity) {
        // A prior attempt already captured the pre-update state for this
        // version transition. Reuse it — re-snapshotting after a failed
        // mid-extract would capture a mixed/new tree as "old".
        installer::emit_progress(app, "snapshot", 100, "Reusing existing backup".into());
    } else {
        installer::emit_progress(app, "snapshot", 0, "Backing up current version".into());
        let from = cfg.installed_version.clone().unwrap_or_else(|| "unknown".into());
        let snap_result = if os == "macos" {
            snapshot::create_snapshot_macos(install_dir, &from, identity).map(|_| ())
        } else {
            installer::list_entries(&tmp, format).and_then(|entries| {
                snapshot::create_snapshot(install_dir, &entries, &from, identity).map(|_| ())
            })
        };
        if let Err(e) = snap_result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        installer::emit_progress(app, "snapshot", 100, "Backup complete".into());
    }

    let app2 = app.clone();
    let extract_result =
        installer::extract_archive(&tmp, format, install_dir, &mut |done, total| {
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

    installer::emit_progress(app, "verify", 50, "Verifying installation".into());
    let target = launcher::executable_path(install_dir, os)?;
    if !target.exists() {
        return Err(error::LauncherError::Extract(format!(
            "install incomplete: {} missing",
            target.display()
        )));
    }
    installer::emit_progress(app, "verify", 100, "Done".into());

    cfg.installed_version = Some(identity.to_string());
    cfg.save(cfg_path)?;
    Ok(())
}

/// Install or update Quetoo for the active channel.
///
/// Fresh install: download the stable game-data bundle (engine + assets). The
/// Snapshot pre-release ships only engine binaries, so a Pre-Release install
/// then overlays the snapshot engine on top of the stable bundle. Updates and
/// channel switches overlay just the engine asset, snapshotting first so they
/// can be rolled back. Every path finishes by syncing game data to the latest
/// quetoo-data manifest.
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
    let channel = cfg.channel;

    // Phase 1: ensure the engine+data bundle exists (always from Stable).
    let mut bundle_just_installed_stable = false;
    if !cfg.bundle_installed {
        let stable = fetch_latest_release(&client).await?;
        install_bundle(&app, &client, &install_dir, os, arch, &stable).await?;
        cfg.bundle_installed = true;
        cfg.installed_version = Some(stable.tag_name);
        cfg.save(&path)?;
        bundle_just_installed_stable = channel == Channel::Stable;
    }

    // Phase 2: overlay the active channel's engine update on top of the bundle,
    // unless a fresh Stable bundle already provided the correct engine.
    if !bundle_just_installed_stable {
        let release = fetch_for_channel(&client, channel).await?;
        let identity = latest_identity(channel, &release);
        apply_engine_update(
            &app, &client, &mut cfg, &path, &install_dir, os, arch, &release, &identity,
        )
        .await?;
    }

    // Phase 3: bring game data current. Non-fatal — offline play stays allowed.
    let _ = data::run_sync(&app, &client, &install_dir, false).await;
    Ok(())
}

/// Sync the `default` game data set to match quetoo-data's manifest. No-op
/// (returns a `skipped` summary) when nothing is installed yet. `verify = true`
/// forces a full re-hash ("Verify data").
#[tauri::command]
async fn sync_data(
    app: AppHandle,
    verify: bool,
) -> std::result::Result<data::SyncSummary, error::LauncherError> {
    let cfg = Config::load(&config_path(&app)?)?;
    let install_dir = match cfg.install_dir {
        Some(dir) if cfg.bundle_installed => dir,
        _ => {
            return Ok(data::SyncSummary {
                skipped: true,
                ..Default::default()
            })
        }
    };
    let client = http_client();
    data::run_sync(&app, &client, &install_dir, verify).await
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
    // Preflight: rollback overwrites bin/quetoo.exe; a running game would die
    // mid-restore with os error 32 (Windows file-locked).
    ensure_game_not_running(&install_dir, std::env::consts::OS)?;
    let from_version = snapshot::rollback(&install_dir)?;
    cfg.installed_version = Some(from_version);
    cfg.save(&path)?;
    Ok(())
}

/// Shared teardown for `reinstall` and `uninstall`: refuse if the game is
/// running or the target doesn't look like a Quetoo install, mark the config
/// not-installed BEFORE deleting (a partial wipe must self-heal via plain
/// Install), then wipe the directory contents.
///
/// `install_dir` is kept in the config — it is the user's remembered location
/// preference and is not cleared here.
fn guarded_wipe(
    cfg: &mut Config,
    cfg_path: &std::path::Path,
    install_dir: &std::path::Path,
    os: &str,
) -> Result<()> {
    ensure_game_not_running(install_dir, os)?;

    if !installer::is_safe_reinstall_target(install_dir)? {
        return Err(error::LauncherError::Config(format!(
            "{} doesn't look like a Quetoo install; refusing to delete it",
            install_dir.display()
        )));
    }

    // Reset config BEFORE the deletion loop: if a locked file interrupts the
    // wipe mid-way, the config must already reflect NotInstalled so that a
    // plain Install can self-heal. Saving config after a partial wipe would
    // leave bundle_installed=true on a gutted tree, causing the sanity guard
    // to refuse a retry.
    cfg.installed_version = None;
    cfg.bundle_installed = false;
    cfg.save(cfg_path)?;

    if install_dir.exists() {
        for entry in std::fs::read_dir(install_dir)? {
            let p = entry?.path();
            if p.is_dir() {
                std::fs::remove_dir_all(&p)?;
            } else {
                std::fs::remove_file(&p)?;
            }
        }
    }
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

    guarded_wipe(&mut cfg, &path, &install_dir, std::env::consts::OS)?;
    install_or_update(app).await
}

/// Remove the game from the install directory. Optionally also delete the
/// per-user Quetoo data (settings, screenshots, demos, downloaded maps),
/// which is shared by every Quetoo install and mod on this machine.
#[tauri::command]
async fn uninstall(
    app: AppHandle,
    delete_user_data: bool,
) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    let install_dir = cfg
        .install_dir
        .clone()
        .ok_or_else(|| error::LauncherError::Config("no install directory set".into()))?;

    guarded_wipe(&mut cfg, &path, &install_dir, std::env::consts::OS)?;

    if install_dir.exists() {
        // Best effort: a handle held on the dir (Explorer) must not fail us.
        let _ = std::fs::remove_dir(&install_dir);
    }

    if delete_user_data {
        let user_dir = qconfig::quetoo_user_dir()?;
        if user_dir.exists() {
            std::fs::remove_dir_all(&user_dir)?;
        }
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerList {
    servers: Vec<browser::ServerInfo>,
    master_ok: bool,
}

/// Query the master server and all favorites concurrently.
#[tauri::command]
async fn get_servers(app: AppHandle) -> std::result::Result<ServerList, error::LauncherError> {
    use std::collections::HashSet;
    use std::net::SocketAddrV4;

    let cfg = Config::load(&config_path(&app)?)?;

    // Collect favorite addresses (skip unparseable ones silently).
    let favorite_addrs: HashSet<SocketAddrV4> = cfg
        .favorites
        .iter()
        .filter_map(|s| s.parse::<SocketAddrV4>().ok())
        .collect();

    // Probe favorites immediately — they must not wait out a dead master's
    // 1500ms timeout (that's the exact case master_ok exists for).
    let mut handles: Vec<_> = favorite_addrs
        .iter()
        .copied()
        .map(|addr| tokio::spawn(async move { browser::probe_server(addr, true).await }))
        .collect();

    let (master_addrs, master_ok) = match browser::fetch_master_list().await {
        Ok(list) => (list, true),
        Err(_) => (Vec::new(), false),
    };

    // Probe master-only addresses (favorites already in flight = dedupe).
    let master_only: HashSet<SocketAddrV4> = master_addrs
        .into_iter()
        .filter(|a| !favorite_addrs.contains(a))
        .collect();
    handles.extend(
        master_only
            .into_iter()
            .map(|addr| tokio::spawn(async move { browser::probe_server(addr, false).await })),
    );

    let mut servers: Vec<browser::ServerInfo> = Vec::new();
    for handle in handles {
        // Defensively handle panicked probe tasks — they must not poison the command.
        match handle.await {
            Ok(Some(info)) => servers.push(info),
            Ok(None) => {}
            Err(_) => {} // task panicked — skip
        }
    }

    // Ping ascending with a stable addr tiebreak so equal-ping rows (e.g. two
    // dead favorites at 999) don't swap places on every auto-refresh.
    servers.sort_by(|a, b| a.ping.cmp(&b.ping).then_with(|| a.addr.cmp(&b.addr)));

    Ok(ServerList { servers, master_ok })
}

/// Launch the installed game connected to the given server address.
#[tauri::command]
async fn join_server(
    app: AppHandle,
    addr: String,
) -> std::result::Result<(), error::LauncherError> {
    let cfg = Config::load(&config_path(&app)?)?;
    let install_dir = cfg
        .install_dir
        .ok_or_else(|| error::LauncherError::Config("no install directory set".into()))?;
    if !cfg.bundle_installed {
        return Err(error::LauncherError::Launch(
            "Quetoo is not installed".into(),
        ));
    }
    // Validate that addr parses as a SocketAddrV4.
    addr.parse::<std::net::SocketAddrV4>()
        .map_err(|_| error::LauncherError::Config(format!("not a valid server address: {addr}")))?;
    launcher::launch_with_args(&install_dir, &["+connect", &addr])
}

/// Add a server address to the favorites list.
#[tauri::command]
async fn add_favorite(
    app: AppHandle,
    addr: String,
) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    let normalized = config::normalize_favorite(&addr)?;
    if !cfg.favorites.contains(&normalized) {
        cfg.favorites.push(normalized);
        cfg.save(&path)?;
    }
    Ok(())
}

/// Remove a server address from the favorites list.
#[tauri::command]
async fn remove_favorite(
    app: AppHandle,
    addr: String,
) -> std::result::Result<(), error::LauncherError> {
    let path = config_path(&app)?;
    let mut cfg = Config::load(&path)?;
    // Match the exact stored string, or the normalized form for round-trip safety.
    let normalized = config::normalize_favorite(&addr).unwrap_or_else(|_| addr.clone());
    let before = cfg.favorites.len();
    cfg.favorites.retain(|s| s != &addr && s != &normalized);
    if cfg.favorites.len() != before {
        cfg.save(&path)?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Show the launcher version in the window title (always matches the
            // built version via package info, so it can't drift).
            let version = app.package_info().version.to_string();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("Quetoo Launcher v{version}"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            set_install_dir,
            set_channel,
            install_or_update,
            sync_data,
            rollback_update,
            reinstall,
            uninstall,
            play,
            get_quetoo_settings,
            save_quetoo_settings,
            default_quetoo_settings,
            get_servers,
            join_server,
            add_favorite,
            remove_favorite
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
