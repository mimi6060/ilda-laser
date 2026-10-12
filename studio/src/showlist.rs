//! Lecteur de shows (T-444): an ordered list of saved shows that plays one
//! after the other, with the elapsed and remaining times.
//!
//! What the list is: items naming shows of `studio-data/shows/` by name,
//! each with two options (*Suivre le tempo du système*, *Attendre le
//! timecode*), and *Boucler la liste*. It is content: `showlist.json`, the
//! project (`Project::show_list`), the history; locked in show mode.
//! Playing it (*Lancer*, *Suivant*, *Précédent*, *Arrêter*) is playback.
//!
//! How it plays: the list drives the one timeline player
//! (`timeline::Player`). Each engine frame (`frame`, before the timeline
//! advances) it watches the loaded show; when the show reaches its end it
//! loads the next one, already read and checked, and starts it in the same
//! frame, so the gap between the last frame of a show and the first of
//! the next is one engine frame.
//!
//! **Preparation**: as soon as an item starts, a worker thread (never the
//! engine's) reads every show of the list (for the times), then the next
//! playable show, and decodes its song ahead through the isolated decoder
//! (`MediaStore::prepare`, T-298's time limit). An item whose show is gone
//! or unreadable is marked *Introuvable*, one whose song can't be decoded
//! *Morceau illisible*: both are skipped with a warning, the list goes on.
//!
//! What it never does:
//! - It never arms. Disarmed, it plays in the preview like the timeline.
//! - An emergency stop (or blackout) halts the show (`Player::halt`) and
//!   stops the list; a disarm does too. After the reset nothing starts
//!   again without *Lancer*.
//! - A show launched by hand (the *Show* menu, a grid cell, `timeline.play`
//!   on another show) suspends it (*Suspendu*, *Reprendre la liste*).
//! - A show in loop (`timeline.loop`) never passes to the next one.
//!
//! *Attendre le timecode*: the item is loaded and waits; incoming timecode
//! (T-168, TC-IN armed, the external gate open) starts it as it starts any
//! loaded show, and its end (the timecode leaving the show) passes to the
//! next item.

use crate::audio::media::MediaStore;
use crate::timeline::{Clock, Show, ShowStore, TimeBase, Transport};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const MAX_ITEMS: usize = 200;
/// A show stopped this close to its end (in show units) has ended: the
/// timecode leaving the show halts it a little before.
const END_TOLERANCE: f64 = 0.25;
/// BPM of a show without a tempo map.
const DEFAULT_BPM: f64 = 120.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowList {
    pub items: Vec<ShowListItem>,
    /// *Boucler la liste*: after the last item, the first one.
    pub loop_list: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowListItem {
    /// Unique in the list; 0 (a new item) gets one when the list is set.
    pub id: u64,
    /// A show name (`timeline::valid_show_name`).
    pub show: String,
    /// The show plays on the tempo clock as it is; unticked, the item sets
    /// the clock to the show's BPM (source *Manuel*) when it starts.
    pub follow_system_tempo: bool,
    /// Loaded and held until the incoming timecode starts it.
    pub wait_timecode: bool,
}

impl Default for ShowListItem {
    fn default() -> Self {
        Self { id: 0, show: String::new(), follow_system_tempo: true, wait_timecode: false }
    }
}

impl ShowList {
    /// At most `MAX_ITEMS`, names trimmed, ids unique and non-zero (an
    /// item keeps its id, duplicates and new items get fresh ones).
    pub fn sanitize(&mut self) {
        self.items.truncate(MAX_ITEMS);
        let mut seen = std::collections::HashSet::new();
        let mut next = self.items.iter().map(|i| i.id).max().unwrap_or(0);
        for item in &mut self.items {
            item.show = item.show.trim().to_string();
            if item.id == 0 || !seen.insert(item.id) {
                next += 1;
                item.id = next;
                seen.insert(next);
            }
        }
    }

    /// Why a list sent by the page can't be saved, in French.
    pub fn validate(&self) -> Result<(), String> {
        if self.items.len() > MAX_ITEMS {
            return Err(format!("{MAX_ITEMS} shows au plus dans la liste"));
        }
        if let Some(bad) = self.items.iter().find(|i| !crate::timeline::valid_show_name(i.show.trim())) {
            return Err(format!("nom de show invalide : « {} »", bad.show));
        }
        Ok(())
    }
}

/// What the list does now. Runtime only, never saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListTransport {
    #[default]
    Stopped,
    Playing,
    /// The item is loaded and waits for the incoming timecode.
    Waiting,
    /// A show was launched by hand: *Reprendre la liste*.
    Suspended,
}

/// Why an item is skipped.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ItemProblem {
    /// *Introuvable*: the show is gone or unreadable.
    Missing(String),
    /// *Morceau illisible*: its song can't be decoded.
    BadSong(String),
}

impl ItemProblem {
    fn label_fr(&self) -> &'static str {
        match self {
            ItemProblem::Missing(_) => "Introuvable",
            ItemProblem::BadSong(_) => "Morceau illisible",
        }
    }
}

/// What the times need of a show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShowMeta {
    pub base: TimeBase,
    /// `Show::end()`, in show units.
    pub end: f64,
    /// First tempo point, else 120.
    pub bpm: f64,
}

impl ShowMeta {
    fn of(show: &Show) -> Self {
        let bpm = show.tempo_map.first().map_or(DEFAULT_BPM, |p| p.bpm as f64);
        Self { base: show.time_base, end: show.end(), bpm: if bpm.is_finite() && bpm > 0.0 { bpm } else { DEFAULT_BPM } }
    }
}

