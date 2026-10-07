//! The cue deck: which cues are playing right now, and what a press on a
//! grid cell does to that list. A press can toggle a cue, restart it, or -
//! for flash and solo - hold it only while the key or pad stays down, after
//! which whatever played before comes back untouched.
//!
//! Cues are "latched" (they stay until stopped) or "held" (a flash: gone on
//! release). Exclusive groups and the `max_active` limiter only ever stop
//! latched cues, so a flash can't kill what it temporarily covers.
//!
//! The newest active cue is the *primary*: its look lives in
//! `Shared::settings`, so the look panel and the `look.*` controls edit the
//! cue on top. `controls::press_cue` keeps the two in sync.
//!
//! Transitions (T-158, transition.rs): a cue starting with one fades (or
//! morphs) in, and the cues it stops stay in `ending`, drawn while they
//! fade out. With *Coupe* (the default) nothing of this exists.
//!
//! Quantised launch (T-159): with *Temps* or *Mesure*, a latched cue
//! pressed between two boundaries waits in `pending` (« en attente ») and
//! starts on the next one, counting its time from it; a press just after
//! a boundary (less than ⅛ beat) starts at once, from that boundary.
//! Flashes never wait. *Mode beat*: every N beats, each layer shows the
//! next of its latched cues.

use crate::controls::ControlSource;
use crate::engine::Settings;
use crate::layers::LAYER_COUNT;
use crate::program::CueProgram;
use crate::transition::{CuePhase, Fade, LookFx, Transition, TransitionKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Number of exclusive groups a cue can join (1..=MAX_GROUP).
pub const MAX_GROUP: u8 = 8;
/// Notes and tags on a cue (T-430): at most this many tags per cue, …
pub const MAX_TAGS: usize = 8;
/// … each at most this many characters, …
pub const MAX_TAG_LEN: usize = 24;
/// … and notes of at most this many characters.
pub const MAX_NOTES_LEN: usize = 500;

/// A tag as compared (T-430): trimmed, lower case, without accents, so
/// « Intro », « intro » and « ÍNTRO » are the same tag.
pub fn tag_key(tag: &str) -> String {
    tag.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// Tags kept as typed, but trimmed and cut to `MAX_TAG_LEN` characters;
/// empty ones and repeats (by `tag_key`) dropped; at most `MAX_TAGS`.
pub fn sanitize_tags(tags: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    for t in tags.drain(..) {
        let t: String = t.trim().chars().take(MAX_TAG_LEN).collect::<String>().trim_end().to_string();
        if !t.is_empty() && kept.len() < MAX_TAGS && seen.insert(tag_key(&t)) {
            kept.push(t);
        }
    }
    *tags = kept;
}

/// Notes cut to `MAX_NOTES_LEN` characters; only blanks = none.
pub fn sanitize_notes(notes: &mut String) {
    if notes.trim().is_empty() {
        notes.clear();
    } else if notes.chars().count() > MAX_NOTES_LEN {
        *notes = notes.chars().take(MAX_NOTES_LEN).collect();
    }
}
pub const MAX_ACTIVE_LIMIT: u8 = 16;

/// What a press on a cue does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickMode {
    /// First press starts, second press stops.
    #[default]
    Toggle,
    /// Plays only while held; in « Un cue » mode it also hides the others.
    Flash,
    /// Plays only while held, alone: every other cue is hidden meanwhile.
    Solo,
    /// Every press starts the cue from the beginning.
    Restart,
}

pub const CLICK_MODES: [ClickMode; 4] = [ClickMode::Toggle, ClickMode::Flash, ClickMode::Solo, ClickMode::Restart];
pub const CLICK_MODE_LABELS: [&str; 4] = ["Basculer", "Flash", "Solo", "Relancer"];

/// When a latched cue starts after its press (T-159).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quantize {
    /// At once.
    #[default]
    Off,
    /// On the next beat.
    Beat,
    /// On the next bar's « one ».
    Bar,
}

pub const QUANTIZE_MODES: [Quantize; 3] = [Quantize::Off, Quantize::Beat, Quantize::Bar];
pub const QUANTIZE_LABELS: [&str; 3] = ["Off", "Temps", "Mesure"];
/// A press this late after a boundary (beats) starts at once, on it.
pub const CATCH_UP_BEATS: f64 = 0.125;
/// *Mode beat* choices: beats between two changes (0 = off).
pub const BEAT_EVERY: [u8; 6] = [0, 1, 2, 4, 8, 16];
pub const BEAT_EVERY_LABELS: [&str; 6] = ["Off", "1 temps", "2 temps", "4 temps", "8 temps", "16 temps"];

impl Quantize {
    /// The beat a launch pressed at `beat` starts on; `None` = at once.
    pub fn boundary(self, beat: f64, beats_per_bar: u8) -> Option<f64> {
        let grid = match self {
            Quantize::Off => return None,
            Quantize::Beat => 1.0,
            Quantize::Bar => beats_per_bar.max(1) as f64,
        };
        Some(crate::tempo::quantize(beat, grid, CATCH_UP_BEATS))
    }
}

/// Per-cue properties from the grid (« Propriétés du cue »). `None` means
/// "use the deck's setting".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CueSlot {
    pub mode: Option<ClickMode>,
    /// Exclusive group 1..=8: starting a cue stops the other cues of its group.
    pub group: Option<u8>,
    /// Layer 1..=4 (layers.rs); `None` = layer 1.
    pub layer: Option<u8>,
    /// A show (timeline.rs) the cell plays instead of its cue.
    pub show: Option<String>,
    /// Projection zones (projection.rs) the cue plays on; empty = the
    /// default zones.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<u8>,
    /// A cue program (T-157) set on this cue (`POST /api/cues/program`):
    /// replaces the cue's own when it starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<CueProgram>,
    /// The cue's own transition (T-158), instead of the grid's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
    /// « Protégé de Tout arrêter » (T-423): `cue.stop_all`, « Arrêter les
    /// cues » and « Tout arrêter » leave it playing. Its own « Arrêter »
    /// stops it; the e-stop, blackout and disarm cut it like everything.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub protect_stop: bool,
    /// « Protégé du solo »: stays shown while another cue is held in Solo.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub protect_solo: bool,
    /// Scan preset (T-415, `scan.rs`) the cue is traced with; `None` = the
    /// figure's, else the output's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
    /// The operator's free notes on the cue (T-430, « seulement avec
    /// brouillard »). Shown escaped, never interpreted.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Tags (« intro », « drop »…) the grid filter dims cells by.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The cell's colour band; set, it wins over a user cue's own colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// The cue's own live modifiers (T-144, « Enregistrer dans le cue »):
    /// the cue level starts from them each time the cue starts. Neutral
    /// in older files.
    #[serde(skip_serializing_if = "crate::levels::is_neutral")]
    pub modifiers: crate::live::LiveModifiers,
    /// A figure cue's per-part modifiers (T-441): part 1..=8 → modifiers
    /// applied to that part's points only, before `modifiers`. Empty in
    /// older files (and on other cues: they have no parts).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub part_modifiers: BTreeMap<u8, crate::live::LiveModifiers>,
    /// Speed group (T-419, speed.rs): 1..=4 = A..D, 0 = the master clock;
    /// `None` = the layer's group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_group: Option<u8>,
    /// Referenced presets (T-422, pools.rs): per family, a preset whose
    /// values replace the cue's own, resolved every frame. None in older
    /// files.
    #[serde(skip_serializing_if = "crate::pools::PresetRefs::is_empty")]
    pub refs: crate::pools::PresetRefs,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveCue {
    /// Instance id, unique for the life of the process: a restart gets a
    /// new one, so its animation starts over.
    pub id: u64,
    /// Preset id.
    pub cue: String,
    pub group: Option<u8>,
    /// Layer 1..=4, taken from the grid when the cue started.
    pub layer: u8,
    pub started_s: f64,
    /// The beat its local time counts from: its quantised boundary.
    pub started_beat: f64,
    /// Flash/solo: removed on release.
    pub held: bool,
    /// While held, only solo cues are shown.
    pub solo: bool,
    /// The cue's look. For the primary (newest) cue, `Shared::settings` is
    /// the live copy and this one is refreshed from it before every change.
    pub settings: Settings,
    /// Fading (or morphing) in (*Démarrage*); `None` once playing.
    pub fade: Option<Fade>,
    /// Who started it (« En cours »), set by `controls::press_cue_as`.
    pub source: ControlSource,
    /// Its executor level (T-418, *Fondu au curseur*, *Temp*, *Noir
    /// tenu*), 0..1, 1 for a cue started from the grid: a gain on its
    /// colours before the mixer and the safety stage. Set every frame by
    /// `executors::sync_levels`.
    pub exec_level: f32,
}

impl ActiveCue {
    /// How far in it is at `now_s`: 1 once playing, its fade-in gain
    /// (a morph's progress) while starting.
    pub fn level(&self, now_s: f64) -> f32 {
        match self.fade {
            Some(f) if !f.done(now_s) => match f.kind {
                TransitionKind::Morph => f.progress(now_s),
                kind => Fade::gain_in(kind, f.progress(now_s)),
            },
            _ => 1.0,
        }
    }
}

