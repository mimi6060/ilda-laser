//! Undo / redo and the history of edits (T-285).
//!
//! The lighting desk's « Oops »: an edit made while preparing (a cue
//! overwritten, a page deleted, a palette changed) can be taken back.
//! The history is kept by the studio, in memory, for the open project:
//! `CAP` entries, the oldest forgotten beyond; a new edit after an undo
//! empties the redo stack; opening, creating or recovering a project
//! empties both (`clear`).
//!
//! How an entry is made: every editing route listed in `sections_for`
//! is wrapped by `web.rs`. The sections it may change are copied (as
//! JSON) before the route runs and again after it answered 2xx; the
//! sections that differ make one entry, with a French label built from
//! the difference (« Scène « Intro » supprimée », « Page 2 renommée »).
//! Undo puts the « before » copies back, redo the « after » ones, each
//! through the same store setters and checks as a manual change; a
//! venue goes through `web::apply_venue`, so a change of the projectors'
//! outputs is still refused while armed. Before restoring, the sections
//! must still be what the entry left: if something outside the history
//! changed them since, the step is refused and the history emptied.
//!
//! The boundary, what is **never** in the history:
//!
//! - live performance: cue presses, masters and live modifiers, layer
//!   faders, the mixer, tempo and tap, the grid's click / multi /
//!   transition / quantise modes (controls), playlist, timeline and cue
//!   list transport, festival palette picks, scenes played;
//! - safety and the rig: arming, the e-stop, blackout, interlocks,
//!   calibration, safety zones, horizon, strobe limiter, power and colour
//!   caps, audience mode, presence, the checklist, outputs. Undo never
//!   arms, disarms or loosens any of them: they keep their own
//!   confirmations and their own log;
//! - the timeline editor and the figure editor keep their own undo (in
//!   the page) for the show or figure being edited; the history only
//!   holds what reaches the studio from the figure editor (a figure
//!   saved or deleted). Shows are not in it.
//!
//! In show mode (T-283) `/api/undo` and `/api/redo` are refused with 423,
//! like every edit (they are not in `showmode::allowed`).

use crate::Shared;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::sync::{Mutex, MutexGuard};

/// Undo levels kept.
pub const CAP: usize = 50;

/// A part of the studio an edit can change, restored as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Scenes,
    /// The per-cue properties (`CueDeck::slots`): « Propriétés du cue »,
    /// cue programs. Not the grid modes, which are live controls.
    CueProps,
    /// The grid's size and pages (`CueGrid`).
    Pages,
    UserCues,
    Palettes,
    ScanPresets,
    Lfos,
    AudioRoutes,
    CueLists,
    Figures,
    /// The 3D object library (T-462).
    Objects,
    Venue,
    /// Projection zones (T-012).
    Projection,
    /// The FX grid's cells (T-146). Not what plays, nor the *Action*
    /// faders, destination and click mode: those are live controls.
    Fx,
    /// The executor bank's assignments (T-418). Not the faders' positions
    /// nor what is held: those are live.
    Executors,
    /// The custom desk's layout (T-427). Not what its faders hold: those
    /// are the controls' values.
    Desks,
    /// The macro list (T-425). Not what runs: that is playback.
    Macros,
    /// The show list (T-444). Not what plays: that is playback.
    ShowList,
    /// Referenced presets (T-422).
    Pools,
    /// The agenda (T-455): its switch and entries. Not what it played.
    Agenda,
    /// The autopilot's pages and public content (T-148). Not its switch,
    /// rhythm and order: those are live controls.
    Autopilot,
}

use Section::*;

const ALL: [Section; 21] = [Scenes, CueProps, Pages, UserCues, Palettes, ScanPresets, Lfos, AudioRoutes, CueLists, Figures, Objects, Venue, Projection, Fx, Executors, Desks, Macros, ShowList, Pools, Agenda, Autopilot];

impl Section {
    pub fn key(self) -> &'static str {
        match self {
            Scenes => "scenes",
            CueProps => "cue_props",
            Pages => "pages",
            UserCues => "user_cues",
            Palettes => "palettes",
            ScanPresets => "scan_presets",
            Lfos => "lfos",
            AudioRoutes => "audio_routes",
            CueLists => "cue_lists",
            Figures => "figures",
            Objects => "objects",
            Venue => "venue",
            Projection => "projection",
            Fx => "fx",
            Executors => "executors",
            Desks => "desks",
            Macros => "macros",
            ShowList => "showlist",
            Pools => "pools",
            Agenda => "agenda",
            Autopilot => "autopilot",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        ALL.into_iter().find(|s| s.key() == key)
    }
}

