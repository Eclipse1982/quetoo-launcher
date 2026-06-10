use crate::error::{LauncherError, Result};
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

/// Snapshot every file in `entries` (archive-relative paths) that already
/// exists in `install_dir`. Files that don't exist yet are recorded as
/// `files_added` so rollback can delete them. Replaces any prior snapshot.
pub fn create_snapshot(
    install_dir: &Path,
    entries: &[String],
    from_version: &str,
    to_version: &str,
) -> Result<Manifest> {
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

fn write_manifest(install_dir: &Path, manifest: &Manifest) -> Result<()> {
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|e| LauncherError::Rollback(e.to_string()))?;
    std::fs::write(manifest_path(install_dir), text)?;
    Ok(())
}

/// Undo the last update: restore replaced files, delete added files, drop
/// the snapshot. Returns the version rolled back to.
pub fn rollback(install_dir: &Path) -> Result<String> {
    let manifest = load_manifest(install_dir)?
        .ok_or_else(|| LauncherError::Rollback("no snapshot to roll back to".into()))?;
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

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
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
}
