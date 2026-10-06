//! The executor bank (T-418): faders and buttons the operator assigns.
//!
//! A console keeps what is stored (cues, layers, group masters, the cue
//! list) apart from what the hands hold: pages of 8 executors, each a
//! fader and two buttons, given an object and a function. The ideas
//! (executor, Flash, Temp, pages that cut nothing) come from public
//! console documentation; the model and the labels are our own.
//!
//! - *Fondu au curseur* (cue): raising the fader from 0 starts the cue,
//!   its level follows the fader, back to 0 stops it. The level is
//!   `ActiveCue::exec_level`, a gain of at most 1 on that cue's colours,
//!   applied before the mixer, the layer dimmers, the master and the
//!   whole safety stage: it only ever dims.
//! - *Niveau*: a cue's own level while it plays, a layer's dimmer, a zone
//!   group's master (T-420); all inhibitive, 0..1.
//! - *Vitesse*: a layer's animation speed, or a speed group's multiple
//!   (T-419, speed.rs): the fader's fifths are ×¼, ×½, ×1, ×2, ×4, applied
//!   on the next bar when *Appliquer au début de la mesure* is on. Timing
//!   only: the strobe and flash limiters keep the last word.
//! - Buttons: *Basculer*, *Flash* (full level while held), *Temp* (on
//!   while held, at the fader's level), *Noir tenu* (dark while held),
//!   *Go* (the cue list).
//!
//! Changing page cuts nothing: what plays keeps playing, its page tab is
//! marked, and a hardware fader on `exec.cur.<n>.fader` catches the new
//! page's value with the MIDI pickup rule (T-208), since `current` reads
//! the real level.
//!
//! Safety: no executor targets arming, the e-stop reset or any `safety.*`
//! control; nothing here arms or disarms. The assignments are content
//! (`POST /api/executors`: in the history, refused in show mode); playing
//! them goes through the control registry (`exec.*`), allowed in show
//! mode, and external inputs reach them through `external::apply`.

use crate::controls::{self, ControlInput, ControlSource};
use crate::cues::ClickMode;
use crate::layers::LAYER_COUNT;
use crate::Shared;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const EXEC_PER_PAGE: usize = 8;
pub const EXEC_MAX_PAGES: usize = 8;
/// Page names and executor labels, in characters.
pub const MAX_NAME: usize = 32;

