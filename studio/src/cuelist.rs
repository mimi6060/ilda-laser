//! Cue lists with « Go » (T-410): an ordered list of steps, played one
//! after the other like a lighting console's sequence. Each step is a
//! complete state (a cue, a scene, or *Noir*): there is no tracking. A
//! step comes in with its own fade, after an optional delay, and starts on
//! *Go* or by itself (*Après n s*, *À la suite*, *Sur le temps*, *Sur
//! événement audio*).
//!
//! The player (`ListPlayer`) has its own clock, advanced by the engine
//! frame and frozen by *Pause*, so a paused fade or delay picks up exactly
//! where it stopped. It renders through the normal path: its looks join
//! the cue looks of `Engine::tick` on the list's layer, its fades are
//! `LookFx` like the grid's transitions (T-158), and the mixer, the live
//! stage, the zones, the calibration and the whole safety stage come
//! after. Nothing here arms: the e-stop pauses the list (it never starts
//! again by itself), and a step can't fire `transport.arm` or any control
//! a controller may not drive.
//!
//! *Règle du noir*: while the look before a step is dark (nothing, *Noir*
//! or a zero brightness), the step never morphs from it: it fades in on
//! its final geometry, so nothing is seen sliding into place.

use crate::controls::{ControlRegistry, ControlSource};
use crate::engine::{AudioFeatures, Settings, AUDIO_EVENTS};
use crate::layers::LAYER_COUNT;
use crate::transition::{Fade, LookFx, TransitionKind};
use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Animator ids of list steps: far above the deck's and the timeline's.
pub const ID_BASE: u64 = 1 << 56;
pub const MAX_STEPS: usize = 999;
pub const MAX_LISTS: usize = 100;
/// Longest delay, fade or *Après n s* wait, in seconds.
pub const MAX_SECONDS: f32 = 3600.0;
pub const MAX_BEATS: u32 = 256;
/// Trigger labels, in `StepTrigger` order (for the UI and docs).
pub const TRIGGER_LABELS: [&str; 5] = ["Go", "Après n s", "À la suite", "Sur le temps (tous les n temps)", "Sur événement audio"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CueList {
    pub name: String,
    /// Layer 1..=4 the list plays on.
    pub layer: u8,
    /// After the last step, *Go* goes back to the first.
    pub wrap: bool,
    pub steps: Vec<Step>,
}

impl Default for CueList {
    fn default() -> Self {
        Self { name: String::new(), layer: 1, wrap: false, steps: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    /// Unique within the list (the UI's row key).
    pub id: u64,
    pub label: String,
    pub source: StepSource,
    pub trigger: StepTrigger,
    /// Seconds between the trigger and the step's start.
    pub delay_s: f32,
    pub fade_in_s: f32,
    /// How long this step takes to go when the next one comes.
    pub fade_out_s: f32,
    /// How the geometry goes from the step before (T-158 kinds).
    pub transition: TransitionKind,
    /// Separate times for geometry, colour and brightness; `None` = the
    /// fade-in for all three.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split: Option<SplitTiming>,
    /// Keep the colour of the step before (« Garder la couleur »).
    pub keep_color: bool,
    /// A control of the registry fired when the step starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fire: Option<StepAction>,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            id: 0,
            label: String::new(),
            source: StepSource::Black,
            trigger: StepTrigger::Go,
            delay_s: 0.0,
            fade_in_s: 0.0,
            fade_out_s: 0.0,
            transition: TransitionKind::Fade,
            split: None,
            keep_color: false,
            fire: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepSource {
    #[default]
    Black,
    /// A cue of the grid (built-in or user cue), by preset id.
    Cue { id: String },
    /// A saved scene, by name.
    Scene { name: String },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepTrigger {
    /// Waits for *Go*.
    #[default]
    Go,
    /// `s` seconds after the step before started.
    After { s: f32 },
    /// As soon as the step before has finished its fade.
    Follow,
    /// On the next multiple of `n` beats of the tempo clock.
    Beats { n: u32 },
    /// On the next audio event (`engine::AUDIO_EVENTS`: kick, snare, drop…).
    Audio { event: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitTiming {
    pub geometry_s: f32,
    pub color_s: f32,
    pub brightness_s: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepAction {
    pub control: String,
    #[serde(default = "one")]
    pub value: f32,
}

fn one() -> f32 {
    1.0
}

fn secs(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, MAX_SECONDS) } else { 0.0 }
}

impl CueList {
    /// Brings every number into range and gives each step a unique id.
    /// The name and the controls are checked by `validate`.
    pub fn sanitize(&mut self) {
        self.name = self.name.trim().to_string();
        self.layer = self.layer.clamp(1, LAYER_COUNT as u8);
        self.steps.truncate(MAX_STEPS);
        let mut next = self.steps.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        let mut seen = std::collections::HashSet::new();
        for st in &mut self.steps {
            if st.id == 0 || !seen.insert(st.id) {
                st.id = next;
                seen.insert(next);
                next += 1;
            }
            st.label = st.label.trim().chars().take(80).collect();
            st.delay_s = secs(st.delay_s);
            st.fade_in_s = secs(st.fade_in_s);
            st.fade_out_s = secs(st.fade_out_s);
            st.split = st.split.map(|sp| SplitTiming { geometry_s: secs(sp.geometry_s), color_s: secs(sp.color_s), brightness_s: secs(sp.brightness_s) });
            st.trigger = match std::mem::take(&mut st.trigger) {
                StepTrigger::After { s } => StepTrigger::After { s: secs(s) },
                StepTrigger::Beats { n } => StepTrigger::Beats { n: n.clamp(1, MAX_BEATS) },
                t => t,
            };
            if let Some(f) = &mut st.fire {
                f.value = if f.value.is_finite() { f.value } else { 1.0 };
            }
        }
    }

    /// Checks what `sanitize` can't fix: the name, the audio events, and
    /// the controls the steps fire (they must exist, be allowed from a
    /// controller, and never arm or drive a list). French errors.
    pub fn validate(&self, reg: &ControlRegistry) -> Result<()> {
        if !crate::timeline::valid_show_name(&self.name) {
            bail!("nom de liste invalide : « {} » (lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus)", self.name);
        }
        for (i, st) in self.steps.iter().enumerate() {
            if let StepTrigger::Audio { event } = &st.trigger {
                if !AUDIO_EVENTS.contains(&event.as_str()) {
                    bail!("pas {} : événement audio inconnu « {event} »", i + 1);
                }
            }
            if let Some(f) = &st.fire {
                check_action(f, reg).with_context(|| format!("pas {}", i + 1))?;
            }
        }
        Ok(())
    }
}

/// A step may fire only a control a controller may drive: never
/// `transport.arm` (nor any non-`external` control), and never a list
/// control (a step that presses *Go* would run away).
pub fn check_action(action: &StepAction, reg: &ControlRegistry) -> Result<()> {
    let Some(desc) = reg.get(&action.control) else { bail!("contrôle inconnu : « {} »", action.control) };
    if desc.id == "transport.arm" || !desc.external {
        bail!("le contrôle « {} » ne peut pas être déclenché par un pas", desc.id);
    }
    if desc.id.starts_with("list.") {
        bail!("un pas ne peut pas piloter une liste (« {} »)", desc.id);
    }
    Ok(())
}

/// The saved lists, `<data-dir>/cuelists.json`.
pub struct CueListStore {
    path: Option<PathBuf>,
    lists: Vec<CueList>,
}

impl CueListStore {
    pub fn load_or_create(path: PathBuf) -> Self {
        let mut lists: Vec<CueList> = crate::load_json(&path);
        lists.iter_mut().for_each(CueList::sanitize);
        Self { path: Some(path), lists }
    }

    pub fn in_memory() -> Self {
        Self { path: None, lists: Vec::new() }
    }

    pub fn list(&self) -> &[CueList] {
        &self.lists
    }

    pub fn get(&self, name: &str) -> Option<&CueList> {
        self.lists.iter().find(|l| l.name == name)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Memory only; the caller writes the file (project open, T-286).
    pub fn replace_in_memory(&mut self, lists: Vec<CueList>) {
        self.lists = lists;
    }

    /// Replaces every list and saves (undo / redo, T-285): the lists were
    /// checked when they were first saved.
    pub fn set_all(&mut self, lists: Vec<CueList>) -> Result<()> {
        self.lists = lists;
        self.save()
    }

    /// Replaces the list with the same name, else appends; then saves.
    pub fn upsert(&mut self, mut list: CueList, reg: &ControlRegistry) -> Result<()> {
        list.sanitize();
        list.validate(reg)?;
        let full = self.lists.len() >= MAX_LISTS;
        match self.lists.iter_mut().find(|l| l.name == list.name) {
            Some(l) => *l = list,
            None if full => bail!("trop de listes de cues ({MAX_LISTS} au plus)"),
            None => self.lists.push(list),
        }
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<bool> {
        let before = self.lists.len();
        self.lists.retain(|l| l.name != name);
        if self.lists.len() == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let json = serde_json::to_vec_pretty(&self.lists).context("échec de la mise en forme des listes")?;
        crate::datafile::replace(path, &json).with_context(|| format!("impossible d'écrire {}", path.display()))
    }
}

// ---------- the player ----------

/// Where the list is, for the API and the UI.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// Before the first step (*Début*), or on a step that is fully in.
    Idle,
    /// A step is waiting for its delay.
    Delay { left_s: f32 },
    /// A step is fading in (and the one before out); `t` is 0..1.
    Fading { t: f32 },
    Holding,
}

/// The fade times a step comes in with.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Timing {
    kind: TransitionKind,
    geometry: f64,
    color: f64,
    brightness: f64,
    out: f64,
}

impl Timing {
    fn of(st: &Step) -> Self {
        let (g, c, b) = match st.split {
            // *Coupe*: no time at all.
            _ if st.transition == TransitionKind::Cut => (0.0, 0.0, 0.0),
            Some(sp) => (sp.geometry_s, sp.color_s, sp.brightness_s),
            None => (st.fade_in_s, st.fade_in_s, st.fade_in_s),
        };
        let out = if st.transition == TransitionKind::Cut { 0.0 } else { st.fade_out_s as f64 };
        Timing { kind: st.transition, geometry: g as f64, color: c as f64, brightness: b as f64, out }
    }

    fn fade_in(&self) -> f64 {
        self.geometry.max(self.color).max(self.brightness)
    }
}

/// 0 at the start, 1 once `len` has gone by (at once for a zero length).
fn progress(elapsed: f64, len: f64) -> f32 {
    if len <= 0.0 {
        1.0
    } else {
        (elapsed / len).clamp(0.0, 1.0) as f32
    }
}

fn lerp(a: f32, b: f32, u: f32) -> f32 {
    a + (b - a) * u
}

/// The step on show (its look, or `None` for *Noir*).
#[derive(Clone, Debug, PartialEq)]
struct Current {
    id: u64,
    look: Option<Settings>,
    timing: Timing,
    /// The kind actually used (a morph from the dark is a fade).
    kind: TransitionKind,
    /// Colour and brightness it comes from (split timing): lerped to its own.
    from_color: Option<[u8; 3]>,
    from_brightness: Option<f32>,
}

/// The step before, still drawn while it fades out.
#[derive(Clone, Debug, PartialEq)]
struct Leaving {
    id: u64,
    look: Settings,
    kind: TransitionKind,
    start: f64,
    len: f64,
    /// Its level when it started to leave (it may not have been fully in).
    from_gain: f32,
    /// Drawn by the new step's morph, not on its own.
    absorbed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Delayed {
    index: usize,
    left: f64,
}

/// The loaded list and where it is. Memory only: a restart is at *Début*.
#[derive(Clone, Debug, Default)]
pub struct ListPlayer {
    pub list: Option<CueList>,
    /// The step on show; `None` = before the first (*Début*).
    pub index: Option<usize>,
    pub paused: bool,
    /// Who drove it last (« En cours »).
    pub source: ControlSource,
    /// The player's own seconds: they stop while paused.
    clock: f64,
    /// The tempo clock's beat at the last frame.
    beat: f64,
    /// The audio event counters at the last frame.
    events: [u64; AUDIO_EVENTS.len()],
    next_id: u64,
    cur: Option<Current>,
    out: Option<Leaving>,
    delay: Option<Delayed>,
    /// When the current step started: player clock, beat.
    begun_at: f64,
    /// The beat a *Sur le temps* wait counts from (the step's start, or
    /// the end of a pause).
    beat_base: f64,
    /// Its whole transition (fade in and the old step's fade out).
    fade_len: f64,
    /// Whether `events` holds this list's counters (not stale ones).
    synced: bool,
    /// Controls fired by steps that started, for the caller to apply.
    pub fired: Vec<StepAction>,
    /// A step with a look started since the caller last looked.
    pub started_look: bool,
}

/// What a step source shows: the caller's look-up (cues, scenes).
pub type Resolve<'a> = &'a dyn Fn(&StepSource) -> Option<Settings>;

impl ListPlayer {
    pub fn load(&mut self, list: CueList) {
        self.top();
        self.synced = false;
        self.list = Some(list);
    }

    /// The loaded list was saved again: keep the place when it still exists.
    pub fn refresh(&mut self, list: &CueList) {
        if self.list.as_ref().is_some_and(|l| l.name == list.name) {
            let len = list.steps.len();
            self.list = Some(list.clone());
            if self.index.is_some_and(|i| i >= len) || self.delay.is_some_and(|d| d.index >= len) {
                self.top();
            }
        }
    }

    /// *Début*: before the first step, nothing out of the list.
    pub fn top(&mut self) {
        self.index = None;
        self.cur = None;
        self.out = None;
        self.delay = None;
        self.paused = false;
    }

    /// Forgets the list (deleted, another project).
    pub fn unload(&mut self) {
        self.top();
        self.list = None;
    }

    fn len(&self) -> usize {
        self.list.as_ref().map_or(0, |l| l.steps.len())
    }

    /// The step *Go* would start next.
    pub fn next_index(&self) -> Option<usize> {
        let len = self.len();
        if let Some(d) = self.delay {
            return Some(d.index);
        }
        match self.index {
            _ if len == 0 => None,
            None => Some(0),
            Some(i) if i + 1 < len => Some(i + 1),
            Some(_) if self.list.as_ref().is_some_and(|l| l.wrap) => Some(0),
            Some(_) => None,
        }
    }

    pub fn phase(&self) -> Phase {
        if let Some(d) = self.delay {
            return Phase::Delay { left_s: d.left.max(0.0) as f32 };
        }
        if self.index.is_none() {
            return Phase::Idle;
        }
        let t = progress(self.clock - self.begun_at, self.fade_len);
        if t < 1.0 {
            Phase::Fading { t }
        } else {
            Phase::Holding
        }
    }

    /// *Go*. Paused: resumes. During a delay: the step starts now. During
    /// a fade: the fade ends, then the next step starts (with its delay).
    /// On the last step: nothing, or the first again with *Boucler*.
    pub fn go(&mut self, resolve: Resolve) {
        if self.paused {
            self.resume();
            return;
        }
        if let Some(d) = self.delay.take() {
            self.begin(d.index, d.index, resolve);
            return;
        }
        self.finish_fade();
        if let Some(n) = self.next_index() {
            self.start(n, resolve);
        }
    }

    /// *Retour*: the step before, at once, with the times of the step left.
    pub fn back(&mut self, resolve: Resolve) {
        self.paused = false;
        self.delay = None;
        self.finish_fade();
        if let Some(i) = self.index.filter(|&i| i > 0) {
            self.begin(i - 1, i, resolve);
        }
    }

    /// *Aller à*: step `index` at once, with its own times.
    pub fn goto(&mut self, index: usize, resolve: Resolve) -> bool {
        if index >= self.len() {
            return false;
        }
        self.paused = false;
        self.delay = None;
        self.finish_fade();
        self.begin(index, index, resolve);
        true
    }

    /// *Pause*: freezes a fade or a delay; again (or *Go*) resumes.
    pub fn pause(&mut self) {
        if self.paused {
            self.resume();
        } else if self.index.is_some() || self.delay.is_some() {
            self.paused = true;
        }
    }

    /// The e-stop: paused, and it stays so until *Go* or *Pause*.
    pub fn halt(&mut self) {
        if self.index.is_some() || self.delay.is_some() {
            self.paused = true;
        }
    }

    fn resume(&mut self) {
        self.paused = false;
        // A *Sur le temps* step counts its beats from now.
        self.beat_base = self.beat;
    }

    /// Ends the running transition at once (the new step fully in).
    fn finish_fade(&mut self) {
        self.out = None;
        self.begun_at = self.begun_at.min(self.clock - self.fade_len);
    }

    /// Starts step `n`: after its delay, or now.
    fn start(&mut self, n: usize, resolve: Resolve) {
        let delay = self.list.as_ref().map_or(0.0, |l| l.steps[n].delay_s as f64);
        if delay > 0.0 {
            self.delay = Some(Delayed { index: n, left: delay });
        } else {
            self.begin(n, n, resolve);
        }
    }

    /// The level and settings of the current step as drawn now.
    fn drawn(&self, cur: &Current) -> (f32, Option<Settings>) {
        let el = self.clock - self.begun_at;
        let look = cur.look.clone().map(|mut l| {
            if let Some(c) = cur.from_color {
                let p = progress(el, cur.timing.color);
                l.color = std::array::from_fn(|i| lerp(c[i] as f32, l.color[i] as f32, p).round().clamp(0.0, 255.0) as u8);
            }
            if let Some(b) = cur.from_brightness {
                l.brightness = lerp(b, l.brightness, progress(el, cur.timing.brightness));
            }
            l
        });
        // A morph draws at full (its brightness, if split, is in the look).
        let level = match cur.kind {
            TransitionKind::Morph => 1.0,
            kind => Fade::gain_in(kind, progress(el, cur.timing.brightness)),
        };
        (level, look)
    }

    /// Step `index` comes in now, with the times of step `timing_of`.
    fn begin(&mut self, index: usize, timing_of: usize, resolve: Resolve) {
        let Some(list) = &self.list else { return };
        let (step, timing) = (list.steps[index].clone(), Timing::of(&list.steps[timing_of]));
        let mut look = resolve(&step.source);
        // What was on show: it leaves, from where it is.
        let before = self.cur.take().map(|c| {
            let (level, drawn) = self.drawn(&c);
            (c.id, level, drawn, c.look.map(|l| l.color))
        });
        let before_look = before.as_ref().and_then(|(id, level, drawn, _)| Some((*id, *level, drawn.clone()?))).filter(|(_, level, l)| *level > 0.0 && l.brightness > 0.0);
        if step.keep_color {
            if let (Some(l), Some((_, _, _, Some(c)))) = (look.as_mut(), before.as_ref()) {
                l.color = *c;
            }
        }
        // *Règle du noir*: from the dark (or into it), never a morph.
        let dark = before_look.is_none();
        let kind = match timing.kind {
            TransitionKind::Morph if dark || look.is_none() => TransitionKind::Fade,
            k => k,
        };
        let morph = kind == TransitionKind::Morph;
        let split = list.steps[timing_of].split.is_some();
        let (from_color, from_brightness) = match (&before_look, split) {
            (Some((_, _, l)), true) if !step.keep_color => (Some(l.color), morph.then_some(l.brightness)),
            (Some((_, _, l)), true) => (None, morph.then_some(l.brightness)),
            _ => (None, None),
        };
        let out_len = if morph { timing.geometry } else { timing.out };
        self.out = before_look.filter(|_| out_len > 0.0).map(|(id, level, l)| Leaving { id, look: l, kind, start: self.clock, len: out_len, from_gain: level, absorbed: morph });
        self.next_id += 1;
        let id = ID_BASE + self.next_id;
        let has_look = look.is_some();
        self.cur = Some(Current { id, look, timing, kind, from_color, from_brightness });
        self.index = Some(index);
        self.begun_at = self.clock;
        self.beat_base = self.beat;
        self.fade_len = if has_look { timing.fade_in() } else { 0.0 }.max(if self.out.is_some() { out_len } else { 0.0 });
        self.started_look |= has_look;
        if let Some(f) = step.fire {
            self.fired.push(f);
        }
    }

    /// One engine frame: `dt` seconds (frozen while paused), the tempo
    /// clock's beat and this frame's audio. Delays run out, finished
    /// fades end, and the next step starts by itself when its trigger
    /// says so (one step at most per frame).
    pub fn tick(&mut self, dt: f64, beat: f64, audio: &AudioFeatures, resolve: Resolve) {
        let events: [u64; AUDIO_EVENTS.len()] = std::array::from_fn(|i| audio.counter(AUDIO_EVENTS[i]).unwrap_or(0));
        let mut before = std::mem::replace(&mut self.events, events);
        if !self.synced {
            // Counters from before the list was loaded are not events.
            before = events;
            self.synced = true;
        }
        self.beat = beat;
        if self.list.is_none() || self.paused {
            return;
        }
        self.clock += dt.max(0.0);
        if let Some(d) = &mut self.delay {
            d.left -= dt.max(0.0);
            if d.left <= 1e-9 {
                let n = d.index;
                self.delay = None;
                self.begin(n, n, resolve);
            }
            return;
        }
        if self.out.as_ref().is_some_and(|o| self.clock - o.start >= o.len) {
            self.out = None;
        }
        let (Some(i), Some(n)) = (self.index, self.next_index()) else { return };
        let Some(list) = &self.list else { return };
        if n == i && list.steps.len() == 1 {
            // A one-step loop never restarts itself.
            return;
        }
        let since = self.clock - self.begun_at;
        let due = match &list.steps[n].trigger {
            StepTrigger::Go => false,
            StepTrigger::After { s } => since + 1e-9 >= *s as f64,
            StepTrigger::Follow => since + 1e-9 >= self.fade_len,
            StepTrigger::Beats { n } => {
                let n = (*n).max(1) as f64;
                (beat / n).floor() > (self.beat_base / n).floor()
            }
            StepTrigger::Audio { event } => AUDIO_EVENTS.iter().position(|e| e == event).is_some_and(|k| events[k] > before[k]),
        };
        if due {
            self.start(n, resolve);
        }
    }

    /// The step before while it is still leaving.
    fn leaving(&self) -> Option<&Leaving> {
        self.out.as_ref().filter(|o| self.clock - o.start < o.len)
    }

    /// The looks to draw this frame, as (layer, animator id, settings):
    /// the step leaving first, then the step on show.
    pub fn looks(&self) -> Vec<(u8, u64, Settings)> {
        let layer = self.list.as_ref().map_or(1, |l| l.layer);
        let mut out = Vec::new();
        if let Some(o) = self.leaving() {
            out.push((layer, o.id, o.look.clone()));
        }
        if let Some(c) = &self.cur {
            if let (_, Some(look)) = self.drawn(c) {
                out.push((layer, c.id, look));
            }
        }
        out
    }

    /// What the fades do to those looks this frame (transition::apply).
    pub fn fx(&self) -> Vec<(u64, LookFx)> {
        let mut fx = Vec::new();
        let el = self.clock - self.begun_at;
        if let Some(c) = self.cur.as_ref().filter(|c| c.look.is_some()) {
            match (self.leaving(), c.kind) {
                (Some(o), TransitionKind::Morph) if o.absorbed => {
                    let p = progress(el, c.timing.geometry);
                    if p < 1.0 {
                        fx.push((c.id, LookFx::MorphFrom { from: o.id, p }));
                    }
                }
                // A morph's brightness (split timing) is in its look.
                (_, TransitionKind::Morph) => {}
                (_, kind) => {
                    let p = progress(el, c.timing.brightness);
                    if p < 1.0 {
                        fx.push((c.id, LookFx::Gain(Fade::gain_in(kind, p))));
                    }
                }
            }
        }
        if let Some(o) = self.leaving() {
            if o.absorbed {
                fx.push((o.id, LookFx::Absorbed));
            } else {
                let q = progress(self.clock - o.start, o.len);
                fx.push((o.id, LookFx::Gain(o.from_gain * Fade::gain_out(o.kind, q))));
            }
        }
        fx
    }

    /// `cuelist` in `/api/frame` and `/api/cuelists`; `null` when no list is loaded.
    pub fn view(&self) -> Value {
        let Some(list) = &self.list else { return Value::Null };
        let label = |i: usize| list.steps.get(i).map(|s| s.label.as_str());
        let phase = self.phase();
        let progress = match phase {
            Phase::Fading { t } => Some(t),
            Phase::Delay { left_s } => {
                let total = self.delay.map_or(0.0, |d| list.steps[d.index].delay_s);
                Some(if total > 0.0 { 1.0 - left_s / total } else { 1.0 })
            }
            _ => None,
        };
        let next = if self.delay.is_some() { None } else { self.next_index() };
        json!({
            "name": list.name,
            "layer": list.layer,
            "steps": list.steps.len(),
            "index": self.index,
            "label": self.index.and_then(label),
            "phase": phase,
            "progress": progress,
            "paused": self.paused,
            "waiting": self.delay.map(|d| d.index),
            "next": next,
            "next_label": next.and_then(label),
            "next_trigger": next.and_then(|n| list.steps.get(n)).map(|s| &s.trigger),
            "source": self.source,
            "source_label": self.source.label_fr(),
        })
    }

    /// Whether anything of the list is on show or about to be.
    pub fn is_active(&self) -> bool {
        self.list.is_some() && (self.index.is_some() || self.delay.is_some())
    }
}

// ---------- Shared-level actions (controls, API, engine) ----------

/// The look a step source shows now: a cue as a grid press would start
/// it, a scene's look; `None` for *Noir* or a source that is gone.
pub fn resolve(s: &Shared, src: &StepSource) -> Option<Settings> {
    match src {
        StepSource::Black => None,
        StepSource::Cue { id } => crate::controls::cue_settings(s, id),
        StepSource::Scene { name } => s.scenes.get(name).map(|sc| sc.settings.clone()),
    }
}

/// Runs `f` on the player with the studio's look-up, then applies what
/// the steps started: their controls (as a controller would, never
/// arming), the manual look and playlist giving way like for a cue, and
/// the safety log while armed.
fn with_player(s: &mut Shared, source: ControlSource, f: impl FnOnce(&mut ListPlayer, Resolve)) {
    let mut p = std::mem::take(&mut s.cue_list);
    let before = p.index;
    {
        let shared: &Shared = s;
        let resolve = |src: &StepSource| resolve(shared, src);
        f(&mut p, &resolve);
    }
    if p.index != before || p.started_look {
        p.source = source;
    }
    let fired = std::mem::take(&mut p.fired);
    let started = std::mem::take(&mut p.started_look);
    if started {
        s.playlist = None;
        s.look_on = false;
        if s.gate.is_armed() {
            if let (Some(list), Some(i)) = (&p.list, p.index) {
                s.gate.log().record("cuelist_step", Some(source_id(source)), json!({ "name": list.name, "step": i + 1, "label": list.steps[i].label }));
            }
        }
    }
    s.cue_list = p;
    for a in fired {
        // Checked when the list was saved; checked again, as a controller.
        if check_action(&a, &s.controls).is_ok() {
            if let Err(e) = crate::controls::apply_as(s, &a.control, crate::controls::ControlInput::Value(a.value), crate::interlock::ArmSource::System) {
                log::warn!("liste de cues : {} : {e:?}", a.control);
            }
        }
    }
}

fn source_id(source: ControlSource) -> &'static str {
    match source {
        ControlSource::Keyboard => "keyboard",
        ControlSource::Midi => "midi",
        ControlSource::Dmx => "dmx",
        ControlSource::Osc => "osc",
        ControlSource::Api => "api",
        ControlSource::Network => "network",
        ControlSource::Audio => "audio",
        ControlSource::System | ControlSource::Timeline => "system",
        _ => "ui",
    }
}

pub const NO_LIST: &str = "aucune liste de cues chargée";
pub const ESTOP_FR: &str = "arrêt d'urgence actif : réinitialiser avant de reprendre la liste";

fn ready(s: &Shared) -> Result<(), &'static str> {
    if s.cue_list.list.is_none() {
        return Err(NO_LIST);
    }
    if s.estop.is_latched() {
        return Err(ESTOP_FR);
    }
    Ok(())
}

/// `list.go` / *GO*. Refused with the e-stop latched: the list waits.
pub fn go(s: &mut Shared, source: ControlSource) -> Result<(), &'static str> {
    ready(s)?;
    with_player(s, source, |p, r| p.go(r));
    Ok(())
}

pub fn back(s: &mut Shared, source: ControlSource) -> Result<(), &'static str> {
    ready(s)?;
    with_player(s, source, |p, r| p.back(r));
    Ok(())
}

/// *Aller à* step `step` (0-based).
pub fn goto(s: &mut Shared, step: usize, source: ControlSource) -> Result<(), &'static str> {
    ready(s)?;
    let mut ok = true;
    with_player(s, source, |p, r| ok = p.goto(step, r));
    if ok { Ok(()) } else { Err("pas inconnu") }
}

/// *Pause* (again: resume). Allowed with the e-stop latched, but resuming isn't.
pub fn pause(s: &mut Shared, source: ControlSource) -> Result<(), &'static str> {
    if s.cue_list.list.is_none() {
        return Err(NO_LIST);
    }
    if s.cue_list.paused && s.estop.is_latched() {
        return Err(ESTOP_FR);
    }
    s.cue_list.pause();
    s.cue_list.source = source;
    Ok(())
}

/// *Début*: always allowed (it only takes light away).
pub fn top(s: &mut Shared, source: ControlSource) -> Result<(), &'static str> {
    if s.cue_list.list.is_none() {
        return Err(NO_LIST);
    }
    s.cue_list.top();
    s.cue_list.source = source;
    Ok(())
}