/// The sections a POST route may change, the most telling first (the
/// label comes from the first one that changed). `None`: the route is
/// not recorded (playback, safety, rig, or no state).
pub fn sections_for(path: &str) -> Option<&'static [Section]> {
    Some(match path {
        "/api/scenes/save" | "/api/scenes/delete" => &[Scenes],
        "/api/cues/slot" | "/api/cues/program" | "/api/cues/modifiers" | "/api/cues/parts" => &[CueProps],
        "/api/cues/store" | "/api/cues/update" | "/api/cues/program/store" | "/api/abstracts/store" | "/api/grid/edit" => &[UserCues, Pages, CueProps],
        "/api/grid" | "/api/grid/page" | "/api/grid/pages" | "/api/grid/move" | "/api/grid/clear" => &[Pages, UserCues, CueProps],
        "/api/palettes" => &[Palettes],
        "/api/scan-presets" => &[ScanPresets],
        "/api/lfos" => &[Lfos],
        "/api/audio/routes" | "/api/audio/preset" => &[AudioRoutes],
        "/api/cuelists" | "/api/cuelists/delete" => &[CueLists],
        "/api/figures" | "/api/figures/delete" => &[Figures],
        "/api/objects" | "/api/objects/import" | "/api/objects/delete" => &[Objects],
        "/api/venue" | "/api/venue/preset" => &[Venue],
        "/api/projection" | "/api/zones/align/apply" => &[Projection],
        "/api/fx" => &[Fx],
        "/api/executors" => &[Executors],
        "/api/desks" => &[Desks],
        "/api/macros" => &[Macros],
        "/api/showlist" => &[ShowList],
        "/api/agenda" => &[Agenda],
        "/api/pools" | "/api/pools/delete" | "/api/pools/capture" => &[Pools],
        "/api/autopilot" => &[Autopilot],
        _ => return None,
    })
}

/// One section as it is now. Called under the lock: copies only.
pub fn capture(s: &Shared, sec: Section) -> Value {
    match sec {
        Scenes => json!(s.scenes.list()),
        CueProps => json!(s.deck.slots),
        Pages => json!(s.grid),
        UserCues => json!(s.user_cues.list()),
        Palettes => json!(s.palettes.list()),
        ScanPresets => json!(s.scan_presets.user()),
        Lfos => json!(s.lfos.list()),
        AudioRoutes => json!(s.routes.routing()),
        CueLists => json!(s.cue_lists.list()),
        Figures => json!(s.figures.list()),
        Objects => json!(s.objects.list()),
        Venue => json!(s.venue.get()),
        // Without the group masters and the running chasers (T-170):
        // playing values, never undone, never a reason for a stale undo.
        Projection => json!(s.projection.get().without_playing()),
        Fx => json!(s.fx.grid().cells()),
        Executors => json!(s.executors.bank),
        Desks => json!(s.desks.tabs),
        Macros => json!(s.macros.list()),
        // The items only: *Boucler la liste* is a transport switch (allowed
        // in show mode, from MIDI), not an edit.
        ShowList => json!(s.showlist.list().items),
        Pools => json!(s.pools.list()),
        Agenda => json!(s.agenda.agenda()),
        Autopilot => crate::autopilot::capture(s),
    }
}

/// `capture` of several sections, keyed by `Section::key`.
pub fn snapshot(s: &Shared, sections: &[Section]) -> Map<String, Value> {
    sections.iter().map(|&sec| (sec.key().to_string(), capture(s, sec))).collect()
}

/// A refusal: HTTP status and French message.
pub type Refusal = (u16, String);