/// Show units to seconds: Temps shows at the tempo clock's BPM.
fn seconds(base: TimeBase, units: f64, bpm: f64) -> f64 {
    match base {
        TimeBase::Seconds => units,
        TimeBase::Beats => units * 60.0 / bpm.max(1.0),
    }
}

/// The state of the list, in `/api/state` (`showlist`) and `/api/showlist`.
#[derive(Clone, Debug, Serialize)]
pub struct ShowListState {
    pub transport: ListTransport,
    pub index: Option<usize>,
    pub elapsed_s: f64,
    pub remaining_s: f64,
    pub list_remaining_s: f64,
    /// Some time comes from a Temps show at the current tempo (« ≈ »).
    pub approx: bool,
    /// The next item is read, checked and its song decoded.
    pub next_ready: bool,
    /// Its index, when ready.
    pub next_index: Option<usize>,
    /// The worker is preparing (*Préparation…*).
    pub preparing: bool,
    /// The last warning (an item skipped, the list stopped by a stop).
    pub problem: Option<String>,
    pub loop_list: bool,
    /// Engine times of the last frame of the outgoing show and the first
    /// of the next, at the last change of show (seconds since startup).
    pub last_out_t: Option<f64>,
    pub first_in_t: Option<f64>,
    /// Their difference, in milliseconds.
    pub gap_ms: Option<f64>,
    /// Changes with the list (the page fetches it again).
    pub rev: u64,
}

/// What the worker found: times of every show, and the next item.
#[derive(Default)]
struct Shelf {
    gen: u64,
    done: bool,
    metas: HashMap<String, Result<ShowMeta, String>>,
    next: Option<(usize, Show)>,
    problems: Vec<(u64, ItemProblem)>,
}

/// One preparation: which shows to read, in which order to look for the
/// next one (indexes into `items`).
struct Job {
    gen: u64,
    dir: PathBuf,
    items: Vec<ShowListItem>,
    candidates: Vec<usize>,
    media: Option<Arc<MediaStore>>,
}

impl Job {
    fn run(self, shelf: &Mutex<Shelf>) {
        let store = ShowStore::new(self.dir.clone());
        let mut shows: HashMap<String, Result<Show, String>> = HashMap::new();
        for item in &self.items {
            if !shows.contains_key(&item.show) {
                let loaded = store.load(&item.show).map_err(|e| format!("{e:#}"));
                shows.insert(item.show.clone(), loaded);
            }
        }
        let metas = shows.iter().map(|(n, r)| (n.clone(), r.as_ref().map(ShowMeta::of).map_err(Clone::clone))).collect();
        let mut problems = Vec::new();
        let mut next = None;
        for &i in &self.candidates {
            let item = &self.items[i];
            let show = match &shows[&item.show] {
                Ok(show) => show,
                Err(e) => {
                    problems.push((item.id, ItemProblem::Missing(e.clone())));
                    continue;
                }
            };
            if let (Some(song), Some(media)) = (show.song(), &self.media) {
                if let Err(e) = media.prepare(&song.file) {
                    problems.push((item.id, ItemProblem::BadSong(format!("{e:#}"))));
                    continue;
                }
            }
            next = Some((i, show.clone()));
            break;
        }
        let mut s = shelf.lock().unwrap();
        if s.gen == self.gen {
            *s = Shelf { gen: self.gen, done: true, metas, next, problems };
        }
    }
}

/// How preparations run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepMode {
    /// A worker thread each (the studio).
    Thread,
    /// On the caller's thread (unit tests: the same run replays).
    #[cfg_attr(not(test), allow(dead_code))]
    Inline,
}

/// `Shared::showlist`: the list and its playback.
pub struct ShowListRunner {
    list: ShowList,
    path: Option<PathBuf>,
    pub transport: ListTransport,
    index: Option<usize>,
    /// The playing item's id: survives a reordering of the list.
    item_id: Option<u64>,
    problem: Option<String>,
    problems: HashMap<u64, ItemProblem>,
    metas: HashMap<String, Result<ShowMeta, String>>,
    shelf: Arc<Mutex<Shelf>>,
    gen: u64,
    preparing: bool,
    next: Option<(usize, Show)>,
    mode: PrepMode,
    was_armed: bool,
    /// The current item's show has played (its end can be told from a stop).
    seen_playing: bool,
    /// The current show reached its end before the next was ready.
    awaiting: bool,
    /// Engine time of the last frame the current show played, and its position.
    last_t: Option<f64>,
    last_pos: f64,
    last_out_t: Option<f64>,
    first_in_t: Option<f64>,
    rev: u64,
    /// Started by the agenda (T-455): this playback ignores *Boucler la
    /// liste* and ends after the last item.
    once: bool,
}

impl Default for ShowListRunner {
    fn default() -> Self {
        Self {
            list: ShowList::default(),
            path: None,
            transport: ListTransport::Stopped,
            index: None,
            item_id: None,
            problem: None,
            problems: HashMap::new(),
            metas: HashMap::new(),
            shelf: Arc::default(),
            gen: 0,
            preparing: false,
            next: None,
            mode: PrepMode::Thread,
            was_armed: false,
            seen_playing: false,
            awaiting: false,
            last_t: None,
            last_pos: 0.0,
            last_out_t: None,
            first_in_t: None,
            rev: 0,
            once: false,
        }
    }
}

impl ShowListRunner {
    /// `showlist.json` (missing: an empty list).
    pub fn load(path: PathBuf) -> Self {
        let mut list: ShowList = crate::load_json(&path);
        list.sanitize();
        Self { list, path: Some(path), ..Self::default() }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { mode: PrepMode::Inline, ..Self::default() }
    }