/// Loads a saved list, at *Début*.
pub fn load(s: &mut Shared, name: &str) -> Result<(), &'static str> {
    let list = s.cue_lists.get(name).cloned().ok_or("liste de cues inconnue")?;
    s.cue_list.load(list);
    s.cue_list.source = ControlSource::Ui;
    Ok(())
}

/// Once per engine frame at `t`, after the deck (`controls::deck_frame`):
/// the e-stop pauses the list; otherwise it moves on by `dt`.
pub fn frame(s: &mut Shared, dt: f64, beat: f64, audio: &AudioFeatures) {
    if s.cue_list.list.is_none() {
        return;
    }
    if s.estop.is_latched() {
        s.cue_list.halt();
    }
    with_player(s, ControlSource::System, |p, r| p.tick(dt, beat, audio, r));
}

/// `GET /api/cuelists`.
pub fn view(s: &Shared) -> Value {
    json!({
        "lists": s.cue_lists.list(),
        "player": s.cue_list.view(),
        "triggers": TRIGGER_LABELS,
        "audio_events": AUDIO_EVENTS,
    })
}

/// `POST /api/cuelists`: saves one list (by name); the loaded copy follows.
pub fn save(s: &mut Shared, list: CueList) -> Result<()> {
    let reg = &s.controls;
    s.cue_lists.upsert(list.clone(), reg)?;
    if let Some(saved) = s.cue_lists.get(list.name.trim()).cloned() {
        s.cue_list.refresh(&saved);
    }
    Ok(())
}