/// Puts a section back, through the same setter and checks as a manual
/// change. Never arms, disarms or touches a safety setting.
fn restore(s: &mut Shared, sec: Section, v: &Value) -> Result<(), Refusal> {
    fn from<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, Refusal> {
        serde_json::from_value(v.clone()).map_err(|e| (500, format!("historique illisible : {e}")))
    }
    let bad = |e: anyhow::Error| (400, format!("{e:#}"));
    let io = |e: anyhow::Error| (500, format!("{e:#}"));
    match sec {
        Scenes => s.scenes.set_all(from(v)?).map_err(io)?,
        CueProps => {
            s.deck.slots = from(v)?;
            s.deck.sanitize();
            s.deck.save();
        }
        Pages => {
            let grid: crate::grid::CueGrid = from(v)?;
            s.grid.set_config(&grid);
            crate::grid::refresh(s);
            s.grid.save();
        }
        UserCues => {
            s.user_cues.replace_in_memory(from(v)?);
            s.user_cues.save().map_err(io)?;
            crate::usercues::refresh(s);
        }
        Palettes => s.palettes.set(from(v)?).map_err(bad)?,
        ScanPresets => s.scan_presets.set(from(v)?).map_err(bad)?,
        Lfos => {
            let list = from(v)?;
            s.lfos.set(list, &s.controls).map_err(bad)?;
        }
        AudioRoutes => {
            let routing = from(v)?;
            s.routes.set(routing, &s.controls).map_err(bad)?;
        }
        CueLists => {
            let lists: Vec<crate::cuelist::CueList> = from(v)?;
            for l in &lists {
                l.validate(&s.controls).map_err(|e| (400, format!("liste de cues « {} » : {e:#}", l.name)))?;
            }
            s.cue_lists.set_all(lists).map_err(io)?;
            // The loaded list follows its restored version, or goes.
            match s.cue_list.list.as_ref().and_then(|l| s.cue_lists.get(&l.name)).cloned() {
                Some(l) => s.cue_list.refresh(&l),
                None => s.cue_list.unload(),
            }
        }
        Figures => {
            let target: Vec<crate::figures::Figure> = from(v)?;
            let gone: Vec<String> = s.figures.list().iter().filter(|f| !target.iter().any(|t| t.name == f.name)).map(|f| f.name.clone()).collect();
            for name in gone {
                s.figures.remove(&name).map_err(io)?;
            }
            for fig in target {
                if s.figures.get(&fig.name) != Some(&fig) {
                    s.figures.save(fig).map_err(bad)?;
                }
            }
            crate::figures::refresh(s);
        }
        Objects => {
            let target: Vec<crate::wire::WireObject> = from(v)?;
            let gone: Vec<String> = s.objects.list().iter().filter(|o| !target.iter().any(|t| t.name == o.name)).map(|o| o.name.clone()).collect();
            for name in gone {
                s.objects.remove(&name).map_err(io)?;
            }
            for obj in target {
                if s.objects.get(&obj.name) != Some(&obj) {
                    s.objects.save(obj).map_err(bad)?;
                }
            }
            crate::wire::refresh(s);
        }
        Venue => crate::web::apply_venue(s, from(v)?).map_err(crate::web::VenueRefusal::status_and_text)?,
        Projection => s.projection.set(from(v)?).map_err(bad)?,
        Fx => {
            let cells = from(v)?;
            s.fx.set_cells(cells, &s.controls).map_err(bad)?;
        }
        Executors => {
            s.executors.set_bank(from(v)?);
            s.executors.save();
        }
        Desks => {
            s.desks.set_tabs(from(v)?);
            s.desks.save();
        }
        // Checked again (a cue a step names may be gone since).
        Macros => crate::macros::set(s, from(v)?).map_err(|e| (400, e.message_fr))?,
        ShowList => {
            let list = crate::showlist::ShowList { items: from(v)?, loop_list: s.showlist.list().loop_list };
            crate::showlist::set(s, list).map_err(|e| (400, e))?
        }
        // Straight to the store: undoing a preset's creation removes it
        // even if a cue refers to it since (that cue plays its own values).
        Pools => s.pools.set(from(v)?).map_err(bad)?,
        Agenda => crate::agenda::set(s, from(v)?).map_err(|e| (400, e))?,
        // Checked again (a page it names may be gone since).
        Autopilot => crate::autopilot::set_config(s, from(v)?).map_err(|e| (400, e))?,
    }
    Ok(())
}

/// One edit: what the changed sections were before and after it.
#[derive(Clone, Debug, Serialize)]
pub struct EditEntry {
    pub id: u64,
    /// French, e.g. « Cue « Éventail » modifiée ».
    pub label: String,
    /// UTC, ISO 8601.
    pub at: String,
    /// The changed sections' keys, comma-separated.
    pub section: String,
    #[serde(skip)]
    pub before: Map<String, Value>,
    #[serde(skip)]
    pub after: Map<String, Value>,
}

/// The two stacks, newest last.
#[derive(Debug)]
pub struct EditHistory {
    undo: Vec<EditEntry>,
    redo: Vec<EditEntry>,
    cap: usize,
    next_id: u64,
    /// Bumped on every change, so the page knows when to reload the list.
    pub rev: u64,
}

impl Default for EditHistory {
    fn default() -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), cap: CAP, next_id: 1, rev: 0 }
    }
}

impl EditHistory {
    /// A new edit: on top of the undo stack (the oldest goes beyond the
    /// cap); the redo stack is emptied.
    pub fn push(&mut self, label: String, before: Map<String, Value>, after: Map<String, Value>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let section = before.keys().cloned().collect::<Vec<_>>().join(",");
        self.undo.push(EditEntry { id, label, at: crate::project::utc_now(), section, before, after });
        if self.undo.len() > self.cap {
            let over = self.undo.len() - self.cap;
            self.undo.drain(..over);
        }
        self.redo.clear();
        self.rev += 1;
        id
    }

    /// `clear`, handing back what it held so the caller drops it with the
    /// studio lock released (a long history holds whole sections, T-488).
    pub fn take(&mut self) -> Vec<EditEntry> {
        let mut gone = std::mem::take(&mut self.undo);
        gone.append(&mut self.redo);
        if !gone.is_empty() {
            self.rev += 1;
        }
        gone
    }