/// A stopped cue still drawn while it fades out (*Fin*).
#[derive(Clone, Debug, PartialEq)]
pub struct EndingCue {
    pub cue: ActiveCue,
    pub fade: Fade,
}

/// A latched cue waiting for its quantised start (« en attente »).
#[derive(Clone, Debug, PartialEq)]
pub struct PendingLaunch {
    /// Preset id (the grid cell's cue).
    pub cue: String,
    pub mode: ClickMode,
    pub at_beat: f64,
    /// The look it starts with, taken at the press.
    pub settings: Settings,
    /// Who pressed it (« En cours »), handed to the cue when it starts.
    pub source: ControlSource,
}

/// The playing cues plus the grid's trigger settings. Saved to
/// `studio-data/grid.json` without the playing list.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CueDeck {
    #[serde(skip)]
    pub active: Vec<ActiveCue>,
    /// Stopped cues fading out (T-158), oldest first.
    #[serde(skip)]
    pub ending: Vec<EndingCue>,
    /// Latched cues waiting for their quantised start (T-159).
    #[serde(skip)]
    pub pending: Vec<PendingLaunch>,
    #[serde(skip)]
    next_id: u64,
    #[serde(skip)]
    path: Option<PathBuf>,
    /// The tempo-clock beat of the last frame (`tick`), for the beat mode.
    #[serde(skip)]
    beat: f64,
    /// The manual look (scene, playlist, look panel), set aside while only
    /// flashes play, and given back when they end.
    #[serde(skip)]
    pub parked: Option<Settings>,
    /// Default mode for cues without their own.
    pub click_mode: ClickMode,
    /// « Multi »: cues add up. Off (« Un cue »): a new cue replaces the
    /// others of its layer (with one layer in use: all the others).
    pub multi: bool,
    /// Latched cues allowed at once; the oldest is stopped beyond it.
    pub max_active: u8,
    /// Per-cue properties, keyed by preset id.
    pub slots: BTreeMap<String, CueSlot>,
    /// The grid's transition (T-158); *Coupe* by default.
    pub transition: Transition,
    /// Launch quantisation (T-159); *Off* by default.
    pub quantize: Quantize,
    /// *Mode beat* (T-159): every this many beats, each layer shows the
    /// next of its latched cues; 0 = off.
    pub beat_every: u8,
}

impl Default for CueDeck {
    fn default() -> Self {
        Self {
            active: Vec::new(),
            ending: Vec::new(),
            pending: Vec::new(),
            next_id: 1,
            path: None,
            beat: 0.0,
            parked: None,
            click_mode: ClickMode::Toggle,
            multi: false,
            max_active: 4,
            slots: BTreeMap::new(),
            transition: Transition::default(),
            quantize: Quantize::Off,
            beat_every: 0,
        }
    }
}

/// When a press happened: wall clock and tempo clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct At {
    pub s: f64,
    pub beat: f64,
    /// 0 = unknown (120 is used).
    pub bpm: f64,
    pub beats_per_bar: u8,
    /// *Cadence* (T-419): live fades last `length / rate`. 0 (or anything
    /// outside 0.5..=2) = 100 %.
    pub rate: f64,
}

impl At {
    /// A transition's length in seconds at this press, under the *Cadence*.
    fn fade_len(&self, t: &Transition) -> f64 {
        let rate = if self.rate.is_finite() && self.rate > 0.0 { self.rate.clamp(0.5, 2.0) } else { 1.0 };
        t.length.seconds(self.bpm) / rate
    }

    /// Seconds per beat.
    fn beat_len(&self) -> f64 {
        60.0 / if self.bpm.is_finite() && self.bpm > 0.0 { self.bpm } else { 120.0 }
    }

    /// The same clock at beat `beat` (earlier or later).
    fn at_beat(self, beat: f64) -> At {
        At { s: self.s + (beat - self.beat) * self.beat_len(), beat, ..self }
    }
}