/// What an executor drives.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecTarget {
    #[default]
    None,
    /// A built-in or user cue (preset id).
    Cue { id: String },
    /// Layer 1..=4.
    Layer { n: u8 },
    /// A zone group's master (T-420), 1-based.
    GroupMaster { group: usize },
    /// Speed group 1..=4 = A..D (T-419, speed.rs).
    SpeedGroup { n: u8 },
    /// The loaded cue list (T-410).
    CueList,
    /// A macro (T-425): « Go » launches it (again from its first step).
    Macro { id: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaderFn {
    #[default]
    Level,
    FadeIn,
    Speed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonFn {
    #[default]
    None,
    Toggle,
    Flash,
    Temp,
    BlackHeld,
    Go,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Executor {
    /// Shown instead of the object's name when set.
    pub label: String,
    pub target: ExecTarget,
    pub fader: FaderFn,
    pub buttons: [ButtonFn; 2],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecPage {
    pub name: String,
    /// Completed to `EXEC_PER_PAGE` by `sanitize`.
    pub handles: Vec<Executor>,
}

/// The bank as saved (`executors.json`, the project's `executors`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutorBank {
    pub pages: Vec<ExecPage>,
}

impl Default for ExecutorBank {
    /// One empty page « Page 1 ».
    fn default() -> Self {
        let mut bank = ExecutorBank { pages: Vec::new() };
        bank.sanitize();
        bank
    }
}

/// Whether `f` is a fader function for `target`.
pub fn fader_fits(target: &ExecTarget, f: FaderFn) -> bool {
    match target {
        ExecTarget::Cue { .. } => matches!(f, FaderFn::FadeIn | FaderFn::Level),
        ExecTarget::Layer { .. } => matches!(f, FaderFn::Level | FaderFn::Speed),
        ExecTarget::SpeedGroup { .. } => f == FaderFn::Speed,
        _ => f == FaderFn::Level,
    }
}

/// Whether `b` is a button function for `target`.
pub fn button_fits(target: &ExecTarget, b: ButtonFn) -> bool {
    if b == ButtonFn::None {
        return true;
    }
    match target {
        ExecTarget::Cue { .. } => matches!(b, ButtonFn::Toggle | ButtonFn::Flash | ButtonFn::Temp | ButtonFn::BlackHeld),
        ExecTarget::Layer { .. } | ExecTarget::GroupMaster { .. } => matches!(b, ButtonFn::Toggle | ButtonFn::Flash | ButtonFn::BlackHeld),
        ExecTarget::CueList | ExecTarget::Macro { .. } => b == ButtonFn::Go,
        _ => false,
    }
}

/// The default fader function of a target.
fn default_fader(target: &ExecTarget) -> FaderFn {
    match target {
        ExecTarget::Cue { .. } => FaderFn::FadeIn,
        ExecTarget::SpeedGroup { .. } => FaderFn::Speed,
        _ => FaderFn::Level,
    }
}

fn clip(text: &str) -> String {
    text.trim().chars().take(MAX_NAME).collect()
}

impl ExecutorBank {
    /// 1..=8 pages of exactly 8 executors, names clipped, and every
    /// function that doesn't fit its object back to the default (a hand-
    /// edited or older file loads, never with a function it can't run).
    pub fn sanitize(&mut self) {
        self.pages.truncate(EXEC_MAX_PAGES);
        if self.pages.is_empty() {
            self.pages.push(ExecPage::default());
        }
        for (i, page) in self.pages.iter_mut().enumerate() {
            page.name = clip(&page.name);
            if page.name.is_empty() {
                page.name = format!("Page {}", i + 1);
            }
            page.handles.resize_with(EXEC_PER_PAGE, Executor::default);
            for h in &mut page.handles {
                h.label = clip(&h.label);
                match &mut h.target {
                    ExecTarget::Cue { id } | ExecTarget::Macro { id } if id.trim().is_empty() => h.target = ExecTarget::None,
                    ExecTarget::Layer { n } if !(1..=LAYER_COUNT as u8).contains(n) => h.target = ExecTarget::None,
                    ExecTarget::GroupMaster { group } if !(1..=crate::projection::GROUP_MASTER_CONTROLS).contains(group) => h.target = ExecTarget::None,
                    _ => {}
                }
                if !fader_fits(&h.target, h.fader) {
                    h.fader = default_fader(&h.target);
                }
                for b in &mut h.buttons {
                    if !button_fits(&h.target, *b) {
                        *b = ButtonFn::None;
                    }
                }
            }
        }
    }

    /// `POST /api/executors`: refused rather than repaired. `cue_known`
    /// says whether a preset id exists, `macro_known` a macro id (T-425).
    pub fn validate(&self, cue_known: impl Fn(&str) -> bool, macro_known: impl Fn(&str) -> bool) -> Result<()> {
        if self.pages.is_empty() || self.pages.len() > EXEC_MAX_PAGES {
            bail!("de 1 à {EXEC_MAX_PAGES} pages d'exécuteurs");
        }
        for (i, page) in self.pages.iter().enumerate() {
            let p = i + 1;
            if page.name.chars().count() > MAX_NAME {
                bail!("page {p} : nom trop long ({MAX_NAME} caractères au plus)");
            }
            if page.handles.len() > EXEC_PER_PAGE {
                bail!("page {p} : {EXEC_PER_PAGE} exécuteurs au plus");
            }
            for (j, h) in page.handles.iter().enumerate() {
                let n = j + 1;
                if h.label.chars().count() > MAX_NAME {
                    bail!("exécuteur {p}.{n} : libellé trop long ({MAX_NAME} caractères au plus)");
                }
                match &h.target {
                    ExecTarget::None | ExecTarget::CueList => {}
                    ExecTarget::Cue { id } => {
                        if !cue_known(id) {
                            bail!("exécuteur {p}.{n} : cue inconnu « {id} »");
                        }
                    }
                    ExecTarget::Layer { n: l } => {
                        if !(1..=LAYER_COUNT as u8).contains(l) {
                            bail!("exécuteur {p}.{n} : calque {l} inexistant (1 à {LAYER_COUNT})");
                        }
                    }
                    ExecTarget::GroupMaster { group } => {
                        if !(1..=crate::projection::GROUP_MASTER_CONTROLS).contains(group) {
                            bail!("exécuteur {p}.{n} : groupe {group} hors limites");
                        }
                    }
                    ExecTarget::SpeedGroup { n: g } => {
                        if !(1..=crate::speed::SPEED_GROUPS as u8).contains(g) {
                            bail!("exécuteur {p}.{n} : groupe de vitesse {g} inexistant (1 à 4)");
                        }
                    }
                    ExecTarget::Macro { id } => {
                        if !macro_known(id) {
                            bail!("exécuteur {p}.{n} : macro inconnue « {id} »");
                        }
                    }
                }
                if !fader_fits(&h.target, h.fader) {
                    bail!("exécuteur {p}.{n} : fonction de curseur impossible pour cet objet");
                }
                if h.buttons.iter().any(|b| !button_fits(&h.target, *b)) {
                    bail!("exécuteur {p}.{n} : fonction de bouton impossible pour cet objet");
                }
            }
        }
        Ok(())
    }

    fn get(&self, page: usize, n: usize) -> Option<&Executor> {
        self.pages.get(page)?.handles.get(n)
    }
}

/// What the hands hold now. Memory only: every start with faders at their
/// rest and nothing held.
#[derive(Debug, Default)]
pub struct ExecRuntime {
    /// The page shown (0-based); the `exec.cur.*` aliases point at it.
    pub page: usize,
    /// Fader positions of cue and cue-list executors, by (page, handle);
    /// none = at rest. Layers and groups read their object instead.
    faders: HashMap<(usize, usize), f32>,
    /// Buttons held: (page, handle, button).
    held: HashMap<(usize, usize, usize), bool>,
    /// *Fondu au curseur* executors whose fader started their cue.
    engaged: HashMap<(usize, usize), bool>,
    /// Group masters a *Basculer* turned off: the level to go back to.
    group_off: HashMap<usize, f32>,
}

impl ExecRuntime {
    fn is_held(&self, page: usize, n: usize, b: usize) -> bool {
        self.held.get(&(page, n, b)).copied().unwrap_or(false)
    }

    fn is_engaged(&self, page: usize, n: usize) -> bool {
        self.engaged.get(&(page, n)).copied().unwrap_or(false)
    }

    /// Forgets what a handle held (its assignment changed).
    fn forget(&mut self, page: usize, n: usize) {
        self.faders.remove(&(page, n));
        self.engaged.remove(&(page, n));
        self.held.retain(|k, _| (k.0, k.1) != (page, n));
    }
}

/// The bank, where it is saved, and what the hands hold.
#[derive(Debug, Default)]
pub struct Executors {
    pub bank: ExecutorBank,
    path: Option<PathBuf>,
    pub rt: ExecRuntime,
    /// Bumped on every change of the bank, so the page reloads it.
    pub rev: u64,
}

impl Executors {
    /// `executors.json`, or one empty page.
    pub fn load(path: PathBuf) -> Self {
        let mut bank: ExecutorBank = crate::load_json(&path);
        bank.sanitize();
        Self { bank, path: Some(path), ..Self::default() }
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
            crate::save_json(path, &self.bank);
        }
    }

    /// Takes a new bank (an edit, an undo, a project): sanitised; the
    /// handles whose assignment changed let go of what they held. Memory
    /// only: the caller saves.
    pub fn set_bank(&mut self, mut bank: ExecutorBank) {
        bank.sanitize();
        for p in 0..EXEC_MAX_PAGES {
            for n in 0..EXEC_PER_PAGE {
                let (old, new) = (self.bank.get(p, n), bank.get(p, n));
                if old.map(|e| (&e.target, e.fader)) != new.map(|e| (&e.target, e.fader)) || old.map(|e| e.buttons) != new.map(|e| e.buttons) {
                    self.rt.forget(p, n);
                }
            }
        }
        self.bank = bank;
        self.rt.page = self.rt.page.min(self.bank.pages.len() - 1);
        self.rev += 1;
    }
}

// ---------- control ids ----------

/// One of an executor's three handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Fader,
    Button(usize),
}

/// What an `exec.*` id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecId {
    /// (page 0-based, handle 0-based, which); `cur` ids resolve to the
    /// page shown.
    Handle(usize, usize, Handle),
    PageNext,
    PagePrev,
    /// 0-based.
    Page(usize),
}

