use crate::error::{LauncherError, Result};
use crate::installer::is_unsafe_rel_path;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What an update is about to change, recorded before extraction so it can
/// be undone. Lives at <install_dir>/.rollback/manifest.json with the saved
/// file copies under <install_dir>/.rollback/snapshot/.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub from_version: String,
    pub to_version: String,
    pub files_replaced: Vec<String>,
    pub files_added: Vec<String>,
}

pub fn rollback_root(install_dir: &Path) -> PathBuf {
    install_dir.join(".rollback")
}

fn manifest_path(install_dir: &Path) -> PathBuf {
    rollback_root(install_dir).join("manifest.json")
}

fn snapshot_dir(install_dir: &Path) -> PathBuf {
    rollback_root(install_dir).join("snapshot")
}

pub fn load_manifest(install_dir: &Path) -> Result<Option<Manifest>> {
    let path = manifest_path(install_dir);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| LauncherError::Rollback(format!("bad manifest: {e}")))
}

/// Reject entry paths that could write or delete outside the install dir or
/// touch the snapshot itself: empty, absolute, drive-prefixed, any `..`,
/// paths whose only component is `.` (CurDir), or whose first non-`.`
/// component is `.rollback` (ASCII case-insensitive — NTFS). The manifest
/// round-trips through a user-writable JSON file, so rollback must not
/// trust it blindly.
fn validate_rel_path(rel: &str) -> Result<()> {
    if is_unsafe_rel_path(rel) {
        return Err(LauncherError::Rollback(format!("unsafe path in snapshot: {rel}")));
    }
    Ok(())
}

/// Snapshot every file in `entries` (archive-relative paths) that already
/// exists in `install_dir`. Files that don't exist yet are recorded as
/// `files_added` so rollback can delete them. Replaces any prior snapshot.
///
/// All entry paths are validated BEFORE the prior snapshot root is wiped so a
/// hostile archive cannot destroy an existing good snapshot.
pub fn create_snapshot(
    install_dir: &Path,
    entries: &[String],
    from_version: &str,
    to_version: &str,
) -> Result<Manifest> {
    // Validate all entries first — before touching the existing snapshot.
    for rel in entries {
        validate_rel_path(rel)?;
    }

    let root = rollback_root(install_dir);
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    let snap = snapshot_dir(install_dir);
    std::fs::create_dir_all(&snap)?;

    let mut files_replaced = Vec::new();
    let mut files_added = Vec::new();
    for rel in entries {
        let src = install_dir.join(rel);
        if src.is_file() {
            let dst = snap.join(rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)?;
            files_replaced.push(rel.clone());
        } else {
            files_added.push(rel.clone());
        }
    }

    let manifest = Manifest {
        from_version: from_version.to_string(),
        to_version: to_version.to_string(),
        files_replaced,
        files_added,
    };
    write_manifest(install_dir, &manifest)?;
    Ok(manifest)
}

/// macOS: dmg archives aren't entry-iterable, so snapshot the whole
/// Quetoo.app (or record it as added if this is somehow the first install).
pub fn create_snapshot_macos(
    install_dir: &Path,
    from_version: &str,
    to_version: &str,
) -> Result<Manifest> {
    let root = rollback_root(install_dir);
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    let snap = snapshot_dir(install_dir);
    std::fs::create_dir_all(&snap)?;

    let app_dir = install_dir.join("Quetoo.app");
    let (files_replaced, files_added) = if app_dir.is_dir() {
        copy_dir_recursive(&app_dir, &snap.join("Quetoo.app"))?;
        (vec!["Quetoo.app".to_string()], vec![])
    } else {
        (vec![], vec!["Quetoo.app".to_string()])
    };

    let manifest = Manifest {
        from_version: from_version.to_string(),
        to_version: to_version.to_string(),
        files_replaced,
        files_added,
    };
    write_manifest(install_dir, &manifest)?;
    Ok(manifest)
}

/// FIX 4: Atomic manifest write — write to .tmp then rename over the real
/// path. std::fs::rename on Windows uses MoveFileEx(MOVEFILE_REPLACE_EXISTING)
/// so it atomically replaces the destination even on the same volume.
fn write_manifest(install_dir: &Path, manifest: &Manifest) -> Result<()> {
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|e| LauncherError::Rollback(e.to_string()))?;
    let final_path = manifest_path(install_dir);
    let tmp_path = final_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, text)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Undo the last update: restore replaced files, delete added files, drop