impl CueDeck {
    /// Loads `grid.json`, or starts empty. Changes are saved back to `path`.
    pub fn load(path: PathBuf) -> Self {
        let mut deck: CueDeck = crate::load_json(&path);
        deck.path = Some(path);
        deck.sanitize();
        deck
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(Path::new(path), self);
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Takes the grid settings of `other` (a project's grid, T-286); the
    /// playing cues stay, within the new `max_active`. Memory only.
    pub fn set_config(&mut self, other: &CueDeck) {
        self.click_mode = other.click_mode;
        self.multi = other.multi;
        self.slots = other.slots.clone();
        self.transition = other.transition;
        self.quantize = other.quantize;
        self.beat_every = other.beat_every;
        self.sanitize();
        self.set_max_active(other.max_active);
    }

    /// Clamps and drops out-of-range grid properties.
    pub fn sanitize(&mut self) {
        self.max_active = self.max_active.clamp(1, MAX_ACTIVE_LIMIT);
        self.next_id = self.next_id.max(1);
        self.transition = self.transition.sanitized();
        if !BEAT_EVERY.contains(&self.beat_every) {
            self.beat_every = 0;
        }
        for slot in self.slots.values_mut() {
            slot.group = slot.group.filter(|g| (1..=MAX_GROUP).contains(g));
            slot.layer = slot.layer.filter(|n| (2..=LAYER_COUNT as u8).contains(n));
            slot.show = slot.show.take().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            slot.zones.retain(|z| (1..=crate::projection::MAX_ZONES).contains(z));
            slot.zones.sort_unstable();
            slot.zones.dedup();
            slot.program = slot.program.take().filter(|p| p.validate().is_ok());
            slot.modifiers = crate::levels::for_storage(&slot.modifiers);
            slot.part_modifiers.retain(|part, _| (1..=crate::figures::MAX_PARTS).contains(part));
            for m in slot.part_modifiers.values_mut() {
                *m = crate::levels::for_storage(m);
            }
            slot.part_modifiers.retain(|_, m| !crate::levels::is_neutral(m));
            slot.transition = slot.transition.map(Transition::sanitized);
            sanitize_notes(&mut slot.notes);
            sanitize_tags(&mut slot.tags);
            slot.speed_group = slot.speed_group.filter(|g| crate::speed::check_group(*g).is_ok());
        }
        self.slots.retain(|_, slot| *slot != CueSlot::default());
    }

    pub fn set_slot(&mut self, cue: &str, slot: CueSlot) {
        self.slots.insert(cue.to_string(), slot);
        self.sanitize();
    }

    pub fn set_max_active(&mut self, n: u8) {
        self.max_active = n.clamp(1, MAX_ACTIVE_LIMIT);
        self.enforce_limit(None);
    }

    pub fn slot(&self, cue: &str) -> CueSlot {
        self.slots.get(cue).cloned().unwrap_or_default()
    }

    /// The scan preset set on a cue's cell (T-415), without cloning the slot.
    pub fn slot_scan(&self, cue: &str) -> Option<&str> {
        self.slots.get(cue).and_then(|s| s.scan.as_deref())
    }

    /// The distinct tags of the grid's cues (T-430), sorted without case
    /// or accents, each with how many cues carry it; the spelling shown is
    /// the first met.
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut by_key: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for t in self.slots.values().flat_map(|s| &s.tags) {
            by_key.entry(tag_key(t)).or_insert_with(|| (t.clone(), 0)).1 += 1;
        }
        by_key.into_values().collect()
    }

    /// The program set on `cue` from the grid, if any.
    pub fn program(&self, cue: &str) -> Option<&CueProgram> {
        self.slots.get(cue)?.program.as_ref()
    }

    /// `look` as cue `cue` plays it: with the grid's program when it has one.
    pub fn with_program(&self, cue: &str, mut look: Settings) -> Settings {
        if let Some(p) = self.program(cue) {
            look.program = Some(p.clone());
        }
        look
    }

    /// The mode a press on `cue` uses: its own, else the deck's.
    pub fn mode_of(&self, cue: &str) -> ClickMode {
        self.slot(cue).mode.unwrap_or(self.click_mode)
    }

    /// The transition `cue` starts and stops with: its own, else the
    /// grid's. Flashes and solos cut unless it says otherwise.
    pub fn transition_of(&self, cue: &str, held: bool) -> Transition {
        let t = self.slot(cue).transition.unwrap_or(self.transition);
        if held && !t.in_flash {
            Transition::CUT
        } else {
            t
        }
    }

    /// A key or pad went down. `mode` overrides the cue's mode (Shift +
    /// letter flashes). `settings` builds the look of a newly started cue.
    pub fn press(&mut self, cue: &str, mode: Option<ClickMode>, at: At, settings: impl FnOnce() -> Settings) {
        let mode = mode.unwrap_or_else(|| self.mode_of(cue));
        let latched = self.active.iter().position(|a| a.cue == cue && !a.held);
        let pending = self.pending.iter().position(|p| p.cue == cue);
        match mode {
            ClickMode::Toggle => match (latched, pending) {
                (Some(i), _) => {
                    self.pending.retain(|p| p.cue != cue);
                    let a = self.active.remove(i);
                    let t = self.transition_of(cue, false);
                    self.retire(a, t, at, None);
                }
                // A second press on a waiting cue takes it back.
                (None, Some(j)) => {
                    self.pending.remove(j);
                }
                (None, None) => self.launch(cue, mode, at, settings()),
            },
            ClickMode::Restart => {
                if pending.is_none() {
                    self.launch(cue, mode, at, settings());
                }
            }
            ClickMode::Flash => {
                if !self.active.iter().any(|a| a.cue == cue) {
                    // With a single cue at a time, a flash takes over the output.
                    self.start(cue, at, settings(), true, !self.multi, Vec::new());
                }
            }
            ClickMode::Solo => {
                if !self.active.iter().any(|a| a.cue == cue && a.held) {
                    self.start(cue, at, settings(), true, true, Vec::new());
                }
            }
        }
    }

    /// A latched start: now, or on the next quantised boundary.
    fn launch(&mut self, cue: &str, mode: ClickMode, at: At, settings: Settings) {
        match self.quantize.boundary(at.beat, at.beats_per_bar) {
            Some(b) if b > at.beat => self.pending.push(PendingLaunch { cue: cue.to_string(), mode, at_beat: b, settings, source: ControlSource::Ui }),
            // On the boundary or just after it: at once, counting from it.
            Some(b) => self.begin(cue, mode, at.at_beat(b), settings),
            None => self.begin(cue, mode, at, settings),
        }
    }

    /// A latched cue starts now (`at`: its start, maybe a hair ago).
    fn begin(&mut self, cue: &str, mode: ClickMode, at: At, settings: Settings) {
        let latched = self.active.iter().position(|a| a.cue == cue && !a.held);
        match (mode, latched) {
            (ClickMode::Restart, Some(i)) => {
                let old = self.active.remove(i);
                self.start(cue, at, settings, false, false, vec![old]);
            }
            // Started meanwhile (another launch): nothing more.
            (_, Some(_)) => {}
            (_, None) => self.start(cue, at, settings, false, false, Vec::new()),
        }
    }

    /// Waiting cues whose boundary is reached start now, from it. Returns
    /// whether any did.
    pub fn fire_due(&mut self, at: At) -> bool {
        if !self.pending.iter().any(|p| p.at_beat <= at.beat) {
            return false;
        }
        let (due, wait): (Vec<PendingLaunch>, Vec<PendingLaunch>) = std::mem::take(&mut self.pending).into_iter().partition(|p| p.at_beat <= at.beat);
        self.pending = wait;
        for p in due {
            let first = self.next_id;
            self.begin(&p.cue, p.mode, at.at_beat(p.at_beat), p.settings);
            // Credited to whoever pressed it, not to the clock.
            for a in self.active.iter_mut().filter(|a| a.id >= first) {
                a.source = p.source;
            }
        }
        true
    }

    /// The key or pad came up: a held flash/solo of `cue` ends. Latched
    /// cues ignore releases.
    pub fn release(&mut self, cue: &str, at: At) {
        let t = self.transition_of(cue, true);
        self.retire_where(|a| a.held && a.cue == cue, t, at);
    }

    /// Every held flash/solo ends (the page that held them is gone): at once.
    pub fn release_all_held(&mut self) {
        self.active.retain(|a| !a.held);
    }

    /// Everything stops, protected cues included (a scene shown by hand).
    pub fn stop_all(&mut self) {
        self.active.clear();
        self.ending.clear();
        self.pending.clear();
    }

    /// Every instance of `cue` goes at once (a user cue deleted).
    pub fn forget(&mut self, cue: &str) {
        self.active.retain(|a| a.cue != cue);
        self.ending.retain(|e| e.cue.cue != cue);
        self.pending.retain(|p| p.cue != cue);
    }

    /// E-stop / blackout: transitions end at once (fading cues are gone,
    /// starting ones are simply playing) and nothing waits any more.
    pub fn cut_transitions(&mut self) {
        self.ending.clear();
        self.pending.clear();
        for a in &mut self.active {
            a.fade = None;
        }
    }

    /// `cue.stop_all`, « Arrêter les cues », « Tout arrêter » (T-423): every
    /// cue but those protected from « Tout arrêter » stops at once, with
    /// its fade-out and its waiting launch. Protected cues keep all three.
    pub fn stop_unprotected(&mut self) {
        let slots = &self.slots;
        let kept = |cue: &str| slots.get(cue).is_some_and(|s| s.protect_stop);
        self.active.retain(|a| kept(&a.cue));
        self.ending.retain(|e| kept(&e.cue.cue));
        self.pending.retain(|p| kept(&p.cue));
    }

    /// Whether `cue` stays shown while another cue is held in Solo.
    fn solo_proof(&self, cue: &str) -> bool {
        self.slots.get(cue).is_some_and(|s| s.protect_solo)
    }

    /// « Vider » a layer: every cue on it stops at once, held ones included.
    pub fn clear_layer(&mut self, layer: u8) {
        self.active.retain(|a| a.layer != layer);
        self.ending.retain(|e| e.cue.layer != layer);
        let on_layer: Vec<String> = self.pending.iter().filter(|p| self.slot(&p.cue).layer.unwrap_or(1) == layer).map(|p| p.cue.clone()).collect();
        self.pending.retain(|p| !on_layer.contains(&p.cue));
    }

    /// Once per frame: the beat (for the beat mode); finished transitions end.
    pub fn tick(&mut self, at: At) {
        self.beat = at.beat;
        self.ending.retain(|e| !e.fade.done(at.s));
        for a in &mut self.active {
            if a.fade.is_some_and(|f| f.done(at.s)) {
                a.fade = None;
            }
        }
    }

    /// `cue` leaves the playing list: kept while it fades out with `t`
    /// (into the new cue `into` for a morph), else gone.
    ///
    /// A cue stopped while still fading in fades out from where it is,
    /// never from full: its fade-out starts part-way, at its current level
    /// (and it is never a morph's source, which would draw it at full).
    fn retire(&mut self, cue: ActiveCue, t: Transition, at: At, into: Option<u64>) {
        if t.is_cut() {
            return;
        }
        let level = cue.level(at.s);
        let into = into.filter(|_| level >= 1.0);
        let kind = match (t.kind, into) {
            (TransitionKind::Morph, None) => TransitionKind::Fade,
            (kind, _) => kind,
        };
        let length_s = at.fade_len(&t);
        let start_s = at.s - Fade::out_progress_at(kind, level) as f64 * length_s;
        let fade = Fade { kind, start_s, length_s, partner: into };
        self.ending.push(EndingCue { cue, fade });
    }

    fn retire_where(&mut self, stop: impl Fn(&ActiveCue) -> bool, t: Transition, at: At) {
        let mut i = 0;
        while i < self.active.len() {
            if stop(&self.active[i]) {
                let a = self.active.remove(i);
                self.retire(a, t, at, None);
            } else {
                i += 1;
            }
        }
    }

    /// Starts `cue`; `replaced` are cues it takes the place of (a restart's
    /// old instance), to which the layer or group rule adds.
    fn start(&mut self, cue: &str, at: At, settings: Settings, held: bool, solo: bool, mut replaced: Vec<ActiveCue>) {
        let slot = self.slot(cue);
        let (group, layer) = (slot.group, slot.layer.unwrap_or(1));
        if !held {
            let multi = self.multi;
            // One cue per layer: a beam on layer 2 stays over the tunnel on layer 1.
            let hit = |a: &ActiveCue| !a.held && if multi { group.is_some() && a.group == group } else { a.layer == layer };
            let mut i = 0;
            while i < self.active.len() {
                if hit(&self.active[i]) {
                    replaced.push(self.active.remove(i));
                } else {
                    i += 1;
                }
            }
        }
        let t = self.transition_of(cue, held);
        let id = self.next_id;
        self.next_id += 1;
        // A morph turns the first cue replaced that is fully in into this
        // one; the others fade from where they are.
        let from = (t.kind == TransitionKind::Morph && !t.is_cut()).then(|| replaced.iter().find(|a| a.level(at.s) >= 1.0).map(|a| a.id)).flatten();
        for a in replaced {
            let into = (Some(a.id) == from).then_some(id);
            self.retire(a, t, at, into);
        }
        let fade = (!t.is_cut()).then(|| Fade { kind: t.kind, start_s: at.s, length_s: at.fade_len(&t), partner: from });
        self.active.push(ActiveCue {
            id,
            cue: cue.to_string(),
            group,
            layer,
            started_s: at.s,
            started_beat: at.beat,
            held,
            solo,
            settings,
            fade,
            source: ControlSource::Ui,
            exec_level: 1.0,
        });
        self.enforce_limit(Some(at));
    }

    /// Stop the oldest latched cues until at most `max_active` remain
    /// (fading out with the grid's transition when `at` is known).
    fn enforce_limit(&mut self, at: Option<At>) {
        while self.active.iter().filter(|a| !a.held).count() > self.max_active.max(1) as usize {
            let Some(oldest) = self.active.iter().position(|a| !a.held) else { break };
            let a = self.active.remove(oldest);
            if let Some(at) = at {
                self.retire(a, self.transition, at, None);
            }
        }
    }

    /// The cues that actually reach the output, oldest first: held solo
    /// cues hide everything else but the cues protected from the solo; a
    /// held flash hides the latched cues of its group; in beat mode, each
    /// layer shows one of its latched cues.
    pub fn visible(&self) -> Vec<&ActiveCue> {
        if self.active.iter().any(|a| a.held && a.solo) {
            return self.active.iter().filter(|a| (a.held && a.solo) || self.solo_proof(&a.cue)).collect();
        }
        let flashed_groups: Vec<u8> = self.active.iter().filter(|a| a.held).filter_map(|a| a.group).collect();
        let shown: Vec<&ActiveCue> = self.active.iter().filter(|a| a.held || !a.group.is_some_and(|g| flashed_groups.contains(&g))).collect();
        if self.beat_every == 0 {
            return shown;
        }
        let step = (self.beat / self.beat_every as f64).floor() as i64;
        let mut keep: Vec<u64> = Vec::new();
        for layer in 1..=LAYER_COUNT as u8 {
            let latched: Vec<u64> = shown.iter().filter(|a| !a.held && a.layer == layer).map(|a| a.id).collect();
            if !latched.is_empty() {
                keep.push(latched[step.rem_euclid(latched.len() as i64) as usize]);
            }
        }
        shown.into_iter().filter(|a| a.held || keep.contains(&a.id)).collect()
    }

    /// The cues fading out that are drawn (none while a solo is held).
    pub fn ending_visible(&self) -> impl Iterator<Item = &EndingCue> {
        let solo = self.active.iter().any(|a| a.held && a.solo);
        self.ending.iter().filter(move |_| !solo)
    }

    /// A playing or fading-out cue instance.
    pub fn find(&self, id: u64) -> Option<&ActiveCue> {
        self.active.iter().find(|a| a.id == id).or_else(|| self.ending.iter().map(|e| &e.cue).find(|a| a.id == id))
    }

    /// Where instance `id` is in its life cycle at `now_s`.
    pub fn phase(&self, id: u64, now_s: f64) -> Option<CuePhase> {
        if let Some(a) = self.active.iter().find(|a| a.id == id) {
            return Some(match a.fade {
                Some(f) if !f.done(now_s) => CuePhase::Starting(f.progress(now_s)),
                _ => CuePhase::Playing,
            });
        }
        let e = self.ending.iter().find(|e| e.cue.id == id)?;
        Some(CuePhase::Finishing(e.fade.progress(now_s)))
    }

    /// What the running transitions do to each drawn look at `now_s`
    /// (transition::apply). Empty when nothing is fading: every look is
    /// drawn as it is.
    pub fn transition_fx(&self, now_s: f64) -> Vec<(u64, LookFx)> {
        let mut fx = Vec::new();
        if self.ending.is_empty() && self.active.iter().all(|a| a.fade.is_none()) {
            return fx;
        }
        let mut drawn: Vec<u64> = self.visible().iter().map(|a| a.id).collect();
        drawn.extend(self.ending_visible().map(|e| e.cue.id));
        // A morph: the new cue draws both; its old cue is not drawn by itself.
        let morphing = |a: &ActiveCue| match a.fade {
            Some(f) if f.kind == TransitionKind::Morph && !f.done(now_s) => f.partner.filter(|from| drawn.contains(from)),
            _ => None,
        };
        for a in &self.active {
            let Some(f) = a.fade else { continue };
            let p = f.progress(now_s);
            if let Some(from) = morphing(a) {
                fx.push((a.id, LookFx::MorphFrom { from, p }));
            } else if p < 1.0 {
                fx.push((a.id, LookFx::Gain(Fade::gain_in(f.kind, p))));
            }
        }
        for e in &self.ending {
            let into = e.fade.partner.and_then(|id| self.active.iter().find(|a| a.id == id && drawn.contains(&a.id)));
            if into.is_some_and(|a| morphing(a) == Some(e.cue.id)) {
                fx.push((e.cue.id, LookFx::Absorbed));
            } else {
                fx.push((e.cue.id, LookFx::Gain(Fade::gain_out(e.fade.kind, e.fade.progress(now_s)))));
            }
        }
        fx
    }

    /// The cue on show at `now_s` (for an output that plays a cue by
    /// itself, like the ShowNET SD card over Art-Net): of the drawn cues,
    /// newest first, the first at half its level or more, else the one
    /// drawn most; none when nothing is drawn. Waiting launches are not
    /// drawn, so never on show.
    pub fn on_show(&self, now_s: f64) -> Option<&str> {
        let fx = self.transition_fx(now_s);
        let weight = |id: u64| match fx.iter().find(|(i, _)| *i == id).map(|(_, f)| *f) {
            None => 1.0,
            Some(LookFx::Gain(g)) => g,
            Some(LookFx::MorphFrom { p, .. }) => p,
            Some(LookFx::Absorbed) => fx
                .iter()
                .find_map(|(_, f)| match f {
                    LookFx::MorphFrom { from, p } if *from == id => Some(1.0 - p),
                    _ => None,
                })
                .unwrap_or(0.0),
        };
        let drawn: Vec<&ActiveCue> = self.ending_visible().map(|e| &e.cue).chain(self.visible()).collect();
        if let Some(a) = drawn.iter().rev().find(|a| weight(a.id) >= 0.5) {
            return Some(a.cue.as_str());
        }
        drawn.iter().rev().map(|a| (a, weight(a.id))).filter(|(_, w)| *w > 0.0).fold(None, |best: Option<(&&ActiveCue, f32)>, (a, w)| match best {
            Some((_, bw)) if bw >= w => best,
            _ => Some((a, w)),
        }).map(|(a, _)| a.cue.as_str())
    }

    /// Id the next started cue will get.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn primary(&self) -> Option<&ActiveCue> {
        self.active.last()
    }
}

