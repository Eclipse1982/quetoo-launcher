use crate::error::{LauncherError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

// ── Section A — user dir + paths ─────────────────────────────────────────────

/// Pure path builder mirroring SDL_GetPrefPath("WickedOldGames", "Quetoo").
/// `env` returns the value of an environment variable, if set.
fn user_dir_from_env(os: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf> {
    let base = match os {
        "windows" => env("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| LauncherError::Config("APPDATA not set".into()))?,
        "macos" => {
            let home = env("HOME").ok_or_else(|| LauncherError::Config("HOME not set".into()))?;
            PathBuf::from(home).join("Library").join("Application Support")
        }
        "linux" => {
            if let Some(xdg) = env("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
                PathBuf::from(xdg)
            } else {
                let home = env("HOME").ok_or_else(|| LauncherError::Config("HOME not set".into()))?;
                PathBuf::from(home).join(".local").join("share")
            }
        }
        other => return Err(LauncherError::UnsupportedPlatform(other.to_string())),
    };
    Ok(base.join("WickedOldGames").join("Quetoo"))
}

/// Quetoo per-user directory for the current OS.
pub fn quetoo_user_dir() -> Result<PathBuf> {
    user_dir_from_env(std::env::consts::OS, &|k| std::env::var(k).ok())
}

/// Path to the user-owned autoexec.cfg (base game = "default").
pub fn autoexec_path() -> Result<PathBuf> {
    Ok(quetoo_user_dir()?.join("default").join("autoexec.cfg"))
}

// ── Section B — Settings model + curated tables ───────────────────────────────

/// (cvar name, default value) for the curated cvar fields, in display order.
pub const CVARS: &[(&str, &str)] = &[
    // Video
    ("r_fullscreen", "1"), ("r_fullscreen_width", "0"), ("r_fullscreen_height", "0"),
    ("r_window_width", "1920"), ("r_window_height", "1080"), ("r_swap_interval", "1"),
    ("cl_max_fps", "-1"), ("r_draw_scale", "1"), ("r_anisotropy", "16"),
    ("r_antialias", "0"), ("r_modulate", "1"), ("r_saturation", "1"),
    ("r_bloom", "4"), ("r_shadows", "1"),
    // Audio
    ("s_volume", "1"), ("s_effects_volume", "1"), ("s_music_volume", "0.5"),
    ("s_ambient_volume", "1"), ("s_hrtf", "0"), ("cg_hit_sound", "1"),
    // Mouse
    ("m_sensitivity", "3.0"), ("m_sensitivity_zoom", "1.0"), ("m_invert", "0"),
    ("m_interpolate", "0"), ("cg_run", "1"),
    // Player
    ("name", ""), ("skin", "qforcer/default"), ("hand", "1"),
    ("auto_switch", "1"), ("hook_style", "pull"),
    // View & HUD
    ("cg_fov", "110"), ("cg_fov_zoom", "55"), ("cg_draw_hud", "1"),
    ("cg_draw_weapon", "1"), ("cg_draw_weapon_bob", "1"), ("cg_bob", "1"),
    ("cg_draw_blend_damage", "1"), ("cl_draw_counters", "1"),
    ("cl_draw_net_graph", "1"), ("cg_third_person_chasecam", "0"),
    // Crosshair
    ("cg_draw_crosshair", "1"), ("cg_draw_crosshair_scale", "1"),
    ("cg_draw_crosshair_color", "default"), ("cg_draw_crosshair_alpha", "1.0"),
    ("cg_draw_crosshair_health", "0"), ("cg_draw_crosshair_pulse", "1"),
];

/// (action label, bind command, default key) for curated bindings, in display order.
pub const BINDINGS: &[(&str, &str, &str)] = &[
    // Movement
    ("Move forward", "+forward", "w"), ("Move back", "+back", "s"),
    ("Move left", "+move_left", "a"), ("Move right", "+move_right", "d"),
    ("Jump", "+move_up", "space"), ("Crouch", "+move_down", "c"),
    ("Run/Walk", "+speed", "left shift"), ("Center view", "center_view", "home"),
    // Combat
    ("Attack", "+attack", "mouse 1"), ("Hook", "+hook", "mouse 2"),
    ("Zoom", "+ZOOM", "left alt"),
    ("Next weapon", "cg_weapon_next", "mouse wheel down"),
    ("Previous weapon", "cg_weapon_previous", "mouse wheel up"),
    ("Last weapon", "weapon_last", ""), ("Show score", "+score", "tab"),
    ("Kill/respawn", "kill", ""),
    // Weapons
    ("Blaster", "use blaster", "1"), ("Shotgun", "use shotgun", "2"),
    ("Super shotgun", "use super shotgun", "3"), ("Machinegun", "use machinegun", "4"),
    ("Hand grenades", "use hand grenades", "g"),
    ("Grenade launcher", "use grenade launcher", "5"),
    ("Rocket launcher", "use rocket launcher", "6"),
    ("Hyperblaster", "use hyperblaster", "7"),
    ("Lightning gun", "use lightning gun", "8"), ("Railgun", "use railgun", "9"),
    ("BFG10K", "use bfg10k", "0"),
    // Communication
    ("Chat", "cl_message_mode", "t"), ("Team chat", "cl_message_mode_2", "y"),
    // Misc
    ("Screenshot", "r_screenshot", "f12"), ("Toggle console", "cl_toggle_console", "`"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// cvar name -> value, for the curated CVARS.
    pub cvars: BTreeMap<String, String>,
    /// bind command -> key, for the curated BINDINGS.
    pub bindings: BTreeMap<String, String>,
}

impl Settings {
    /// Settings populated with the documented defaults.
    pub fn defaults() -> Settings {
        Settings {
            cvars: CVARS.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
            bindings: BINDINGS.iter().map(|(_, cmd, key)| (cmd.to_string(), key.to_string())).collect(),
        }
    }
}

// ── Section C — tokenizer + parse ─────────────────────────────────────────────

/// Split a config line into tokens, treating double-quoted runs as one token
/// (quotes stripped). Returns an empty vec for comment/blank lines.
pub fn tokenize(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with("//") {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    for ch in trimmed.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

/// The command portion of a tokenized `bind <key> <command...>` line, ignoring
/// any trailing inline `// comment` tokens.
fn bind_command(tokens: &[String]) -> String {
    tokens[2..]
        .iter()
        .take_while(|tok| !tok.starts_with("//"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse autoexec.cfg text into Settings (defaults for anything not present).
pub fn parse_settings(text: &str) -> Settings {
    let mut settings = Settings::defaults();
    let managed_cvars: Vec<&str> = CVARS.iter().map(|(n, _)| *n).collect();
    let managed_cmds: Vec<&str> = BINDINGS.iter().map(|(_, c, _)| *c).collect();

    for line in text.lines() {
        let t = tokenize(line);
        if t.len() >= 3 && t[0] == "set" && managed_cvars.contains(&t[1].as_str()) {
            settings.cvars.insert(t[1].clone(), t[2].clone());
        } else if t.len() >= 3 && t[0] == "bind" {
            let command = bind_command(&t);
            if managed_cmds.contains(&command.as_str()) {
                settings.bindings.insert(command, t[1].clone());
            }
        }
    }
    settings
}

// ── Section D — round-trip write + load/save ──────────────────────────────────

/// Quote a token for writing if it is empty or contains whitespace.
fn quote_if_needed(s: &str) -> String {
    if s.is_empty() || s.chars().any(|c| c.is_whitespace()) {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

/// Produce new autoexec.cfg text: keep every unmanaged line verbatim, and
/// update-in-place (or append) the managed `set`/`bind` lines from `settings`.
pub fn render_autoexec(existing: &str, settings: &Settings) -> String {
    let managed_cvars: Vec<&str> = CVARS.iter().map(|(n, _)| *n).collect();
    let managed_cmds: Vec<&str> = BINDINGS.iter().map(|(_, c, _)| *c).collect();

    let mut written_cvars: std::collections::BTreeSet<String> = Default::default();
    let mut written_binds: std::collections::BTreeSet<String> = Default::default();
    let mut out: Vec<String> = Vec::new();

    for line in existing.lines() {
        let t = tokenize(line);
        if t.len() >= 3 && t[0] == "set" && managed_cvars.contains(&t[1].as_str()) {
            let name = &t[1];
            if written_cvars.contains(name) {
                continue; // collapse duplicates
            }
            let value = settings.cvars.get(name).cloned().unwrap_or_default();
            if name == "name" && value.is_empty() {
                continue; // don't write an empty name
            }
            out.push(format!("set {} {}", name, quote_if_needed(&value)));
            written_cvars.insert(name.clone());
        } else if t.len() >= 3 && t[0] == "bind" && managed_cmds.contains(&bind_command(&t).as_str()) {
            let command = bind_command(&t);
            if written_binds.contains(&command) {
                continue;
            }
            let key = settings.bindings.get(&command).cloned().unwrap_or_default();
            if key.is_empty() {
                written_binds.insert(command);
                continue;
            }
            out.push(format!("bind {} {}", quote_if_needed(&key), quote_if_needed(&command)));
            written_binds.insert(command);
        } else {
            out.push(line.to_string()); // verbatim
        }
    }

    // Append any managed entries not already present.
    for (name, _) in CVARS {
        if !written_cvars.contains(*name) {
            let value = settings.cvars.get(*name).cloned().unwrap_or_default();
            if *name == "name" && value.is_empty() {
                continue;
            }
            out.push(format!("set {} {}", name, quote_if_needed(&value)));
        }
    }
    for (_, command, _) in BINDINGS {
        if !written_binds.contains(*command) {
            let key = settings.bindings.get(*command).cloned().unwrap_or_default();
            if key.is_empty() {
                continue;
            }
            out.push(format!("bind {} {}", quote_if_needed(&key), quote_if_needed(command)));
        }
    }

    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Read settings from autoexec.cfg (defaults if missing).
pub fn load_settings() -> Result<Settings> {
    let path = autoexec_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(parse_settings(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::defaults()),
        Err(e) => Err(LauncherError::from(e)),
    }
}

/// Write settings to autoexec.cfg, preserving unmanaged lines. Creates dir/file.
pub fn save_settings(settings: &Settings) -> Result<()> {
    let path = autoexec_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let text = render_autoexec(&existing, settings);
    std::fs::write(&path, text)?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn windows_user_dir() {
        let env = env_with(&[("APPDATA", "C:\\Users\\J\\AppData\\Roaming")]);
        let p = user_dir_from_env("windows", &env).unwrap();
        assert!(p.ends_with("WickedOldGames/Quetoo") || p.ends_with("WickedOldGames\\Quetoo"));
        assert!(p.to_string_lossy().contains("Roaming"));
    }

    #[test]
    fn macos_user_dir() {
        let env = env_with(&[("HOME", "/Users/j")]);
        let p = user_dir_from_env("macos", &env).unwrap();
        assert_eq!(p, PathBuf::from("/Users/j/Library/Application Support/WickedOldGames/Quetoo"));
    }

    #[test]
    fn linux_prefers_xdg_then_falls_back() {
        let xdg = env_with(&[("XDG_DATA_HOME", "/x"), ("HOME", "/home/j")]);
        assert_eq!(user_dir_from_env("linux", &xdg).unwrap(), PathBuf::from("/x/WickedOldGames/Quetoo"));
        let home_only = env_with(&[("HOME", "/home/j")]);
        assert_eq!(user_dir_from_env("linux", &home_only).unwrap(), PathBuf::from("/home/j/.local/share/WickedOldGames/Quetoo"));
    }

    #[test]
    fn tokenize_handles_quotes_and_comments() {
        assert_eq!(tokenize("bind \"mouse 1\" +attack"), vec!["bind", "mouse 1", "+attack"]);
        assert_eq!(tokenize("set m_sensitivity \"3.0\""), vec!["set", "m_sensitivity", "3.0"]);
        assert_eq!(tokenize("  set cg_fov 110  "), vec!["set", "cg_fov", "110"]);
        assert!(tokenize("// a comment").is_empty());
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn parse_reads_managed_cvars_and_binds() {
        let text = "set cg_fov \"95\"\nbind q +move_up\nset m_sensitivity 5\n";
        let s = parse_settings(text);
        assert_eq!(s.cvars.get("cg_fov").unwrap(), "95");
        assert_eq!(s.cvars.get("m_sensitivity").unwrap(), "5");
        assert_eq!(s.bindings.get("+move_up").unwrap(), "q");
        assert_eq!(s.cvars.get("cg_draw_weapon").unwrap(), "1");
    }

    #[test]
    fn parse_ignores_unmanaged_lines() {
        let text = "set r_fullscreen 1\nbind p screenshot\n";
        let s = parse_settings(text);
        assert_eq!(s, Settings::defaults());
    }

    #[test]
    fn render_preserves_unmanaged_and_updates_managed() {
        let existing = "// my config\n\
                        set r_fullscreen 1\n\
                        set cg_fov 90\n\
                        bind \"mouse 1\" +attack\n\
                        bind p screenshot\n";
        let mut s = Settings::defaults();
        s.cvars.insert("cg_fov".into(), "120".into());
        s.bindings.insert("+attack".into(), "mouse 3".into());
        let out = render_autoexec(existing, &s);
        assert!(out.contains("// my config"));
        assert!(out.contains("set r_fullscreen 1"));
        assert!(out.contains("bind p screenshot"));
        assert!(out.contains("set cg_fov 120"));
        assert!(!out.contains("set cg_fov 90"));
        assert!(out.contains("bind \"mouse 3\" +attack"));
        assert!(out.contains("+move_up"));
    }

    #[test]
    fn render_then_parse_roundtrips() {
        let mut s = Settings::defaults();
        s.cvars.insert("name".into(), "Eclipse 1982".into());
        s.cvars.insert("m_sensitivity".into(), "4.5".into());
        s.bindings.insert("+move_up".into(), "mouse 3".into());
        let text = render_autoexec("", &s);
        let parsed = parse_settings(&text);
        assert_eq!(parsed.cvars.get("name").unwrap(), "Eclipse 1982");
        assert_eq!(parsed.cvars.get("m_sensitivity").unwrap(), "4.5");
        assert_eq!(parsed.bindings.get("+move_up").unwrap(), "mouse 3");
    }

    #[test]
    fn empty_name_is_not_written() {
        let s = Settings::defaults();
        let out = render_autoexec("", &s);
        assert!(!out.contains("set name"));
    }

    #[test]
    fn render_collapses_duplicate_managed_lines() {
        let existing = "set cg_fov 90\nset cg_fov 95\n";
        let mut s = Settings::defaults();
        s.cvars.insert("cg_fov".into(), "120".into());
        let out = render_autoexec(existing, &s);
        // Count only exact "set cg_fov <value>" lines, not cg_fov_zoom etc.
        let fov_count = out.lines()
            .filter(|l| l.starts_with("set cg_fov ") && !l.starts_with("set cg_fov_"))
            .count();
        assert_eq!(fov_count, 1);
        assert!(out.contains("set cg_fov 120"));
    }

    #[test]
    fn bind_with_inline_comment_is_managed_in_place() {
        // An annotated managed bind should be recognized and updated in place,
        // not passed through and then duplicated by an appended entry.
        let existing = "bind w +forward // my note\n";
        let mut s = Settings::defaults();
        s.bindings.insert("+forward".into(), "up".into());
        let out = render_autoexec(existing, &s);
        assert_eq!(out.matches("+forward").count(), 1);
        assert!(out.contains("bind up +forward"));
        // parsing also reads the annotated line
        assert_eq!(parse_settings(existing).bindings.get("+forward").unwrap(), "w");
    }

    #[test]
    fn multiword_bind_command_round_trips() {
        let s = {
            let mut s = Settings::defaults();
            s.bindings.insert("use railgun".into(), "9".into());
            s
        };
        let text = render_autoexec("", &s);
        assert!(text.contains("bind 9 \"use railgun\""), "got: {text}");
        let parsed = parse_settings(&text);
        assert_eq!(parsed.bindings.get("use railgun"), Some(&"9".to_string()));
    }

    #[test]
    fn empty_key_bind_is_not_written() {
        let mut s = Settings::defaults();
        s.bindings.insert("kill".into(), "".into());
        let text = render_autoexec("", &s);
        assert!(!text.contains("kill"), "unbound action leaked: {text}");
    }

    #[test]
    fn existing_bind_line_removed_when_unbound() {
        let mut s = Settings::defaults();
        s.bindings.insert("kill".into(), "".into());
        let text = render_autoexec("bind x kill\n", &s);
        assert!(!text.contains("kill"), "stale bind survived: {text}");
    }

    #[test]
    fn new_defaults_present() {
        let d = Settings::defaults();
        assert_eq!(d.cvars.get("r_fullscreen"), Some(&"1".to_string()));
        assert_eq!(d.cvars.get("s_music_volume"), Some(&"0.5".to_string()));
        assert_eq!(d.cvars.get("hook_style"), Some(&"pull".to_string()));
        assert_eq!(d.bindings.get("+move_down"), Some(&"c".to_string()));
        assert_eq!(d.bindings.get("cg_weapon_next"), Some(&"mouse wheel down".to_string()));
        assert_eq!(d.bindings.get("+ZOOM"), Some(&"left alt".to_string()));
        assert_eq!(d.bindings.get("kill"), Some(&"".to_string()));
        assert_eq!(d.cvars.len(), 46);
        assert_eq!(d.bindings.len(), 31);
    }

    #[test]
    fn full_tables_round_trip() {
        let d = Settings::defaults();
        let text = render_autoexec("", &d);
        let parsed = parse_settings(&text);
        // Unbound-by-default actions aren't written, so they parse as defaults
        // (empty) — full equality must still hold.
        assert_eq!(parsed, d);
    }
}
