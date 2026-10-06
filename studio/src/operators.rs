//! Vues enregistrées et préférences d'opérateur (T-426).
//!
//! An operator is a name (no password, no rights: see T-261 and T-283 for
//! the locks). Each one has its saved views (the interface's layout:
//! workspace, panel tab, 2D/3D, cue page, timeline zoom) and its interface
//! preferences (what the page keeps in `localStorage`, opaque here). They
//! belong to the person and the machine, not to the show: they live in
//! `<data-dir>/operators/<name>.json`, outside any project, and the last
//! operator used in `operators/_current.json`, so another browser on the
//! same machine opens the same profile.
//!
//! Display only: nothing here reads or writes the settings, the deck, the
//! output, the arming or a safety setting. A missing folder or a damaged
//! file gives the empty « Défaut » profile (a damaged file is set aside
//! like any other data file, never a safety one).

use crate::datafile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Views per operator: 8 on keys (slots 1–8) and 24 named ones.
pub const MAX_VIEWS: usize = 32;
/// Slots recalled from the keyboard (Alt+F1…).
pub const MAX_SLOT: u8 = 8;
/// Longest operator or view name, in characters.
pub const MAX_NAME: usize = 32;
/// The interface preferences, serialized, at most.
pub const MAX_PREFS_BYTES: usize = 64 * 1024;
/// A whole profile as sent by the page, at most (views included).
pub const MAX_BODY_BYTES: usize = 256 * 1024;
/// The operator nobody chose; it can't be deleted.
pub const DEFAULT_NAME: &str = "Défaut";
const DIR: &str = "operators";
const CURRENT_FILE: &str = "_current.json";
/// Path segments of the API that can't be operator names.
const RESERVED: [&str; 2] = ["current", "delete"];

/// One writer for the whole folder: two pages saving at once never
/// interleave a profile and `_current.json`.
static WRITER: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OperatorPrefs {
    pub name: String,
    pub views: Vec<View>,
    /// Interface keys, opaque for the server.
    pub prefs: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub name: String,
    /// 1..=8: recalled by its key.
    pub slot: Option<u8>,
    pub workspace: String,
    pub live_panel: String,
    pub creation_tab: String,
    /// "2d" | "3d" | "both".
    pub stage_mode: String,
    /// beam3d.js render options, as they are.
    pub render3d: Value,
    pub cue_page: usize,
    pub timeline: TimelineView,
    /// Empty until the panels can be resized.
    pub sizes: BTreeMap<String, f32>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            name: String::new(),
            slot: None,
            workspace: "live".into(),
            live_panel: "direct".into(),
            creation_tab: "look".into(),
            stage_mode: "2d".into(),
            render3d: Value::Object(Map::new()),
            cue_page: 0,
            timeline: TimelineView::default(),
            sizes: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimelineView {
    /// 1.0 = the default zoom.
    pub zoom: f32,
    /// Left edge, in the show's unit.
    pub scroll: f32,
    pub follow: bool,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self { zoom: 1.0, scroll: 0.0, follow: true }
    }
}

/// Letters, digits, spaces, `-` and `_`; 1 to 32 characters, no space at
/// either end, not starting with `_` (the folder's own files), and not
/// one of the API's words. Never a path: no `/`, `.` or `\`.
pub fn valid_name(name: &str) -> bool {
    let n = name.chars().count();
    (1..=MAX_NAME).contains(&n)
        && name.trim() == name
        && !name.starts_with('_')
        && name.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_')
        && !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(name))
}

fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

fn path_of(data_dir: &Path, name: &str) -> PathBuf {
    dir(data_dir).join(format!("{name}.json"))
}

fn read<T: serde::de::DeserializeOwned + Default>(data_dir: &Path, path: &Path) -> T {
    // Damaged files go to the data folder's quarantine, like the others.
    datafile::load_or_default(path, &datafile::beside(&dir(data_dir)), false)
}

/// One view's own checks (whether its slot is free is checked with the others).
fn check_view(v: &View) -> Result<(), (u16, String)> {
    let n = v.name.chars().count();
    if n == 0 || n > MAX_NAME || v.name.trim().is_empty() {
        return Err((400, "nom de vue invalide (1 à 32 caractères)".into()));
    }
    if v.slot.is_some_and(|s| s == 0 || s > MAX_SLOT) {
        return Err((400, format!("emplacement de vue : 1 à {MAX_SLOT}")));
    }
    let text_ok = [&v.workspace, &v.live_panel, &v.creation_tab, &v.stage_mode].iter().all(|s| s.len() <= 32);
    if !text_ok || !v.timeline.zoom.is_finite() || !v.timeline.scroll.is_finite() || v.sizes.values().any(|x| !x.is_finite()) {
        return Err((400, format!("vue « {} » invalide", v.name)));
    }
    if serde_json::to_vec(&v.render3d).map_or(usize::MAX, |b| b.len()) > 4096 {
        return Err((413, format!("vue « {} » : options 3D trop grandes", v.name)));
    }
    Ok(())
}