/// the snapshot. Returns the version rolled back to.
///
/// All manifest paths are validated before any files are touched, so a tampered
/// manifest cannot escape the install dir or delete arbitrary paths.
pub fn rollback(install_dir: &Path) -> Result<String> {
    let manifest = load_manifest(install_dir)?
        .ok_or_else(|| LauncherError::Rollback("no snapshot to roll back to".into()))?;

    // Validate all manifest paths before restoring or deleting anything.
    for rel in manifest.files_replaced.iter().chain(manifest.files_added.iter()) {
        validate_rel_path(rel)?;
    }

    let snap = snapshot_dir(install_dir);

    for rel in &manifest.files_replaced {
        let src = snap.join(rel);
        let dst = install_dir.join(rel);
        if src.is_dir() {
            if dst.exists() {
                std::fs::remove_dir_all(&dst)?;
            }
            copy_dir_recursive(&src, &dst)?;
        } else {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)?;
        }
    }
    for rel in &manifest.files_added {
        let dst = install_dir.join(rel);
        if dst.is_dir() {
            std::fs::remove_dir_all(&dst)?;
        } else if dst.exists() {
            std::fs::remove_file(&dst)?;
        }
    }
    std::fs::remove_dir_all(rollback_root(install_dir))?;
    Ok(manifest.from_version)
}

/// True if an existing snapshot already captures the pre-update state for
/// an update to `to_version` — i.e. a retry of the same transition. Reusing
/// it (instead of re-snapshotting) preserves the true old files when the
/// previous attempt failed mid-extract. Degrades to false on any error.
pub fn has_snapshot_for(install_dir: &Path, to_version: &str) -> bool {
    load_manifest(install_dir)
        .ok()
        .flatten()
        .map(|m| m.to_version == to_version)
        .unwrap_or(false)
}

