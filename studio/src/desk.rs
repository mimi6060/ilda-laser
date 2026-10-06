//! The custom desk (T-427): pages of controls the operator builds.
//!
//! A desk is a set of tabs; each tab is a grid (12 × 8 cells by default)
//! where the operator places buttons, faders, XY pads, labels and small
//! previews, then plays them full screen on a touch monitor. The idea
//! (user-built control pages) comes from public console documentation;
//! the model, layout and labels are our own.
//!
//! Safety: a desk creates no new path to the engine. Every component
//! drives the control registry through `POST /api/control` (checked like
//! a controller: clamped, external-only, gate and show-mode rules) or a
//! cue through `POST /api/cue`. No binding may target arming or any
//! `safety.*` control except the emergency stop (`forbidden`), and the
//! server checks it again on every save. The layout is content
//! (`POST /api/desks`: in the history, refused in show mode); playing it
//! is not.

use crate::controls::{ControlDesc, ControlKind, ControlRegistry};
use crate::Shared;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const MAX_TABS: usize = 8;
pub const MAX_ITEMS: usize = 96;
/// Tab names and component labels, in characters.
pub const MAX_NAME: usize = 32;
pub const COLS: (u8, u8) = (4, 24);
pub const ROWS: (u8, u8) = (2, 16);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeskTab {
    pub name: String,
    pub cols: u8,
    pub rows: u8,
    pub items: Vec<DeskItem>,
}