fn prefs_size(p: &Map<String, Value>) -> usize {
    serde_json::to_vec(p).map_or(usize::MAX, |b| b.len())
}

/// Checks a profile before it is written: its name, the views (slots
/// 1..=8 used once), the preferences' size.
pub fn check(p: &OperatorPrefs) -> Result<(), (u16, String)> {
    if !valid_name(&p.name) {
        return Err((400, "nom d'opérateur invalide : lettres, chiffres, espaces, - et _, 32 caractères au plus".into()));
    }
    if p.views.len() > MAX_VIEWS {
        return Err((400, format!("{MAX_VIEWS} vues au plus")));
    }
    let mut slots = [false; MAX_SLOT as usize + 1];
    for v in &p.views {
        check_view(v)?;
        if let Some(s) = v.slot {
            if std::mem::replace(&mut slots[s as usize], true) {
                return Err((400, format!("emplacement de vue {s} utilisé deux fois")));
            }
        }
    }
    let size = prefs_size(&p.prefs);
    if size > MAX_PREFS_BYTES {
        return Err((413, format!("préférences trop grandes ({} Ko, {} Ko au plus)", size / 1024, MAX_PREFS_BYTES / 1024)));
    }
    Ok(())
}

/// `name`'s profile; empty when it has no file yet, is damaged or the name
/// is not valid. Views that fail the checks are dropped, the rest kept.
pub fn load(data_dir: &Path, name: &str) -> OperatorPrefs {
    if !valid_name(name) {
        return OperatorPrefs { name: DEFAULT_NAME.into(), ..Default::default() };
    }
    let mut p: OperatorPrefs = read(data_dir, &path_of(data_dir, name));
    p.name = name.to_string();
    let mut seen = [false; MAX_SLOT as usize + 1];
    p.views.retain_mut(|v| {
        if let Some(s) = v.slot {
            if s == 0 || s > MAX_SLOT || std::mem::replace(&mut seen[s as usize], true) {
                v.slot = None;
            }
        }
        check_view(v).is_ok()
    });
    p.views.truncate(MAX_VIEWS);
    if prefs_size(&p.prefs) > MAX_PREFS_BYTES {
        p.prefs.clear();
    }
    p
}

/// Writes `p` (atomic: a cut write leaves the old file).
pub fn save(data_dir: &Path, p: &OperatorPrefs) -> Result<(), (u16, String)> {
    check(p)?;
    let _w = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    write(&path_of(data_dir, &p.name), &serde_json::to_vec_pretty(p).map_err(|e| (500, e.to_string()))?)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), (u16, String)> {
    let fail = |e: anyhow::Error| (500, format!("impossible d'enregistrer le profil d'opérateur : {e:#}"));
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| fail(e.into()))?;
    }
    datafile::write_atomic(path, bytes).map_err(fail)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Current {
    name: String,
}

/// The last operator used (« Défaut » when none or not valid).
pub fn current(data_dir: &Path) -> String {
    let c: Current = read(data_dir, &dir(data_dir).join(CURRENT_FILE));
    if valid_name(&c.name) { c.name } else { DEFAULT_NAME.into() }
}

pub fn set_current(data_dir: &Path, name: &str) -> Result<(), (u16, String)> {
    if !valid_name(name) {
        return Err((400, "nom d'opérateur invalide".into()));
    }
    let _w = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    write(&dir(data_dir).join(CURRENT_FILE), &serde_json::to_vec_pretty(&json!({ "name": name })).unwrap_or_default())
}

/// The operators with a file, « Défaut » and the current one always
/// included, sorted.
pub fn names(data_dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir(data_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_string))
        .filter(|n| valid_name(n))
        .collect();
    out.push(DEFAULT_NAME.into());
    out.push(current(data_dir));
    out.sort_by_key(|n| n.to_lowercase());
    out.dedup();
    out
}

/// `GET /api/operators`.
pub fn summary(data_dir: &Path) -> Value {
    json!({ "current": current(data_dir), "names": names(data_dir), "default": DEFAULT_NAME, "max_views": MAX_VIEWS, "max_slot": MAX_SLOT })
}