    /// Another project: nothing of the previous one can be undone.
    pub fn clear(&mut self) {
        if !self.undo.is_empty() || !self.redo.is_empty() {
            self.undo.clear();
            self.redo.clear();
            self.rev += 1;
        }
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// For `/api/state` and `/api/frame`: counts, the next labels, `rev`.
    pub fn summary(&self) -> Value {
        json!({
            "undo": self.undo.len(),
            "redo": self.redo.len(),
            "undo_label": self.undo.last().map(|e| &e.label),
            "redo_label": self.redo.last().map(|e| &e.label),
            "rev": self.rev,
        })
    }

    /// `GET /api/history`: both stacks, newest first.
    pub fn view(&self) -> Value {
        json!({
            "undo": self.undo.iter().rev().collect::<Vec<_>>(),
            "redo": self.redo.iter().rev().collect::<Vec<_>>(),
            "cap": self.cap,
            "rev": self.rev,
        })
    }
}

/// One recorded edit (or undo / redo) at a time, across HTTP threads:
/// taken before the engine lock, never inside it.
static EDITS: Mutex<()> = Mutex::new(());

pub fn edit_lock() -> MutexGuard<'static, ()> {
    EDITS.lock().unwrap_or_else(|e| e.into_inner())
}

/// After `path` answered 2xx: the sections that changed since `before`
/// become one entry. Nothing changed: nothing recorded.
pub fn record(s: &mut Shared, path: &str, before: Map<String, Value>) {
    let mut changed_before = Map::new();
    let mut changed_after = Map::new();
    for (key, old) in before {
        let Some(sec) = Section::from_key(&key) else { continue };
        let now = capture(s, sec);
        if now != old {
            changed_before.insert(key.clone(), old);
            changed_after.insert(key, now);
        }
    }
    if changed_after.is_empty() {
        return;
    }
    let label = label_for(s, path, &changed_before, &changed_after);
    s.history.push(label, changed_before, changed_after);
}

fn stack(h: &EditHistory, redo: bool) -> &[EditEntry] {
    if redo {
        &h.redo
    } else {
        &h.undo
    }
}

/// `POST /api/undo` (`redo` false) and `POST /api/redo`: one step, or
/// with `to` every step down to and including entry `to` (the history
/// panel). Returns the labels done and the new summary.
pub fn step(shared: &Mutex<Shared>, redo: bool, to: Option<u64>) -> Result<Value, Refusal> {
    let _edit = edit_lock();
    let mut guard = shared.lock().unwrap();
    let s = &mut *guard;
    if let Some(id) = to {
        if !stack(&s.history, redo).iter().any(|e| e.id == id) {
            return Err((404, "entrée d'historique inconnue".into()));
        }
    }
    let mut done = Vec::new();
    while let Some(entry) = stack(&s.history, redo).last().cloned() {
        let (expected, target) = if redo { (&entry.before, &entry.after) } else { (&entry.after, &entry.before) };
        // Still what the entry left? Otherwise restoring would lose a
        // change made outside the history.
        // The venue and the projection zones are safety settings: not
        // undone nor redone under the safety lock (T-261).
        if target.keys().any(|k| matches!(Section::from_key(k), Some(Venue | Projection))) && crate::safetylock::check(s, std::time::Instant::now()) {
            return Err((423, crate::safetylock::LOCKED_FR.into()));
        }
        let moved = expected.iter().any(|(key, v)| Section::from_key(key).is_none_or(|sec| capture(s, sec) != *v));
        if moved {
            s.history.clear();
            let verb = if redo { "rétablir" } else { "annuler" };
            return Err((409, format!("Impossible d'{verb} « {} » : modifié depuis hors de l'historique. Historique effacé.", entry.label)));
        }
        for (key, v) in target {
            let sec = Section::from_key(key).expect("checked above");
            restore(s, sec, v).map_err(|(code, msg)| (code, format!("« {} » : {msg}", entry.label)))?;
        }
        // What the stores kept (sanitised), so the other way checks against it.
        let now: Map<String, Value> = target.keys().filter_map(|k| Section::from_key(k).map(|sec| (k.clone(), capture(s, sec)))).collect();
        let mut entry = if redo { s.history.redo.pop() } else { s.history.undo.pop() }.expect("the entry is on top");
        if redo {
            entry.after = now;
            s.history.undo.push(entry.clone());
        } else {
            entry.before = now;
            s.history.redo.push(entry.clone());
        }
        s.history.rev += 1;
        done.push(entry.label.clone());
        if to.is_none_or(|id| id == entry.id) {
            break;
        }
    }
    if done.is_empty() {
        return Err((409, if redo { "Rien à rétablir" } else { "Rien à annuler" }.into()));
    }
    // Edits change looks the engine may be showing: redraw.
    s.settings_rev += 1;
    Ok(json!({ "done": done, "history": s.history.summary() }))
}

// ---------- labels ----------

/// What happened to a named list: the first difference found.
#[derive(Debug, PartialEq)]
enum ListChange {
    Added(String),
    Removed(String),
    Changed(String),
    Reordered,
}

/// Compares two JSON arrays of objects by `key`; `name` is what is shown.
fn list_change(before: &Value, after: &Value, key: &str, name: &str) -> Option<ListChange> {
    let (Some(a), Some(b)) = (before.as_array(), after.as_array()) else { return None };
    let shown = |item: &Value| item.get(name).or_else(|| item.get(key)).and_then(Value::as_str).unwrap_or_default().to_string();
    let find = |list: &[Value], k: &Value| list.iter().find(|i| i.get(key) == Some(k)).cloned();
    for item in b {
        match item.get(key).and_then(|k| find(a, k)) {
            None => return Some(ListChange::Added(shown(item))),
            Some(old) if old != *item => return Some(ListChange::Changed(shown(item))),
            Some(_) => {}
        }
    }
    for item in a {
        if item.get(key).and_then(|k| find(b, k)).is_none() {
            return Some(ListChange::Removed(shown(item)));
        }
    }
    (a != b).then_some(ListChange::Reordered)
}

