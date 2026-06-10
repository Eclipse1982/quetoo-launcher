use crate::error::{LauncherError, Result};
use crate::github::Asset;
use futures_util::StreamExt;
use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, Emitter};

/// One event type for every install phase. The frontend listens to
/// `install-progress` and renders phase + percent + detail.
#[derive(Clone, Serialize)]
pub struct InstallProgress {
    pub phase: &'static str, // "download" | "snapshot" | "extract" | "verify"
    pub percent: u8,
    pub detail: String,
}

pub fn emit_progress(app: &AppHandle, phase: &'static str, percent: u8, detail: String) {
    let _ = app.emit("install-progress", InstallProgress { phase, percent, detail });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    Dmg,
}

pub fn detect_format(file_name: &str) -> Result<ArchiveFormat> {
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

/// Stream-download `asset` to `dest`, emitting download progress.
pub async fn download_asset(
    app: &AppHandle,
    client: &reqwest::Client,
    asset: &Asset,
    dest: &Path,
) -> Result<()> {
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
    let mut file = std::fs::File::create(dest)?;
    let mut stream = resp.bytes_stream();
    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LauncherError::Network(e.to_string()))?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        let percent = if total > 0 { (downloaded * 100 / total) as u8 } else { 0 };
        emit_progress(
            app,
            "download",
            percent,
            format!(
                "{:.1} MB / {:.1} MB",
                downloaded as f64 / 1_048_576.0,
                total as f64 / 1_048_576.0
            ),
        );
    }
    file.flush()?;
    Ok(())
}

/// List the relative file paths (no directories) inside an archive.
/// Not supported for dmg (macOS snapshots the whole Quetoo.app instead).
pub fn list_entries(archive: &Path, format: ArchiveFormat) -> Result<Vec<String>> {
    match format {
        ArchiveFormat::Zip => {
            let file = std::fs::File::open(archive)?;
            let mut zip = zip::ZipArchive::new(file)
                .map_err(|e| LauncherError::Extract(e.to_string()))?;
            let mut out = Vec::new();
            for i in 0..zip.len() {
                let entry = zip
                    .by_index(i)
                    .map_err(|e| LauncherError::Extract(e.to_string()))?;
                if entry.is_dir() {
                    continue;
                }
                if let Some(name) = entry.enclosed_name() {
                    out.push(name.to_string_lossy().replace('\\', "/"));
                }
            }
            Ok(out)
        }
        ArchiveFormat::TarGz => {
            let file = std::fs::File::open(archive)?;
            let decoder = flate2::read::GzDecoder::new(file);
            let mut tar = tar::Archive::new(decoder);
            let mut out = Vec::new();
            for entry in tar.entries().map_err(|e| LauncherError::Extract(e.to_string()))? {
                let entry = entry.map_err(|e| LauncherError::Extract(e.to_string()))?;
                if entry.header().entry_type().is_dir() {
                    continue;
                }
                let path = entry.path().map_err(|e| LauncherError::Extract(e.to_string()))?;
                out.push(path.to_string_lossy().replace('\\', "/"));
            }
            Ok(out)
        }
        ArchiveFormat::Dmg => Err(LauncherError::Extract(
            "entry listing not supported for dmg".into(),
        )),
    }
}

/// Extract `archive` into `dest`, calling `progress(done, total)` per entry.
pub fn extract_archive(
    archive: &Path,
    format: ArchiveFormat,
    dest: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    match format {
        ArchiveFormat::Zip => extract_zip(archive, dest, progress),
        ArchiveFormat::TarGz => extract_targz(archive, dest, progress),
        ArchiveFormat::Dmg => {
            progress(0, 1);
            extract_dmg(archive, dest)?;
            progress(1, 1);
            Ok(())
        }
    }
}

fn extract_zip(archive: &Path, dest: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| LauncherError::Extract(e.to_string()))?;
    let total = zip.len();
    for i in 0..total {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| LauncherError::Extract(e.to_string()))?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
        }
        progress(i + 1, total);
    }
    Ok(())
}

fn extract_targz(
    archive: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<()> {
    // First pass: count entries so progress has a denominator.
    let total = {
        let file = std::fs::File::open(archive)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        tar.entries()
            .map_err(|e| LauncherError::Extract(e.to_string()))?
            .count()
    };
    let file = std::fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    for (i, entry) in tar
        .entries()
        .map_err(|e| LauncherError::Extract(e.to_string()))?
        .enumerate()
    {
        let mut entry = entry.map_err(|e| LauncherError::Extract(e.to_string()))?;
        entry
            .unpack_in(dest)
            .map_err(|e| LauncherError::Extract(e.to_string()))?;
        progress(i + 1, total);
    }
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

    use std::io::Write as _;

    /// Build a small zip with bin/quetoo.exe and lib/game.dll inside `dir`.
    fn make_test_zip(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("test.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zip.add_directory("bin/", opts).unwrap();
        zip.start_file("bin/quetoo.exe", opts).unwrap();
        zip.write_all(b"new-exe").unwrap();
        zip.start_file("lib/game.dll", opts).unwrap();
        zip.write_all(b"new-dll").unwrap();
        zip.finish().unwrap();
        path
    }

    fn make_test_targz(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("test.tar.gz");
        let file = std::fs::File::create(&path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "bin/quetoo", &b"new-bin"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn list_entries_zip_returns_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_test_zip(dir.path());
        let mut entries = list_entries(&archive, ArchiveFormat::Zip).unwrap();
        entries.sort();
        assert_eq!(entries, vec!["bin/quetoo.exe".to_string(), "lib/game.dll".to_string()]);
    }

    #[test]
    fn list_entries_targz_returns_files() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_test_targz(dir.path());
        assert_eq!(
            list_entries(&archive, ArchiveFormat::TarGz).unwrap(),
            vec!["bin/quetoo".to_string()]
        );
    }

    #[test]
    fn extract_zip_writes_files_and_reports_progress() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_test_zip(dir.path());
        let dest = dir.path().join("install");
        let mut calls: Vec<(usize, usize)> = vec![];
        extract_archive(&archive, ArchiveFormat::Zip, &dest, &mut |d, t| calls.push((d, t)))
            .unwrap();
        assert_eq!(std::fs::read(dest.join("bin/quetoo.exe")).unwrap(), b"new-exe");
        assert_eq!(std::fs::read(dest.join("lib/game.dll")).unwrap(), b"new-dll");
        assert!(!calls.is_empty());
        let (done, total) = *calls.last().unwrap();
        assert_eq!(done, total);
    }

    #[test]
    fn extract_targz_writes_files() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_test_targz(dir.path());
        let dest = dir.path().join("install");
        extract_archive(&archive, ArchiveFormat::TarGz, &dest, &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(dest.join("bin/quetoo")).unwrap(), b"new-bin");
    }
}