    /// `load`, preparing on the caller's thread (tests).
    #[cfg(test)]
    pub fn load_inline(path: PathBuf) -> Self {
        Self { mode: PrepMode::Inline, ..Self::load(path) }
    }

    pub fn list(&self) -> &ShowList {
        &self.list
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn index(&self) -> Option<usize> {
        self.index
    }

    /// *Boucler la liste*, unless the agenda started this playback.
    fn loops(&self) -> bool {
        self.list.loop_list && !self.once
    }

    /// The agenda's « play it once » (T-455).
    pub fn set_once(&mut self, once: bool) {
        self.once = once;
    }

    pub fn is_active(&self) -> bool {
        matches!(self.transport, ListTransport::Playing | ListTransport::Waiting)
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(path, &self.list);
        }
    }

    /// A list already checked (or a project's): the playing item keeps
    /// playing where it went; if it was removed, the list stops chaining.
    pub fn replace_in_memory(&mut self, mut list: ShowList) {
        list.sanitize();
        self.list = list;
        self.rev += 1;
        self.problems.retain(|id, _| self.list.items.iter().any(|i| i.id == *id));
        self.index = self.item_id.and_then(|id| self.list.items.iter().position(|i| i.id == id));
        if self.index.is_none() {
            self.item_id = None;
            if self.transport != ListTransport::Stopped {
                self.transport = ListTransport::Stopped;
                self.awaiting = false;
            }
        }
    }

    /// Emergency stop: nothing chains any more, until *Lancer*.
    pub fn halt(&mut self) {
        if self.transport != ListTransport::Stopped {
            self.transport = ListTransport::Stopped;
            self.awaiting = false;
            self.problem = Some("Arrêt d'urgence : la liste est arrêtée (relancer avec « Lancer »)".into());
        }
    }

    fn item(&self) -> Option<&ShowListItem> {
        self.index.and_then(|i| self.list.items.get(i))
    }

    /// The items to try after `from`, in order: the rest of the list,
    /// then (with the loop) from the top back to `from` itself.
    fn candidates_after(&self, from: Option<usize>) -> Vec<usize> {
        let n = self.list.items.len();
        let start = from.map_or(0, |i| i + 1);
        let mut out: Vec<usize> = (start..n).collect();
        if self.loops() {
            out.extend(0..start.min(n));
        }
        out
    }

    /// Pick up what the worker found, if it is for the current request.
    fn poll(&mut self) {
        let Ok(mut shelf) = self.shelf.try_lock() else { return };
        if shelf.gen != self.gen || !shelf.done {
            return;
        }
        shelf.done = false;
        self.metas = std::mem::take(&mut shelf.metas);
        self.next = shelf.next.take();
        for (id, p) in shelf.problems.drain(..) {
            self.problems.insert(id, p);
        }
        self.preparing = false;
        // New times or problems: the page fetches the list again.
        self.rev += 1;
    }

    /// Times of an item's show: seconds, and whether at the current tempo.
    fn duration(&self, item: &ShowListItem, bpm: f64) -> Option<(f64, bool)> {
        if self.problems.contains_key(&item.id) {
            return None;
        }
        let m = self.metas.get(&item.show)?.as_ref().ok()?;
        let bpm = if item.follow_system_tempo { bpm } else { m.bpm };
        Some((seconds(m.base, m.end, bpm), m.base == TimeBase::Beats))
    }

    /// The state now; `pos` = the timeline's position, `c` its clock.
    pub fn state(&self, timeline: &crate::timeline::Player, c: &Clock) -> ShowListState {
        let mut approx = false;
        let (mut elapsed_s, mut remaining_s) = (0.0, 0.0);
        let current = self.item().filter(|i| timeline.show.as_ref().is_some_and(|s| s.name == i.show) && self.transport != ListTransport::Stopped);
        if let (Some(_), Some(show)) = (current, timeline.show.as_ref()) {
            let pos = timeline.position_at(c).min(show.end());
            elapsed_s = seconds(show.time_base, pos, c.bpm);
            remaining_s = seconds(show.time_base, (show.end() - pos).max(0.0), c.bpm);
            approx |= show.time_base == TimeBase::Beats;
        }
        let rest_from = match (self.transport, self.index) {
            (ListTransport::Stopped, _) | (_, None) => 0,
            (_, Some(i)) => i + 1,
        };
        let mut list_remaining_s = remaining_s;
        for item in self.list.items.iter().skip(rest_from) {
            if let Some((d, a)) = self.duration(item, c.bpm) {
                list_remaining_s += d;
                approx |= a;
            }
        }
        let gap_ms = match (self.last_out_t, self.first_in_t) {
            (Some(a), Some(b)) => Some((b - a) * 1000.0),
            _ => None,
        };
        ShowListState {
            transport: self.transport,
            index: self.index,
            elapsed_s,
            remaining_s,
            list_remaining_s,
            approx,
            next_ready: self.next.is_some(),
            next_index: self.next.as_ref().map(|(i, _)| *i),
            preparing: self.preparing,
            problem: self.problem.clone(),
            loop_list: self.list.loop_list,
            last_out_t: self.last_out_t,
            first_in_t: self.first_in_t,
            gap_ms,
            rev: self.rev,
        }
    }

