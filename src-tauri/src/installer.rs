use crate::error::{LauncherError, Result};
use crate::github::Asset;
use futures_util::StreamExt;
use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    Zip,
    TarGz,
    Dmg,
}

fn detect_format(file_name: &str) -> Result<ArchiveFormat> {
    if file_name.ends_with(".zip") {
        Ok(ArchiveFormat::Zip)
    } else if file_name.ends_with(".tar.gz") {
        Ok(ArchiveFormat::TarGz)
    } else if file_name.ends_with(".dmg") {
        Ok(ArchiveFormat::Dmg)
    } else {
        Err(LauncherError::Extract(format!("unknown archive: {file_name}")))
    }
}

/// Download `asset` into `install_dir`/<tmp> and extract it into `install_dir`.
/// Emits `download-progress` events as bytes arrive.
pub async fn download_and_install(
    app: &AppHandle,
    client: &reqwest::Client,
    asset: &Asset,
    install_dir: &Path,
) -> Result<()> {
    let format = detect_format(&asset.name)?;
    std::fs::create_dir_all(install_dir)?;
    let tmp = install_dir.join(format!(".{}", asset.name));

    // --- stream download ---
    let resp = client
        .get(&asset.browser_download_url)
        .header("User-Agent", "quetoo-launcher")
        .send()
        .await
        .map_err(|e| LauncherError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(LauncherError::Network(format!("HTTP {}", resp.status())));
    }
    let total = resp.content_length().unwrap_or(asset.size);
    let mut downloaded: u64 = 0;
    let mut file = std::fs::File::create(&tmp)?;
    let mut stream = resp.bytes_stream();
    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LauncherError::Network(e.to_string()))?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        let _ = app.emit("download-progress", DownloadProgress { downloaded, total });
    }
    file.flush()?;
    drop(file);

    // --- extract ---
    let result = match format {
        ArchiveFormat::Zip => extract_zip(&tmp, install_dir),
        ArchiveFormat::TarGz => extract_targz(&tmp, install_dir),
        ArchiveFormat::Dmg => extract_dmg(&tmp, install_dir),
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| LauncherError::Extract(e.to_string()))?;
    zip.extract(dest).map_err(|e| LauncherError::Extract(e.to_string()))?;
    Ok(())
}

fn extract_targz(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(dest).map_err(|e| LauncherError::Extract(e.to_string()))?;
    Ok(())
}

fn extract_dmg(archive: &Path, dest: &Path) -> Result<()> {
    // Mount, copy Quetoo.app, detach. macOS only.
    let mount = tempfile_mount(archive)?;
    let app_src = Path::new(&mount).join("Quetoo.app");
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(&app_src)
        .arg(dest)
        .status()
        .map_err(|e| LauncherError::Extract(e.to_string()))?;
    let _ = std::process::Command::new("hdiutil")
        .arg("detach")
        .arg(&mount)
        .status();
    if !status.success() {
        return Err(LauncherError::Extract("failed to copy Quetoo.app".into()));
    }
    Ok(())
}

fn tempfile_mount(archive: &Path) -> Result<String> {
    let mount_point = std::env::temp_dir()
        .join(format!("quetoo-mnt-{}", std::process::id()));
    std::fs::create_dir_all(&mount_point)?;
    let status = std::process::Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-mountpoint"])
        .arg(&mount_point)
        .arg(archive)
        .status()
        .map_err(|e| LauncherError::Extract(e.to_string()))?;
    if !status.success() {
        return Err(LauncherError::Extract("hdiutil attach failed".into()));
    }
    Ok(mount_point.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        assert_eq!(detect_format("quetoo-x86_64-pc-windows.zip").unwrap(), ArchiveFormat::Zip);
        assert_eq!(detect_format("quetoo-x86_64-pc-linux.tar.gz").unwrap(), ArchiveFormat::TarGz);
        assert_eq!(detect_format("quetoo-universal-apple-darwin.dmg").unwrap(), ArchiveFormat::Dmg);
        assert!(detect_format("quetoo.rpm").is_err());
    }
}