/// « Scène « Intro » supprimée »: noun, feminine, participle (masculine)
/// for an addition.
fn named(change: Option<ListChange>, noun: &str, plural: &str, fem: bool, added: &str) -> String {
    let e = if fem { "e" } else { "" };
    match change {
        Some(ListChange::Added(n)) => format!("{noun} « {n} » {added}{e}"),
        Some(ListChange::Removed(n)) => format!("{noun} « {n} » supprimé{e}"),
        Some(ListChange::Changed(n)) => format!("{noun} « {n} » modifié{e}"),
        Some(ListChange::Reordered) => format!("{plural} réordonné{e}s"),
        None => format!("{plural} modifié{e}s"),
    }
}

/// The French label of an edit, from what changed.
fn label_for(s: &Shared, path: &str, before: &Map<String, Value>, after: &Map<String, Value>) -> String {
    let order = sections_for(path).unwrap_or(&[]);
    let Some(&sec) = order.iter().find(|sec| after.contains_key(sec.key())) else { return "Modification".into() };
    let (old, new) = (&before[sec.key()], &after[sec.key()]);
    let cue_name = |id: &str| s.presets.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string());
    match (path, sec) {
        ("/api/grid/move", Pages) => "Cue déplacée dans la grille".into(),
        ("/api/grid/clear", Pages) => "Case de la grille vidée".into(),
        (_, Scenes) => named(list_change(old, new, "name", "name"), "Scène", "Scènes", true, "enregistré"),
        (_, UserCues) => named(list_change(old, new, "id", "name"), "Cue", "Cues", true, "créé"),
        (_, Palettes) => named(list_change(old, new, "name", "name"), "Palette", "Palettes", true, "ajouté"),
        (_, Autopilot) => "Pilote automatique modifié".into(),
        (_, Pools) => named(list_change(old, new, "id", "name"), "Préréglage", "Préréglages", false, "ajouté"),
        (_, ScanPresets) => named(list_change(old, new, "id", "name"), "Préréglage de balayage", "Préréglages de balayage", false, "ajouté"),
        (_, CueLists) => named(list_change(old, new, "name", "name"), "Liste de cues", "Listes de cues", true, "créé"),
        (_, Figures) => named(list_change(old, new, "name", "name"), "Figure", "Figures", true, "enregistré"),
        ("/api/objects/import", Objects) => named(list_change(old, new, "name", "name"), "Objet 3D", "Objets 3D", false, "importé"),
        (_, Objects) => named(list_change(old, new, "name", "name"), "Objet 3D", "Objets 3D", false, "enregistré"),
        (_, CueProps) => {
            let (a, b) = (old.as_object(), new.as_object());
            let id = match (a, b) {
                (Some(a), Some(b)) => a.keys().chain(b.keys()).find(|k| a.get(*k) != b.get(*k)).cloned(),
                _ => None,
            };
            match id {
                Some(id) if path == "/api/cues/program" => format!("Programme du cue « {} » modifié", cue_name(&id)),
                Some(id) if path == "/api/cues/modifiers" => format!("Modificateurs du cue « {} » enregistrés", cue_name(&id)),
                Some(id) if path == "/api/cues/parts" => format!("Parties du cue « {} » modifiées", cue_name(&id)),
                Some(id) => format!("Propriétés du cue « {} » modifiées", cue_name(&id)),
                None => "Propriétés des cues modifiées".into(),
            }
        }
        (_, Pages) => {
            // Default pages (an empty list) as the operator sees them.
            let shown = |v: &Value| {
                let mut v = v.clone();
                if v["pages"].as_array().is_some_and(Vec::is_empty) {
                    let cap = v["cols"].as_u64().unwrap_or(8) * v["rows"].as_u64().unwrap_or(5);
                    v["pages"] = json!(crate::grid::default_pages(&s.presets, cap as usize));
                }
                v
            };
            page_label(&shown(old), &shown(new))
        }
        (_, Lfos) => {
            let n = |v: &Value| v.as_array().map_or(0, Vec::len);
            match n(new).cmp(&n(old)) {
                std::cmp::Ordering::Greater => "LFO ajouté".into(),
                std::cmp::Ordering::Less => "LFO supprimé".into(),
                std::cmp::Ordering::Equal => "LFO modifié".into(),
            }
        }
        (_, AudioRoutes) if path == "/api/audio/preset" => "Préréglage audio appliqué".into(),
        (_, AudioRoutes) => "Liens audio modifiés".into(),
        (_, Venue) => "Lieu et projecteurs modifiés".into(),
        // Only the chasers changed (T-170): say so.
        (_, Projection) if only_chasers_changed(old, new) => "Chenillards de zones modifiés".into(),
        (_, Projection) => "Zones de projection modifiées".into(),
        (_, Fx) => fx_label(old, new),
        (_, Executors) => exec_label(old, new),
        (_, Desks) => desk_label(old, new),
        (_, Macros) => named(list_change(old, new, "id", "name"), "Macro", "Macros", true, "créé"),
        (_, ShowList) => "Liste de shows modifiée".into(),
        (_, Agenda) if old.get("enabled") != new.get("enabled") => if new["enabled"] == json!(true) { "Agenda activé".into() } else { "Agenda désactivé".into() },
        (_, Agenda) => "Agenda modifié".into(),
    }
}