/// Parses `exec.<page>.<n>.<handle>`, `exec.cur.<n>.<handle>`,
/// `exec.page.next|prev|<page>` (1-based in the id). `page_shown` resolves
/// the `cur` aliases.
pub fn parse(id: &str, page_shown: usize) -> Option<ExecId> {
    let rest = id.strip_prefix("exec.")?;
    if let Some(p) = rest.strip_prefix("page.") {
        return match p {
            "next" => Some(ExecId::PageNext),
            "prev" => Some(ExecId::PagePrev),
            n => {
                let n: usize = n.parse().ok()?;
                (1..=EXEC_MAX_PAGES).contains(&n).then_some(ExecId::Page(n - 1))
            }
        };
    }
    let mut parts = rest.split('.');
    let (page, n, which) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let page = match page {
        "cur" => page_shown,
        p => p.parse::<usize>().ok().filter(|p| (1..=EXEC_MAX_PAGES).contains(p))? - 1,
    };
    let n = n.parse::<usize>().ok().filter(|n| (1..=EXEC_PER_PAGE).contains(n))? - 1;
    let handle = match which {
        "fader" => Handle::Fader,
        "button" => Handle::Button(0),
        "button2" => Handle::Button(1),
        _ => return None,
    };
    Some(ExecId::Handle(page, n, handle))
}

/// The registry entries (`controls.rs`): (id, French label, kind).
pub fn control_ids() -> Vec<(String, String, Handle)> {
    let mut out = Vec::new();
    let mut handles = |prefix: String, label: &dyn Fn(usize) -> String| {
        for n in 1..=EXEC_PER_PAGE {
            out.push((format!("{prefix}.{n}.fader"), format!("{} : curseur", label(n)), Handle::Fader));
            out.push((format!("{prefix}.{n}.button"), format!("{} : bouton 1", label(n)), Handle::Button(0)));
            out.push((format!("{prefix}.{n}.button2"), format!("{} : bouton 2", label(n)), Handle::Button(1)));
        }
    };
    for p in 1..=EXEC_MAX_PAGES {
        handles(format!("exec.{p}"), &|n| format!("Exécuteur {p}.{n}"));
    }
    handles("exec.cur".into(), &|n| format!("Exécuteur {n} (page affichée)"));
    out
}

