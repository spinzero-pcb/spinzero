//! Reads the active KiCad colour theme + the project's drawing defaults so the
//! viewer and the SVGs render in the *user's* palette instead of a baked-in copy.
//!
//! KiCad keeps colours in an application-global theme file (NOT inside the
//! project): `<config>/colors/<theme>.json`, selected by
//! `<config>/kicad_common.json` → `appearance.color_theme` (absent ⇒ "user").
//! The config dir is `%APPDATA%\kicad` on Windows, `~/.config/kicad` on Linux
//! (or `$XDG_CONFIG_HOME/kicad`), `~/Library/Preferences/kicad` on macOS, each
//! with a version subdir (e.g. `9.0`); `KICAD*_CONFIG_HOME` overrides the root.
//!
//! When the theme can't be found — e.g. reviewing a third party's board on a
//! machine without their KiCad config — `load()` returns an empty palette and
//! every caller keeps its existing KiCad-Default value. So this only ever
//! *overrides* monochrome/baked defaults with the real theme, never regresses.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// Resolved KiCad palette: each map is `key -> #RRGGBB` for the keys present in
/// the active theme. `schematic` keys are flat (`wire`, `worksheet`,
/// `dnp_marker`, …); `board` keys flatten the nested `copper` group
/// (`copper.f`, `copper.in1`, …) alongside the flat ones (`f_silks`,
/// `edge_cuts`, …). Empty when no theme was found.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Theme {
    pub schematic: BTreeMap<String, String>,
    pub board: BTreeMap<String, String>,
}

/// Drawing defaults read from a project's `.kicad_pro` (`schematic.drawing`),
/// converted to mm. A `None` field means the renderer keeps its own default.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Drawing {
    /// Default wire/line width (mm). KiCad stores mils; 6 mil = 0.1524 mm.
    pub line_thickness_mm: Option<f64>,
    /// Default text height (mm). 50 mil = 1.27 mm.
    pub text_size_mm: Option<f64>,
}

impl Theme {
    pub fn is_empty(&self) -> bool {
        self.schematic.is_empty() && self.board.is_empty()
    }
    /// A schematic colour by KiCad key (e.g. `worksheet`, `dnp_marker`, `wire`).
    pub fn sch(&self, key: &str) -> Option<&str> {
        self.schematic.get(key).map(String::as_str)
    }
    /// A board colour by flattened KiCad key (e.g. `copper.f`, `f_silks`).
    pub fn board(&self, key: &str) -> Option<&str> {
        self.board.get(key).map(String::as_str)
    }
}

/// Parse KiCad's `rgb(r,g,b)` / `rgba(r,g,b,a)` into `(r, g, b, a)`; `a` defaults to
/// `1.0` for an opaque `rgb(…)`. None on a non-colour string.
fn rgba_parts(s: &str) -> Option<(f64, f64, f64, f64)> {
    let inner = s.trim().strip_prefix("rgb")?;
    let inner = inner.trim_start_matches('a');
    let inner = inner.trim().strip_prefix('(')?.strip_suffix(')')?;
    let parts: Vec<f64> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() < 3 {
        return None;
    }
    Some((parts[0], parts[1], parts[2], parts.get(3).copied().unwrap_or(1.0)))
}

/// Parse KiCad's `rgb(r,g,b)` / `rgba(r,g,b,a)` into `#RRGGBB`. Alpha is dropped —
/// for most layers we only need the hue; per-primitive transparency is the viewer's
/// concern. Translucent layers (mask/paste) are composited separately, see [`composite_over`].
fn rgb_hex(s: &str) -> Option<String> {
    let (r, g, b, _) = rgba_parts(s)?;
    Some(format!("#{:02X}{:02X}{:02X}", r as u8, g as u8, b as u8))
}

