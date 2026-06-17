//! Reading game-data images (crosshairs, player skins) for the launcher UI,
//! returned as base64 `data:` URLs so the webview can render them directly
//! without configuring a custom asset protocol for a user-chosen install dir.

use crate::error::{LauncherError, Result};
use crate::installer;
use base64::Engine;
use serde::Serialize;
use std::path::Path;

fn data_url_png(bytes: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Read a PNG at `<install_dir>/share/default/<rel>` as a base64 data URL.
/// `rel` is guarded against path traversal / the reserved rollback area.
pub fn read_data_image(install_dir: &Path, rel: &str) -> Result<String> {
    if installer::is_unsafe_rel_path(rel) {
        return Err(LauncherError::Config(format!("unsafe path: {rel}")));
    }
    let mut path = install_dir.join("share").join("default");
    for comp in rel.split('/') {
        if !comp.is_empty() {
            path = path.join(comp);
        }
    }
    let bytes = std::fs::read(&path)?;
    Ok(data_url_png(&bytes))
}

/// One selectable player model/skin with its preview icon.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinInfo {
    /// `"model/skin"` — the value for the `skin` cvar.
    pub id: String,
    pub model: String,
    pub skin: String,
    /// base64 data URL of `<skin>_i.png`, if present.
    pub icon: Option<String>,
}

/// Scan `<install_dir>/share/default/players/<model>/*.skin` for selectable
/// model/skin combinations, attaching each one's `<skin>_i.png` icon. Returns an
/// empty list (not an error) when no players directory exists yet.
pub fn list_skins(install_dir: &Path) -> Result<Vec<SkinInfo>> {
    let players = install_dir.join("share").join("default").join("players");
    let mut out: Vec<SkinInfo> = Vec::new();
    let Ok(models) = std::fs::read_dir(&players) else {
        return Ok(out);
    };
    for model_entry in models.flatten() {
        if !model_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let model = model_entry.file_name().to_string_lossy().into_owned();
        let model_dir = model_entry.path();
        let Ok(files) = std::fs::read_dir(&model_dir) else {
            continue;
        };
        let mut skins: Vec<String> = files
            .flatten()
            .filter_map(|f| {
                let name = f.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".skin").map(|s| s.to_string())
            })
            .collect();
        skins.sort();
        skins.dedup();
        for skin in skins {
            let icon_path = model_dir.join(format!("{skin}_i.png"));
            let icon = std::fs::read(&icon_path).ok().map(|b| data_url_png(&b));
            out.push(SkinInfo {
                id: format!("{model}/{skin}"),
                model: model.clone(),
                skin,
                icon,
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn read_data_image_rejects_unsafe_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_data_image(dir.path(), "../evil.png").is_err());
        assert!(read_data_image(dir.path(), ".rollback/x.png").is_err());
    }

    #[test]
    fn read_data_image_returns_data_url() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("share").join("default").join("pics");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::File::create(p.join("ch1.png"))
            .unwrap()
            .write_all(b"PNGDATA")
            .unwrap();
        let url = read_data_image(dir.path(), "pics/ch1.png").unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn list_skins_empty_when_no_players_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_skins(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn list_skins_finds_models_and_skins_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let qf = dir
            .path()
            .join("share")
            .join("default")
            .join("players")
            .join("qforcer");
        std::fs::create_dir_all(&qf).unwrap();
        std::fs::write(qf.join("default.skin"), "x").unwrap();
        std::fs::write(qf.join("ctf.skin"), "x").unwrap();
        std::fs::write(qf.join("default_i.png"), b"icon").unwrap();
        std::fs::write(qf.join("animation.cfg"), "x").unwrap(); // ignored

        let skins = list_skins(dir.path()).unwrap();
        let ids: Vec<&str> = skins.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["qforcer/ctf", "qforcer/default"]);

        let def = skins.iter().find(|s| s.skin == "default").unwrap();
        assert!(def.icon.as_ref().unwrap().starts_with("data:image/png;base64,"));
        let ctf = skins.iter().find(|s| s.skin == "ctf").unwrap();
        assert!(ctf.icon.is_none());
    }
}