    /// Each item with its show's duration and its problem, for the page.
    fn items_view(&self, bpm: f64) -> Vec<Value> {
        self.list
            .items
            .iter()
            .map(|item| {
                let meta = self.metas.get(&item.show);
                let status = match (self.problems.get(&item.id), meta) {
                    (Some(p), _) => json!({ "kind": p.label_fr(), "message": match p { ItemProblem::Missing(m) | ItemProblem::BadSong(m) => m } }),
                    (None, Some(Err(e))) => json!({ "kind": "Introuvable", "message": e }),
                    _ => Value::Null,
                };
                let d = self.duration(item, bpm);
                json!({
                    "id": item.id,
                    "show": item.show,
                    "follow_system_tempo": item.follow_system_tempo,
                    "wait_timecode": item.wait_timecode,
                    "duration_s": d.map(|(s, _)| s),
                    "approx": d.is_some_and(|(_, a)| a),
                    "problem": status,
                })
            })
            .collect()
    }
}

// ---------- operations on the studio ----------

/// Ask the worker for the times of every show and the next item after
/// `from` (none: only the times).
fn prepare(s: &mut Shared, from: Option<usize>, want_next: bool) {
    let r = &mut s.showlist;
    r.gen += 1;
    r.next = None;
    r.preparing = true;
    let candidates = if want_next { r.candidates_after(from) } else { Vec::new() };
    *r.shelf.lock().unwrap() = Shelf { gen: r.gen, ..Shelf::default() };
    let media = s.song.enabled().then(|| Arc::clone(&s.media));
    let job = Job { gen: r.gen, dir: s.shows.dir().to_path_buf(), items: r.list.items.clone(), candidates, media };
    let shelf = Arc::clone(&r.shelf);
    match r.mode {
        PrepMode::Inline => {
            job.run(&shelf);
            r.poll();
        }
        PrepMode::Thread => {
            let spawned = std::thread::Builder::new().name("showlist-prepare".into()).spawn(move || job.run(&shelf));
            if let Err(e) = spawned {
                log::warn!("lecteur de shows : préparation impossible ({e})");
                r.preparing = false;
            }
        }
    }
}

/// The times again (a show was saved, the list changed): the next item is
/// prepared again if the list plays.
pub fn refresh(s: &mut Shared) {
    let (active, index) = (s.showlist.transport != ListTransport::Stopped, s.showlist.index);
    prepare(s, index, active);
}

/// Replace the list (the page, the history, a project): checked, saved.
pub fn set(s: &mut Shared, mut list: ShowList) -> Result<(), String> {
    list.validate()?;
    list.sanitize();
    s.showlist.replace_in_memory(list);
    s.showlist.save();
    refresh(s);
    Ok(())
}

/// *Boucler la liste* (also `showlist.loop`): saved with the list.
pub fn set_loop(s: &mut Shared, on: bool) {
    if s.showlist.list.loop_list != on {
        s.showlist.list.loop_list = on;
        s.showlist.rev += 1;
        s.showlist.save();
        refresh(s);
    }
}

/// Start item `i` with its show: tempo, load, play (or wait for the
/// timecode); the next item is prepared. Never arms.
fn begin(s: &mut Shared, i: usize, show: Show, at: Option<&Clock>) -> Result<(), String> {
    let item = s.showlist.list.items[i].clone();
    if !item.follow_system_tempo {
        let t = at.map_or_else(|| s.now_s(), |c| c.t);
        s.tempo.set_bpm_manual(ShowMeta::of(&show).bpm, t);
    }
    s.timeline.load(show);
    {
        let r = &mut s.showlist;
        r.index = Some(i);
        r.item_id = Some(item.id);
        r.problems.remove(&item.id);
        r.seen_playing = false;
        r.awaiting = false;
        r.last_t = None;
        r.last_pos = 0.0;
    }
    if item.wait_timecode {
        s.showlist.transport = ListTransport::Waiting;
    } else {
        // In the engine, on the frame's clock: the show plays this frame.
        let c = match at {
            Some(c) => Clock { bpm: s.tempo.bpm, ..*c },
            None => s.timeline_clock(),
        };
        crate::controls::timeline_play_at(s, &c).map_err(str::to_string)?;
        s.showlist.transport = ListTransport::Playing;
    }
    prepare(s, Some(i), true);
    Ok(())
}

/// Read and start the first playable item from `i` on (the operator's
/// *Lancer*, *Suivant*, *Précédent*: on their thread, like a grid cell
/// loading its show). Missing ones are skipped and marked.
fn start_from(s: &mut Shared, order: Vec<usize>) -> Result<(), String> {
    let mut skipped = Vec::new();
    for i in order {
        let item = s.showlist.list.items[i].clone();
        match s.shows.load(&item.show) {
            Ok(show) => {
                note_skipped(s, &skipped);
                return begin(s, i, show, None);
            }
            Err(e) => {
                s.showlist.problems.insert(item.id, ItemProblem::Missing(format!("{e:#}")));
                skipped.push(item.show);
            }
        }
    }
    note_skipped(s, &skipped);
    s.showlist.transport = ListTransport::Stopped;
    Err("aucun show de la liste n'est disponible".into())
}

fn note_skipped(s: &mut Shared, names: &[String]) {
    if !names.is_empty() {
        let list = names.iter().map(|n| format!("« {n} »")).collect::<Vec<_>>().join(", ");
        let msg = format!("Introuvable, sauté : {list}");
        log::warn!("lecteur de shows : {msg}");
        s.showlist.problem = Some(msg);
    }
}

/// What may stop the operator from starting the list.
fn may_start(s: &Shared) -> Result<(), String> {
    if s.estop.is_latched() {
        return Err("arrêt d'urgence actif : réinitialiser avant de lancer la liste".into());
    }
    if s.showlist.list.items.is_empty() {
        return Err("la liste de shows est vide".into());
    }
    if let Some(name) = s.timeline.unsaved() {
        return Err(format!("le show « {name} » a des modifications non enregistrées : l'enregistrer avant de lancer la liste"));
    }
    Ok(())
}