/// The projection section changed in its chasers only (T-170).
fn only_chasers_changed(old: &Value, new: &Value) -> bool {
    let strip = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("chasers");
        }
        v
    };
    old.get("chasers") != new.get("chasers") && strip(old) == strip(new)
}

/// « Effet « Respiration » modifié », « Effet 2.3 supprimé »…: the
/// first cell that changed.
fn fx_label(old: &Value, new: &Value) -> String {
    let cell = |v: &Value, l: usize, c: usize| v.get(l).and_then(|line| line.get(c)).cloned().unwrap_or(Value::Null);
    for l in 0..crate::fx::LINES {
        for c in 0..crate::fx::CELLS {
            let (a, b) = (cell(old, l, c), cell(new, l, c));
            if a == b {
                continue;
            }
            let name = |v: &Value| v.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(|n| format!("« {n} »"));
            let shown = name(&b).or_else(|| name(&a)).unwrap_or_else(|| format!("{}.{}", l + 1, c + 1));
            let what = match (a.is_null(), b.is_null()) {
                (true, _) => "ajouté",
                (_, true) => "supprimé",
                _ => "modifié",
            };
            return format!("Effet {shown} {what}");
        }
    }
    "Grille d'effets modifiée".into()
}

/// « Exécuteur 1.3 affecté », « Page d'exécuteurs « Beams » ajoutée »…:
/// the first difference.
fn exec_label(old: &Value, new: &Value) -> String {
    let pages = |v: &Value| v.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    let (a, b) = (pages(old), pages(new));
    let name = |p: &Value| p.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    if b.len() > a.len() {
        return format!("Page d'exécuteurs « {} » ajoutée", b.last().map(name).unwrap_or_default());
    }
    if b.len() < a.len() {
        return "Page d'exécuteurs supprimée".into();
    }
    for (i, (p, q)) in a.iter().zip(&b).enumerate() {
        if name(p) != name(q) {
            return format!("Page d'exécuteurs {} renommée", i + 1);
        }
        let (hp, hq) = (p.get("handles").and_then(Value::as_array), q.get("handles").and_then(Value::as_array));
        if let (Some(hp), Some(hq)) = (hp, hq) {
            if let Some(n) = hp.iter().zip(hq).position(|(x, y)| x != y) {
                let empty = hq[n].get("target").and_then(|t| t.get("type")).and_then(Value::as_str) == Some("none");
                return format!("Exécuteur {}.{} {}", i + 1, n + 1, if empty { "vidé" } else { "affecté" });
            }
        }
    }
    "Exécuteurs modifiés".into()
}

/// « Onglet de pupitre « Intro » ajouté », « Pupitre modifié »…
fn desk_label(old: &Value, new: &Value) -> String {
    let tabs = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let (a, b) = (tabs(old), tabs(new));
    let name = |t: &Value| t.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    if b.len() > a.len() {
        return format!("Onglet de pupitre « {} » ajouté", b.last().map(name).unwrap_or_default());
    }
    if b.len() < a.len() {
        return "Onglet de pupitre supprimé".into();
    }
    for (t, u) in a.iter().zip(&b) {
        if name(t) != name(u) {
            return format!("Onglet de pupitre « {} » renommé", name(u));
        }
        let n = |v: &Value| v.get("items").and_then(Value::as_array).map_or(0, Vec::len);
        match n(u).cmp(&n(t)) {
            std::cmp::Ordering::Greater => return format!("Composant ajouté au pupitre « {} »", name(u)),
            std::cmp::Ordering::Less => return format!("Composant supprimé du pupitre « {} »", name(u)),
            _ if t != u => return format!("Pupitre « {} » modifié", name(u)),
            _ => {}
        }
    }
    "Pupitre modifié".into()
}