/// KiCad's solder-mask and paste layers are semi-transparent films (`rgba` with alpha
/// ~0.4/0.9); painting their raw hue opaquely reads far too saturated (e.g. B.Mask as
/// bright cyan instead of the faint teal KiCad shows). Approximate KiCad's on-canvas
/// look by compositing the layer's colour over the board background at its own alpha,
/// so the stored opaque hue matches what the designer sees in KiCad. Falls back to the
/// KiCad-default board background `rgb(0,16,35)` when the theme omits one.
fn composite_over(fg: &str, bg: (f64, f64, f64)) -> Option<String> {
    let (r, g, b, a) = rgba_parts(fg)?;
    let mix = |f: f64, k: f64| (f * a + k * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    Some(format!("#{:02X}{:02X}{:02X}", mix(r, bg.0), mix(g, bg.1), mix(b, bg.2)))
}

/// Flatten a JSON colour object into `out`. String leaves become hex entries;
/// nested objects (KiCad's `board.copper.{f,b,in1…}`) recurse with a dotted
/// prefix (`copper.f`). Non-colour leaves (`override_item_colors: false`) skip.
fn collect(obj: &Value, prefix: &str, out: &mut BTreeMap<String, String>) {
    let Some(map) = obj.as_object() else { return };
    for (k, v) in map {
        match v {
            Value::String(s) => {
                if let Some(hex) = rgb_hex(s) {
                    out.insert(format!("{prefix}{k}"), hex);
                }
            }
            Value::Object(_) => collect(v, &format!("{prefix}{k}."), out),
            _ => {}
        }
    }
}

/// KiCad's config root for this platform, honouring `KICAD*_CONFIG_HOME`.
fn kicad_config_root() -> Option<PathBuf> {
    for var in ["KICAD_CONFIG_HOME", "KICAD9_CONFIG_HOME", "KICAD8_CONFIG_HOME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return Some(PathBuf::from(v));
            }
        }
    }
    let base: Option<PathBuf> = {
        #[cfg(target_os = "windows")]
        {
            std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join("kicad"))
        }
        #[cfg(target_os = "macos")]
        {
            std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library/Preferences/kicad"))
        }
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
                .map(|c| c.join("kicad"))
        }
    };
    base
}

/// Versioned config dirs that hold `colors/`, **highest version first**. Some
/// KiCad layouts put `colors/` directly under the root; others nest it under a
/// version dir (`9.0`, `10.0`). Several versions can be installed side by side,
/// and the newest one's `colors/` is often empty (a fresh install whose user
/// still keeps their real theme under the previous version), so `load()` walks
/// these in order and uses the first that yields an actual theme.
fn config_dirs() -> Vec<PathBuf> {
    kicad_config_root().map(|r| config_dirs_in(&r)).unwrap_or_default()
}

/// Candidate config dirs under `root` (highest version first). Pulled out of
/// `config_dirs` so the version-ordering / fallback is unit-testable without
/// touching the real KiCad config.
fn config_dirs_in(root: &Path) -> Vec<PathBuf> {
    if root.join("colors").is_dir() {
        return vec![root.to_path_buf()];
    }
    let mut dirs: Vec<(f64, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.join("colors").is_dir() {
                let ver = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                dirs.push((ver, p));
            }
        }
    }
    dirs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    dirs.into_iter().map(|(_, p)| p).collect()
}