/// The looks to render this frame, as (animator id, settings). `primary`
/// is `Shared::settings`: the newest cue's live look, or the manual look
/// (scene, playlist, look panel) when no cue plays and `look_on` is set.
/// Instance id 0 is the manual look. Cues fading out (T-158) come first.
pub fn looks(deck: &CueDeck, primary: &Settings, look_on: bool) -> Vec<(u64, Settings)> {
    // The operator's look brightness applies to every cue.
    let mut out: Vec<(u64, Settings)> =
        deck.ending_visible().map(|e| (e.cue.id, Settings { brightness: primary.brightness, ..e.cue.settings.clone() })).collect();
    let Some(top) = deck.primary() else {
        if look_on {
            out.push((0, primary.clone()));
        }
        return out;
    };
    out.extend(deck.visible().into_iter().map(|a| {
        if a.id == top.id {
            (a.id, primary.clone())
        } else {
            (a.id, Settings { brightness: primary.brightness, ..a.settings.clone() })
        }
    }));
    out
}

/// `looks` with the layer of each look (the manual look is on layer 1).
pub fn layered_looks(deck: &CueDeck, primary: &Settings, look_on: bool) -> Vec<(u8, u64, Settings)> {
    looks(deck, primary, look_on)
        .into_iter()
        .map(|(id, settings)| (deck.find(id).map_or(1, |a| a.layer), id, settings))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(multi: bool) -> CueDeck {
        CueDeck { multi, ..Default::default() }
    }

    fn press(d: &mut CueDeck, cue: &str) {
        d.press(cue, None, At::default(), Settings::default);
    }

    fn press_as(d: &mut CueDeck, cue: &str, mode: ClickMode) {
        d.press(cue, Some(mode), At::default(), Settings::default);
    }

    fn playing(d: &CueDeck) -> Vec<&str> {
        d.active.iter().map(|a| a.cue.as_str()).collect()
    }

    fn shown(d: &CueDeck) -> Vec<&str> {
        d.visible().iter().map(|a| a.cue.as_str()).collect()
    }

    #[test]
    fn part_modifiers_load_from_older_files_and_stay_in_range() {
        let slot: CueSlot = serde_json::from_str(r#"{"layer":2}"#).unwrap();
        assert!(slot.part_modifiers.is_empty());
        assert!(!serde_json::to_string(&slot).unwrap().contains("part_modifiers"));
        let mut d = CueDeck::default();
        let dim = crate::live::LiveModifiers { brightness: 0.5, freeze: true, ..Default::default() };
        let parts = BTreeMap::from([(0, dim.clone()), (2, dim.clone()), (9, dim.clone()), (3, crate::live::LiveModifiers::default())]);
        d.set_slot("figure:a", CueSlot { part_modifiers: parts, ..Default::default() });
        let kept = &d.slot("figure:a").part_modifiers;
        assert_eq!(kept.keys().copied().collect::<Vec<_>>(), [2], "parts 1..=8, neutral ones dropped");
        assert!(!kept[&2].freeze && kept[&2].brightness == 0.5, "stored without the momentary ones");
        let back: CueSlot = serde_json::from_str(&serde_json::to_string(&d.slot("figure:a")).unwrap()).unwrap();
        assert_eq!(back.part_modifiers, *kept);
    }

    #[test]
    fn toggle_starts_then_stops() {
        let mut d = deck(false);
        press(&mut d, "a");
        assert_eq!(playing(&d), ["a"]);
        d.release("a", At::default());
        assert_eq!(playing(&d), ["a"], "a release does not stop a latched cue");
        press(&mut d, "a");
        assert!(playing(&d).is_empty());
    }

    #[test]
    fn single_mode_replaces_and_multi_adds() {
        let mut d = deck(false);
        press(&mut d, "a");
        press(&mut d, "b");
        assert_eq!(playing(&d), ["b"]);
        let mut d = deck(true);
        press(&mut d, "a");
        press(&mut d, "b");
        assert_eq!(playing(&d), ["a", "b"]);
    }

    #[test]
    fn flash_plays_only_while_held_and_brings_back_what_played() {
        for multi in [false, true] {
            let mut d = deck(multi);
            press(&mut d, "a");
            let before: Vec<u64> = d.active.iter().map(|a| a.id).collect();
            press_as(&mut d, "f", ClickMode::Flash);
            assert!(shown(&d).contains(&"f"));
            // « Un cue »: the flash takes over; « Multi »: it adds up.
            assert_eq!(shown(&d).contains(&"a"), multi);
            d.release("f", At::default());
            assert_eq!(d.active.iter().map(|a| a.id).collect::<Vec<_>>(), before, "multi {multi}");
            assert_eq!(shown(&d), ["a"]);
        }
    }

    #[test]
    fn flash_mode_from_the_deck_and_from_a_slot() {
        let mut d = deck(true);
        d.click_mode = ClickMode::Flash;
        press(&mut d, "a");
        assert_eq!(playing(&d), ["a"]);
        d.release("a", At::default());
        assert!(playing(&d).is_empty());
        d.click_mode = ClickMode::Toggle;
        d.set_slot("b", CueSlot { mode: Some(ClickMode::Flash), group: None, layer: None, show: None, ..Default::default() });
        press(&mut d, "b");
        d.release("b", At::default());
        assert!(playing(&d).is_empty());
    }

    #[test]
    fn flashing_a_running_cue_changes_nothing() {
        let mut d = deck(true);
        press(&mut d, "a");
        press_as(&mut d, "a", ClickMode::Flash);
        assert_eq!(d.active.len(), 1);
        d.release("a", At::default());
        assert_eq!(playing(&d), ["a"]);
    }

    #[test]
    fn solo_hides_the_others_until_released() {
        let mut d = deck(true);
        press(&mut d, "a");
        press(&mut d, "b");
        press_as(&mut d, "s", ClickMode::Solo);
        assert_eq!(shown(&d), ["s"]);
        assert_eq!(playing(&d), ["a", "b", "s"]);
        d.release("s", At::default());
        assert_eq!(shown(&d), ["a", "b"]);
    }

    #[test]
    fn solo_of_a_running_cue_shows_only_it() {
        let mut d = deck(true);
        press(&mut d, "a");
        press(&mut d, "b");
        press_as(&mut d, "a", ClickMode::Solo);
        assert_eq!(shown(&d), ["a"]);
        d.release("a", At::default());
        assert_eq!(shown(&d), ["a", "b"]);
    }

    #[test]
    fn restart_gives_a_new_instance_each_press() {
        let mut d = deck(false);
        press_as(&mut d, "a", ClickMode::Restart);
        let first = d.active[0].id;
        press_as(&mut d, "a", ClickMode::Restart);
        assert_eq!(d.active.len(), 1);
        assert_ne!(d.active[0].id, first);
    }

    #[test]
    fn same_group_replaces_other_groups_stay() {
        let mut d = deck(true);
        d.set_slot("a", CueSlot { mode: None, group: Some(1), layer: None, show: None, ..Default::default() });
        d.set_slot("b", CueSlot { mode: None, group: Some(1), layer: None, show: None, ..Default::default() });
        d.set_slot("c", CueSlot { mode: None, group: Some(2), layer: None, show: None, ..Default::default() });
        press(&mut d, "a");
        press(&mut d, "c");
        press(&mut d, "free");
        press(&mut d, "b");
        assert_eq!(playing(&d), ["c", "free", "b"]);
    }

    #[test]
    fn a_flash_in_a_group_hides_its_group_mates_only_while_held() {
        let mut d = deck(true);
        d.set_slot("a", CueSlot { mode: None, group: Some(3), layer: None, show: None, ..Default::default() });
        d.set_slot("f", CueSlot { mode: Some(ClickMode::Flash), group: Some(3), layer: None, show: None, ..Default::default() });
        press(&mut d, "a");
        press(&mut d, "other");
        press(&mut d, "f");
        assert_eq!(shown(&d), ["other", "f"]);
        d.release("f", At::default());
        assert_eq!(shown(&d), ["a", "other"]);
    }

    #[test]
    fn limiter_stops_the_oldest_latched_cue() {
        let mut d = deck(true);
        for c in ["1", "2", "3", "4", "5"] {
            press(&mut d, c);
        }
        assert_eq!(playing(&d), ["2", "3", "4", "5"]);
        // Flashes don't count and are never the ones stopped.
        press_as(&mut d, "f", ClickMode::Flash);
        assert_eq!(playing(&d), ["2", "3", "4", "5", "f"]);
        press(&mut d, "6");
        assert_eq!(playing(&d), ["3", "4", "5", "f", "6"]);
        d.set_max_active(2);
        assert_eq!(playing(&d), ["5", "f", "6"]);
    }

    #[test]
    fn looks_use_the_live_settings_for_the_newest_cue() {
        let mut d = deck(true);
        d.press("a", None, At::default(), || Settings { scale: 0.1, brightness: 1.0, ..Default::default() });
        d.press("b", None, At::default(), || Settings { scale: 0.2, ..Default::default() });
        let live = Settings { scale: 0.9, brightness: 0.3, ..Default::default() };
        let l = looks(&d, &live, false);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].1.scale, l[0].1.brightness), (0.1, 0.3));
        assert_eq!(l[1].1.scale, 0.9);
        d.stop_all();
        assert!(looks(&d, &live, false).is_empty(), "nothing plays after the last cue stops");
        assert_eq!(looks(&d, &live, true), vec![(0, live.clone())]);
    }

    fn on_layer(d: &mut CueDeck, cue: &str, layer: u8) {
        d.set_slot(cue, CueSlot { layer: Some(layer), ..d.slot(cue) });
    }

    #[test]
    fn single_mode_replaces_only_within_a_layer() {
        let mut d = deck(false);
        on_layer(&mut d, "beam", 2);
        press(&mut d, "a");
        press(&mut d, "beam");
        assert_eq!(playing(&d), ["a", "beam"]);
        press(&mut d, "b");
        assert_eq!(playing(&d), ["beam", "b"], "b replaces a on layer 1, the beam stays");
        let l = layered_looks(&d, &Settings::default(), false);
        assert_eq!(l.iter().map(|(n, _, _)| *n).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(layered_looks(&deck(false), &Settings::default(), true)[0].0, 1, "the manual look is on layer 1");
    }

    #[test]
    fn clearing_a_layer_stops_its_cues_only() {
        let mut d = deck(true);
        on_layer(&mut d, "x", 3);
        on_layer(&mut d, "f", 3);
        press(&mut d, "a");
        press(&mut d, "x");
        press_as(&mut d, "f", ClickMode::Flash);
        d.clear_layer(3);
        assert_eq!(playing(&d), ["a"]);
        d.release("f", At::default());
        assert_eq!(playing(&d), ["a"]);
    }

    #[test]
    fn a_flash_on_another_layer_still_returns_to_what_played() {
        for multi in [false, true] {
            let mut d = deck(multi);
            on_layer(&mut d, "f", 4);
            press(&mut d, "a");
            press_as(&mut d, "f", ClickMode::Flash);
            assert_eq!(shown(&d).contains(&"a"), multi, "« Un cue »: the flash still takes over the output");
            d.release("f", At::default());
            assert_eq!(shown(&d), ["a"]);
        }
    }

    #[test]
    fn grid_json_round_trips_without_the_playing_list() {
        let mut d = deck(true);
        d.set_slot("a", CueSlot { mode: Some(ClickMode::Solo), group: Some(9), layer: None, show: None, ..Default::default() });
        press(&mut d, "a");
        let back: CueDeck = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(back.active.is_empty());
        assert!(back.multi);
        assert_eq!(back.slot("a"), CueSlot { mode: Some(ClickMode::Solo), group: None, layer: None, show: None, ..Default::default() }, "group 9 is out of range");
        on_layer(&mut d, "b", 9);
        on_layer(&mut d, "c", 1);
        assert!(!d.slots.contains_key("b") && !d.slots.contains_key("c"), "layer 9 is out of range, layer 1 is the default");
        on_layer(&mut d, "c", 4);
        let back: CueDeck = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back.slot("c").layer, Some(4));
        let old: CueDeck = serde_json::from_str("{}").unwrap();
        assert_eq!((old.click_mode, old.multi, old.max_active), (ClickMode::Toggle, false, 4));
    }

    #[test]
    fn projection_zones_of_a_cue_are_kept_sorted_and_in_range() {
        let mut d = deck(false);
        d.set_slot("a", CueSlot { zones: vec![3, 0, 1, 3, 9], ..Default::default() });
        assert_eq!(d.slot("a").zones, [1, 3]);
        d.set_slot("b", CueSlot { zones: vec![0, 9], ..Default::default() });
        assert!(!d.slots.contains_key("b"), "no valid zone left: the default slot");
        let json = serde_json::to_string(&d).unwrap();
        let back: CueDeck = serde_json::from_str(&json).unwrap();
        assert_eq!(back.slot("a").zones, [1, 3]);
        // A grid.json from before T-012: no zones, the default zones.
        let old: CueDeck = serde_json::from_str(r#"{"slots":{"a":{"layer":2}}}"#).unwrap();
        assert!(old.slot("a").zones.is_empty());
    }

    #[test]
    fn a_cue_program_is_saved_with_the_grid_and_plays_with_the_cue() {
        let mut d = deck(false);
        let program = CueProgram { layers: vec![crate::engine::Content::Wave], ..Default::default() };
        d.set_slot("a", CueSlot { program: Some(program.clone()), ..Default::default() });
        let back: CueDeck = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back.program("a"), Some(&program));
        assert_eq!(back.with_program("a", Settings::default()).program, Some(program));
        assert_eq!(back.with_program("b", Settings::default()).program, None);
        // A grid.json from before T-157 has none; one that isn't valid any
        // more is dropped.
        let old: CueDeck = serde_json::from_str(r#"{"slots":{"a":{"layer":2}}}"#).unwrap();
        assert_eq!(old.program("a"), None);
        let bad = CueProgram { lanes: vec![crate::program::Lane { target: "nope".into(), keys: Vec::new() }], ..Default::default() };
        d.set_slot("c", CueSlot { program: Some(bad), ..Default::default() });
        assert!(!d.slots.contains_key("c"));
    }

    // ---------- transitions (T-158) ----------

    fn at(s: f64, beat: f64) -> At {
        At { s, beat, bpm: 120.0, beats_per_bar: 4, rate: 1.0 }
    }

    fn with(kind: TransitionKind, seconds: f32) -> CueDeck {
        CueDeck { transition: Transition { kind, length: crate::transition::TransitionLength::Seconds(seconds), in_flash: false }, ..deck(false) }
    }

    fn fx_of(d: &CueDeck, now: f64, cue: &str) -> Option<LookFx> {
        let id = d.active.iter().find(|a| a.cue == cue).map(|a| a.id).or_else(|| d.ending.iter().find(|e| e.cue.cue == cue).map(|e| e.cue.id))?;
        d.transition_fx(now).into_iter().find(|(i, _)| *i == id).map(|(_, f)| f)
    }

    #[test]
    fn a_cut_leaves_nothing_in_between() {
        let mut d = deck(false);
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("b", None, at(1.0, 2.0), Settings::default);
        assert!(d.ending.is_empty() && d.active.iter().all(|a| a.fade.is_none()));
        assert!(d.transition_fx(1.0).is_empty(), "no transition: every look drawn as it is");
        assert_eq!(looks(&d, &Settings::default(), false).len(), 1);
    }

    #[test]
    fn a_fade_draws_both_cues_at_half_way() {
        let mut d = with(TransitionKind::Fade, 1.0);
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.tick(at(5.0, 10.0));
        d.press("b", None, at(10.0, 20.0), Settings::default);
        assert_eq!(playing(&d), ["b"]);
        assert_eq!(d.ending.len(), 1, "a is still drawn while it fades out");
        assert_eq!(fx_of(&d, 10.5, "a"), Some(LookFx::Gain(0.5)));
        assert_eq!(fx_of(&d, 10.5, "b"), Some(LookFx::Gain(0.5)));
        assert_eq!(looks(&d, &Settings::default(), false).len(), 2);
        let a = d.ending[0].cue.id;
        assert_eq!(d.phase(a, 10.5), Some(CuePhase::Finishing(0.5)));
        assert_eq!(d.phase(d.active[0].id, 10.5), Some(CuePhase::Starting(0.5)));
        assert_eq!(layered_looks(&d, &Settings::default(), false)[0].1, a, "the fading cue keeps its layer");
        d.tick(at(11.0, 22.0));
        assert!(d.ending.is_empty() && d.active[0].fade.is_none());
        assert_eq!(d.phase(d.active[0].id, 11.0), Some(CuePhase::Playing));
        assert!(d.transition_fx(11.0).is_empty());
    }

    #[test]
    fn fade_through_black_never_draws_both() {
        let mut d = with(TransitionKind::FadeThroughBlack, 1.0);
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("b", None, at(10.0, 20.0), Settings::default);
        assert_eq!(fx_of(&d, 10.25, "a"), Some(LookFx::Gain(0.5)));
        assert_eq!(fx_of(&d, 10.25, "b"), Some(LookFx::Gain(0.0)));
        assert_eq!(fx_of(&d, 10.75, "a"), Some(LookFx::Gain(0.0)));
        assert_eq!(fx_of(&d, 10.75, "b"), Some(LookFx::Gain(0.5)));
    }

    #[test]
    fn a_morph_pairs_the_replaced_cue_with_the_new_one() {
        let mut d = with(TransitionKind::Morph, 2.0);
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("b", None, at(10.0, 20.0), Settings::default);
        let a = d.ending[0].cue.id;
        assert_eq!(fx_of(&d, 10.5, "a"), Some(LookFx::Absorbed));
        assert_eq!(fx_of(&d, 10.5, "b"), Some(LookFx::MorphFrom { from: a, p: 0.25 }));
        // Toggled off with nothing new: a plain fade out.
        d.press("b", None, at(20.0, 40.0), Settings::default);
        d.tick(at(20.0, 40.0));
        assert_eq!(fx_of(&d, 21.0, "b"), Some(LookFx::Gain(0.5)));
        // In « Multi » a new cue replaces nothing: it fades in.
        let mut d = CueDeck { multi: true, ..with(TransitionKind::Morph, 2.0) };
        d.press("a", None, at(0.0, 0.0), Settings::default);
        assert_eq!(fx_of(&d, 1.0, "a"), Some(LookFx::Gain(0.5)));
    }

    #[test]
    fn a_cue_can_override_the_grid_transition() {
        let mut d = deck(false);
        let fade = Transition { kind: TransitionKind::Fade, length: crate::transition::TransitionLength::Beats(2.0), in_flash: false };
        d.set_slot("b", CueSlot { transition: Some(fade), ..Default::default() });
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("b", None, at(10.0, 20.0), Settings::default);
        // 2 beats at 120 BPM = 1 s.
        assert_eq!(fx_of(&d, 10.5, "b"), Some(LookFx::Gain(0.5)));
        d.tick(at(19.0, 38.0));
        d.press("c", None, at(20.0, 40.0), Settings::default);
        assert!(d.ending.is_empty(), "c cuts: b goes at once");
        assert!(d.transition_fx(20.0).is_empty());
    }

    #[test]
    fn flashes_react_at_once_unless_asked() {
        let mut d = with(TransitionKind::Fade, 1.0);
        d.press("f", Some(ClickMode::Flash), at(0.0, 0.0), Settings::default);
        assert!(d.active[0].fade.is_none());
        d.release("f", at(1.0, 2.0));
        assert!(d.ending.is_empty() && d.active.is_empty());
        d.transition.in_flash = true;
        d.press("f", Some(ClickMode::Flash), at(2.0, 4.0), Settings::default);
        assert!(d.active[0].fade.is_some());
        d.release("f", at(3.0, 6.0));
        assert_eq!(d.ending.len(), 1);
    }

    #[test]
    fn stop_all_clear_layer_and_the_estop_cut_at_once() {
        let mut d = CueDeck { quantize: Quantize::Beat, ..with(TransitionKind::Fade, 4.0) };
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("b", None, at(1.0, 2.0), Settings::default);
        d.press("c", None, at(1.2, 2.4), Settings::default);
        assert_eq!((d.ending.len(), d.pending.len()), (1, 1));
        d.cut_transitions();
        assert!(d.ending.is_empty() && d.pending.is_empty() && d.active.iter().all(|a| a.fade.is_none()));
        assert!(d.transition_fx(1.5).is_empty());
        d.press("c", None, at(2.0, 4.0), Settings::default);
        d.press("d", None, at(2.2, 4.4), Settings::default);
        d.stop_all();
        assert!(d.active.is_empty() && d.ending.is_empty() && d.pending.is_empty());
        d.press("a", None, at(3.0, 6.0), Settings::default);
        d.press("b", None, at(3.2, 6.4), Settings::default);
        d.clear_layer(1);
        assert!(d.active.is_empty() && d.ending.is_empty() && d.pending.is_empty());
    }

    #[test]
    fn the_cue_on_show_follows_transitions_and_ignores_waiting_launches() {
        let mut d = with(TransitionKind::FadeThroughBlack, 1.0);
        assert_eq!(d.on_show(0.0), None);
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.tick(at(5.0, 10.0));
        assert_eq!(d.on_show(5.0), Some("a"));
        d.press("b", None, at(10.0, 20.0), Settings::default);
        assert_eq!(d.on_show(10.1), Some("a"), "the old cue still shows, going down");
        assert_eq!(d.on_show(10.5), None, "black in between");
        assert_eq!(d.on_show(10.9), Some("b"));
        let mut d = with(TransitionKind::Morph, 1.0);
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.tick(at(5.0, 10.0));
        d.press("b", None, at(10.0, 20.0), Settings::default);
        assert_eq!(d.on_show(10.2), Some("a"));
        assert_eq!(d.on_show(10.7), Some("b"));
        // Waiting for its bar: still the old one.
        let mut d = CueDeck { quantize: Quantize::Bar, ..deck(false) };
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("b", None, at(5.2, 10.4), Settings::default);
        assert_eq!(d.primary().map(|a| a.cue.as_str()), Some("a"));
        assert_eq!(d.on_show(5.2), Some("a"));
        // Beat mode: the one shown, not the newest.
        let mut d = CueDeck { beat_every: 1, ..deck(true) };
        press(&mut d, "a");
        press(&mut d, "b");
        d.tick(at(0.0, 0.5));
        assert_eq!(d.on_show(0.0), Some("a"));
        d.tick(at(0.5, 1.5));
        assert_eq!(d.on_show(0.5), Some("b"));
    }

    #[test]
    fn a_cue_stopped_while_fading_in_fades_out_from_where_it_is() {
        let gain = |d: &CueDeck, now: f64, cue: &str| match fx_of(d, now, cue) {
            Some(LookFx::Gain(g)) => g,
            Some(LookFx::MorphFrom { p, .. }) => p,
            Some(LookFx::Absorbed) => 0.0,
            None => 1.0,
        };
        for (kind_in, kind_out) in [
            (TransitionKind::Fade, TransitionKind::Fade),
            (TransitionKind::Fade, TransitionKind::FadeThroughBlack),
            (TransitionKind::FadeThroughBlack, TransitionKind::Fade),
            (TransitionKind::Morph, TransitionKind::Morph),
            (TransitionKind::FadeThroughBlack, TransitionKind::FadeThroughBlack),
        ] {
            for stop_at in [10.1, 10.3, 10.6, 10.9] {
                let mut d = with(kind_in, 1.0);
                d.press("a", None, at(0.0, 0.0), Settings::default);
                d.tick(at(5.0, 10.0));
                d.press("b", None, at(10.0, 20.0), Settings::default);
                let before = gain(&d, stop_at, "b");
                assert!(before < 1.0, "{kind_in:?}: b is still coming in");
                // b replaced by c (with kind_out) while still fading in.
                d.transition.kind = kind_out;
                d.press("c", None, at(stop_at, stop_at * 2.0), Settings::default);
                let ending = d.ending.iter().find(|e| e.cue.cue == "b").expect("b fades out");
                assert_eq!(ending.fade.partner, None, "never a morph's source while not fully in");
                let after = gain(&d, stop_at, "b");
                assert!(after <= before + 1e-5, "{kind_in:?} → {kind_out:?} at {stop_at}: {before} jumped to {after}");
                // And only goes down from there.
                let mut last = after;
                for k in 1..=20 {
                    let g = gain(&d, stop_at + k as f64 * 0.05, "b");
                    assert!(g <= last + 1e-5, "{kind_in:?} → {kind_out:?}: went up {last} → {g}");
                    last = g;
                }
                assert_eq!(last, 0.0);
            }
        }
        // Toggled off while fading in: the same.
        let mut d = CueDeck { multi: true, ..with(TransitionKind::Fade, 1.0) };
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("a", None, at(0.25, 0.5), Settings::default);
        assert_eq!(fx_of(&d, 0.25, "a"), Some(LookFx::Gain(0.25)), "a quarter in: fades out from a quarter, not from full");
    }

    // ---------- quantised launch (T-159) ----------

    #[test]
    fn quantised_launch_waits_for_the_boundary() {
        let mut d = CueDeck { quantize: Quantize::Beat, ..deck(false) };
        d.press("a", None, at(5.2, 10.4), Settings::default);
        assert!(d.active.is_empty());
        assert_eq!(d.pending[0].at_beat, 11.0);
        assert!(!d.fire_due(at(5.45, 10.9)));
        assert!(d.fire_due(at(5.51, 11.02)));
        assert_eq!(d.active[0].started_beat, 11.0, "its time counts from the boundary");
        assert!((d.active[0].started_s - 5.5).abs() < 1e-9);
        assert!(d.pending.is_empty());
    }

    #[test]
    fn a_press_just_after_the_beat_starts_at_once_from_it() {
        let mut d = CueDeck { quantize: Quantize::Beat, ..deck(false) };
        d.press("a", None, at(5.525, 11.05), Settings::default);
        assert!(d.pending.is_empty());
        assert_eq!(d.active[0].started_beat, 11.0);
    }

    #[test]
    fn bar_quantisation_and_off() {
        let mut d = CueDeck { quantize: Quantize::Bar, ..deck(false) };
        d.press("a", None, at(5.2, 10.4), Settings::default);
        assert_eq!(d.pending[0].at_beat, 12.0);
        let mut d = CueDeck { quantize: Quantize::Bar, ..deck(false) };
        d.press("a", None, At { beats_per_bar: 3, ..at(5.2, 10.4) }, Settings::default);
        assert_eq!(d.pending[0].at_beat, 12.0, "3/4: next multiple of 3");
        let mut d = deck(false);
        d.press("a", None, at(5.2, 10.4), Settings::default);
        assert_eq!(d.active[0].started_beat, 10.4, "Off: at once");
    }

    #[test]
    fn a_second_press_takes_a_waiting_cue_back_and_flashes_never_wait() {
        let mut d = CueDeck { quantize: Quantize::Beat, ..deck(true) };
        d.press("a", None, at(5.2, 10.4), Settings::default);
        d.press("a", None, at(5.3, 10.6), Settings::default);
        assert!(d.pending.is_empty() && d.active.is_empty());
        d.press("f", Some(ClickMode::Flash), at(5.3, 10.6), Settings::default);
        assert_eq!(playing(&d), ["f"]);
        // A restart waits too, and its old instance plays until then.
        d.press("r", None, at(6.0, 12.0), Settings::default);
        let first = d.active.last().unwrap().id;
        d.press("r", Some(ClickMode::Restart), at(6.2, 12.4), Settings::default);
        assert_eq!(d.active.last().unwrap().id, first);
        d.fire_due(at(6.5, 13.0));
        assert_ne!(d.active.last().unwrap().id, first);
        assert_eq!(d.active.iter().filter(|a| a.cue == "r").count(), 1);
    }

    #[test]
    fn a_quantised_cue_replaces_on_its_boundary_with_its_transition() {
        let mut d = CueDeck { quantize: Quantize::Beat, ..with(TransitionKind::Fade, 1.0) };
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("b", None, at(5.2, 10.4), Settings::default);
        assert_eq!(playing(&d), ["a"], "a plays until b's beat");
        d.fire_due(at(5.5, 11.0));
        assert_eq!(playing(&d), ["b"]);
        assert_eq!(fx_of(&d, 6.0, "a"), Some(LookFx::Gain(0.5)));
    }

    // ---------- beat mode (T-159) ----------

    #[test]
    fn beat_mode_shows_one_latched_cue_per_layer_in_turn() {
        let mut d = CueDeck { beat_every: 2, ..deck(true) };
        on_layer(&mut d, "beam", 2);
        for c in ["a", "b", "c", "beam"] {
            press(&mut d, c);
        }
        let at_beat = |d: &mut CueDeck, beat: f64| {
            d.tick(at(beat / 2.0, beat));
            shown(d).into_iter().map(String::from).collect::<Vec<_>>()
        };
        assert_eq!(at_beat(&mut d, 0.5), ["a", "beam"]);
        assert_eq!(at_beat(&mut d, 2.0), ["b", "beam"]);
        assert_eq!(at_beat(&mut d, 5.9), ["c", "beam"]);
        assert_eq!(at_beat(&mut d, 6.0), ["a", "beam"]);
        d.beat_every = 0;
        assert_eq!(shown(&d), ["a", "b", "c", "beam"]);
    }

    #[test]
    fn transition_quantize_and_beat_mode_are_saved_with_the_grid() {
        let mut d = CueDeck { quantize: Quantize::Bar, beat_every: 4, ..with(TransitionKind::Morph, 2.0) };
        d.set_slot("a", CueSlot { transition: Some(Transition { kind: TransitionKind::Fade, ..Default::default() }), ..Default::default() });
        let back: CueDeck = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!((back.transition, back.quantize, back.beat_every), (d.transition, Quantize::Bar, 4));
        assert_eq!(back.slot("a").transition.map(|t| t.kind), Some(TransitionKind::Fade));
        let mut other = deck(false);
        other.set_config(&back);
        assert_eq!((other.transition, other.quantize, other.beat_every), (d.transition, Quantize::Bar, 4));
        // A grid.json from before T-158: cut, no quantisation, no beat mode.
        let old: CueDeck = serde_json::from_str(r#"{"multi":true,"slots":{"a":{"layer":2}}}"#).unwrap();
        assert_eq!((old.transition, old.quantize, old.beat_every), (Transition::default(), Quantize::Off, 0));
        assert_eq!(old.slot("a").transition, None);
        let mut bad: CueDeck = serde_json::from_str(r#"{"beat_every":3,"transition":{"kind":"fade","length":{"beats":-2}}}"#).unwrap();
        bad.sanitize();
        assert_eq!(bad.beat_every, 0);
        assert_eq!(bad.transition.length, crate::transition::TransitionLength::Beats(0.0));
        assert!(bad.transition.is_cut(), "a zero length is a cut");
    }

    #[test]
    fn a_damaged_grid_is_quarantined_and_never_overwritten() {
        let (dir, path) = crate::datafile::tests::with_damaged("grid", "grid.json");
        let deck = CueDeck::load(path.clone());
        assert_eq!(deck.max_active, CueDeck::default().max_active);
        deck.save();
        assert!(path.exists());
        crate::datafile::tests::check_kept(&dir, "grid.json", false);
    }

    /// T-423: « Tout arrêter » leaves protected cues; `stop_all` (a scene
    /// shown by hand) stops everything.
    #[test]
    fn stop_unprotected_keeps_protected_cues() {
        let mut d = deck(true);
        d.set_slot("keep", CueSlot { protect_stop: true, ..Default::default() });
        for cue in ["a", "keep", "b"] {
            press(&mut d, cue);
        }
        d.stop_unprotected();
        assert_eq!(playing(&d), ["keep"]);
        d.stop_unprotected();
        assert_eq!(playing(&d), ["keep"]);
        d.stop_all();
        assert!(playing(&d).is_empty());
        // Without any protection, everything stops.
        let mut d = deck(true);
        press(&mut d, "a");
        press(&mut d, "b");
        d.stop_unprotected();
        assert!(playing(&d).is_empty());
    }

    /// With transitions (T-158/T-159): unprotected cues lose their fade-out
    /// and their waiting launch too; protected ones keep theirs. The e-stop
    /// path (`cut_transitions`) still drops everything waiting or fading.
    #[test]
    fn stop_unprotected_drops_fades_and_waits_of_unprotected_cues_only() {
        let mut d = CueDeck { quantize: Quantize::Beat, ..with(TransitionKind::Fade, 4.0) };
        d.set_slot("p", CueSlot { protect_stop: true, ..Default::default() });
        d.set_slot("q", CueSlot { protect_stop: true, layer: Some(2), ..Default::default() });
        d.set_slot("w", CueSlot { layer: Some(2), ..Default::default() });
        d.press("a", None, at(0.0, 0.0), Settings::default);
        d.press("p", None, at(1.0, 2.0), Settings::default); // a fades out under p
        d.press("x", None, at(1.2, 2.4), Settings::default); // waits for beat 3
        d.press("q", None, at(1.2, 2.4), Settings::default);
        d.press("w", None, at(1.3, 2.6), Settings::default);
        assert_eq!(d.ending.iter().map(|e| e.cue.cue.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(d.pending.iter().map(|p| p.cue.as_str()).collect::<Vec<_>>(), ["x", "q", "w"]);
        d.stop_unprotected();
        assert_eq!(playing(&d), ["p"]);
        assert!(d.ending.is_empty(), "a's fade-out goes with it");
        assert_eq!(d.pending.iter().map(|p| p.cue.as_str()).collect::<Vec<_>>(), ["q"], "only the protected launch still waits");
        // p fading out (replaced by another cue) keeps its fade-out.
        d.press("p2", None, at(2.0, 4.0), Settings::default);
        d.fire_due(at(2.0, 4.0));
        assert!(d.ending.iter().any(|e| e.cue.cue == "p"));
        d.stop_unprotected();
        assert!(d.ending.iter().any(|e| e.cue.cue == "p"));
        d.cut_transitions();
        assert!(d.ending.is_empty() && d.pending.is_empty());
    }

    #[test]
    fn a_solo_proof_cue_stays_shown_under_a_held_solo() {
        let mut d = deck(true);
        d.set_slot("bg", CueSlot { protect_solo: true, ..Default::default() });
        press(&mut d, "a");
        press(&mut d, "bg");
        press_as(&mut d, "s", ClickMode::Solo);
        assert_eq!(shown(&d), ["bg", "s"]);
        d.release("s", At::default());
        assert_eq!(shown(&d), ["a", "bg"]);
    }

    #[test]
    fn a_grid_without_the_protection_flags_loads_with_both_off() {
        let old: CueDeck = serde_json::from_str(r#"{"slots":{"a":{"layer":2}}}"#).unwrap();
        let slot = old.slot("a");
        assert!(!slot.protect_stop && !slot.protect_solo);
        // Off flags aren't written; on ones come back.
        assert!(!serde_json::to_string(&old).unwrap().contains("protect"));
        let mut d = deck(false);
        d.set_slot("a", CueSlot { protect_stop: true, protect_solo: true, ..Default::default() });
        let back: CueDeck = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(back.slot("a").protect_stop && back.slot("a").protect_solo);
    }

    #[test]
    fn a_grid_without_notes_or_tags_loads_with_them_empty() {
        let old: CueDeck = serde_json::from_str(r#"{"slots":{"a":{"layer":2}}}"#).unwrap();
        let slot = old.slot("a");
        assert_eq!((slot.notes.as_str(), slot.tags.len(), slot.color), ("", 0, None));
        let text = serde_json::to_string(&old).unwrap();
        assert!(!text.contains("notes") && !text.contains("tags") && !text.contains("color"));
    }

    #[test]
    fn tags_are_cut_deduplicated_and_capped() {
        let mut d = deck(false);
        let long = "x".repeat(100);
        let mut tags: Vec<String> = ["Intro", "intro", " ", "ÍNTRO", &long, "drop"].iter().map(|s| s.to_string()).collect();
        tags.extend((1..=10).map(|n| format!("t{n}")));
        d.set_slot("a", CueSlot { tags, notes: "n".repeat(900), color: Some([1, 2, 3]), ..Default::default() });
        let slot = d.slot("a");
        assert_eq!(slot.tags.len(), MAX_TAGS, "a 9th tag is dropped");
        assert_eq!(slot.tags[0], "Intro", "kept as first typed");
        assert_eq!(slot.tags[1].chars().count(), MAX_TAG_LEN);
        assert_eq!(slot.tags[2], "drop");
        assert_eq!(slot.notes.chars().count(), MAX_NOTES_LEN);
        assert_eq!(slot.color, Some([1, 2, 3]));
        // Only blanks = nothing left: the slot goes away.
        d.set_slot("a", CueSlot { tags: vec!["  ".into()], notes: " \n".into(), ..Default::default() });
        assert!(d.slots.is_empty());
    }

    #[test]
    fn tag_keys_ignore_case_and_accents() {
        assert_eq!(tag_key(" Été "), "ete");
        assert_eq!(tag_key("CALME"), tag_key("calme"));
        let mut d = deck(false);
        d.set_slot("a", CueSlot { tags: vec!["Intro".into(), "drop".into()], ..Default::default() });
        d.set_slot("b", CueSlot { tags: vec!["intro".into()], ..Default::default() });
        assert_eq!(d.tags(), vec![("drop".to_string(), 1), ("Intro".to_string(), 2)]);
    }
    /// T-419: the *Cadence* divides a live transition (2 s at 200 % = 1 s,
    /// 4 s at 50 %); a slot saved before T-419 has no speed group.
    #[test]
    fn the_rate_master_scales_live_transitions_and_old_slots_have_no_group() {
        use crate::transition::TransitionLength;
        let t = Transition { kind: TransitionKind::Fade, length: TransitionLength::Seconds(2.0), in_flash: false };
        for (rate, len) in [(0.0, 2.0), (1.0, 2.0), (2.0, 1.0), (0.5, 4.0), (9.0, 1.0), (f64::NAN, 2.0)] {
            let mut d = CueDeck { transition: t, ..deck(false) };
            d.press("a", None, At { rate, ..At::default() }, Settings::default);
            assert_eq!(d.active[0].fade.map(|f| f.length_s), Some(len), "rate {rate}");
        }
        let slot: CueSlot = serde_json::from_str(r#"{"layer":2}"#).unwrap();
        assert_eq!(slot.speed_group, None);
        let mut d = deck(false);
        d.set_slot("a", CueSlot { speed_group: Some(9), ..Default::default() });
        assert_eq!(d.slot("a").speed_group, None, "an unknown group is dropped");
        d.set_slot("a", CueSlot { speed_group: Some(2), ..Default::default() });
        assert_eq!(serde_json::from_str::<CueSlot>(&serde_json::to_string(&d.slot("a")).unwrap()).unwrap().speed_group, Some(2));
    }
}