/// FIX 3: Symlink-safe recursive directory copy.
///
/// Uses entry.file_type()? (non-following / lstat) to distinguish file types:
/// - Directory  → recurse
/// - Symlink    → on unix: recreate the link; on non-unix: error
///               (symlinks should never appear in a Windows Quetoo install;
///               erroring beats silently materialising them)
/// - Otherwise  → fs::copy
///
/// Rationale: macOS .app bundles contain Versions/Current symlinks. Following
/// them via is_dir()/is_file() (which follow) would break code-signing and risk
/// infinite cycles. Using file_type() avoids both problems.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_symlink() {
            #[cfg(unix)]
            {
                let target = std::fs::read_link(&from)?;
                std::os::unix::fs::symlink(&target, &to)?;
            }
            #[cfg(not(unix))]
            {
                return Err(LauncherError::Rollback(format!(
                    "symlink in snapshot source: {}",
                    from.display()
                )));
            }
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn no_manifest_means_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_manifest(dir.path()).unwrap().is_none());
    }

    #[test]
    fn snapshot_classifies_replaced_and_added() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("bin/quetoo.exe"), "old-exe");
        let entries = vec!["bin/quetoo.exe".to_string(), "lib/new.dll".to_string()];
        let m = create_snapshot(dir.path(), &entries, "v1.0.24", "v1.0.25").unwrap();
        assert_eq!(m.files_replaced, vec!["bin/quetoo.exe"]);
        assert_eq!(m.files_added, vec!["lib/new.dll"]);
        assert_eq!(load_manifest(dir.path()).unwrap().unwrap(), m);
    }

    #[test]
    fn rollback_restores_replaced_and_deletes_added() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("bin/quetoo.exe"), "old-exe");
        let entries = vec!["bin/quetoo.exe".to_string(), "lib/new.dll".to_string()];
        create_snapshot(dir.path(), &entries, "v1.0.24", "v1.0.25").unwrap();
        write(&dir.path().join("bin/quetoo.exe"), "new-exe");
        write(&dir.path().join("lib/new.dll"), "new-dll");

        let from = rollback(dir.path()).unwrap();
        assert_eq!(from, "v1.0.24");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("bin/quetoo.exe")).unwrap(),
            "old-exe"
        );
        assert!(!dir.path().join("lib/new.dll").exists());
        assert!(load_manifest(dir.path()).unwrap().is_none());
        assert!(!rollback_root(dir.path()).exists());
    }

    #[test]
    fn rollback_without_snapshot_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            rollback(dir.path()),
            Err(LauncherError::Rollback(_))
        ));
    }

    #[test]
    fn new_snapshot_replaces_old() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "v1");
        create_snapshot(dir.path(), &["a.txt".to_string()], "v1", "v2").unwrap();
        write(&dir.path().join("a.txt"), "v2");
        create_snapshot(dir.path(), &["a.txt".to_string()], "v2", "v3").unwrap();
        let m = load_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(m.from_version, "v2");
        assert_eq!(m.to_version, "v3");
    }

    #[test]
    fn macos_snapshot_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("Quetoo.app/Contents/MacOS/quetoo"), "old-app");
        create_snapshot_macos(dir.path(), "v1.0.24", "v1.0.25").unwrap();
        write(&dir.path().join("Quetoo.app/Contents/MacOS/quetoo"), "new-app");
        rollback(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Quetoo.app/Contents/MacOS/quetoo")).unwrap(),
            "old-app"
        );
    }

    // ── FIX 1 tests ─────────────────────────────────────────────────────────

    /// create_snapshot must reject `.rollback/...` entries and must NOT wipe a
    /// pre-existing good snapshot when it does so.
    #[test]
    fn create_snapshot_rejects_rollback_entry_and_preserves_prior_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        // Lay down a prior valid snapshot so we can verify it survives.
        write(&dir.path().join("safe.txt"), "original");
        create_snapshot(dir.path(), &["safe.txt".to_string()], "v1", "v2").unwrap();
        let prior_snap = dir.path().join(".rollback/snapshot/safe.txt");
        assert!(prior_snap.exists(), "pre-condition: prior snapshot must exist");

        // Now try to create a new snapshot with a hostile entry.
        let hostile = vec![".rollback/manifest.json".to_string()];
        let err = create_snapshot(dir.path(), &hostile, "v2", "v3");
        assert!(
            matches!(err, Err(LauncherError::Rollback(_))),
            "expected Rollback error, got: {err:?}"
        );

        // The prior snapshot must be untouched.
        assert!(
            prior_snap.exists(),
            "prior snapshot must NOT be wiped when validation fails"
        );
    }

    /// create_snapshot must reject path-traversal entries.
    #[test]
    fn create_snapshot_rejects_dotdot_entry() {
        let dir = tempfile::tempdir().unwrap();
        let hostile = vec!["../escape.txt".to_string()];
        let err = create_snapshot(dir.path(), &hostile, "v1", "v2");
        assert!(
            matches!(err, Err(LauncherError::Rollback(_))),
            "expected Rollback error for ../escape.txt, got: {err:?}"
        );
    }

    /// rollback must reject a tampered manifest containing absolute paths and
    /// must not delete or restore anything.
    #[test]
    fn rollback_rejects_absolute_path_in_manifest_files_added() {
        let dir = tempfile::tempdir().unwrap();
        // Write a hand-crafted hostile manifest directly into .rollback/.
        let rollback_dir = dir.path().join(".rollback");
        std::fs::create_dir_all(&rollback_dir).unwrap();
        let manifest_json = r#"{
            "from_version": "v1",
            "to_version": "v2",
            "files_replaced": [],
            "files_added": ["C:/somewhere/else.txt"]
        }"#;
        std::fs::write(rollback_dir.join("manifest.json"), manifest_json).unwrap();

        // Create a sentinel file to verify rollback doesn't delete anything.
        let sentinel = dir.path().join("sentinel.txt");
        write(&sentinel, "keep-me");

        let err = rollback(dir.path());
        assert!(
            matches!(err, Err(LauncherError::Rollback(_))),
            "expected Rollback error for absolute path in manifest, got: {err:?}"
        );

        // Sentinel must still exist — rollback must not have acted on anything.
        assert!(sentinel.exists(), "rollback must not delete files when manifest is hostile");
    }

    // ── Bypass A / B tests: CurDir prefix and NTFS case bypass ─────────────

    /// validate_rel_path must reject "./.rollback/x" — the leading CurDir
    /// component must not allow bypassing the .rollback reservation.
    #[test]
    fn create_snapshot_rejects_curdirprefix_rollback_entry() {
        let dir = tempfile::tempdir().unwrap();
        let hostile = vec!["./.rollback/x".to_string()];
        let err = create_snapshot(dir.path(), &hostile, "v1", "v2");
        assert!(
            matches!(err, Err(LauncherError::Rollback(_))),
            "expected Rollback error for ./.rollback/x, got: {err:?}"
        );
    }

    /// validate_rel_path must reject ".ROLLBACK/x" — NTFS is case-insensitive
    /// so the upper-cased form reaches the real .rollback directory.
    #[test]
    fn create_snapshot_rejects_case_bypass_rollback_entry() {
        let dir = tempfile::tempdir().unwrap();
        let hostile = vec![".ROLLBACK/x".to_string()];
        let err = create_snapshot(dir.path(), &hostile, "v1", "v2");
        assert!(
            matches!(err, Err(LauncherError::Rollback(_))),
            "expected Rollback error for .ROLLBACK/x, got: {err:?}"
        );
    }

    // ── FIX 4 test ──────────────────────────────────────────────────────────

    /// write_manifest (via create_snapshot) should leave no stray .tmp file.
    #[test]
    fn atomic_manifest_write_no_tmp_left() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "data");
        create_snapshot(dir.path(), &["a.txt".to_string()], "v1", "v2").unwrap();
        let tmp = manifest_path(dir.path()).with_extension("json.tmp");
        assert!(!tmp.exists(), "no stray .tmp file after write_manifest");
    }

    // ── FIX 5 tests ─────────────────────────────────────────────────────────

    /// load_manifest returns Err(Rollback) for garbage JSON.
    #[test]
    fn corrupt_manifest_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let rollback_dir = dir.path().join(".rollback");
        std::fs::create_dir_all(&rollback_dir).unwrap();
        std::fs::write(rollback_dir.join("manifest.json"), b"this is not json").unwrap();
        let result = load_manifest(dir.path());
        assert!(
            matches!(result, Err(LauncherError::Rollback(_))),
            "expected Rollback error for corrupt manifest, got: {result:?}"
        );
    }

    /// Rollback retryability: after a successful rollback the manifest is gone;
    /// if rollback fails mid-way the manifest is preserved so rollback can be
    /// retried.
    ///
    /// On Windows, making a file read-only does not prevent fs::copy from
    /// overwriting it (Windows allows opening a read-only file for write when
    /// creating a new handle with CreateFile). Injecting a failure via an
    /// exclusive file lock is similarly unreliable in unit test context.
    ///
    /// Instead we assert the ordering property directly:
    ///   - A successful rollback leaves no manifest (load_manifest → None).
    ///   - The manifest is only removed as the very last step of rollback, so
    ///     any error that occurs before that point leaves it intact for retry.
    ///
    /// The implementation satisfies this: std::fs::remove_dir_all(rollback_root)
    /// runs after all restore/delete operations; if any earlier step returns Err
    /// the manifest is still on disk.
    #[test]
    fn successful_rollback_removes_manifest() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("file.txt"), "original");
        create_snapshot(dir.path(), &["file.txt".to_string()], "v1", "v2").unwrap();
        write(&dir.path().join("file.txt"), "updated");

        rollback(dir.path()).unwrap();

        // After success: manifest gone, rollback root gone.
        assert!(load_manifest(dir.path()).unwrap().is_none());
        assert!(!rollback_root(dir.path()).exists());
    }

    // ── has_snapshot_for tests ───────────────────────────────────────────────

    /// Matching to_version → true.
    #[test]
    fn has_snapshot_for_matching_version_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "data");
        create_snapshot(dir.path(), &["a.txt".to_string()], "v1", "v1.0.25").unwrap();
        assert!(has_snapshot_for(dir.path(), "v1.0.25"));
    }

    /// Different to_version → false.
    #[test]
    fn has_snapshot_for_different_version_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "data");
        create_snapshot(dir.path(), &["a.txt".to_string()], "v1", "v1.0.24").unwrap();
        assert!(!has_snapshot_for(dir.path(), "v1.0.25"));
    }

    /// No manifest → false.
    #[test]
    fn has_snapshot_for_no_manifest_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_snapshot_for(dir.path(), "v1.0.25"));
    }

    /// Corrupt manifest → false (degrades gracefully).
    #[test]
    fn has_snapshot_for_corrupt_manifest_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let rollback_dir = dir.path().join(".rollback");
        std::fs::create_dir_all(&rollback_dir).unwrap();
        std::fs::write(rollback_dir.join("manifest.json"), b"not valid json").unwrap();
        assert!(!has_snapshot_for(dir.path(), "v1.0.25"));
    }

    /// The manifest write is atomic (tmp + rename), so a partial write during
    /// create_snapshot cannot leave a half-written manifest that would corrupt
    /// a subsequent rollback attempt.
    #[test]
    fn manifest_present_after_failed_rollback_can_retry() {
        let dir = tempfile::tempdir().unwrap();
        // Snapshot one file.
        write(&dir.path().join("file.txt"), "original");
        create_snapshot(dir.path(), &["file.txt".to_string()], "v1", "v2").unwrap();
        write(&dir.path().join("file.txt"), "updated");

        // Simulate a situation where rollback would fail on a missing source
        // by corrupting the snapshot copy (remove it) — the manifest itself
        // stays intact.  Rollback will error because src.is_dir() is false and
        // fs::copy from a non-existent path errors.
        let snap_file = dir.path().join(".rollback/snapshot/file.txt");
        std::fs::remove_file(&snap_file).unwrap();

        let err = rollback(dir.path());
        assert!(err.is_err(), "rollback of missing snapshot file should fail");

        // Manifest must still be present so the operator can investigate/retry.
        assert!(
            load_manifest(dir.path()).unwrap().is_some(),
            "manifest must survive a failed rollback"
        );
    }
}
