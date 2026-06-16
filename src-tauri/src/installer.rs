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

/// Progress label for the data-sync "check/seed" pass.
pub fn data_progress_label(app: &AppHandle, checked: usize, total: usize) {
    emit_progress(
        app,
        "data",
        percent(checked as u64, total as u64),
        format!("Checking {checked}/{total} files"),
    );
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

/// True if wiping `dir` cannot destroy user data: missing, effectively empty
/// (only launcher-owned artifacts: `.rollback` and `.quetoo-*` download temps),
/// or a Quetoo layout (has `bin/` or `Quetoo.app`). Protects against a
/// mis-pointed install dir being wiped by Reinstall.
pub fn is_safe_reinstall_target(dir: &std::path::Path) -> crate::error::Result<bool> {
    if !dir.exists() {
        return Ok(true);
    }
    let mut has_other = false;
    for entry in std::fs::read_dir(dir)? {
        let name = entry?.file_name();
        let s = name.to_string_lossy();
        // Only artifacts the launcher itself creates count as ignorable:
        // the rollback directory (.rollback) and download temps (.quetoo-*).
        // Other dot entries (.git, .vscode, ...) are user data — refuse.
        let launcher_owned =
            s.eq_ignore_ascii_case(".rollback") || s.to_ascii_lowercase().starts_with(".quetoo");
        if !launcher_owned {
            has_other = true;
        }
    }
    if !has_other {
        return Ok(true);
    }
    Ok(dir.join("bin").exists() || dir.join("Quetoo.app").exists())
}

/// True if `rel` escapes the install dir or touches the reserved .rollback
/// area: empty, absolute, drive-prefixed, any `..`, or whose first
/// non-`.` component is `.rollback` (ASCII case-insensitive — NTFS).
pub(crate) fn is_unsafe_rel_path(rel: &str) -> bool {
    use std::path::Component;
    let path = std::path::Path::new(rel);
    let mut first_normal: Option<&std::ffi::OsStr> = None;
    for c in path.components() {
        match c {
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return true,
            Component::CurDir => continue,
            Component::Normal(name) => {
                first_normal.get_or_insert(name);
            }
        }
    }
    match first_normal {
        None => true, // empty or "." only
        Some(name) => name
            .to_str()
            .map(|s| s.eq_ignore_ascii_case(".rollback"))
            .unwrap_or(true), // non-UTF8 first component: reject
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
                    let rel = name.to_string_lossy().replace('\\', "/");
                    // Archives must not write into the launcher's rollback area;
                    // is_unsafe_rel_path also catches ./-prefix and case variants.
                    if is_unsafe_rel_path(&rel) {
                        continue;
                    }
                    out.push(rel);
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
                let rel = p.to_string_lossy().replace('\\', "/");
                // is_unsafe_rel_path covers absolute, .., ./-prefix, and
                // case-insensitive .rollback reservation.
                if is_unsafe_rel_path(&rel) {
                    continue;
                }
                out.push(rel);
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
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            // Archives must never write into the reserved .rollback area;
            // extraction is the enforcement point because the install flow
            // doesn't gate on list_entries.
            if !is_unsafe_rel_path(&rel_str) {
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
        }
        // Always report progress for every entry (including skipped entries)
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
        // Archives must never write into the reserved .rollback area;
        // extraction is the enforcement point because the install flow
        // doesn't gate on list_entries.
        let skip = match entry.path() {
            Ok(p) => {
                let rel = p.to_string_lossy().replace('\\', "/");
                !rel.is_empty() && is_unsafe_rel_path(&rel)
            }
            Err(_) => true, // non-UTF8 or unreadable path: skip
        };
        if !skip {
            entry
                .unpack_in(dest)
                .map_err(|e| LauncherError::Extract(e.to_string()))?;
        }
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
    fn sanity_missing_or_empty_dir_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_safe_reinstall_target(&dir.path().join("nope")).unwrap());
        assert!(is_safe_reinstall_target(dir.path()).unwrap());
    }

    #[test]
    fn sanity_quetoo_layout_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        assert!(is_safe_reinstall_target(dir.path()).unwrap());
    }

    #[test]
    fn sanity_unrelated_dir_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("family-photos.txt"), "precious").unwrap();
        assert!(!is_safe_reinstall_target(dir.path()).unwrap());
    }

    /// A dir containing only launcher-owned artifacts (.rollback/ and a dot-prefixed
    /// download temp) is safe to wipe — is_safe_reinstall_target must return true.
    #[test]
    fn launcher_artifacts_only_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        // .rollback/manifest.json  (the rollback directory)
        let rollback = dir.path().join(".rollback");
        std::fs::create_dir_all(&rollback).unwrap();
        std::fs::write(rollback.join("manifest.json"), "{}").unwrap();
        // .quetoo-x86_64-pc-windows.zip  (a download temp)
        std::fs::write(
            dir.path().join(".quetoo-x86_64-pc-windows.zip"),
            b"partial",
        )
        .unwrap();
        assert!(
            is_safe_reinstall_target(dir.path()).unwrap(),
            "dir with only launcher-owned dot-prefixed entries must be safe"
        );
    }

    /// Non-launcher dot entries are user data: a .git-only dir must be refused.
    #[test]
    fn dot_git_only_dir_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        assert!(
            !is_safe_reinstall_target(dir.path()).unwrap(),
            "a dir whose only entry is .git must not be wiped"
        );
    }

    /// A Quetoo layout that also has a user file is still safe because bin/ is present.
    #[test]
    fn quetoo_layout_with_extra_user_file_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("my-save.txt"), "saves").unwrap();
        assert!(
            is_safe_reinstall_target(dir.path()).unwrap(),
            "Quetoo layout (bin/ present) must be safe even with extra user files"
        );
    }

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

    // --- Bypass A/B/C tests (TDD red phase) ---

    /// Build a zip with:
    ///   safe.txt             – normal entry that must be extracted
    ///   ./.rollback/evil.txt – CurDir bypass (bypass A)
    ///   .ROLLBACK/evil2.txt  – NTFS case bypass (bypass B)
    ///   bin/.rollback/x      – interior .rollback, must NOT be filtered
    fn make_bypass_zip(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("bypass.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zip.start_file("safe.txt", opts).unwrap();
        zip.write_all(b"safe-content").unwrap();
        zip.start_file("./.rollback/evil.txt", opts).unwrap();
        zip.write_all(b"evil-curdirbypass").unwrap();
        zip.start_file(".ROLLBACK/evil2.txt", opts).unwrap();
        zip.write_all(b"evil-casebypass").unwrap();
        zip.start_file("bin/.rollback/x", opts).unwrap();
        zip.write_all(b"interior-ok").unwrap();
        zip.finish().unwrap();
        path
    }

    /// list_entries must omit ./.rollback/evil.txt and .ROLLBACK/evil2.txt
    /// but must keep bin/.rollback/x (interior .rollback is not reserved).
    #[test]
    fn list_entries_zip_skips_curdirprefix_and_case_rollback_entries() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_bypass_zip(dir.path());
        let mut entries = list_entries(&archive, ArchiveFormat::Zip).unwrap();
        entries.sort();
        // safe.txt and bin/.rollback/x accepted; both hostile entries omitted.
        assert!(entries.contains(&"safe.txt".to_string()), "safe.txt must be listed");
        assert!(entries.contains(&"bin/.rollback/x".to_string()), "interior bin/.rollback/x must be listed");
        assert!(
            !entries.iter().any(|e| e.contains("evil")),
            "hostile .rollback entries must be filtered: {entries:?}"
        );
    }

    /// extract_archive must NOT write ./.rollback/evil.txt or .ROLLBACK/evil2.txt
    /// under dest, MUST write safe.txt and bin/.rollback/x, and progress must
    /// complete with done == total.
    #[test]
    fn extract_zip_skips_bypass_rollback_entries_and_progress_completes() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_bypass_zip(dir.path());
        let dest = dir.path().join("install");
        let mut calls: Vec<(usize, usize)> = vec![];
        extract_archive(&archive, ArchiveFormat::Zip, &dest, &mut |d, t| {
            calls.push((d, t))
        })
        .unwrap();

        // Hostile entries must NOT appear under dest.
        assert!(
            !dest.join(".rollback").join("evil.txt").exists(),
            "bypass A: ./.rollback/evil.txt must not be extracted"
        );
        // Case bypass: check both capitalizations since the FS may fold case.
        assert!(
            !dest.join(".rollback").join("evil2.txt").exists()
                && !dest.join(".ROLLBACK").join("evil2.txt").exists(),
            "bypass B: .ROLLBACK/evil2.txt must not be extracted"
        );

        // Safe sibling must be present.
        assert!(dest.join("safe.txt").exists(), "safe.txt must be extracted");
        assert_eq!(
            std::fs::read(dest.join("safe.txt")).unwrap(),
            b"safe-content"
        );

        // Interior .rollback must be present.
        assert!(
            dest.join("bin").join(".rollback").join("x").exists(),
            "bin/.rollback/x (interior) must be extracted"
        );

        // Progress must complete: last call has done == total.
        assert!(!calls.is_empty());
        let (done, total) = *calls.last().unwrap();
        assert_eq!(done, total, "progress must complete even when hostile entries are skipped");
    }

    /// Build a tar.gz with:
    ///   safe.txt           – normal entry
    ///   .rollback/evil.txt – bypass C (tar path, no enclosed_name guard)
    fn make_bypass_targz(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("bypass.tar.gz");
        let file = std::fs::File::create(&path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);

        let mut header = tar::Header::new_gnu();
        header.set_size(12);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "safe.txt", &b"safe-content"[..]).unwrap();

        let mut header2 = tar::Header::new_gnu();
        header2.set_size(9);
        header2.set_mode(0o644);
        header2.set_cksum();
        tar.append_data(&mut header2, ".rollback/evil.txt", &b"evil-tar1"[..]).unwrap();

        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    /// list_entries (tar) must omit .rollback/evil.txt.
    #[test]
    fn list_entries_targz_skips_rollback_entry() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_bypass_targz(dir.path());
        let entries = list_entries(&archive, ArchiveFormat::TarGz).unwrap();
        assert!(entries.contains(&"safe.txt".to_string()), "safe.txt must be listed");
        assert!(
            !entries.iter().any(|e| e.contains("evil")),
            "hostile .rollback entry must be omitted: {entries:?}"
        );
    }

    /// extract_archive (tar) must NOT write .rollback/evil.txt under dest,
    /// MUST write safe.txt, and progress must complete.
    #[test]
    fn extract_targz_skips_rollback_entry_and_progress_completes() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_bypass_targz(dir.path());
        let dest = dir.path().join("install");
        let mut calls: Vec<(usize, usize)> = vec![];
        extract_archive(&archive, ArchiveFormat::TarGz, &dest, &mut |d, t| {
            calls.push((d, t))
        })
        .unwrap();

        assert!(
            !dest.join(".rollback").join("evil.txt").exists(),
            "bypass C: .rollback/evil.txt must not be extracted from tar"
        );
        assert!(dest.join("safe.txt").exists(), "safe.txt must be extracted from tar");

        // Progress must complete.
        let real_calls: Vec<_> = calls.iter().filter(|&&(_, t)| t > 0).collect();
        assert!(!real_calls.is_empty(), "expected progress calls");
        let &(done, total) = real_calls.last().unwrap();
        assert_eq!(done, total, "last progress call must have done == total");
    }
}