/// A write to an `exec.*` control (from `controls::apply`). `None`: not an
/// executor id.
pub fn apply(s: &mut Shared, id: &str, input: ControlInput, src: Option<&'static str>, source: ControlSource) -> Option<Result<(), &'static str>> {
    let parsed = parse(id, s.executors.rt.page)?;
    let pages = s.executors.bank.pages.len();
    Some(match parsed {
        ExecId::PageNext => {
            s.executors.rt.page = (s.executors.rt.page + 1) % pages;
            Ok(())
        }
        ExecId::PagePrev => {
            s.executors.rt.page = (s.executors.rt.page + pages - 1) % pages;
            Ok(())
        }
        ExecId::Page(p) if p < pages => {
            s.executors.rt.page = p;
            Ok(())
        }
        ExecId::Page(_) => Err("cette page d'exécuteurs n'existe pas"),
        ExecId::Handle(p, _, _) if p >= pages => Err("cette page d'exécuteurs n'existe pas"),
        ExecId::Handle(p, n, Handle::Fader) => {
            let v = match input {
                ControlInput::Value(v) | ControlInput::Norm(v) if v.is_finite() => v.clamp(0.0, 1.0),
                _ => 0.0,
            };
            set_fader(s, p, n, v, src, source)
        }
        ExecId::Handle(p, n, Handle::Button(b)) => {
            let down = match input {
                ControlInput::Value(v) | ControlInput::Norm(v) => v >= 0.5,
            };
            press(s, p, n, b, down, src, source)
        }
    })
}

/// The current value of an `exec.*` control (LED feedback, pickup, the
/// page): a fader's level, whether a button is held.
pub fn current(s: &Shared, id: &str) -> Option<Value> {
    match parse(id, s.executors.rt.page)? {
        ExecId::Handle(p, n, Handle::Fader) => Some(json!(fader_value(s, p, n))),
        ExecId::Handle(p, n, Handle::Button(b)) => Some(json!(s.executors.rt.is_held(p, n, b))),
        _ => None,
    }
}

/// Where executor (page, n)'s fader is, 0..1: its object's level for a
/// layer or a group, its own position otherwise.
pub fn fader_value(s: &Shared, page: usize, n: usize) -> f32 {
    let Some(e) = s.executors.bank.get(page, n) else { return 0.0 };
    match (&e.target, e.fader) {
        (ExecTarget::Layer { n: l }, FaderFn::Level) => s.mixer.layer(*l).dimmer.clamp(0.0, 1.0),
        (ExecTarget::Layer { n: l }, FaderFn::Speed) => (s.mixer.layer(*l).modifiers.speed / 2.0).clamp(0.0, 1.0),
        (ExecTarget::GroupMaster { group }, _) => s.projection.master(*group).unwrap_or(1.0).clamp(0.0, 1.0),
        // A speed group: where its multiple (or the waiting one) sits on
        // the fader; a free BPM keeps the fader where it was put.
        (ExecTarget::SpeedGroup { n: g }, _) => match s.speed.target(*g) {
            Some(crate::speed::SpeedMode::Multiple { mult }) => speed_fader(mult),
            _ => s.executors.rt.faders.get(&(page, n)).copied().unwrap_or(0.5),
        },
        (ExecTarget::Cue { .. }, FaderFn::Level) => s.executors.rt.faders.get(&(page, n)).copied().unwrap_or(1.0),
        _ => s.executors.rt.faders.get(&(page, n)).copied().unwrap_or(0.0),
    }
}

/// A *Vitesse* fader (0..1) on a speed group: its nearest fifth, ×¼ at 0,
/// ×1 in the middle, ×4 at the top. Not a number: none.
fn speed_mult(v: f32) -> Option<f32> {
    v.is_finite().then(|| crate::speed::SPEED_MULTS[(v.clamp(0.0, 1.0) * 4.0).round() as usize])
}

/// Where a multiple sits on a *Vitesse* fader.
fn speed_fader(mult: f32) -> f32 {
    crate::speed::SPEED_MULTS.iter().position(|&m| m == mult).map_or(0.5, |i| i as f32 / 4.0)
}

/// The latched instance of `cue` is playing, or its launch waits (T-159).
fn cue_latched(s: &Shared, cue: &str) -> bool {
    s.deck.active.iter().any(|a| a.cue == cue && !a.held) || s.deck.pending.iter().any(|p| p.cue == cue)
}

fn set_fader(s: &mut Shared, page: usize, n: usize, v: f32, src: Option<&'static str>, source: ControlSource) -> Result<(), &'static str> {
    let Some(e) = s.executors.bank.get(page, n).cloned() else { return Err("exécuteur inconnu") };
    match (&e.target, e.fader) {
        (ExecTarget::Cue { id }, FaderFn::FadeIn) => {
            let before = fader_value(s, page, n);
            s.executors.rt.faders.insert((page, n), v);
            let engaged = s.executors.rt.is_engaged(page, n);
            if v > 0.0 && before <= 0.0 && !engaged {
                // Up from 0: the cue starts (unless it already plays, which
                // the fader then takes over).
                if !cue_latched(s, id) && !controls::press_cue_as(s, id, Some(ClickMode::Toggle), true, src, source) {
                    return Err("ce cue n'existe pas");
                }
                s.executors.rt.engaged.insert((page, n), true);
            } else if v <= 0.0 && engaged {
                // Back to 0: it stops (fading out with its transition).
                s.executors.rt.engaged.remove(&(page, n));
                if s.deck.active.iter().any(|a| a.cue == *id && !a.held) {
                    controls::press_cue_as(s, id, Some(ClickMode::Toggle), true, src, source);
                } else {
                    controls::stop_pending(s, id);
                }
            }
        }
        (ExecTarget::Layer { n: l }, FaderFn::Level) => {
            s.mixer.layer_mut(*l).dimmer = v;
            s.mixer_dirty = true;
        }
        (ExecTarget::Layer { n: l }, FaderFn::Speed) => {
            s.mixer.layer_mut(*l).modifiers.speed = v * 2.0;
            s.mixer_dirty = true;
        }
        (ExecTarget::GroupMaster { group }, _) => {
            if !s.projection.set_master(*group, v) {
                return Err("ce groupe de zones n'existe pas");
            }
            s.executors.rt.group_off.remove(group);
            s.groups_dirty = true;
        }
        (ExecTarget::SpeedGroup { n: g }, _) => {
            let mult = speed_mult(v).ok_or("valeur invalide")?;
            s.executors.rt.faders.insert((page, n), v);
            let (beat, t, bpb) = s.speed_clock();
            s.speed.set_mode(*g, crate::speed::SpeedMode::Multiple { mult }, beat, t, bpb)?;
            s.speed_dirty = true;
        }
        // A cue's level, the cue list, an empty executor: the position only.
        _ => {
            s.executors.rt.faders.insert((page, n), v);
        }
    }
    sync_levels(s);
    Ok(())
}

fn press(s: &mut Shared, page: usize, n: usize, b: usize, down: bool, src: Option<&'static str>, source: ControlSource) -> Result<(), &'static str> {
    let Some(e) = s.executors.bank.get(page, n).cloned() else { return Err("exécuteur inconnu") };
    let was = s.executors.rt.is_held(page, n, b);
    if down == was && down {
        // Auto-repeat or a second controller: one press.
        return Ok(());
    }
    s.executors.rt.held.insert((page, n, b), down);
    let f = e.buttons[b];
    let r = match (&e.target, f) {
        (_, ButtonFn::None) => Ok(()),
        (ExecTarget::Cue { id }, ButtonFn::Toggle) => {
            if down && !controls::press_cue_as(s, id, Some(ClickMode::Toggle), true, src, source) {
                Err("ce cue n'existe pas")
            } else {
                Ok(())
            }
        }
        // Held instances: they end on release. A cue already playing just
        // takes the level the button gives it (`sync_levels`).
        (ExecTarget::Cue { id }, ButtonFn::Flash | ButtonFn::Temp) => {
            if (down || was) && !controls::press_cue_as(s, id, Some(ClickMode::Flash), down, src, source) {
                Err("ce cue n'existe pas")
            } else {
                Ok(())
            }
        }
        (ExecTarget::Layer { n: l }, ButtonFn::Toggle) => {
            if down {
                let layer = s.mixer.layer_mut(*l);
                layer.mute = !layer.mute;
                s.mixer_dirty = true;
            }
            Ok(())
        }
        (ExecTarget::GroupMaster { group }, ButtonFn::Toggle) => {
            if down {
                toggle_group(s, *group)
            } else {
                Ok(())
            }
        }
        (ExecTarget::CueList, ButtonFn::Go) => {
            if down {
                crate::cuelist::go(s, source)
            } else {
                Ok(())
            }
        }
        // A macro (T-425): launched on the press, like its `macro.<id>.go`.
        (ExecTarget::Macro { id }, ButtonFn::Go) => {
            if down {
                crate::macros::start(s, id, source)
            } else {
                Ok(())
            }
        }
        // Flash / Noir tenu on a layer or a group, Noir tenu on a cue:
        // while held, read by the engine every frame (`layer_overrides`,
        // `group_overrides`, `sync_levels`).
        _ => Ok(()),
    };
    sync_levels(s);
    r
}

/// *Basculer* on a group master: off (remembering its level), or back.
fn toggle_group(s: &mut Shared, group: usize) -> Result<(), &'static str> {
    let Some(level) = s.projection.master(group) else { return Err("ce groupe de zones n'existe pas") };
    let next = if level > 0.0 {
        s.executors.rt.group_off.insert(group, level);
        0.0
    } else {
        s.executors.rt.group_off.remove(&group).unwrap_or(1.0)
    };
    s.projection.set_master(group, next);
    s.groups_dirty = true;
    Ok(())
}

/// Every executor of the bank with its (page, handle).
fn all(bank: &ExecutorBank) -> impl Iterator<Item = (usize, usize, &Executor)> {
    bank.pages.iter().enumerate().flat_map(|(p, page)| page.handles.iter().enumerate().map(move |(n, e)| (p, n, e)))
}

/// What the executors make of cue `cue`'s level: `None` when none has a
/// say (full level).
fn cue_level(s: &Shared, cue: &str) -> Option<f32> {
    let rt = &s.executors.rt;
    let (mut black, mut flash, mut level): (bool, bool, Option<f32>) = (false, false, None);
    for (p, n, e) in all(&s.executors.bank) {
        if !matches!(&e.target, ExecTarget::Cue { id } if id == cue) {
            continue;
        }
        let fader = fader_value(s, p, n);
        for (b, f) in e.buttons.iter().enumerate() {
            if !rt.is_held(p, n, b) {
                continue;
            }
            match f {
                ButtonFn::BlackHeld => black = true,
                ButtonFn::Flash => flash = true,
                ButtonFn::Temp => level = Some(level.map_or(fader, |l: f32| l.min(fader))),
                _ => {}
            }
        }
        let says = match e.fader {
            FaderFn::FadeIn => rt.is_engaged(p, n),
            FaderFn::Level => true,
            FaderFn::Speed => false,
        };
        if says {
            level = Some(level.map_or(fader, |l: f32| l.min(fader)));
        }
    }
    if black {
        Some(0.0)
    } else if flash {
        Some(1.0)
    } else {
        level.map(|l| l.clamp(0.0, 1.0))
    }
}

/// Every playing cue's `exec_level` from the executors, from scratch (an
/// executor reassigned or emptied lets its cue go back to full). Cues
/// fading out keep the level they stopped at.
pub fn sync_levels(s: &mut Shared) {
    let levels: Vec<f32> = s.deck.active.iter().map(|a| cue_level(s, &a.cue).unwrap_or(1.0)).collect();
    for (a, level) in s.deck.active.iter_mut().zip(levels) {
        a.exec_level = level;
    }
}

/// Once per engine frame, before the looks are read.
pub fn frame(s: &mut Shared) {
    sync_levels(s);
}

/// The colour gain of each drawn cue instance below full level (playing
/// or fading out), for the engine: applied to the rendered look before
/// the mixer and the safety stage.
pub fn gains(s: &Shared) -> HashMap<u64, f32> {
    s.deck
        .active
        .iter()
        .chain(s.deck.ending.iter().map(|e| &e.cue))
        .filter(|a| a.exec_level < 1.0)
        .map(|a| (a.id, if a.exec_level.is_finite() { a.exec_level.clamp(0.0, 1.0) } else { 0.0 }))
        .collect()
}

/// The held buttons of a function on objects matching `pick`.
fn held_on<'a>(s: &'a Shared, f: ButtonFn) -> impl Iterator<Item = &'a ExecTarget> + 'a {
    all(&s.executors.bank).filter(move |(p, n, e)| e.buttons.iter().enumerate().any(|(b, x)| *x == f && s.executors.rt.is_held(*p, *n, b))).map(|(_, _, e)| &e.target)
}

/// The mixer the engine draws with: *Flash* held on a layer puts it at
/// full and unmuted, *Noir tenu* mutes it, for as long as it is held.
/// Dimmers stay within 0..1; the stored mixer is untouched.
pub fn layer_overrides(s: &Shared, mixer: &mut crate::layers::Mixer) {
    let flashed: Vec<u8> = held_on(s, ButtonFn::Flash).filter_map(|t| if let ExecTarget::Layer { n } = t { Some(*n) } else { None }).collect();
    let black: Vec<u8> = held_on(s, ButtonFn::BlackHeld).filter_map(|t| if let ExecTarget::Layer { n } = t { Some(*n) } else { None }).collect();
    for n in flashed {
        let l = mixer.layer_mut(n);
        l.dimmer = 1.0;
        l.mute = false;
    }
    for n in black {
        mixer.layer_mut(n).mute = true;
    }
}

/// The group masters the engine draws with: *Flash* held = 100 %,
/// *Noir tenu* = 0, for as long as it is held.
pub fn group_overrides(s: &Shared, cfg: &mut crate::projection::ProjectionConfig) {
    for (f, level) in [(ButtonFn::Flash, 1.0), (ButtonFn::BlackHeld, 0.0)] {
        let groups: Vec<usize> = held_on(s, f).filter_map(|t| if let ExecTarget::GroupMaster { group } = t { Some(*group) } else { None }).collect();
        for g in groups {
            if let Some(group) = cfg.groups.get_mut(g.wrapping_sub(1)) {
                group.master = level;
            }
        }
    }
}

/// Every held button lets go (the page that held them is gone, like
/// `controls::release_held`): held cue instances end with it.
pub fn release_all(s: &mut Shared) {
    s.executors.rt.held.clear();
    sync_levels(s);
}

/// Whether executor (page, n)'s object is active: its cue plays or
/// waits, a button is held, its fader started its cue, or the cue list
/// it drives runs.
fn is_active(s: &Shared, page: usize, n: usize, e: &Executor) -> bool {
    let rt = &s.executors.rt;
    if (0..2).any(|b| rt.is_held(page, n, b)) || rt.is_engaged(page, n) {
        return true;
    }
    match &e.target {
        ExecTarget::Cue { id } => s.deck.active.iter().any(|a| a.cue == *id) || s.deck.pending.iter().any(|p| p.cue == *id),
        ExecTarget::CueList => s.cue_list.is_active(),
        ExecTarget::Macro { id } => s.macros.player.is_running(id),
        _ => false,
    }
}

/// `/api/frame` › `executors`: the page shown, its faders and buttons,
/// which executors of it are active, and which pages hold something
/// active (their tab is marked).
pub fn view(s: &Shared) -> Value {
    let rt = &s.executors.rt;
    let page = rt.page;
    let levels: Vec<f32> = (0..EXEC_PER_PAGE).map(|n| (fader_value(s, page, n) * 1000.0).round() / 1000.0).collect();
    let held: Vec<[bool; 2]> = (0..EXEC_PER_PAGE).map(|n| [rt.is_held(page, n, 0), rt.is_held(page, n, 1)]).collect();
    let active: Vec<bool> = (0..EXEC_PER_PAGE).map(|n| s.executors.bank.get(page, n).is_some_and(|e| is_active(s, page, n, e))).collect();
    let active_pages: Vec<bool> =
        s.executors.bank.pages.iter().enumerate().map(|(p, pg)| pg.handles.iter().enumerate().any(|(n, e)| is_active(s, p, n, e))).collect();
    json!({ "page": page, "rev": s.executors.rev, "levels": levels, "held": held, "active": active, "active_pages": active_pages })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;

    fn cue_of(s: &Shared, i: usize) -> String {
        s.presets[i].id.clone()
    }

    fn bank_with(handles: Vec<(usize, usize, Executor)>) -> ExecutorBank {
        let mut bank = ExecutorBank { pages: vec![ExecPage::default(), ExecPage::default()] };
        bank.sanitize();
        for (p, n, e) in handles {
            bank.pages[p].handles[n] = e;
        }
        bank
    }

    fn fade_in(cue: &str) -> Executor {
        Executor { target: ExecTarget::Cue { id: cue.into() }, fader: FaderFn::FadeIn, buttons: [ButtonFn::Flash, ButtonFn::Temp], ..Default::default() }
    }

    fn ctl(s: &mut Shared, id: &str, v: f32) {
        controls::apply(s, id, ControlInput::Value(v), true).unwrap_or_else(|e| panic!("{id}: {e:?}"));
    }

    fn latched<'a>(s: &'a Shared, cue: &str) -> Vec<&'a crate::cues::ActiveCue> {
        s.deck.active.iter().filter(|a| a.cue == cue && !a.held).collect()
    }

    #[test]
    fn the_default_bank_is_one_empty_page_and_old_files_load() {
        let bank = ExecutorBank::default();
        assert_eq!(bank.pages.len(), 1);
        assert_eq!(bank.pages[0].name, "Page 1");
        assert_eq!(bank.pages[0].handles.len(), EXEC_PER_PAGE);
        assert!(bank.pages[0].handles.iter().all(|h| *h == Executor::default()));
        let empty: ExecutorBank = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, bank);
        // A hand-edited file: too many pages, a short page, a function the
        // object can't run, a layer that doesn't exist.
        let mut odd: ExecutorBank = serde_json::from_value(json!({ "pages": [
            { "name": "  ", "handles": [{ "target": { "type": "layer", "n": 9 }, "buttons": ["go", "flash"] }] },
            { "handles": [{ "target": { "type": "cue", "id": "x" }, "fader": "speed", "buttons": ["go", "temp"] }] },
            {}, {}, {}, {}, {}, {}, {}, {}
        ]}))
        .unwrap();
        odd.sanitize();
        assert_eq!(odd.pages.len(), EXEC_MAX_PAGES);
        assert_eq!(odd.pages[0].name, "Page 1");
        assert_eq!(odd.pages[0].handles[0], Executor::default());
        assert_eq!(odd.pages[1].handles[0].fader, FaderFn::FadeIn);
        assert_eq!(odd.pages[1].handles[0].buttons, [ButtonFn::None, ButtonFn::Temp]);
    }

    #[test]
    fn validation_refuses_what_cannot_run() {
        let known = |id: &str| id == "a";
        let ok = bank_with(vec![(0, 0, fade_in("a")), (1, 2, Executor { target: ExecTarget::CueList, buttons: [ButtonFn::Go, ButtonFn::None], ..Default::default() })]);
        ok.validate(known, |id| id == "m1").unwrap();
        for bad in [
            Executor { target: ExecTarget::Cue { id: "zz".into() }, ..fade_in("a") },
            Executor { target: ExecTarget::Layer { n: 5 }, ..Default::default() },
            Executor { target: ExecTarget::Layer { n: 1 }, fader: FaderFn::FadeIn, ..Default::default() },
            Executor { target: ExecTarget::CueList, buttons: [ButtonFn::Flash, ButtonFn::None], ..Default::default() },
            Executor { target: ExecTarget::SpeedGroup { n: 5 }, fader: FaderFn::Speed, ..Default::default() },
            Executor { target: ExecTarget::SpeedGroup { n: 1 }, fader: FaderFn::Level, ..Default::default() },
            Executor { target: ExecTarget::Macro { id: "m".into() }, ..Default::default() },
            Executor { target: ExecTarget::Macro { id: "m1".into() }, buttons: [ButtonFn::Flash, ButtonFn::None], ..Default::default() },
            Executor { buttons: [ButtonFn::Toggle, ButtonFn::None], ..Default::default() },
        ] {
            let b = bank_with(vec![(0, 3, bad.clone())]);
            assert!(b.validate(known, |id| id == "m1").is_err(), "{bad:?}");
        }
        assert!(ExecutorBank { pages: Vec::new() }.validate(known, |_| true).is_err());
        assert!(ExecutorBank { pages: vec![ExecPage::default(); EXEC_MAX_PAGES + 1] }.validate(known, |_| true).is_err());
    }

    #[test]
    fn ids_parse_with_the_shown_page() {
        assert_eq!(parse("exec.3.8.fader", 0), Some(ExecId::Handle(2, 7, Handle::Fader)));
        assert_eq!(parse("exec.1.1.button2", 0), Some(ExecId::Handle(0, 0, Handle::Button(1))));
        assert_eq!(parse("exec.cur.2.button", 4), Some(ExecId::Handle(4, 1, Handle::Button(0))));
        assert_eq!(parse("exec.page.next", 0), Some(ExecId::PageNext));
        assert_eq!(parse("exec.page.8", 0), Some(ExecId::Page(7)));
        for bad in ["exec.9.1.fader", "exec.1.9.fader", "exec.0.1.fader", "exec.1.1.knob", "exec.page.9", "exec.1.1", "exec.1.1.fader.x", "layer.1.dimmer"] {
            assert_eq!(parse(bad, 0), None, "{bad}");
        }
    }

    #[test]
    fn registry_has_every_executor_control_external() {
        let s = shared();
        for p in 1..=EXEC_MAX_PAGES {
            for n in 1..=EXEC_PER_PAGE {
                for h in ["fader", "button", "button2"] {
                    let d = s.controls.get(&format!("exec.{p}.{n}.{h}")).unwrap_or_else(|| panic!("exec.{p}.{n}.{h}"));
                    assert!(d.external && d.group == "exec");
                }
            }
            assert!(s.controls.get(&format!("exec.page.{p}")).is_some());
        }
        for id in ["exec.page.next", "exec.page.prev", "exec.cur.1.fader", "exec.cur.8.button2"] {
            assert!(s.controls.get(id).is_some_and(|d| d.external && d.group == "exec"), "{id}");
        }
        assert!(matches!(s.controls.get("exec.1.1.fader").unwrap().kind, controls::ControlKind::Continuous { min: 0.0, max: 1.0, .. }));
        assert_eq!(s.controls.get("exec.1.1.button").unwrap().kind, controls::ControlKind::Momentary);
    }

    #[test]
    fn fade_in_starts_follows_and_stops_the_cue() {
        let mut s = shared();
        let a = cue_of(&s, 0);
        s.executors.set_bank(bank_with(vec![(0, 0, fade_in(&a))]));
        ctl(&mut s, "exec.1.1.fader", 0.4);
        let first = latched(&s, &a).iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(first.len(), 1, "0 → 0,4 starts the cue");
        assert!((latched(&s, &a)[0].exec_level - 0.4).abs() < 1e-6);
        ctl(&mut s, "exec.1.1.fader", 0.6);
        assert_eq!(latched(&s, &a).iter().map(|c| c.id).collect::<Vec<_>>(), first, "0,4 → 0,6: no restart");
        assert!((latched(&s, &a)[0].exec_level - 0.6).abs() < 1e-6);
        ctl(&mut s, "exec.1.1.fader", 0.4);
        assert_eq!(latched(&s, &a).len(), 1);
        ctl(&mut s, "exec.1.1.fader", 0.0);
        assert!(latched(&s, &a).is_empty(), "back to 0 stops it");
        assert!(!s.gate.is_armed());
        // Again from 0: a new start.
        ctl(&mut s, "exec.1.1.fader", 0.2);
        assert_eq!(latched(&s, &a).len(), 1);
        assert_ne!(latched(&s, &a)[0].id, first[0]);
    }

    #[test]
    fn a_cue_level_never_goes_above_one() {
        let mut s = shared();
        let a = cue_of(&s, 0);
        s.executors.set_bank(bank_with(vec![(0, 0, fade_in(&a))]));
        ctl(&mut s, "exec.1.1.fader", 0.5);
        for input in [ControlInput::Value(7.0), ControlInput::Norm(3.0), ControlInput::Value(f32::NAN), ControlInput::Value(-1.0)] {
            let _ = controls::apply(&mut s, "exec.1.1.fader", input, true);
            frame(&mut s);
            assert!(s.deck.active.iter().all(|c| (0.0..=1.0).contains(&c.exec_level)));
            assert!(gains(&s).values().all(|g| (0.0..=1.0).contains(g)));
        }
    }

    #[test]
    fn flash_temp_and_black_held_press_and_release() {
        let mut s = shared();
        let a = cue_of(&s, 0);
        let mut e = fade_in(&a);
        e.buttons = [ButtonFn::Flash, ButtonFn::Temp];
        let black = Executor { target: ExecTarget::Cue { id: a.clone() }, fader: FaderFn::FadeIn, buttons: [ButtonFn::BlackHeld, ButtonFn::None], ..Default::default() };
        s.executors.set_bank(bank_with(vec![(0, 0, e), (0, 1, black)]));
        // Flash: full level while held, whatever the fader (still 0).
        ctl(&mut s, "exec.1.1.button", 1.0);
        let held: Vec<f32> = s.deck.active.iter().filter(|c| c.cue == a).map(|c| c.exec_level).collect();
        assert_eq!(held, [1.0]);
        assert!(s.deck.active[0].held);
        ctl(&mut s, "exec.1.1.button", 0.0);
        assert!(s.deck.active.is_empty(), "released: gone");
        // Temp: on while held, at the fader's level.
        s.executors.rt.faders.insert((0, 0), 0.3);
        ctl(&mut s, "exec.1.1.button2", 1.0);
        assert_eq!(s.deck.active.len(), 1);
        assert!((s.deck.active[0].exec_level - 0.3).abs() < 1e-6);
        ctl(&mut s, "exec.1.1.button2", 0.0);
        assert!(s.deck.active.is_empty());
        // Flash over a cue running at 0,4 from the fader: 1 while held, back after.
        s.executors.rt.faders.clear();
        ctl(&mut s, "exec.1.1.fader", 0.4);
        ctl(&mut s, "exec.1.1.button", 1.0);
        assert_eq!(latched(&s, &a)[0].exec_level, 1.0);
        ctl(&mut s, "exec.1.1.button", 0.0);
        assert!((latched(&s, &a)[0].exec_level - 0.4).abs() < 1e-6);
        // Noir tenu: dark while held, back on release; the cue keeps playing.
        ctl(&mut s, "exec.1.2.button", 1.0);
        assert_eq!(latched(&s, &a)[0].exec_level, 0.0);
        assert_eq!(gains(&s).values().copied().collect::<Vec<_>>(), [0.0]);
        ctl(&mut s, "exec.1.2.button", 0.0);
        assert!((latched(&s, &a)[0].exec_level - 0.4).abs() < 1e-6);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_page_change_cuts_nothing_and_marks_the_page() {
        let mut s = shared();
        let a = cue_of(&s, 0);
        s.executors.set_bank(bank_with(vec![(0, 0, fade_in(&a))]));
        ctl(&mut s, "exec.1.1.fader", 0.7);
        ctl(&mut s, "exec.page.next", 1.0);
        assert_eq!(s.executors.rt.page, 1);
        assert_eq!(latched(&s, &a).len(), 1, "still playing");
        let v = view(&s);
        assert_eq!(v["page"], 1);
        assert_eq!(v["active_pages"], json!([true, false]));
        assert_eq!(v["levels"][0], 0.0, "page 2's fader 1 is at rest");
        // The alias points at the page shown: page 2's fader, an empty executor.
        ctl(&mut s, "exec.cur.1.fader", 0.9);
        assert!((latched(&s, &a)[0].exec_level - 0.7).abs() < 1e-6, "page 1 untouched");
        ctl(&mut s, "exec.page.prev", 1.0);
        assert_eq!(current(&s, "exec.cur.1.fader"), Some(json!(0.7_f32)), "back on page 1: the real value");
        ctl(&mut s, "exec.cur.1.fader", 0.0);
        assert!(latched(&s, &a).is_empty());
        // Wraps; a page that doesn't exist is refused.
        ctl(&mut s, "exec.page.prev", 1.0);
        assert_eq!(s.executors.rt.page, 1);
        assert!(controls::apply(&mut s, "exec.page.5", ControlInput::Value(1.0), true).is_err());
        assert!(controls::apply(&mut s, "exec.5.1.fader", ControlInput::Value(1.0), true).is_err());
    }

    /// T-419: a *Vitesse* executor on a speed group sets its multiple
    /// (×¼ at the bottom, ×1 in the middle, ×4 at the top) and reads it back.
    #[test]
    fn a_speed_group_executor_sets_the_multiple() {
        use crate::speed::SpeedMode;
        let mut s = shared();
        s.speed.change_on_bar = false;
        let e = Executor { target: ExecTarget::SpeedGroup { n: 1 }, fader: FaderFn::Speed, ..Default::default() };
        let bank = bank_with(vec![(0, 0, e)]);
        bank.validate(|_| true, |_| true).unwrap();
        s.executors.set_bank(bank);
        assert_eq!(current(&s, "exec.1.1.fader"), Some(json!(0.5)), "×1 in the middle");
        ctl(&mut s, "exec.1.1.fader", 0.3);
        assert_eq!(s.speed.groups[0], SpeedMode::Multiple { mult: 0.5 });
        assert_eq!(current(&s, "exec.1.1.fader"), Some(json!(0.25)));
        ctl(&mut s, "exec.1.1.fader", 1.0);
        assert_eq!(s.speed.groups[0], SpeedMode::Multiple { mult: 4.0 });
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn layers_and_groups_follow_their_object() {
        let mut s = shared();
        let layer = Executor { target: ExecTarget::Layer { n: 2 }, fader: FaderFn::Level, buttons: [ButtonFn::Toggle, ButtonFn::BlackHeld], ..Default::default() };
        let flash = Executor { target: ExecTarget::Layer { n: 3 }, fader: FaderFn::Speed, buttons: [ButtonFn::Flash, ButtonFn::None], ..Default::default() };
        s.executors.set_bank(bank_with(vec![(0, 0, layer), (0, 1, flash)]));
        ctl(&mut s, "exec.1.1.fader", 0.25);
        assert_eq!(s.mixer.layer(2).dimmer, 0.25);
        s.mixer.layer_mut(2).dimmer = 0.5;
        assert_eq!(current(&s, "exec.1.1.fader"), Some(json!(0.5)), "reads the layer");
        ctl(&mut s, "exec.1.1.button", 1.0);
        assert!(s.mixer.layer(2).mute);
        ctl(&mut s, "exec.1.1.button", 0.0);
        ctl(&mut s, "exec.1.1.button", 1.0);
        assert!(!s.mixer.layer(2).mute, "toggled back");
        // Noir tenu: the drawn mixer only, while held.
        ctl(&mut s, "exec.1.1.button2", 1.0);
        let mut m = s.mixer.clone();
        layer_overrides(&s, &mut m);
        assert!(m.layer(2).mute && !s.mixer.layer(2).mute);
        ctl(&mut s, "exec.1.1.button2", 0.0);
        // Speed: the fader is the layer's speed / 2.
        ctl(&mut s, "exec.1.2.fader", 0.25);
        assert_eq!(s.mixer.layer(3).modifiers.speed, 0.5);
        s.mixer.layer_mut(3).dimmer = 0.2;
        ctl(&mut s, "exec.1.2.button", 1.0);
        let mut m = s.mixer.clone();
        layer_overrides(&s, &mut m);
        assert_eq!(m.layer(3).dimmer, 1.0, "Flash: full while held");
        ctl(&mut s, "exec.1.2.button", 0.0);
        let mut m = s.mixer.clone();
        layer_overrides(&s, &mut m);
        assert_eq!(m.layer(3).dimmer, 0.2);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn release_all_lets_go_of_held_buttons() {
        let mut s = shared();
        let a = cue_of(&s, 0);
        s.executors.set_bank(bank_with(vec![(0, 0, fade_in(&a))]));
        ctl(&mut s, "exec.1.1.button", 1.0);
        controls::release_held(&mut s);
        assert!(s.deck.active.is_empty());
        assert_eq!(current(&s, "exec.1.1.button"), Some(json!(false)));
    }

    #[test]
    fn reassigning_an_executor_gives_its_cue_back_full_level() {
        let mut s = shared();
        let a = cue_of(&s, 0);
        s.executors.set_bank(bank_with(vec![(0, 0, fade_in(&a))]));
        ctl(&mut s, "exec.1.1.fader", 0.3);
        s.executors.set_bank(bank_with(vec![]));
        frame(&mut s);
        assert_eq!(latched(&s, &a)[0].exec_level, 1.0);
        assert!(gains(&s).is_empty());
    }

    #[test]
    fn no_executor_control_arms() {
        let mut s = shared();
        let a = cue_of(&s, 0);
        s.executors.set_bank(bank_with(vec![
            (0, 0, fade_in(&a)),
            (0, 1, Executor { target: ExecTarget::CueList, buttons: [ButtonFn::Go, ButtonFn::None], ..Default::default() }),
        ]));
        let ids: Vec<String> = s.controls.list().iter().filter(|d| d.group == "exec").map(|d| d.id.clone()).collect();
        for id in &ids {
            for v in [1.0, 0.0, 0.5] {
                let _ = controls::apply(&mut s, id, ControlInput::Value(v), true);
                assert!(!s.gate.is_armed(), "{id}");
            }
        }
    }

    /// T-208: a hardware fader on `exec.cur.1.fader` doesn't jump after a
    /// page change: it catches the new page's value first.
    #[test]
    fn a_hardware_fader_picks_up_after_a_page_change() {
        use crate::midi::decode::MidiMsg;
        let mut s = shared();
        s.deck.multi = true;
        let a = cue_of(&s, 0);
        let b = cue_of(&s, 1);
        s.executors.set_bank(bank_with(vec![(0, 0, fade_in(&a)), (1, 0, fade_in(&b))]));
        let port = "Test";
        let profile = crate::midi::profile::Profile::parse(
            r#"{ "name": "Exec", "mappings": [{ "input": { "kind": "cc", "channel": 0, "number": 7 }, "target": "exec.cur.1.fader", "mode": "absolute", "pickup": true }] }"#,
        )
        .unwrap();
        s.midi.store.save_profile(None, "exec-test", profile).unwrap();
        let d = s.midi.device_mut(port);
        d.profile = "exec-test".into();
        d.connected = true;
        let send = |s: &mut Shared, v: u8| {
            let ev = crate::midi::MidiEvent { port: port.into(), msg: MidiMsg::Cc { channel: 0, number: 7, value: v }, at: std::time::Instant::now() };
            crate::midi::engine::handle_batch(s, &[ev]);
            crate::midi::engine::frame(s, std::time::Instant::now());
        };
        send(&mut s, 0);
        send(&mut s, 64);
        assert_eq!(latched(&s, &a).len(), 1, "page 1: the fader starts cue a");
        ctl(&mut s, "exec.page.next", 1.0);
        // Page 2's fader is at 0; the hardware one at 64: it must come down first.
        send(&mut s, 70);
        assert!(latched(&s, &b).is_empty(), "no jump on page 2");
        send(&mut s, 1);
        send(&mut s, 30);
        assert_eq!(latched(&s, &b).len(), 1, "caught at 0, then it follows");
        assert_eq!(latched(&s, &a).len(), 1, "page 1's cue still plays");
    }
}