/// `POST /api/cuelists/delete`.
pub fn delete(s: &mut Shared, name: &str) -> Result<bool> {
    let gone = s.cue_lists.remove(name)?;
    if gone && s.cue_list.list.as_ref().is_some_and(|l| l.name == name) {
        s.cue_list.unload();
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    /// Looks by source: a cue `c<n>` is a circle of colour n, scale 0.n.
    fn resolver(src: &StepSource) -> Option<Settings> {
        match src {
            StepSource::Black => None,
            StepSource::Cue { id } | StepSource::Scene { name: id } => {
                let n: u8 = id.trim_start_matches('c').parse().unwrap_or(1);
                Some(Settings { color: [n * 10, 0, 0], scale: n as f32 / 10.0, brightness: 1.0, ..Settings::default() })
            }
        }
    }

    fn cue(n: u8) -> StepSource {
        StepSource::Cue { id: format!("c{n}") }
    }

    fn step(source: StepSource, trigger: StepTrigger, fade: f32) -> Step {
        Step { source, trigger, fade_in_s: fade, fade_out_s: fade, ..Step::default() }
    }

    fn player(steps: Vec<Step>) -> ListPlayer {
        let mut list = CueList { name: "L".into(), steps, ..CueList::default() };
        list.sanitize();
        let mut p = ListPlayer::default();
        p.load(list);
        p
    }

    const FRAME: f64 = 1.0 / 60.0;

    fn run(p: &mut ListPlayer, seconds: f64) {
        let frames = (seconds / FRAME).round() as usize;
        for _ in 0..frames {
            let beat = p.beat;
            p.tick(FRAME, beat, &AudioFeatures::default(), &resolver);
        }
    }

    fn go(p: &mut ListPlayer) {
        p.go(&resolver);
    }

    fn scale_on_show(p: &ListPlayer) -> Vec<f32> {
        p.looks().iter().map(|(_, _, s)| s.scale).collect()
    }

    #[test]
    fn go_steps_exactly_one_step_and_stops_on_the_last() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), step(cue(2), StepTrigger::Go, 0.0), step(StepSource::Black, StepTrigger::Go, 0.0)]);
        assert_eq!((p.index, p.phase()), (None, Phase::Idle));
        assert!(p.looks().is_empty(), "nothing out before the first Go");
        go(&mut p);
        assert_eq!(p.index, Some(0));
        assert_eq!(scale_on_show(&p), [0.1]);
        run(&mut p, 5.0);
        assert_eq!(p.index, Some(0), "a Go step waits for Go");
        go(&mut p);
        assert_eq!((p.index, scale_on_show(&p)), (Some(1), vec![0.2]));
        go(&mut p);
        assert_eq!(p.index, Some(2));
        assert!(p.looks().is_empty(), "Noir");
        go(&mut p);
        assert_eq!(p.index, Some(2), "the last step stays without « Boucler »");
        p.list.as_mut().unwrap().wrap = true;
        go(&mut p);
        assert_eq!(p.index, Some(0), "« Boucler »: back to the first");
    }

    #[test]
    fn a_follow_step_starts_when_the_fade_before_ends() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 1.0), step(cue(2), StepTrigger::Follow, 0.5)]);
        go(&mut p);
        let mut frames = 0;
        while p.index == Some(0) {
            run(&mut p, FRAME);
            frames += 1;
            assert!(frames < 200);
        }
        assert!((59..=61).contains(&frames), "started {frames} frames after Go (fade 1 s = 60 frames)");
    }

    #[test]
    fn after_n_seconds_counts_from_the_start_of_the_step_before() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), Step { delay_s: 0.5, ..step(cue(2), StepTrigger::After { s: 2.0 }, 0.0) }]);
        go(&mut p);
        run(&mut p, 1.95);
        assert_eq!(p.index, Some(0));
        run(&mut p, 0.1);
        assert!(matches!(p.phase(), Phase::Delay { .. }), "then its own delay");
        run(&mut p, 0.5);
        assert_eq!(p.index, Some(1));
    }

    #[test]
    fn beats_trigger_starts_on_the_beat_of_the_tempo_clock() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), step(cue(2), StepTrigger::Beats { n: 4 }, 0.0)]);
        // 120 BPM: 2 beats per second; Go on beat 1.3.
        let mut beat = 1.3;
        p.tick(0.0, beat, &AudioFeatures::default(), &resolver);
        go(&mut p);
        let mut started_at = None;
        for _ in 0..300 {
            beat += 2.0 * FRAME;
            p.tick(FRAME, beat, &AudioFeatures::default(), &resolver);
            if p.index == Some(1) {
                started_at = Some(beat);
                break;
            }
        }
        let b = started_at.expect("started");
        assert!((4.0..4.0 + 2.0 * FRAME + 1e-9).contains(&b), "started on beat {b}, want 4 (± 1 frame)");
    }

    #[test]
    fn an_audio_step_starts_on_the_next_event() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), step(cue(2), StepTrigger::Audio { event: "kick".into() }, 0.0)]);
        let mut a = AudioFeatures { kick: 5, snare: 2, ..Default::default() };
        p.tick(FRAME, 0.0, &a, &resolver);
        go(&mut p);
        p.tick(FRAME, 0.0, &a, &resolver);
        a.snare += 1;
        p.tick(FRAME, 0.0, &a, &resolver);
        assert_eq!(p.index, Some(0), "a snare is not a kick");
        a.kick += 1;
        p.tick(FRAME, 0.0, &a, &resolver);
        assert_eq!(p.index, Some(1));
    }

    #[test]
    fn go_during_a_fade_finishes_it_then_goes() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 2.0), step(cue(2), StepTrigger::Go, 2.0), step(cue(3), StepTrigger::Go, 2.0)]);
        go(&mut p);
        go(&mut p);
        run(&mut p, 0.5);
        assert!(matches!(p.phase(), Phase::Fading { .. }));
        go(&mut p);
        assert_eq!(p.index, Some(2));
        // The step left (2) leaves from full; step 1 is long gone.
        let ids: Vec<f32> = scale_on_show(&p);
        assert_eq!(ids.len(), 2);
        assert!((ids[0] - 0.2).abs() < 1e-6 && (ids[1] - 0.3).abs() < 1e-6);
        let fx = p.fx();
        assert!(fx.iter().any(|(_, f)| *f == LookFx::Gain(1.0)), "the step left starts its fade-out at full: {fx:?}");
    }

    #[test]
    fn back_goto_and_top() {
        let mut p = player((1..=4).map(|n| step(cue(n), StepTrigger::Go, 0.0)).collect());
        go(&mut p);
        go(&mut p);
        go(&mut p);
        p.back(&resolver);
        assert_eq!((p.index, scale_on_show(&p)), (Some(1), vec![0.2]));
        assert!(p.goto(3, &resolver));
        assert_eq!(p.index, Some(3));
        assert!(!p.goto(9, &resolver));
        p.top();
        assert_eq!((p.index, p.looks().len()), (None, 0), "Début: nothing out");
        p.back(&resolver);
        assert_eq!(p.index, None);
        go(&mut p);
        assert_eq!(p.index, Some(0), "Go from Début: the first step");
    }

    #[test]
    fn back_uses_the_times_of_the_step_left() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), step(cue(2), StepTrigger::Go, 3.0)]);
        go(&mut p);
        go(&mut p);
        run(&mut p, 5.0);
        p.back(&resolver);
        assert_eq!(p.phase(), Phase::Fading { t: 0.0 }, "step 2's 3 s fade, not step 1's cut");
    }

    #[test]
    fn pause_holds_a_fade_and_go_resumes_it_at_the_same_point() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 2.0)]);
        go(&mut p);
        run(&mut p, 0.5);
        let Phase::Fading { t } = p.phase() else { panic!("fading") };
        p.pause();
        run(&mut p, 3.0);
        assert_eq!(p.phase(), Phase::Fading { t }, "frozen");
        let fx = p.fx();
        go(&mut p);
        assert_eq!(p.index, Some(0), "Go resumes, it doesn't step");
        assert_eq!(p.fx(), fx, "from the same point");
        run(&mut p, 1.6);
        assert_eq!(p.phase(), Phase::Holding);
    }

    #[test]
    fn pause_holds_a_delay_and_an_after_wait() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), step(cue(2), StepTrigger::After { s: 1.0 }, 0.0)]);
        go(&mut p);
        run(&mut p, 0.5);
        p.pause();
        run(&mut p, 5.0);
        assert_eq!(p.index, Some(0));
        p.pause();
        run(&mut p, 0.55);
        assert_eq!(p.index, Some(1));
    }

    #[test]
    fn halt_pauses_and_nothing_restarts_by_itself() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), step(cue(2), StepTrigger::Follow, 0.0)]);
        go(&mut p);
        p.halt();
        run(&mut p, 10.0);
        assert_eq!((p.index, p.paused), (Some(0), true));
    }

    #[test]
    fn a_fade_from_black_never_morphs() {
        // Noir, then a cue that would morph: the black rule makes it a fade.
        let morph = |n| Step { transition: TransitionKind::Morph, ..step(cue(n), StepTrigger::Go, 1.0) };
        let mut p = player(vec![step(StepSource::Black, StepTrigger::Go, 0.0), morph(1), morph(2)]);
        go(&mut p);
        go(&mut p);
        for _ in 0..70 {
            for (_, f) in p.fx() {
                assert!(matches!(f, LookFx::Gain(_)), "from black: only a gain, never {f:?}");
            }
            let looks = p.looks();
            assert!(looks.len() <= 1 && looks.iter().all(|(_, _, s)| (s.scale - 0.1).abs() < 1e-6), "only the final geometry");
            run(&mut p, FRAME);
        }
        // Lit to lit: a real morph.
        go(&mut p);
        run(&mut p, 0.2);
        assert!(p.fx().iter().any(|(_, f)| matches!(f, LookFx::MorphFrom { .. })));
        // From Début (nothing on show) too.
        p.top();
        p.goto(1, &resolver);
        assert!(p.fx().iter().all(|(_, f)| matches!(f, LookFx::Gain(_))));
    }

    #[test]
    fn a_zero_brightness_step_counts_as_black() {
        let dark = |src: &StepSource| resolver(src).map(|s| if *src == cue(1) { Settings { brightness: 0.0, ..s } } else { s });
        let morph = |n| Step { transition: TransitionKind::Morph, ..step(cue(n), StepTrigger::Go, 1.0) };
        let mut p = player(vec![morph(1), morph(2)]);
        p.go(&dark);
        p.tick(2.0, 0.0, &AudioFeatures::default(), &dark);
        p.go(&dark);
        assert!(p.fx().iter().all(|(_, f)| matches!(f, LookFx::Gain(_))), "{:?}", p.fx());
    }

    #[test]
    fn split_timing_colour_slower_than_geometry() {
        let split = Some(SplitTiming { geometry_s: 1.0, color_s: 3.0, brightness_s: 1.0 });
        let morph = |n| Step { transition: TransitionKind::Morph, split, ..step(cue(n), StepTrigger::Go, 0.0) };
        let mut p = player(vec![morph(1), morph(2)]);
        go(&mut p);
        go(&mut p);
        run(&mut p, 1.5);
        assert!(p.fx().is_empty(), "the geometry is there after 1 s");
        let looks = p.looks();
        let (_, _, now) = looks.last().unwrap();
        assert_eq!(now.scale, 0.2);
        assert_eq!(now.color[0], 15, "colour half-way from 10 to 20 after 1.5 of 3 s");
        run(&mut p, 1.6);
        assert_eq!(p.looks().last().unwrap().2.color[0], 20);
        assert_eq!(p.phase(), Phase::Holding);
    }

    #[test]
    fn keep_colour_takes_the_colour_of_the_step_before() {
        let mut p = player(vec![step(cue(1), StepTrigger::Go, 0.0), Step { keep_color: true, ..step(cue(3), StepTrigger::Go, 0.0) }]);
        go(&mut p);
        go(&mut p);
        let looks = p.looks();
        assert_eq!((looks.len(), looks[0].2.color, looks[0].2.scale), (1, [10, 0, 0], 0.3));
    }

    #[test]
    fn a_step_fires_its_control_once() {
        let mut p = player(vec![Step { fire: Some(StepAction { control: "tempo.resync".into(), value: 1.0 }), ..step(cue(1), StepTrigger::Go, 0.0) }]);
        go(&mut p);
        assert_eq!(p.fired.len(), 1);
        run(&mut p, 1.0);
        assert_eq!(p.fired.len(), 1);
    }

    #[test]
    fn a_one_step_wrapping_list_never_runs_away() {
        let mut p = player(vec![step(cue(1), StepTrigger::Follow, 0.0)]);
        p.list.as_mut().unwrap().wrap = true;
        go(&mut p);
        let id = p.cur.as_ref().unwrap().id;
        run(&mut p, 1.0);
        assert_eq!(p.cur.as_ref().unwrap().id, id);
    }

    #[test]
    fn saving_refuses_arming_and_list_controls() {
        let s = test_support::shared();
        let with = |control: &str| CueList { name: "A".into(), steps: vec![Step { fire: Some(StepAction { control: control.into(), value: 1.0 }), ..Step::default() }], ..CueList::default() };
        for bad in ["transport.arm", "list.go", "list.back", "nope"] {
            assert!(with(bad).validate(&s.controls).is_err(), "{bad} refused");
        }
        for good in ["tempo.resync", "timeline.play", "cue.stop_all"] {
            with(good).validate(&s.controls).unwrap();
        }
        let mut store = CueListStore::in_memory();
        assert!(store.upsert(with("transport.arm"), &s.controls).is_err());
        assert!(store.list().is_empty());
        assert!(CueList { name: "../x".into(), ..CueList::default() }.validate(&s.controls).is_err());
        let audio = CueList { name: "A".into(), steps: vec![step(cue(1), StepTrigger::Audio { event: "boom".into() }, 0.0)], ..CueList::default() };
        assert!(audio.validate(&s.controls).is_err());
    }

    #[test]
    fn sanitize_clamps_and_numbers_steps() {
        let mut l = CueList {
            name: " A ".into(),
            layer: 9,
            steps: vec![
                Step { id: 3, fade_in_s: f32::NAN, delay_s: -2.0, ..Step::default() },
                Step { id: 3, trigger: StepTrigger::Beats { n: 0 }, ..Step::default() },
                Step { fade_out_s: 1e9, trigger: StepTrigger::After { s: f32::INFINITY }, ..Step::default() },
            ],
            ..CueList::default()
        };
        l.sanitize();
        assert_eq!((l.name.as_str(), l.layer), ("A", LAYER_COUNT as u8));
        let ids: Vec<u64> = l.steps.iter().map(|s| s.id).collect();
        assert_eq!(ids, [3, 4, 5]);
        assert_eq!((l.steps[0].fade_in_s, l.steps[0].delay_s), (0.0, 0.0));
        assert_eq!(l.steps[1].trigger, StepTrigger::Beats { n: 1 });
        assert_eq!((l.steps[2].fade_out_s, &l.steps[2].trigger), (MAX_SECONDS, &StepTrigger::After { s: 0.0 }));
    }

    #[test]
    fn old_step_json_loads_with_defaults() {
        let l: CueList = serde_json::from_str(r#"{ "name": "A", "steps": [{ "source": { "type": "cue", "id": "x" } }, {}] }"#).unwrap();
        assert_eq!(l.layer, 1);
        assert_eq!(l.steps[0].source, StepSource::Cue { id: "x".into() });
        assert_eq!((l.steps[1].source.clone(), l.steps[1].trigger.clone(), l.steps[1].transition), (StepSource::Black, StepTrigger::Go, TransitionKind::Fade));
    }

    #[test]
    fn shared_go_is_refused_while_the_estop_is_latched_and_never_arms() {
        let mut s = test_support::shared();
        let id = s.presets[0].id.clone();
        let list = CueList { name: "A".into(), steps: vec![step(StepSource::Cue { id: id.clone() }, StepTrigger::Go, 0.0), step(StepSource::Black, StepTrigger::Follow, 0.0)], ..CueList::default() };
        save(&mut s, list).unwrap();
        assert_eq!(super::go(&mut s, ControlSource::Ui), Err(NO_LIST));
        load(&mut s, "A").unwrap();
        super::go(&mut s, ControlSource::Midi).unwrap();
        assert_eq!((s.cue_list.index, s.look_on), (Some(0), false), "a step takes over from the manual look");
        assert_eq!(s.cue_list.looks()[0].2.content, s.presets[0].settings.content);
        s.emergency_stop(crate::interlock::ArmSource::Keyboard);
        frame(&mut s, 1.0, 0.0, &AudioFeatures::default());
        assert!(s.cue_list.paused, "the e-stop pauses the list");
        assert_eq!(s.cue_list.index, Some(0), "and the follow step waits");
        assert_eq!(super::go(&mut s, ControlSource::Ui), Err(ESTOP_FR));
        assert!(pause(&mut s, ControlSource::Ui).is_err(), "nor resumed by Pause");
        top(&mut s, ControlSource::Ui).unwrap();
        s.gate.reset_estop(&s.estop.clone());
        super::go(&mut s, ControlSource::Ui).unwrap();
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_saved_list_reloads_identical_and_a_deleted_one_unloads() {
        let dir = std::env::temp_dir().join(format!("laser-studio-cuelists-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = test_support::shared();
        s.cue_lists = CueListStore::load_or_create(dir.join("cuelists.json"));
        let list = CueList {
            name: "Acte 1".into(),
            layer: 2,
            wrap: true,
            steps: vec![
                Step { label: "Ouverture".into(), split: Some(SplitTiming { geometry_s: 1.0, color_s: 2.0, brightness_s: 0.5 }), ..step(cue(1), StepTrigger::Go, 1.0) },
                Step { fire: Some(StepAction { control: "tempo.resync".into(), value: 1.0 }), keep_color: true, ..step(StepSource::Scene { name: "x".into() }, StepTrigger::Beats { n: 4 }, 0.0) },
            ],
        };
        save(&mut s, list).unwrap();
        let saved = s.cue_lists.get("Acte 1").unwrap().clone();
        let back = CueListStore::load_or_create(dir.join("cuelists.json"));
        assert_eq!(back.list(), [saved]);
        load(&mut s, "Acte 1").unwrap();
        assert!(delete(&mut s, "Acte 1").unwrap());
        assert!(s.cue_list.list.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
