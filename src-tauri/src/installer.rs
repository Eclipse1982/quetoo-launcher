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

/// Integer percent clamped to 100 (guards lying servers / stale sizes).
pub fn percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((done as u128 * 100 / total as u128).min(100)) as u8
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
    let mut last_emitted: Option<u8> = None;
    let mut file = std::fs::File::create(dest)?;
    let mut stream = resp.bytes_stream();
    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LauncherError::Network(e.to_string()))?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        let p = percent(downloaded, total);
        if last_emitted != Some(p) {
            emit_progress(
                app,
                "download",
                p,
                format!(
                    "{:.1} MB / {:.1} MB",
                    downloaded as f64 / 1_048_576.0,
                    total as f64 / 1_048_576.0
                ),
            );
            last_emitted = Some(p);
        }
    }
    // Always emit final 100% so the bar completes even if the last chunk didn't bump the percent.
    let final_mb = downloaded as f64 / 1_048_576.0;
    emit_progress(
        app,
        "download",
        100,
        format!("{:.1} MB / {:.1} MB", final_mb, final_mb),
    );
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
                    // Archives must not write into the launcher's rollback area.
                    let first_component = name.components().next();
                    if matches!(
                        first_component,
                        Some(std::path::Component::Normal(s)) if s == ".rollback"
                    ) {
                        continue;
                    }
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
                let p = entry.path().map_err(|e| LauncherError::Extract(e.to_string()))?;
                if p.is_absolute()
                    || p.components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    continue;
                }
                // Archives must not write into the launcher's rollback area.
                if matches!(
                    p.components().next(),
                    Some(std::path::Component::Normal(s)) if s == ".rollback"
                ) {
                    continue;
                }
                out.push(p.to_string_lossy().replace('\\', "/"));
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
        if let Some(rel) = entry.enclosed_name() {
            let out = dest.join(&rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out)?;
            } else {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut f = std::fs::File::create(&out)?;
                std::io::copy(&mut entry, &mut f)?;
                #[cfg(unix)]
                if let Some(mode) = entry.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
        // Always report progress for every entry (including skipped non-enclosed-name entries)
        // so the progress bar advances uniformly.
        progress(i + 1, total);
    }
    Ok(())
}

fn extract_targz(
    archive: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<()> {
    // Signal activity before the count pass so the caller can show a spinner.
    progress(0, 0);

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

    // --- Fix 3: percent() unit tests ---

    #[test]
    fn percent_zero_total_returns_zero() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(100, 0), 0);
    }

    #[test]
    fn percent_normal() {
        assert_eq!(percent(50, 100), 50);
        assert_eq!(percent(1, 4), 25);
        assert_eq!(percent(100, 100), 100);
    }

    #[test]
    fn percent_done_exceeds_total_clamps_to_100() {
        assert_eq!(percent(200, 100), 100);
        assert_eq!(percent(u64::MAX, 1), 100);
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

    // --- Fix 6a: targz progress callback assertions ---

    #[test]
    fn extract_targz_progress_callback_last_call_is_done_eq_total() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_test_targz(dir.path());
        let dest = dir.path().join("install");
        let mut calls: Vec<(usize, usize)> = vec![];
        extract_archive(&archive, ArchiveFormat::TarGz, &dest, &mut |d, t| {
            calls.push((d, t))
        })
        .unwrap();
        // The preamble progress(0,0) is the first call; filter it out and check the last real call.
        let real_calls: Vec<_> = calls.iter().filter(|&&(_, t)| t > 0).collect();
        assert!(!real_calls.is_empty(), "expected progress calls with total > 0");
        let &(done, total) = real_calls.last().unwrap();
        assert_eq!(done, total, "last progress call must have done == total");
        assert!(*total > 0, "total must be positive");
    }

    // --- Fix 6b: zip-slip tests ---

    /// The zip crate's `start_file` rejects names starting with ".." at the API
    /// level (it would produce an invalid archive), so we cannot build a classic
    /// "../evil.txt" zip-slip archive via the normal writer API.
    ///
    /// Instead we test with an absolute-path entry name.  The zip crate does
    /// accept absolute paths in `start_file` (the path is stored verbatim), so
    /// we can verify that `list_entries` silently skips it (enclosed_name returns
    /// None for absolute paths) and that `extract_archive` also skips it and
    /// still reports done == total at the end.
    fn make_absolute_path_zip(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("slip.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        // A normal safe entry.
        zip.start_file("safe.txt", opts).unwrap();
        zip.write_all(b"safe content").unwrap();
        // An entry whose stored name is an absolute path.
        // enclosed_name() returns None for these, so both list and extract skip it.
        zip.start_file("/etc/passwd", opts).unwrap();
        zip.write_all(b"evil").unwrap();
        zip.finish().unwrap();
        path
    }

    #[test]
    fn list_entries_zip_skips_absolute_path_entry() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_absolute_path_zip(dir.path());
        let entries = list_entries(&archive, ArchiveFormat::Zip).unwrap();
        // Only safe.txt should appear; the absolute-path entry is silently skipped.
        assert_eq!(entries, vec!["safe.txt".to_string()]);
    }

    #[test]
    fn extract_zip_skips_absolute_path_entry_and_progress_completes() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_absolute_path_zip(dir.path());
        let dest = dir.path().join("install");
        let mut calls: Vec<(usize, usize)> = vec![];
        extract_archive(&archive, ArchiveFormat::Zip, &dest, &mut |d, t| {
            calls.push((d, t))
        })
        .unwrap();
        // Nothing written outside dest.
        assert!(!std::path::Path::new("/etc/passwd").exists() ||
            std::fs::read("/etc/passwd").map(|b| b != b"evil").unwrap_or(true),
            "zip-slip: evil file must not be written");
        // safe.txt written inside dest.
        assert_eq!(std::fs::read(dest.join("safe.txt")).unwrap(), b"safe content");
        // Progress bar completes: last call has done == total.
        assert!(!calls.is_empty());
        let (done, total) = *calls.last().unwrap();
        assert_eq!(done, total, "progress bar must complete even when entries are skipped");
    }

    // --- Fix 2: .rollback filtering in list_entries ---

    /// Build a zip that contains a `.rollback/evil.txt` entry.
    fn make_rollback_zip(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("rollback.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zip.start_file("safe.txt", opts).unwrap();
        zip.write_all(b"safe").unwrap();
        // .rollback entry: zip crate accepts this name verbatim.
        zip.start_file(".rollback/evil.txt", opts).unwrap();
        zip.write_all(b"evil").unwrap();
        zip.finish().unwrap();
        path
    }

    /// list_entries must skip entries whose first path component is `.rollback`,
    /// so archives cannot overwrite the launcher's rollback area.
    #[test]
    fn list_entries_zip_skips_rollback_entry() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_rollback_zip(dir.path());
        let entries = list_entries(&archive, ArchiveFormat::Zip).unwrap();
        // Only safe.txt should appear; .rollback/evil.txt must be skipped.
        assert_eq!(entries, vec!["safe.txt".to_string()]);
    }
}