impl Default for DeskTab {
    fn default() -> Self {
        Self { name: "Pupitre 1".into(), cols: 12, rows: 8, items: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeskItem {
    pub id: u64,
    pub kind: DeskKind,
    /// In cells.
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
    /// Empty: the bound control's label.
    pub label: String,
    /// None: the theme's colour.
    pub color: Option<[u8; 3]>,
    pub binding: Binding,
    /// XY pad only.
    pub binding_y: Binding,
    /// XY pad: « Retour au centre » on release.
    pub spring: bool,
    /// Fader orientation.
    pub vertical: bool,
}

impl Default for DeskItem {
    fn default() -> Self {
        Self { id: 0, kind: DeskKind::Button, x: 0, y: 0, w: 2, h: 1, label: String::new(), color: None, binding: Binding::None, binding_y: Binding::None, spring: false, vertical: true }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeskKind {
    #[default]
    Button,
    Fader,
    XyPad,
    Label,
    Preview,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Binding {
    #[default]
    None,
    Control { id: String },
    Cue { id: String },
    /// A macro (T-425), by id: a button launches it (`macro.<id>.go`).
    Macro { id: String },
}

/// Controls no desk component may drive, whatever the registry says:
/// arming, and every `safety.*` control but the emergency stop (a stop is
/// always welcome; the hold-to-run pad and anything added later are not).
/// Every non-external registry control is in here (tested).
pub fn forbidden(id: &str) -> bool {
    let id = id.trim();
    id == "transport.arm" || (id.starts_with("safety.") && id != "safety.estop")
}

/// A control binding's registry entry: `Ok(None)` when the control is
/// unknown (a missing binding, kept and greyed), refused when forbidden or
/// not external.
fn control_entry<'a>(reg: &'a ControlRegistry, id: &str) -> Result<Option<&'a ControlDesc>> {
    if forbidden(id) {
        bail!("« {id} » ne peut pas être placé sur un pupitre (armement et sécurité restent hors pupitre)");
    }
    match reg.get(id) {
        Some(d) if !d.external || forbidden(&d.id) => bail!("« {id} » ne peut pas être placé sur un pupitre"),
        d => Ok(d),
    }
}

fn is_button_kind(k: &ControlKind) -> bool {
    matches!(k, ControlKind::Trigger | ControlKind::Toggle { .. } | ControlKind::Momentary)
}

fn is_continuous(k: &ControlKind) -> bool {
    matches!(k, ControlKind::Continuous { .. })
}

fn overlaps(a: &DeskItem, b: &DeskItem) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// `POST /api/desks`: bounds, overlaps, limits, and every binding's
/// target (forbidden or non-external controls, wrong kinds) refused
/// rather than repaired. Unknown controls, cues and macros pass: they
/// show as « Liaison manquante ».
pub fn validate(tabs: &[DeskTab], reg: &ControlRegistry) -> Result<()> {
    if tabs.len() > MAX_TABS {
        bail!("{MAX_TABS} onglets de pupitre au plus");
    }
    for (t, tab) in tabs.iter().enumerate() {
        let tn = t + 1;
        if tab.name.chars().count() > MAX_NAME {
            bail!("onglet {tn} : nom trop long ({MAX_NAME} caractères au plus)");
        }
        if !(COLS.0..=COLS.1).contains(&tab.cols) || !(ROWS.0..=ROWS.1).contains(&tab.rows) {
            bail!("onglet {tn} : grille de {}×{} à {}×{} cases", COLS.0, ROWS.0, COLS.1, ROWS.1);
        }
        if tab.items.len() > MAX_ITEMS {
            bail!("onglet {tn} : {MAX_ITEMS} composants au plus");
        }
        let mut ids = HashSet::new();
        for (i, it) in tab.items.iter().enumerate() {
            let n = i + 1;
            if !ids.insert(it.id) {
                bail!("onglet {tn} : identifiant de composant en double ({})", it.id);
            }
            if it.w == 0 || it.h == 0 || it.x as u16 + it.w as u16 > tab.cols as u16 || it.y as u16 + it.h as u16 > tab.rows as u16 {
                bail!("onglet {tn}, composant {n} : hors de la grille");
            }
            if it.label.chars().count() > MAX_NAME {
                bail!("onglet {tn}, composant {n} : libellé trop long ({MAX_NAME} caractères au plus)");
            }
            if let Some(o) = tab.items[..i].iter().position(|o| overlaps(o, it)) {
                bail!("onglet {tn} : les composants {} et {n} se chevauchent", o + 1);
            }
            check_binding(it, reg).map_err(|e| anyhow::anyhow!("onglet {tn}, composant {n} : {e}"))?;
        }
    }
    Ok(())
}

fn check_binding(it: &DeskItem, reg: &ControlRegistry) -> Result<()> {
    let control = |b: &Binding, want: fn(&ControlKind) -> bool, what: &str| -> Result<()> {
        match b {
            Binding::None => Ok(()),
            Binding::Control { id } => match control_entry(reg, id)? {
                Some(d) if !want(&d.kind) => bail!("« {id} » ne convient pas à {what}"),
                _ => Ok(()),
            },
            _ => bail!("{what} ne se lie qu'à un contrôle"),
        }
    };
    match it.kind {
        DeskKind::Button => {
            if it.binding_y != Binding::None {
                bail!("un bouton n'a pas de liaison Y");
            }
            match &it.binding {
                Binding::Control { .. } => control(&it.binding, is_button_kind, "un bouton"),
                Binding::Cue { id } | Binding::Macro { id } if id.trim().is_empty() => bail!("liaison sans identifiant"),
                _ => Ok(()),
            }
        }
        DeskKind::Fader => {
            if it.binding_y != Binding::None {
                bail!("un curseur n'a pas de liaison Y");
            }
            control(&it.binding, is_continuous, "un curseur")
        }
        DeskKind::XyPad => {
            control(&it.binding, is_continuous, "un pavé XY")?;
            control(&it.binding_y, is_continuous, "un pavé XY")
        }
        DeskKind::Label | DeskKind::Preview => {
            if it.binding != Binding::None || it.binding_y != Binding::None {
                bail!("une étiquette ou un aperçu ne se lie à rien");
            }
            Ok(())
        }
    }
}

fn clip(text: &str) -> String {
    text.trim().chars().take(MAX_NAME).collect()
}

/// A hand-edited, older or newer file loads: limits, bounds, names; a
/// component that overlaps an earlier one or falls off the grid goes;
/// forbidden bindings and bindings of the wrong shape are dropped. Never
/// fails.
pub fn sanitize(tabs: &mut Vec<DeskTab>) {
    tabs.truncate(MAX_TABS);
    for (t, tab) in tabs.iter_mut().enumerate() {
        tab.name = clip(&tab.name);
        if tab.name.is_empty() {
            tab.name = format!("Pupitre {}", t + 1);
        }
        tab.cols = tab.cols.clamp(COLS.0, COLS.1);
        tab.rows = tab.rows.clamp(ROWS.0, ROWS.1);
        let (cols, rows) = (tab.cols, tab.rows);
        let mut kept: Vec<DeskItem> = Vec::new();
        let mut ids = HashSet::new();
        let mut next = tab.items.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        for mut it in std::mem::take(&mut tab.items) {
            if it.x >= cols || it.y >= rows || kept.len() >= MAX_ITEMS {
                continue;
            }
            it.w = it.w.clamp(1, cols - it.x);
            it.h = it.h.clamp(1, rows - it.y);
            if kept.iter().any(|o| overlaps(o, &it)) {
                continue;
            }
            if it.id == 0 || !ids.insert(it.id) {
                it.id = next;
                next += 1;
                ids.insert(it.id);
            }
            it.label = clip(&it.label);
            let strip = |b: &mut Binding, controls_only: bool| {
                let bad = match b {
                    Binding::None => false,
                    Binding::Control { id } => forbidden(id) || id.trim().is_empty(),
                    Binding::Cue { id } | Binding::Macro { id } => controls_only || id.trim().is_empty(),
                };
                if bad {
                    *b = Binding::None;
                }
            };
            match it.kind {
                DeskKind::Button => {
                    strip(&mut it.binding, false);
                    it.binding_y = Binding::None;
                }
                DeskKind::Fader => {
                    strip(&mut it.binding, true);
                    it.binding_y = Binding::None;
                }
                DeskKind::XyPad => {
                    strip(&mut it.binding, true);
                    strip(&mut it.binding_y, true);
                }
                DeskKind::Label | DeskKind::Preview => {
                    it.binding = Binding::None;
                    it.binding_y = Binding::None;
                }
            }
            kept.push(it);
        }
        tab.items = kept;
    }
}

/// The desks, where they are saved.
#[derive(Debug, Default)]
pub struct Desks {
    pub tabs: Vec<DeskTab>,
    path: Option<PathBuf>,
    /// Bumped on every change, so the page reloads the layout.
    pub rev: u64,
}

impl Desks {
    /// `desks.json`, or no desk.
    pub fn load(path: PathBuf) -> Self {
        let mut tabs: Vec<DeskTab> = crate::load_json(&path);
        sanitize(&mut tabs);
        Self { tabs, path: Some(path), rev: 0 }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(path, &self.tabs);
        }
    }

    /// Takes new tabs (an edit, an undo, a project), sanitised. Memory
    /// only: the caller saves.
    pub fn set_tabs(&mut self, mut tabs: Vec<DeskTab>) {
        sanitize(&mut tabs);
        self.tabs = tabs;
        self.rev += 1;
    }
}

/// What the desk shows, in every state (`/api/frame`): the layout's
/// revision, the value of every bound control (read back from the server,
/// so a move from MIDI or the Direct panel is seen), whether each bound
/// macro runs (`macro:<id>`, T-425), and the bindings that point at
/// nothing (`control:<id>`, `cue:<id>`, `macro:<id>`).
pub fn view(s: &Shared) -> Value {
    let mut values = Map::new();
    let mut missing: Vec<String> = Vec::new();
    for it in s.desks.tabs.iter().flat_map(|t| &t.items) {
        for b in [&it.binding, &it.binding_y] {
            match b {
                Binding::None => {}
                Binding::Control { id } => match s.controls.get(id) {
                    Some(d) if d.external && !forbidden(&d.id) => {
                        if !values.contains_key(id) {
                            if let Some(v) = crate::controls::current(s, d) {
                                values.insert(id.clone(), v);
                            }
                        }
                    }
                    _ => missing.push(format!("control:{id}")),
                },
                Binding::Cue { id } => {
                    if !s.presets.iter().any(|p| p.id == *id) {
                        missing.push(format!("cue:{id}"));
                    }
                }
                Binding::Macro { id } => match s.macros.get(id) {
                    Some(_) => {
                        values.insert(format!("macro:{id}"), json!(s.macros.player.is_running(id)));
                    }
                    None => missing.push(format!("macro:{id}")),
                },
            }
        }
    }
    missing.sort();
    missing.dedup();
    json!({ "rev": s.desks.rev, "values": values, "missing": missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> ControlRegistry {
        ControlRegistry::build(&crate::presets::catalog())
    }

    fn item(id: u64, kind: DeskKind, x: u8, y: u8, w: u8, h: u8) -> DeskItem {
        DeskItem { id, kind, x, y, w, h, ..Default::default() }
    }

    fn ctl(id: &str) -> Binding {
        Binding::Control { id: id.into() }
    }

    fn tab(items: Vec<DeskItem>) -> Vec<DeskTab> {
        vec![DeskTab { items, ..Default::default() }]
    }

    #[test]
    fn a_full_desk_validates_and_round_trips() {
        let mut xy = item(3, DeskKind::XyPad, 4, 0, 4, 4);
        xy.binding = ctl("master.pos_x");
        xy.binding_y = ctl("live.pos_y");
        xy.spring = true;
        let mut f = item(2, DeskKind::Fader, 2, 0, 1, 4);
        f.binding = ctl("master.brightness");
        let mut b = item(1, DeskKind::Button, 0, 0, 2, 1);
        b.binding = ctl("master.strobe.hold");
        b.color = Some([255, 0, 128]);
        let mut c = item(4, DeskKind::Button, 0, 1, 2, 1);
        c.binding = Binding::Cue { id: "some-cue".into() };
        let mut stop = item(5, DeskKind::Button, 0, 2, 2, 1);
        stop.binding = ctl("safety.estop");
        let tabs = tab(vec![b, f, xy, c, stop, item(6, DeskKind::Label, 8, 0, 4, 1), item(7, DeskKind::Preview, 8, 1, 4, 4)]);
        validate(&tabs, &reg()).unwrap();
        let text = serde_json::to_string(&tabs).unwrap();
        let back: Vec<DeskTab> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, tabs);
        // Missing fields take their defaults.
        let t: DeskTab = serde_json::from_str(r#"{"items":[{"id":9}]}"#).unwrap();
        assert_eq!((t.name.as_str(), t.cols, t.rows), ("Pupitre 1", 12, 8));
        assert_eq!(t.items[0], DeskItem { id: 9, ..Default::default() });
        assert!(t.items[0].vertical && !t.items[0].spring);
    }

    #[test]
    fn arming_and_safety_controls_are_refused() {
        let r = reg();
        for id in ["transport.arm", "safety.hold", "safety.anything_new", " transport.arm"] {
            let mut b = item(1, DeskKind::Button, 0, 0, 1, 1);
            b.binding = ctl(id);
            assert!(validate(&tab(vec![b]), &r).is_err(), "{id}");
        }
        // Every control the registry keeps from controllers is forbidden by name too.
        for d in r.list().iter().filter(|d| !d.external) {
            assert!(forbidden(&d.id), "{} is not external but not forbidden", d.id);
        }
        // Sanitising a file drops them.
        let mut b = item(1, DeskKind::Button, 0, 0, 1, 1);
        b.binding = ctl("transport.arm");
        let mut tabs = tab(vec![b]);
        sanitize(&mut tabs);
        assert_eq!(tabs[0].items[0].binding, Binding::None);
    }

    #[test]
    fn bounds_overlaps_kinds_and_limits() {
        let r = reg();
        let bad = |items: Vec<DeskItem>| validate(&tab(items), &r).is_err();
        assert!(bad(vec![item(1, DeskKind::Button, 11, 0, 2, 1)]), "off the right edge");
        assert!(bad(vec![item(1, DeskKind::Button, 0, 7, 1, 2)]), "off the bottom");
        assert!(bad(vec![item(1, DeskKind::Button, 0, 0, 0, 1)]), "zero width");
        assert!(bad(vec![item(1, DeskKind::Button, 0, 0, 2, 2), item(2, DeskKind::Fader, 1, 1, 1, 3)]), "overlap");
        assert!(!bad(vec![item(1, DeskKind::Button, 0, 0, 2, 2), item(2, DeskKind::Fader, 2, 0, 1, 3)]), "side by side");
        assert!(bad(vec![item(1, DeskKind::Button, 0, 0, 1, 1), item(1, DeskKind::Button, 1, 0, 1, 1)]), "same id");
        let mut f = item(1, DeskKind::Fader, 0, 0, 1, 1);
        f.binding = ctl("tempo.tap");
        assert!(bad(vec![f.clone()]), "a fader on a trigger");
        f.binding = Binding::Cue { id: "x".into() };
        assert!(bad(vec![f]), "a fader on a cue");
        let mut b = item(1, DeskKind::Button, 0, 0, 1, 1);
        b.binding = ctl("master.size");
        assert!(bad(vec![b]), "a button on a continuous control");
        let mut l = item(1, DeskKind::Label, 0, 0, 1, 1);
        l.binding = ctl("tempo.tap");
        assert!(bad(vec![l]));
        // Unknown controls are a missing binding, not an error.
        let mut b = item(1, DeskKind::Button, 0, 0, 1, 1);
        b.binding = ctl("grid.9.9.9");
        assert!(!bad(vec![b]));
        // Limits.
        let mut t = DeskTab { cols: 24, rows: 16, ..Default::default() };
        t.items = (0..MAX_ITEMS as u64 + 1).map(|i| item(i + 1, DeskKind::Button, (i % 24) as u8, (i / 24) as u8, 1, 1)).collect();
        assert!(validate(std::slice::from_ref(&t), &r).is_err());
        t.items.pop();
        validate(std::slice::from_ref(&t), &r).unwrap();
        assert!(validate(&vec![DeskTab::default(); MAX_TABS + 1], &r).is_err());
        validate(&vec![DeskTab::default(); MAX_TABS], &r).unwrap();
        assert!(validate(&[DeskTab { cols: 3, ..Default::default() }], &r).is_err());
        assert!(validate(&[DeskTab { rows: 17, ..Default::default() }], &r).is_err());
        assert!(validate(&[DeskTab { name: "x".repeat(MAX_NAME + 1), ..Default::default() }], &r).is_err());
    }

    #[test]
    fn sanitize_repairs_what_a_file_may_hold() {
        let mut tabs = vec![DeskTab { name: "  ".into(), cols: 40, rows: 1, items: vec![
            item(0, DeskKind::Button, 0, 0, 30, 1),
            item(0, DeskKind::Fader, 1, 0, 1, 1), // overlaps the first: dropped
            item(5, DeskKind::Label, 30, 0, 1, 1), // off the grid: dropped
            item(5, DeskKind::Preview, 0, 1, 2, 2),
        ] }; MAX_TABS + 2];
        tabs[0].items[3].binding = Binding::Cue { id: "x".into() };
        sanitize(&mut tabs);
        assert_eq!(tabs.len(), MAX_TABS);
        let t = &tabs[0];
        assert_eq!((t.name.as_str(), t.cols, t.rows), ("Pupitre 1", 24, 2));
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.items[0].w, 24);
        assert_ne!(t.items[0].id, 0);
        assert_ne!(t.items[0].id, t.items[1].id);
        assert_eq!(t.items[1].binding, Binding::None);
        validate(&tabs, &reg()).unwrap();
    }

    #[test]
    fn the_view_reads_values_and_flags_missing_bindings() {
        let mut s = crate::test_support::shared();
        s.live.pos_x = 0.25;
        let mut xy = item(1, DeskKind::XyPad, 0, 0, 2, 2);
        xy.binding = ctl("master.pos_x");
        xy.binding_y = ctl("grid.9.9.9");
        let mut c = item(2, DeskKind::Button, 2, 0, 1, 1);
        c.binding = Binding::Cue { id: "gone".into() };
        let mut m = item(3, DeskKind::Button, 3, 0, 1, 1);
        m.binding = Binding::Macro { id: "m1".into() };
        s.desks.set_tabs(tab(vec![xy, c, m]));
        let v = view(&s);
        assert_eq!(v["values"]["master.pos_x"], 0.25);
        assert_eq!(v["missing"], json!(["control:grid.9.9.9", "cue:gone", "macro:m1"]));
        assert_eq!(v["rev"], 1);
        // T-425: once the macro exists, it is no longer missing, and the
        // view says whether it runs.
        let m = crate::macros::Macro { id: "m1".into(), name: "Montée".into(), ..Default::default() };
        crate::macros::set(&mut s, vec![m]).unwrap();
        let v = view(&s);
        assert_eq!(v["missing"], json!(["control:grid.9.9.9", "cue:gone"]));
        assert_eq!(v["values"]["macro:m1"], false);
    }
}