/// Deletes `name`'s profile (« Défaut » can't be); the current operator
/// goes back to « Défaut » if it was this one.
pub fn delete(data_dir: &Path, name: &str) -> Result<(), (u16, String)> {
    if !valid_name(name) {
        return Err((400, "nom d'opérateur invalide".into()));
    }
    if name == DEFAULT_NAME {
        return Err((409, "le profil « Défaut » ne se supprime pas".into()));
    }
    let was_current = current(data_dir) == name;
    {
        let _w = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        match std::fs::remove_file(path_of(data_dir, name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err((500, format!("impossible de supprimer le profil : {e}"))),
        }
    }
    if was_current {
        set_current(data_dir, DEFAULT_NAME)?;
    }
    Ok(())
}

/// A path segment, percent-decoded (UTF-8); `None` when malformed.
pub fn decode_segment(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-operators-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_never_reach_outside_the_folder() {
        for ok in ["Défaut", "Jean", "Régie 2", "nuit_A-1", "x"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "../x", "..", "a/b", "a\\b", "a.json", " lead", "trail ", "_current", "current", "Delete", "a\0b", &"x".repeat(33)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        let d = temp_dir("bad");
        assert_eq!(load(&d, "../../etc/passwd").name, DEFAULT_NAME);
        assert_eq!(save(&d, &OperatorPrefs { name: "../x".into(), ..Default::default() }).unwrap_err().0, 400);
        assert_eq!(set_current(&d, "a/b").unwrap_err().0, 400);
        assert!(!d.parent().unwrap().join("x.json").exists());
    }

    #[test]
    fn missing_folder_or_damaged_file_gives_an_empty_default() {
        let d = temp_dir("missing");
        assert_eq!(current(&d), DEFAULT_NAME);
        assert_eq!(names(&d), vec![DEFAULT_NAME.to_string()]);
        assert_eq!(load(&d, DEFAULT_NAME), OperatorPrefs { name: DEFAULT_NAME.into(), ..Default::default() });
        std::fs::create_dir_all(d.join(DIR)).unwrap();
        std::fs::write(d.join(DIR).join("Défaut.json"), "{ pas du json").unwrap();
        std::fs::write(d.join(DIR).join(CURRENT_FILE), "[]").unwrap();
        assert_eq!(load(&d, DEFAULT_NAME).views.len(), 0);
        assert_eq!(current(&d), DEFAULT_NAME);
    }

    #[test]
    fn a_partial_file_round_trips_with_defaults() {
        let d = temp_dir("partial");
        std::fs::create_dir_all(d.join(DIR)).unwrap();
        std::fs::write(d.join(DIR).join("Jean.json"), r#"{"views":[{"name":"Jeu","slot":1,"live_panel":"music"},{"name":"","slot":2},{"name":"B","slot":1}],"prefs":{"laserStudio.workspace":"timeline"},"later":true}"#).unwrap();
        let p = load(&d, "Jean");
        assert_eq!(p.name, "Jean");
        assert_eq!(p.views.len(), 2, "the unnamed view is dropped");
        assert_eq!((p.views[0].slot, p.views[0].live_panel.as_str(), p.views[0].workspace.as_str()), (Some(1), "music", "live"));
        assert_eq!(p.views[0].timeline, TimelineView { zoom: 1.0, scroll: 0.0, follow: true });
        assert_eq!(p.views[1].slot, None, "slot 1 taken: kept without its key");
        assert_eq!(p.prefs["laserStudio.workspace"], "timeline");
        save(&d, &p).unwrap();
        assert_eq!(load(&d, "Jean"), p);
        assert_eq!(names(&d), vec![DEFAULT_NAME.to_string(), "Jean".to_string()]);
    }

    #[test]
    fn limits_are_checked() {
        let d = temp_dir("limits");
        let mut p = OperatorPrefs { name: "A".into(), ..Default::default() };
        p.prefs.insert("big".into(), Value::String("x".repeat(MAX_PREFS_BYTES)));
        assert_eq!(save(&d, &p).unwrap_err().0, 413);
        p.prefs.clear();
        p.views = (0..=MAX_VIEWS).map(|i| View { name: format!("v{i}"), ..Default::default() }).collect();
        assert_eq!(save(&d, &p).unwrap_err().0, 400);
        p.views = vec![View { name: "v".into(), slot: Some(9), ..Default::default() }];
        assert_eq!(save(&d, &p).unwrap_err().0, 400);
        p.views = vec![View { name: "v".into(), slot: Some(3), ..Default::default() }, View { name: "w".into(), slot: Some(3), ..Default::default() }];
        assert_eq!(save(&d, &p).unwrap_err().0, 400);
        p.views.pop();
        save(&d, &p).unwrap();
        assert_eq!(load(&d, "A").views[0].slot, Some(3));
    }

    #[test]
    fn current_and_delete() {
        let d = temp_dir("current");
        save(&d, &OperatorPrefs { name: "Marie".into(), ..Default::default() }).unwrap();
        set_current(&d, "Marie").unwrap();
        assert_eq!(current(&d), "Marie");
        assert_eq!(summary(&d)["current"], "Marie");
        assert_eq!(delete(&d, DEFAULT_NAME).unwrap_err().0, 409);
        delete(&d, "Marie").unwrap();
        assert_eq!(current(&d), DEFAULT_NAME, "back to « Défaut »");
        assert_eq!(names(&d), vec![DEFAULT_NAME.to_string()]);
        // An operator chosen before its first save is listed.
        set_current(&d, "Paul").unwrap();
        assert_eq!(names(&d), vec![DEFAULT_NAME.to_string(), "Paul".to_string()]);
    }

    #[test]
    fn segments_are_percent_decoded() {
        assert_eq!(decode_segment("D%C3%A9faut").as_deref(), Some("Défaut"));
        assert_eq!(decode_segment("R%C3%A9gie%202").as_deref(), Some("Régie 2"));
        assert_eq!(decode_segment("%2E%2E%2Fx").as_deref(), Some("../x"));
        assert_eq!(decode_segment("%zz"), None);
        assert_eq!(decode_segment("%FF"), None);
    }
}