/// The active colour-theme name from `kicad_common.json`, defaulting to "user"
/// (KiCad's built-in editable default, which it omits from the file).
fn active_theme_name(config_dir: &Path) -> String {
    if let Ok(txt) = std::fs::read_to_string(config_dir.join("kicad_common.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&txt) {
            if let Some(name) = v["appearance"]["color_theme"].as_str() {
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    "user".to_string()
}

/// Load the active KiCad colour theme (application-global; KiCad has no per-project
/// theme today). Walks the installed versions newest-first and returns the first
/// non-empty palette, so a newer KiCad whose `colors/` is empty falls back to the
/// version that actually holds the user's theme. Empty `Theme` when none is found.
pub fn load() -> Theme {
    for dir in config_dirs() {
        let t = load_from_dir(&dir);
        if !t.is_empty() {
            return t;
        }
    }
    Theme::default()
}

/// Load the active theme out of a single config dir. Empty when its active theme
/// file can't be read/parsed (e.g. the dir's `colors/` is empty).
fn load_from_dir(dir: &Path) -> Theme {
    let name = active_theme_name(dir);
    let path = dir.join("colors").join(format!("{name}.json"));
    let Ok(txt) = std::fs::read_to_string(&path) else { return Theme::default() };
    let Ok(v) = serde_json::from_str::<Value>(&txt) else { return Theme::default() };
    let mut t = Theme::default();
    collect(&v["schematic"], "", &mut t.schematic);
    collect(&v["board"], "", &mut t.board);
    // Mask/paste are translucent in KiCad — `collect` above dropped their alpha and left
    // an over-saturated hue. Re-derive them by compositing the raw rgba over the board
    // background so the opaque hue matches KiCad's display (feedback: F/B.Mask read wrong).
    let bg = v["board"]["background"]
        .as_str()
        .and_then(rgba_parts)
        .map(|(r, g, b, _)| (r, g, b))
        .unwrap_or((0.0, 16.0, 35.0));
    for key in ["f_mask", "b_mask", "f_paste", "b_paste"] {
        if let Some(hex) = v["board"][key].as_str().and_then(|s| composite_over(s, bg)) {
            t.board.insert(key.to_string(), hex);
        }
    }
    t
}

/// Read a project's `schematic.drawing` defaults (mils → mm). Empty on any
/// read/parse problem.
pub fn drawing(pro: &Path) -> Drawing {
    let mut d = Drawing::default();
    let Ok(txt) = std::fs::read_to_string(pro) else { return d };
    let Ok(v) = serde_json::from_str::<Value>(&txt) else { return d };
    let dr = &v["schematic"]["drawing"];
    let mil_to_mm = |m: f64| m * 0.0254;
    if let Some(lt) = dr["default_line_thickness"].as_f64().filter(|x| *x > 0.0) {
        d.line_thickness_mm = Some(mil_to_mm(lt));
    }
    if let Some(ts) = dr["default_text_size"].as_f64().filter(|x| *x > 0.0) {
        d.text_size_mm = Some(mil_to_mm(ts));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kicad_rgb() {
        assert_eq!(rgb_hex("rgb(0, 150, 0)").as_deref(), Some("#009600"));
        assert_eq!(rgb_hex("rgba(220, 9, 13, 0.851)").as_deref(), Some("#DC090D"));
        assert_eq!(rgb_hex("rgb(132, 0, 0)").as_deref(), Some("#840000"));
        assert_eq!(rgb_hex("not-a-color"), None);
    }

    #[test]
    fn composites_translucent_mask_over_background() {
        // KiCad-default background rgb(0,16,35). F.Mask rgba(216,100,255,0.4) composited over
        // it is the muted purple KiCad shows — not the raw, over-saturated hue #D864FF.
        let bg = (0.0, 16.0, 35.0);
        assert_eq!(composite_over("rgba(216, 100, 255, 0.400)", bg).as_deref(), Some("#56327B"));
        // B.Mask rgba(2,255,238,0.4) → a faint teal, far from the opaque bright cyan #02FFEE.
        assert_eq!(composite_over("rgba(2, 255, 238, 0.400)", bg).as_deref(), Some("#017074"));
        // A fully opaque colour is unchanged by compositing.
        assert_eq!(composite_over("rgb(200, 52, 52)", bg).as_deref(), Some("#C83434"));
        assert_eq!(composite_over("not-a-color", bg), None);
    }

    #[test]
    fn newest_version_first_then_falls_back_to_the_one_with_a_theme() {
        // Simulate two side-by-side KiCad installs: 10.0 has an empty colors/ dir,
        // 9.0 holds the user's actual theme. The newest is tried first but yields
        // nothing, so load must fall back to 9.0's palette.
        let root = std::env::temp_dir().join(format!("extract_theme_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("10.0/colors")).unwrap();
        std::fs::create_dir_all(root.join("9.0/colors")).unwrap();
        std::fs::write(
            root.join("9.0/colors/user.json"),
            r#"{ "schematic": { "wire": "rgb(0,150,0)" },
                 "board": { "user_1": "rgb(194,194,194)",
                            "copper": { "f": "rgb(200,52,52)" } } }"#,
        )
        .unwrap();

        // Highest version first.
        let dirs = config_dirs_in(&root);
        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].ends_with("10.0"));
        assert!(dirs[1].ends_with("9.0"));

        // 10.0 yields nothing (empty colors/), 9.0 yields the real palette.
        assert!(load_from_dir(&dirs[0]).is_empty());
        let t = load_from_dir(&dirs[1]);
        assert_eq!(t.board("user_1"), Some("#C2C2C2"));
        assert_eq!(t.board("copper.f"), Some("#C83434"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flattens_schematic_and_board() {
        let v: Value = serde_json::from_str(
            r#"{
              "schematic": { "wire": "rgb(0,150,0)", "worksheet": "rgb(132,0,0)",
                             "override_item_colors": false },
              "board": { "f_silks": "rgb(242,237,161)",
                         "copper": { "f": "rgb(200,52,52)", "b": "rgb(77,127,196)" } }
            }"#,
        )
        .unwrap();
        let mut sch = BTreeMap::new();
        collect(&v["schematic"], "", &mut sch);
        assert_eq!(sch.get("wire").map(String::as_str), Some("#009600"));
        assert_eq!(sch.get("worksheet").map(String::as_str), Some("#840000"));
        assert!(!sch.contains_key("override_item_colors"), "non-colour leaf skipped");

        let mut brd = BTreeMap::new();
        collect(&v["board"], "", &mut brd);
        assert_eq!(brd.get("copper.f").map(String::as_str), Some("#C83434"));
        assert_eq!(brd.get("copper.b").map(String::as_str), Some("#4D7FC4"));
        assert_eq!(brd.get("f_silks").map(String::as_str), Some("#F2EDA1"));
    }
}