/// *Lancer* from item `index` (none: the current one, else the first).
/// Also *Reprendre la liste* (it starts the current item again).
pub fn play(s: &mut Shared, index: Option<usize>) -> Result<(), String> {
    s.showlist.once = false;
    start(s, index)
}

/// The agenda (T-455): the list from its first item, once, without
/// *Boucler la liste* (the saved list keeps it). Never arms.
pub fn play_once(s: &mut Shared) -> Result<(), String> {
    s.showlist.once = true;
    let r = start(s, Some(0));
    if r.is_err() {
        s.showlist.once = false;
    }
    r
}

fn start(s: &mut Shared, index: Option<usize>) -> Result<(), String> {
    may_start(s)?;
    let n = s.showlist.list.items.len();
    let i = index.or(s.showlist.index).unwrap_or(0);
    if i >= n {
        return Err(format!("pas d'élément {} dans la liste", i + 1));
    }
    s.showlist.problem = None;
    let order: Vec<usize> = (i..n).chain(if s.showlist.loops() { 0..i } else { 0..0 }).collect();
    start_from(s, order)
}

/// *Arrêter*: the list, and the show it plays.
pub fn stop(s: &mut Shared) {
    let ours = s.showlist.item().is_some_and(|i| s.timeline.show.as_ref().is_some_and(|sh| sh.name == i.show));
    if s.showlist.transport != ListTransport::Stopped && ours {
        s.timeline.stop();
    }
    let r = &mut s.showlist;
    r.transport = ListTransport::Stopped;
    r.awaiting = false;
    r.once = false;
}

/// *Suivant*: the next playable item (with the loop, the first after the
/// last); past the last, the list stops.
pub fn next(s: &mut Shared) -> Result<(), String> {
    may_start(s)?;
    let order = s.showlist.candidates_after(s.showlist.index);
    if order.is_empty() {
        stop(s);
        return Ok(());
    }
    s.showlist.problem = None;
    // Prepared already (for this list and this item: any change prepares
    // again): no disk read here.
    if let Some((i, show)) = s.showlist.next.take() {
        return begin(s, i, show, None);
    }
    start_from(s, order)
}

/// *Précédent*: the item before (from the first: the first again).
pub fn prev(s: &mut Shared) -> Result<(), String> {
    may_start(s)?;
    let i = s.showlist.index.map_or(0, |i| i.saturating_sub(1));
    s.showlist.problem = None;
    let n = s.showlist.list.items.len();
    start_from(s, (i..n).collect())
}

/// Once per engine frame, before the timeline advances (main.rs). Never
/// reads a file: the next show comes ready from the worker.
pub fn frame(s: &mut Shared, c: &Clock) {
    let armed = s.gate.is_armed();
    let disarmed = std::mem::replace(&mut s.showlist.was_armed, armed) && !armed;
    s.showlist.poll();
    if matches!(s.showlist.transport, ListTransport::Stopped | ListTransport::Suspended) {
        return;
    }
    if s.estop.is_latched() {
        s.timeline.halt(c);
        s.showlist.halt();
        return;
    }
    if disarmed {
        s.timeline.halt(c);
        s.showlist.transport = ListTransport::Stopped;
        s.showlist.awaiting = false;
        s.showlist.problem = Some("Laser éteint : la liste est arrêtée (relancer avec « Lancer »)".into());
        return;
    }
    let Some(item) = s.showlist.item().cloned() else {
        s.showlist.transport = ListTransport::Stopped;
        return;
    };
    if s.showlist.awaiting {
        advance(s, c);
        return;
    }
    let Some(show) = s.timeline.show.as_ref().filter(|sh| sh.name == item.show) else {
        // Another show was loaded by hand.
        s.showlist.transport = ListTransport::Suspended;
        return;
    };
    let end = show.end();
    match s.timeline.transport {
        Transport::Playing => {
            let r = &mut s.showlist;
            r.seen_playing = true;
            if r.transport == ListTransport::Waiting {
                r.transport = ListTransport::Playing;
            }
            let pos = s.timeline.position_at(c);
            if !s.timeline.loop_on && pos >= end {
                advance(s, c);
            } else {
                s.showlist.last_t = Some(c.t);
                s.showlist.last_pos = pos;
            }
        }
        // A pause (by hand, under a test pattern): the list waits.
        Transport::Paused => {}
        Transport::Stopped => {
            let r = &mut s.showlist;
            if r.transport == ListTransport::Waiting {
                return;
            }
            if r.seen_playing && r.last_pos >= end - END_TOLERANCE {
                // The timecode left the show at its end.
                advance(s, c);
            } else if item.wait_timecode {
                // The timecode dropped mid-show: wait for it again.
                r.transport = ListTransport::Waiting;
            } else {
                // Stopped by the operator.
                r.transport = ListTransport::Stopped;
            }
        }
    }
}