/// « Page 2 renommée », « Page « Intro » ajoutée »…
fn page_label(old: &Value, new: &Value) -> String {
    let pages = |v: &Value| v.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    let name = |p: &Value| p.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let (a, b) = (pages(old), pages(new));
    if old.get("cols") != new.get("cols") || old.get("rows") != new.get("rows") {
        return "Taille de la grille modifiée".into();
    }
    if b.len() > a.len() {
        let added = b.iter().find(|p| !a.iter().any(|q| name(q) == name(p))).or(b.last());
        return format!("Page « {} » ajoutée", added.map(name).unwrap_or_default());
    }
    if b.len() < a.len() {
        let gone = a.iter().find(|p| !b.iter().any(|q| name(q) == name(p))).or(a.last());
        return format!("Page « {} » supprimée", gone.map(name).unwrap_or_default());
    }
    for (i, (p, q)) in a.iter().zip(&b).enumerate() {
        if name(p) != name(q) {
            return format!("Page {} renommée", i + 1);
        }
        if p != q {
            return format!("Page « {} » modifiée", name(q));
        }
    }
    "Pages de la grille modifiées".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Settings;
    use crate::scenes::Scene;
    use crate::test_support;

    fn scene(name: &str) -> Scene {
        Scene { name: name.into(), settings: Settings::default(), duration_secs: 4.0 }
    }

    /// An edit of the scenes made the way the HTTP wrapper does it.
    fn edit(shared: &Mutex<Shared>, path: &str, f: impl FnOnce(&mut Shared)) {
        let mut s = shared.lock().unwrap();
        let before = snapshot(&s, sections_for(path).unwrap());
        f(&mut s);
        record(&mut s, path, before);
    }

    fn names(s: &Shared) -> Vec<String> {
        s.scenes.list().iter().map(|sc| sc.name.clone()).collect()
    }

    fn shared() -> Mutex<Shared> {
        let mut s = test_support::shared();
        // Never written: these tests check memory.
        s.scenes.replace_in_memory(Vec::new());
        Mutex::new(s)
    }

    fn mem_edit(shared: &Mutex<Shared>, path: &str, scenes: Vec<Scene>) {
        edit(shared, path, |s| s.scenes.replace_in_memory(scenes));
    }

    #[test]
    fn a_deleted_scene_comes_back_identical_and_redo_deletes_it_again() {
        let shared = shared();
        mem_edit(&shared, "/api/scenes/save", vec![scene("Intro")]);
        mem_edit(&shared, "/api/scenes/save", vec![scene("Intro"), scene("Drop")]);
        mem_edit(&shared, "/api/scenes/delete", vec![scene("Drop")]);
        {
            let s = shared.lock().unwrap();
            let labels: Vec<_> = s.history.undo.iter().map(|e| e.label.as_str()).collect();
            assert_eq!(labels, ["Scène « Intro » enregistrée", "Scène « Drop » enregistrée", "Scène « Intro » supprimée"]);
            assert_eq!(s.history.undo[2].section, "scenes");
        }
        let before = json!(shared.lock().unwrap().scenes.list()[0]);
        step(&shared, false, None).unwrap();
        assert_eq!(names(&shared.lock().unwrap()), ["Intro", "Drop"]);
        step(&shared, true, None).unwrap();
        let s = shared.lock().unwrap();
        assert_eq!(names(&s), ["Drop"]);
        assert_eq!(json!(s.scenes.list()[0]), before);
    }

    #[test]
    fn the_history_keeps_fifty_and_a_new_edit_empties_redo() {
        let shared = shared();
        for i in 0..CAP + 5 {
            mem_edit(&shared, "/api/scenes/save", vec![scene(&format!("S{i}"))]);
        }
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.history.undo_len(), CAP);
            // The five oldest are forgotten.
            assert_eq!(s.history.undo[0].label, "Scène « S5 » enregistrée");
        }
        step(&shared, false, None).unwrap();
        step(&shared, false, None).unwrap();
        assert_eq!(shared.lock().unwrap().history.redo_len(), 2);
        mem_edit(&shared, "/api/scenes/save", vec![scene("Neuve")]);
        let s = shared.lock().unwrap();
        assert_eq!((s.history.undo_len(), s.history.redo_len()), (CAP - 1, 0));
    }

    #[test]
    fn nothing_changed_records_nothing_and_unknown_routes_are_not_edits() {
        let shared = shared();
        edit(&shared, "/api/scenes/save", |_| {});
        assert_eq!(shared.lock().unwrap().history.undo_len(), 0);
        for live in ["/api/cue", "/api/live", "/api/control", "/api/arm", "/api/estop", "/api/layers", "/api/settings", "/api/safety", "/api/calibration", "/api/audience", "/api/outputs/limits", "/api/shows", "/api/scenes/play"] {
            assert!(sections_for(live).is_none(), "{live} must never be in the history");
        }
        assert_eq!(step(&shared, false, None).unwrap_err().0, 409, "nothing to undo");
        assert_eq!(step(&shared, true, None).unwrap_err().0, 409, "nothing to redo");
    }

    #[test]
    fn undo_to_an_entry_undoes_everything_above_it() {
        let shared = shared();
        for name in ["A", "B", "C", "D"] {
            let mut list = shared.lock().unwrap().scenes.list().to_vec();
            list.push(scene(name));
            mem_edit(&shared, "/api/scenes/save", list);
        }
        let b = shared.lock().unwrap().history.undo[1].id;
        let done = step(&shared, false, Some(b)).unwrap();
        assert_eq!(done["done"].as_array().unwrap().len(), 3);
        assert_eq!(names(&shared.lock().unwrap()), ["A"]);
        assert_eq!(step(&shared, false, Some(999)).unwrap_err().0, 404);
        // Redo to the newest brings all back.
        let d = shared.lock().unwrap().history.redo[0].id;
        step(&shared, true, Some(d)).unwrap();
        assert_eq!(names(&shared.lock().unwrap()), ["A", "B", "C", "D"]);
    }

    #[test]
    fn a_section_changed_outside_the_history_is_never_overwritten() {
        let shared = shared();
        mem_edit(&shared, "/api/scenes/save", vec![scene("A")]);
        // E.g. a project file opened by another path.
        shared.lock().unwrap().scenes.replace_in_memory(vec![scene("Autre")]);
        let (code, msg) = step(&shared, false, None).unwrap_err();
        assert_eq!(code, 409);
        assert!(msg.contains("Historique effacé"), "{msg}");
        let s = shared.lock().unwrap();
        assert_eq!(names(&s), ["Autre"]);
        assert_eq!(s.history.undo_len(), 0);
    }

    /// Undo never changes the arm state, the e-stop or a safety setting:
    /// it only touches the sections it lists.
    #[test]
    fn undo_and_redo_never_touch_arming_or_safety() {
        let shared = shared();
        {
            let mut s = shared.lock().unwrap();
            s.gate.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        }
        let safety_before = json!(shared.lock().unwrap().safety.get());
        mem_edit(&shared, "/api/scenes/save", vec![scene("A")]);
        edit(&shared, "/api/palettes", |s| s.palettes.replace_in_memory(vec![crate::live::Palette { name: "Mer".into(), colors: vec![[0, 0, 255], [0, 255, 255]] }]));
        for redo in [false, false, true, true] {
            step(&shared, redo, None).unwrap();
            let s = shared.lock().unwrap();
            assert!(s.gate.is_armed(), "undo/redo never disarms");
            assert!(!s.estop.is_latched());
            assert_eq!(json!(s.safety.get()), safety_before);
        }
        shared.lock().unwrap().gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        step(&shared, false, None).unwrap();
        assert!(!shared.lock().unwrap().gate.is_armed(), "and never arms");
    }

    /// T-146: an FX cell edited comes back on undo; what plays, the
    /// *Action* faders and the destination are live, never in the history.
    #[test]
    fn fx_grid_edits_undo_but_playing_and_action_are_live() {
        let shared = shared();
        let original = shared.lock().unwrap().fx.grid().cells();
        edit(&shared, "/api/fx", |s| {
            let mut cells = s.fx.grid().cells();
            cells[0][0].as_mut().unwrap().name = "Souffle".into();
            cells[1][1] = None;
            s.fx.replace_cells(cells, &s.controls);
        });
        {
            let mut s = shared.lock().unwrap();
            assert_eq!(s.history.undo[0].label, "Effet « Souffle » modifié");
            assert_eq!(s.history.undo[0].section, "fx");
            s.fx.press(0, 0, true, crate::cues::Quantize::Off);
            s.fx.set_action(0, 0.3);
        }
        step(&shared, false, None).unwrap();
        let s = shared.lock().unwrap();
        assert_eq!(s.fx.grid().cells(), original);
        assert_eq!(s.fx.active(0), Some(0), "still playing");
        assert_eq!(s.fx.action(0), 0.3, "the fader is not an edit");
        assert_eq!(fx_label(&json!([[null]]), &json!([[{ "name": "" }]])), "Effet 1.1 ajouté");
    }

    #[test]
    fn labels_say_what_changed() {
        let a = json!([{ "name": "A" }, { "name": "B" }]);
        assert_eq!(list_change(&a, &json!([{ "name": "B" }, { "name": "A" }]), "name", "name"), Some(ListChange::Reordered));
        assert_eq!(list_change(&a, &json!([{ "name": "A" }]), "name", "name"), Some(ListChange::Removed("B".into())));
        assert_eq!(list_change(&a, &json!([{ "name": "A" }, { "name": "B", "x": 1 }]), "name", "name"), Some(ListChange::Changed("B".into())));
        let grid = |pages: Value| json!({ "cols": 8, "rows": 5, "pages": pages });
        let p = |n: &str| json!({ "name": n, "slots": [] });
        assert_eq!(page_label(&grid(json!([p("Un"), p("Deux")])), &grid(json!([p("Un"), p("2")]))), "Page 2 renommée");
        assert_eq!(page_label(&grid(json!([p("Un")])), &grid(json!([p("Un"), p("Intro")]))), "Page « Intro » ajoutée");
        assert_eq!(page_label(&grid(json!([p("Un"), p("Intro")])), &grid(json!([p("Un")]))), "Page « Intro » supprimée");
        let user = |name: &str| json!([{ "id": "u-1", "name": name }]);
        assert_eq!(named(list_change(&user("Éventail"), &json!([{ "id": "u-1", "name": "Éventail", "color": [1, 2, 3] }]), "id", "name"), "Cue", "Cues", true, "créé"), "Cue « Éventail » modifiée");
    }
}
