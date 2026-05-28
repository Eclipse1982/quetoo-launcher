use crate::error::{LauncherError, Result};
use std::path::{Path, PathBuf};

/// Relative path (from the install dir) to the thing we launch, per OS.
/// On macOS this is the `.app` bundle, launched via `open`.
pub fn executable_rel_path(os: &str) -> Result<&'static str> {
    match os {
        "windows" => Ok("bin/quetoo.exe"),
        "linux" => Ok("bin/quetoo"),
        "macos" => Ok("Quetoo.app"),
        other => Err(LauncherError::UnsupportedPlatform(other.to_string())),
    }
}

/// Absolute path to the launch target inside `install_dir`.
pub fn executable_path(install_dir: &Path, os: &str) -> Result<PathBuf> {
    Ok(install_dir.join(executable_rel_path(os)?))
}

/// Launch Quetoo from the given install dir for the current OS.
pub fn launch(install_dir: &Path) -> Result<()> {
    let os = std::env::consts::OS;
    let target = executable_path(install_dir, os)?;
    if !target.exists() {
        return Err(LauncherError::Launch(format!(
            "executable not found at {}",
            target.display()
        )));
    }

    let mut command = if os == "macos" {
        let mut c = std::process::Command::new("open");
        c.arg(&target);
        c
    } else {
        std::process::Command::new(&target)
    };
    // Run the game from the install dir so it finds its data.
    command.current_dir(install_dir);
    command
        .spawn()
        .map_err(|e| LauncherError::Launch(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_executable_path() {
        let p = executable_path(Path::new("/games/quetoo"), "windows").unwrap();
        assert!(p.ends_with("bin/quetoo.exe"));
    }

    #[test]
    fn linux_executable_path() {
        let p = executable_path(Path::new("/games/quetoo"), "linux").unwrap();
        assert!(p.ends_with("bin/quetoo"));
    }

    #[test]
    fn macos_executable_is_app_bundle() {
        let p = executable_path(Path::new("/games/quetoo"), "macos").unwrap();
        assert!(p.ends_with("Quetoo.app"));
    }

    #[test]
    fn unsupported_os_errors() {
        assert!(matches!(
            executable_rel_path("freebsd"),
            Err(LauncherError::UnsupportedPlatform(_))
        ));
    }
}