/// The current show has ended: the next one starts now if it is ready;
/// otherwise the list waits for it (*Préparation…*), or ends.
fn advance(s: &mut Shared, c: &Clock) {
    let last = s.showlist.last_t;
    if s.showlist.next.is_none() && s.showlist.preparing {
        if !s.showlist.awaiting {
            s.showlist.awaiting = true;
            s.timeline.stop();
        }
        return;
    }
    let skipped: Vec<String> = {
        let r = &s.showlist;
        let order = r.candidates_after(r.index);
        let upto = r.next.as_ref().map_or(order.len(), |(i, _)| order.iter().position(|k| k == i).unwrap_or(0));
        order[..upto].iter().map(|&k| &r.list.items[k]).filter(|it| r.problems.contains_key(&it.id)).map(|it| it.show.clone()).collect()
    };
    if !skipped.is_empty() {
        let list = skipped.iter().map(|n| format!("« {n} »")).collect::<Vec<_>>().join(", ");
        let msg = format!("Sauté (introuvable ou morceau illisible) : {list}");
        log::warn!("lecteur de shows : {msg}");
        s.showlist.problem = Some(msg);
    }
    let Some((i, show)) = s.showlist.next.take() else {
        // The end of the list.
        s.timeline.stop();
        let r = &mut s.showlist;
        r.transport = ListTransport::Stopped;
        r.awaiting = false;
        return;
    };
    if let Some(name) = s.timeline.unsaved() {
        // Never drop an edit to chain the next show.
        let msg = format!("le show « {name} » a des modifications non enregistrées : liste suspendue");
        s.showlist.next = Some((i, show));
        s.showlist.transport = ListTransport::Suspended;
        s.showlist.problem = Some(msg);
        return;
    }
    if let Err(why) = begin(s, i, show, Some(c)) {
        s.showlist.transport = ListTransport::Stopped;
        s.showlist.problem = Some(why);
        return;
    }
    s.showlist.last_out_t = last;
    s.showlist.first_in_t = Some(c.t);
}

/// `GET /api/showlist` (and the POST replies): the list, each item's
/// duration and problem, and the state.
pub fn view(s: &Shared) -> Value {
    let c = s.timeline_clock();
    json!({
        "items": s.showlist.items_view(c.bpm),
        "loop_list": s.showlist.list.loop_list,
        "state": s.showlist.state(&s.timeline, &c),
        "timecode_available": true,
    })
}

/// `/api/state` and `/api/frame`: the state only.
pub fn summary(s: &Shared) -> ShowListState {
    s.showlist.state(&s.timeline, &s.timeline_clock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;
    use crate::timeline::{Event, EventSource, Track};

    fn show(name: &str, len: f64) -> Show {
        let event = Event { id: 1, start: 0.0, len, source: EventSource::Cue { id: "nope".into() }, ..Default::default() };
        Show { name: name.into(), tracks: vec![Track { layer: 1, events: vec![event], ..Default::default() }], ..Default::default() }
    }

    /// A studio with its own shows folder and a simulated clock.
    fn studio(tag: &str, shows: &[(&str, f64)]) -> Shared {
        let mut s = shared();
        let dir = std::env::temp_dir().join(format!("laser-studio-showlist-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        s.shows = ShowStore::new(dir.join("shows"));
        let now = std::time::Instant::now();
        s.epoch = now;
        s.sim_now = Some(now);
        for (name, len) in shows {
            s.shows.save(&show(name, *len)).unwrap();
        }
        s
    }

    fn items(names: &[&str]) -> ShowList {
        ShowList { items: names.iter().map(|n| ShowListItem { show: n.to_string(), ..Default::default() }).collect(), loop_list: false }
    }

    /// Advance the simulated clock and run one list frame plus the
    /// timeline's (as the engine does).
    fn tick(s: &mut Shared, dt: f64) {
        let at = s.sim_now.unwrap() + std::time::Duration::from_secs_f64(dt);
        s.sim_now = Some(at);
        let c = s.timeline_clock();
        // As main.rs's `timeline_cues`: the e-stop halts the timeline first.
        if s.estop.is_latched() {
            s.timeline.halt(&c);
        }
        frame(s, &c);
        s.timeline.frame(&c);
    }

    fn run_for(s: &mut Shared, secs: f64) {
        let frames = (secs * 60.0).round() as usize;
        for _ in 0..frames {
            tick(s, 1.0 / 60.0);
        }
    }

    fn loaded(s: &Shared) -> Option<String> {
        s.timeline.show.as_ref().map(|sh| sh.name.clone())
    }

    #[test]
    fn the_list_chains_its_shows_and_stops_after_the_last() {
        let mut s = studio("chain", &[("A", 2.0), ("B", 3.0)]);
        set(&mut s, items(&["A", "B"])).unwrap();
        play(&mut s, None).unwrap();
        assert_eq!((s.showlist.transport, s.showlist.index), (ListTransport::Playing, Some(0)));
        assert!(s.showlist.next.is_some(), "the next one is prepared at once");
        run_for(&mut s, 1.0);
        let st = summary(&s);
        assert!((st.elapsed_s - 1.0).abs() < 0.05 && (st.remaining_s - 1.0).abs() < 0.05, "{st:?}");
        assert!((st.list_remaining_s - 4.0).abs() < 0.05, "{st:?}");
        run_for(&mut s, 1.1);
        assert_eq!((s.showlist.index, loaded(&s).as_deref()), (Some(1), Some("B")));
        assert!(s.timeline.is_playing());
        let gap = summary(&s).gap_ms.unwrap();
        assert!(gap < 100.0 && gap > 0.0, "gap {gap} ms");
        run_for(&mut s, 3.1);
        assert_eq!(s.showlist.transport, ListTransport::Stopped);
        assert!(!s.timeline.is_playing());
        assert!(!s.gate.is_armed(), "the player never arms");
    }

    #[test]
    fn the_loop_goes_back_to_the_first() {
        let mut s = studio("loop", &[("A", 1.0), ("B", 1.0)]);
        set(&mut s, items(&["A", "B"])).unwrap();
        set_loop(&mut s, true);
        play(&mut s, None).unwrap();
        run_for(&mut s, 2.1);
        assert_eq!((s.showlist.index, loaded(&s).as_deref()), (Some(0), Some("A")));
        assert_eq!(s.showlist.transport, ListTransport::Playing);
        // A show in loop never passes to the next.
        s.timeline.loop_on = true;
        run_for(&mut s, 2.5);
        assert_eq!(s.showlist.index, Some(0));
    }

    #[test]
    fn next_and_previous_at_the_ends() {
        let mut s = studio("ends", &[("A", 5.0), ("B", 5.0)]);
        set(&mut s, items(&["A", "B"])).unwrap();
        prev(&mut s).unwrap();
        assert_eq!(s.showlist.index, Some(0), "before the first: the first");
        next(&mut s).unwrap();
        assert_eq!((s.showlist.index, loaded(&s).as_deref()), (Some(1), Some("B")));
        next(&mut s).unwrap();
        assert_eq!(s.showlist.transport, ListTransport::Stopped, "past the last: stop");
        assert!(!s.timeline.is_playing());
        play(&mut s, Some(1)).unwrap();
        prev(&mut s).unwrap();
        assert_eq!(s.showlist.index, Some(0));
        assert!(play(&mut s, Some(7)).is_err());
    }

    #[test]
    fn a_missing_show_is_skipped_with_a_warning() {
        let mut s = studio("missing", &[("A", 1.0), ("C", 1.0)]);
        set(&mut s, items(&["A", "Gone", "C"])).unwrap();
        play(&mut s, None).unwrap();
        assert_eq!(s.showlist.next.as_ref().map(|(i, _)| *i), Some(2));
        run_for(&mut s, 1.1);
        assert_eq!((s.showlist.index, loaded(&s).as_deref()), (Some(2), Some("C")));
        assert_eq!(s.showlist.transport, ListTransport::Playing, "the list goes on");
        assert!(summary(&s).problem.unwrap().contains("Gone"));
        let v = view(&s);
        assert_eq!(v["items"][1]["problem"]["kind"], "Introuvable");
        // Deleted after the list was set, before *Lancer*: skipped too.
        std::fs::remove_file(s.shows.dir().join("A.json")).unwrap();
        play(&mut s, Some(0)).unwrap();
        assert_eq!(s.showlist.index, Some(2));
    }

    #[test]
    fn a_show_lists_tempo_is_never_proposed_to_ableton_link() {
        use crate::ableton_link::fake::{on, proposals, run, Fake};
        let mut s = studio("link", &[]);
        let mut fast = show("Rapide", 10.0);
        fast.tempo_map = vec![crate::timeline::TempoPoint { bpm: 150.0, ..Default::default() }];
        s.shows.save(&fast).unwrap();
        let mut list = items(&["Rapide"]);
        list.items[0].follow_system_tempo = false;
        set(&mut s, list).unwrap();
        let fake = Fake::new(120.0, s.epoch);
        on(&mut s, &fake);
        let mut now = s.sim_now.unwrap();
        run(&mut s, &fake, &mut now, 2);
        play(&mut s, None).unwrap();
        assert_eq!(s.tempo.bpm, 150.0, "the show's tempo, locally");
        run(&mut s, &fake, &mut now, 3);
        assert!(proposals(&fake).is_empty(), "{:?}", proposals(&fake));
        assert!((s.tempo.bpm - 120.0).abs() <= 0.01, "the session wins");
    }

    #[test]
    fn remaining_times_in_seconds_and_in_beats() {
        let mut s = studio("times", &[("A", 10.0)]);
        let mut beats = show("T", 8.0);
        beats.time_base = TimeBase::Beats;
        s.shows.save(&beats).unwrap();
        set(&mut s, items(&["A", "T"])).unwrap();
        let t = s.now_s();
        s.tempo.set_bpm_manual(120.0, t);
        play(&mut s, None).unwrap();
        run_for(&mut s, 3.0);
        let st = summary(&s);
        assert!((st.remaining_s - 7.0).abs() < 0.05, "{st:?}");
        // 8 beats at 120 BPM = 4 s, « ≈ ».
        assert!((st.list_remaining_s - 11.0).abs() < 0.05 && st.approx, "{st:?}");
        // A Temps show's item that doesn't follow the system tempo: its own (120 here).
        let v = view(&s);
        assert_eq!(v["items"][1]["duration_s"], 4.0);
        assert_eq!(v["items"][1]["approx"], true);
    }

    #[test]
    fn an_item_off_the_system_tempo_sets_the_clock_to_its_show() {
        let mut s = studio("tempo", &[]);
        let mut a = show("A", 4.0);
        a.tempo_map = vec![crate::timeline::TempoPoint { at_s: 0.0, bpm: 96.0, beats_per_bar: 4 }];
        s.shows.save(&a).unwrap();
        let mut list = items(&["A"]);
        list.items[0].follow_system_tempo = false;
        set(&mut s, list).unwrap();
        play(&mut s, None).unwrap();
        assert_eq!(s.tempo.bpm, 96.0);
        assert_eq!(s.tempo.source, crate::tempo::TempoSource::Manual);
    }

    #[test]
    fn a_show_launched_by_hand_suspends_the_list() {
        let mut s = studio("suspend", &[("A", 5.0), ("Z", 5.0)]);
        set(&mut s, items(&["A"])).unwrap();
        play(&mut s, None).unwrap();
        run_for(&mut s, 0.5);
        let z = s.shows.load("Z").unwrap();
        s.timeline.load(z);
        crate::controls::timeline_play(&mut s).unwrap();
        tick(&mut s, 0.02);
        assert_eq!(s.showlist.transport, ListTransport::Suspended);
        run_for(&mut s, 6.0);
        assert_eq!(loaded(&s).as_deref(), Some("Z"), "nothing chained while suspended");
        // *Reprendre la liste*.
        play(&mut s, None).unwrap();
        assert_eq!((s.showlist.transport, loaded(&s).as_deref()), (ListTransport::Playing, Some("A")));
    }

    #[test]
    fn an_estop_stops_the_list_and_nothing_restarts_after_the_reset() {
        let mut s = studio("estop", &[("A", 1.0), ("B", 5.0)]);
        set(&mut s, items(&["A", "B"])).unwrap();
        play(&mut s, None).unwrap();
        run_for(&mut s, 0.3);
        s.emergency_stop(crate::interlock::ArmSource::Keyboard);
        tick(&mut s, 0.02);
        assert_eq!(s.showlist.transport, ListTransport::Stopped);
        assert!(!s.timeline.is_playing());
        assert!(play(&mut s, None).is_err(), "refused while latched");
        assert!(next(&mut s).is_err());
        s.gate.reset_estop(&s.estop.clone());
        run_for(&mut s, 3.0);
        assert!(!s.timeline.is_playing() && s.showlist.transport == ListTransport::Stopped, "never by itself");
        assert!(summary(&s).problem.unwrap().contains("Arrêt d'urgence"));
        play(&mut s, None).unwrap();
        assert!(s.timeline.is_playing());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_disarm_stops_the_list() {
        let mut s = studio("disarm", &[("A", 5.0)]);
        set(&mut s, items(&["A"])).unwrap();
        crate::controls::apply(&mut s, "transport.arm", crate::controls::ControlInput::Value(1.0), false).unwrap();
        assert!(s.gate.is_armed());
        play(&mut s, None).unwrap();
        tick(&mut s, 0.02);
        s.gate.disarm(crate::interlock::DisarmReason::User, crate::interlock::ArmSource::Ui);
        tick(&mut s, 0.02);
        assert_eq!(s.showlist.transport, ListTransport::Stopped);
        assert!(!s.timeline.is_playing());
    }

    #[test]
    fn waiting_for_the_timecode_holds_the_item() {
        let mut s = studio("tc", &[("A", 2.0)]);
        let mut list = items(&["A"]);
        list.items[0].wait_timecode = true;
        set(&mut s, list).unwrap();
        play(&mut s, None).unwrap();
        run_for(&mut s, 1.0);
        assert_eq!(s.showlist.transport, ListTransport::Waiting);
        assert_eq!(loaded(&s).as_deref(), Some("A"));
        assert!(!s.timeline.is_playing(), "loaded, not started");
        // The timecode starts it (as timecode::chase does).
        let c = s.timeline_clock();
        s.timeline.play(&c);
        tick(&mut s, 0.02);
        assert_eq!(s.showlist.transport, ListTransport::Playing);
        // Halted at its end by the timecode leaving the show: the list ends.
        run_for(&mut s, 1.9);
        let c = s.timeline_clock();
        s.timeline.halt(&c);
        tick(&mut s, 0.02);
        assert_eq!(s.showlist.transport, ListTransport::Stopped);
    }

    #[test]
    fn unsaved_edits_are_never_dropped() {
        let mut s = studio("unsaved", &[("A", 5.0)]);
        set(&mut s, items(&["A"])).unwrap();
        let a = s.shows.load("A").unwrap();
        s.timeline.load_unsaved(a);
        assert!(play(&mut s, None).unwrap_err().contains("non enregistrées"));
    }

    /// A song that can't be decoded: *Morceau illisible*, skipped.
    #[test]
    fn an_unreadable_song_is_skipped() {
        let s = studio("song", &[("C", 1.0)]);
        let mut a = show("A", 1.0);
        a.audio = Some(crate::timeline::AudioRef { file: "absent.wav".into(), duration_s: 3.0, ..Default::default() });
        s.shows.save(&a).unwrap();
        let media_dir = std::env::temp_dir().join(format!("laser-studio-showlist-media-{}", std::process::id()));
        let items = items(&["A", "C"]).items;
        let job = Job { gen: 1, dir: s.shows.dir().to_path_buf(), items: items.clone(), candidates: vec![0, 1], media: Some(Arc::new(MediaStore::new(&media_dir))) };
        let shelf = Mutex::new(Shelf { gen: 1, ..Default::default() });
        job.run(&shelf);
        let shelf = shelf.into_inner().unwrap();
        assert!(shelf.done);
        assert_eq!(shelf.next.as_ref().map(|(i, _)| *i), Some(1));
        assert!(matches!(shelf.problems.as_slice(), [(id, ItemProblem::BadSong(_))] if *id == items[0].id));
        assert_eq!(ItemProblem::BadSong(String::new()).label_fr(), "Morceau illisible");
    }

    #[test]
    fn lists_sanitize_and_older_files_load() {
        let l: ShowList = serde_json::from_str(r#"{"items":[{"show":" A "},{"id":5,"show":"B"},{"id":5,"show":"C"}]}"#).unwrap();
        assert!(!l.loop_list);
        assert!(l.items[0].follow_system_tempo && !l.items[0].wait_timecode);
        let mut l2 = l.clone();
        l2.sanitize();
        let ids: Vec<u64> = l2.items.iter().map(|i| i.id).collect();
        assert_eq!(ids, [6, 5, 7]);
        assert_eq!(l2.items[0].show, "A");
        assert_eq!(serde_json::from_str::<ShowList>("{}").unwrap(), ShowList::default());
        assert!(items(&["../x"]).validate().is_err());
        let mut big = items(&["A"]);
        big.items = vec![big.items[0].clone(); MAX_ITEMS + 1];
        assert!(big.validate().is_err());
    }
}
