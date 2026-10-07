//! FX grid (T-146): effects played on top of whatever plays, without
//! touching the cues. Four lines of eight cells; a cell is a saved set of
//! modulators (lfo.rs) and/or live modifier changes (live.rs), with a
//! length in beats. One cell per line plays at a time (*Un par ligne*):
//! starting another cell of the line replaces it. A *ponctuel* (drop)
//! cell plays once over its length, rising and falling, then stops by
//! itself. Each line's *Action* fader blends its effect linearly from
//! nothing (0) to the full effect (1); lines add up.
//!
//! Like the LFOs, an effect only moves the engine's *copies* of the look
//! and the master modifiers, before calibration and the whole safety
//! chain: modulators go through the same allow-list (`lfo::modulatable`,
//! never arming, safety, calibration or caps), brightness can only dim,
//! and a strobe in a cell runs before the strobe limiter like the live
//! strobe. Nothing here arms, and an emergency stop ends every effect.
//!
//! The cells, the *Action* faders, the destination and the click mode are
//! saved to `fx.json`; what plays is memory only (nothing plays at start).

use crate::controls::ControlRegistry;
use crate::cues::Quantize;
use crate::engine::Settings;
use crate::lfo::{self, Modulator, Wave};
use crate::live::{ColorOverride, ColorParams, LiveModifiers, Mirror, Rate, Strobe, MAX_PRISM};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const LINES: usize = 4;
pub const CELLS: usize = 8;
/// Modulators one cell may hold.
pub const MAX_CELL_MODULATORS: usize = 8;
const MAX_NAME: usize = 40;
/// Discrete changes (strobe, mirror, prism, colour mode) switch in at
/// this share of the effect: half-way up a drop, or *Action* at 50 %.
const SWITCH_AT: f32 = 0.5;

/// One FX cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxCell {
    pub name: String,
    /// Modulators on any modulatable control (lfo.rs allow-list).
    pub modulators: Vec<Modulator>,
    /// Changes to the master modifiers: every field that differs from the
    /// neutral value (`LiveModifiers::default`) is applied, see `overlay`.
    pub modifiers: Option<LiveModifiers>,
    /// How long a drop lasts (`Beats(n)`: n beats; `Hz(f)`: 1/f s).
    pub length: Rate,
    /// *Ponctuel*: plays once over `length`, then stops by itself.
    pub one_shot: bool,
}

impl Default for FxCell {
    fn default() -> Self {
        Self { name: String::new(), modulators: Vec::new(), modifiers: None, length: Rate::Beats(4.0), one_shot: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxLine {
    /// `CELLS` cells, `None` = empty.
    pub cells: Vec<Option<FxCell>>,
    /// *Action*, 0..1: 0 = no effect, 1 = the full effect.
    pub action: f32,
}

impl Default for FxLine {
    fn default() -> Self {
        Self { cells: vec![None; CELLS], action: 1.0 }
    }
}

/// Where the effects go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxDest {
    /// The master: everything that plays.
    #[default]
    Master,
    /// The cue launched last (the master when no cue plays).
    Cue,
}

pub const DESTS: [FxDest; 2] = [FxDest::Master, FxDest::Cue];
pub const DEST_LABELS: [&str; 2] = ["Maître", "Cue sélectionné"];

/// What a click on a cell does (drops always play once).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxClick {
    /// A click starts the effect, a second click stops it.
    #[default]
    Toggle,
    /// The effect plays while the cell is held.
    Flash,
}

pub const CLICKS: [FxClick; 2] = [FxClick::Toggle, FxClick::Flash];
pub const CLICK_LABELS: [&str; 2] = ["Bascule", "Flash"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxGrid {
    /// `LINES` lines.
    pub lines: Vec<FxLine>,
    pub dest: FxDest,
    pub click: FxClick,
}

impl Default for FxGrid {
    fn default() -> Self {
        Self { lines: vec![FxLine::default(); LINES], dest: FxDest::default(), click: FxClick::default() }
    }
}

/// The cells only (`POST /api/fx`, the edit history): `LINES` rows of
/// `CELLS`.
pub type Cells = Vec<Vec<Option<FxCell>>>;

/// A cell playing on a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playing {
    pub cell: usize,
    /// Started by a held cell (*Flash*): its release stops it.
    held: bool,
    quantize: Quantize,
    /// (seconds, beats) it starts at, set by the first engine frame after
    /// the press (on the next boundary when launches are quantised).
    start: Option<(f64, f64)>,
}

/// The engine's clock for one frame.
#[derive(Clone, Copy, Debug)]
pub struct FxClock {
    /// Seconds and beats of the frame.
    pub t: f64,
    pub beat: f64,
    /// The modulation clock (stands still while « Figer » is held).
    pub mod_t: f64,
    pub mod_beat: f64,
    pub bpm: f64,
    pub beats_per_bar: u8,
    /// Speed groups (T-419): per group (0 = master, 1..=4 = A..D), its
    /// beat on the modulation clock and its beats per master beat.
    /// `None` = everything on the master clock.
    pub groups: Option<[(f64, f64); crate::speed::SPEED_GROUPS + 1]>,
}

impl FxClock {
    /// Group `g`'s beat for a modulator: looping, the group's own beat;
    /// a drop (`since` = master beats since it started), its share of them.
    fn group_beat(&self, g: u8, since: Option<f64>) -> f64 {
        let (beat, ratio) = self.groups.and_then(|gs| gs.get(g as usize).copied()).unwrap_or((self.mod_beat, 1.0));
        match since {
            Some(b) => b * ratio,
            None => beat,
        }
    }
}

fn fin(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

fn sanitize_length(r: Rate) -> Rate {
    match r {
        Rate::Beats(b) => Rate::Beats(fin(b, 4.0).clamp(0.25, 64.0)),
        Rate::Hz(hz) => Rate::Hz(fin(hz, 0.5).clamp(1.0 / 60.0, 20.0)),
    }
}

/// A cell's modifiers within their ranges: the momentary ones released
/// (an effect never holds « Noir » or « Figer »), the colour override
/// rebuilt from its settings.
fn sanitize_modifiers(mut m: LiveModifiers) -> LiveModifiers {
    let d = LiveModifiers::default();
    m.release_momentary();
    m.rot_reverse = false;
    m.sanitize_effects();
    m.brightness = fin(m.brightness, 1.0).clamp(0.0, 1.0);
    m.size = fin(m.size, 1.0).clamp(0.0, 2.0);
    m.size_x = fin(m.size_x, 1.0).clamp(-2.0, 2.0);
    m.size_y = fin(m.size_y, 1.0).clamp(-2.0, 2.0);
    m.pos_x = fin(m.pos_x, 0.0).clamp(-1.0, 1.0);
    m.pos_y = fin(m.pos_y, 0.0).clamp(-1.0, 1.0);
    m.rot_angle = m.rot_angle.map(|a| fin(a, 0.0).clamp(-180.0, 180.0));
    m.rot_speed = m.rot_speed.map(|v| fin(v, 0.0).clamp(-720.0, 720.0));
    m.perspective = fin(m.perspective, d.perspective).clamp(0.0, 1.0);
    m.speed = fin(m.speed, 1.0).clamp(0.0, 4.0);
    m.scan = d.scan;
    m.color_params.hue = fin(m.color_params.hue, 0.0).rem_euclid(360.0);
    m.color_params.spread = fin(m.color_params.spread, 1.0).clamp(0.0, 4.0);
    m.color = m.color_params.build(m.color.mode_index());
    m
}

impl FxCell {
    /// Numbers within their ranges (NaN and infinities → defaults).
    fn sanitized(mut self) -> Self {
        self.name = self.name.trim().chars().take(MAX_NAME).collect();
        self.modulators = self.modulators.into_iter().take(MAX_CELL_MODULATORS).map(Modulator::sanitized).collect();
        self.modifiers = self.modifiers.map(sanitize_modifiers).filter(|m| *m != LiveModifiers::default());
        self.length = sanitize_length(self.length);
        self
    }

    /// Whether it changes anything at all.
    pub fn is_empty(&self) -> bool {
        self.modifiers.is_none() && !self.modulators.iter().any(|m| m.enabled && m.depth > 0.0)
    }
}

/// `CELLS` per line, `LINES` lines: extra ones dropped, missing ones empty.
fn shape_cells(mut cells: Cells) -> Cells {
    cells.truncate(LINES);
    cells.resize(LINES, Vec::new());
    for line in &mut cells {
        line.truncate(CELLS);
        line.resize(CELLS, None);
    }
    cells
}

impl FxGrid {
    pub fn cells(&self) -> Cells {
        self.lines.iter().map(|l| l.cells.clone()).collect()
    }

    /// The right shape, every number in range; cells whose modulators
    /// aren't valid any more lose those modulators.
    fn sanitized(mut self, reg: &ControlRegistry) -> Self {
        let cells = shape_cells(self.cells());
        self.lines.truncate(LINES);
        self.lines.resize(LINES, FxLine::default());
        for (line, cells) in self.lines.iter_mut().zip(cells) {
            line.cells = cells
                .into_iter()
                .map(|c| {
                    c.map(|mut c| {
                        c.modulators.retain(|m| m.part.is_none() && lfo::modulatable(reg, &m.target).is_some());
                        c.sanitized()
                    })
                })
                .collect();
            line.action = fin(line.action, 1.0).clamp(0.0, 1.0);
        }
        self
    }
}

/// The cells' modulators all on modulatable controls, names and counts
/// within their limits.
pub fn validate(cells: &Cells, reg: &ControlRegistry) -> Result<()> {
    if cells.len() > LINES || cells.iter().any(|l| l.len() > CELLS) {
        bail!("{LINES} lignes de {CELLS} effets au maximum");
    }
    for cell in cells.iter().flatten().flatten() {
        if cell.modulators.len() > MAX_CELL_MODULATORS {
            bail!("« {} » : {MAX_CELL_MODULATORS} modulateurs au maximum par effet", cell.name);
        }
        if cell.modulators.iter().any(|m| m.part.is_some()) {
            bail!("« {} » : un effet ne module pas une partie de figure (panneau Modulateurs)", cell.name);
        }
        if let Some(m) = cell.modulators.iter().find(|m| lfo::modulatable(reg, &m.target).is_none()) {
            bail!("« {} » : « {} » ne peut pas être modulé", cell.name, m.target);
        }
    }
    Ok(())
}

/// The grid and what plays on it.
pub struct FxStore {
    path: Option<PathBuf>,
    grid: FxGrid,
    playing: [Option<Playing>; LINES],
    /// Each line's share of its effect on the last frame (0..1).
    levels: [f32; LINES],
    /// Lines whose launch waits for its boundary (T-159).
    waiting: [bool; LINES],
    /// An *Action*, the destination or the click mode changed: saved by
    /// the engine at most once a second.
    pub dirty: bool,
}

impl FxStore {
    /// A missing file starts with our starter effects; a damaged one is
    /// quarantined (T-402) and starts empty.
    pub fn load_or_create(path: PathBuf, reg: &ControlRegistry) -> Self {
        let grid = if path.exists() { crate::load_json::<FxGrid>(&path) } else { starter_grid() };
        Self { path: Some(path), grid: grid.sanitized(reg), playing: [None; LINES], levels: [0.0; LINES], waiting: [false; LINES], dirty: false }
    }

    /// Never written (tests).
    #[cfg(test)]
    pub fn in_memory(reg: &ControlRegistry) -> Self {
        Self { path: None, grid: starter_grid().sanitized(reg), playing: [None; LINES], levels: [0.0; LINES], waiting: [false; LINES], dirty: false }
    }

    pub fn grid(&self) -> &FxGrid {
        &self.grid
    }

    /// `save` without waiting for the disk: the engine's once-a-second
    /// save, written by the writer thread after any earlier `save` (T-482).
    pub fn save_later(&mut self) {
        self.dirty = false;
        if let Some(path) = &self.path {
            crate::datafile::save_json_later(path, &self.grid);
        }
    }

    pub fn save(&mut self) -> Result<()> {
        self.dirty = false;
        let Some(path) = &self.path else { return Ok(()) };
        let json = serde_json::to_string_pretty(&self.grid).context("failed to serialize the FX grid")?;
        crate::datafile::replace(path, json.as_bytes()).with_context(|| format!("failed to write {}", path.display()))
    }

    /// Replaces the cells (checked, numbers clamped) and saves. *Action*,
    /// destination and click mode stay. A playing cell emptied stops.
    pub fn set_cells(&mut self, cells: Cells, reg: &ControlRegistry) -> Result<()> {
        validate(&cells, reg)?;
        self.replace_cells(cells, reg);
        self.save()
    }

    /// `set_cells` in memory only (the caller checked them).
    pub fn replace_cells(&mut self, cells: Cells, reg: &ControlRegistry) {
        let mut grid = self.grid.clone();
        for (line, cells) in grid.lines.iter_mut().zip(shape_cells(cells)) {
            line.cells = cells;
        }
        self.grid = grid.sanitized(reg);
        for (line, p) in self.playing.iter_mut().enumerate() {
            if p.is_some_and(|p| self.grid.lines[line].cells[p.cell].is_none()) {
                *p = None;
            }
        }
    }

    /// The cell at `line`, `cell` (0-based), if any.
    pub fn cell(&self, line: usize, cell: usize) -> Option<&FxCell> {
        self.grid.lines.get(line)?.cells.get(cell)?.as_ref()
    }

    /// The cell playing on `line`.
    pub fn active(&self, line: usize) -> Option<usize> {
        self.playing.get(line).copied().flatten().map(|p| p.cell)
    }

    pub fn action(&self, line: usize) -> f32 {
        self.grid.lines.get(line).map_or(0.0, |l| l.action)
    }

    pub fn set_action(&mut self, line: usize, v: f32) {
        if let Some(l) = self.grid.lines.get_mut(line) {
            l.action = fin(v, 1.0).clamp(0.0, 1.0);
            self.dirty = true;
        }
    }

    pub fn set_dest(&mut self, dest: FxDest) {
        self.grid.dest = dest;
        self.dirty = true;
    }

    pub fn set_click(&mut self, click: FxClick) {
        self.grid.click = click;
        self.dirty = true;
    }

    /// A cell's button went down (`down`) or up. A drop starts (again) on
    /// each press; otherwise *Bascule* starts or stops it on a press and
    /// *Flash* plays it while held. Starting a cell replaces the line's
    /// effect. Launches wait for the boundary `quantize` asks for, like
    /// the cues (T-159). False for an empty cell.
    pub fn press(&mut self, line: usize, cell: usize, down: bool, quantize: Quantize) -> bool {
        let Some(c) = self.cell(line, cell) else { return false };
        let one_shot = c.one_shot;
        let flash = self.grid.click == FxClick::Flash && !one_shot;
        let current = self.playing[line];
        let same = current.is_some_and(|p| p.cell == cell);
        if !down {
            if same && current.is_some_and(|p| p.held) {
                self.playing[line] = None;
            }
            return true;
        }
        if same && !one_shot && !flash {
            self.playing[line] = None;
        } else {
            self.playing[line] = Some(Playing { cell, held: flash, quantize, start: None });
        }
        true
    }

    /// *Arrêter FX*: every line stops at once.
    pub fn stop_all(&mut self) {
        self.playing = [None; LINES];
        self.levels = [0.0; LINES];
        self.waiting = [false; LINES];
    }

    pub fn any_playing(&self) -> bool {
        self.playing.iter().any(Option::is_some)
    }

    /// The engine's frame: launches get their start (on their boundary),
    /// finished drops stop, and each line's share is computed.
    pub fn frame(&mut self, c: &FxClock) {
        for line in 0..LINES {
            self.waiting[line] = false;
            let Some(mut p) = self.playing[line] else {
                self.levels[line] = 0.0;
                continue;
            };
            let Some(cell) = self.cell(line, p.cell) else {
                self.playing[line] = None;
                self.levels[line] = 0.0;
                continue;
            };
            let (one_shot, length) = (cell.one_shot, cell.length);
            let (t0, b0) = *p.start.get_or_insert_with(|| {
                let b0 = p.quantize.boundary(c.beat, c.beats_per_bar).unwrap_or(c.beat);
                (c.t + (b0 - c.beat) * 60.0 / c.bpm.max(1.0), b0)
            });
            self.playing[line] = Some(p);
            let progress = length.cycles(c.t, c.beat) - length.cycles(t0, b0);
            let env = if progress < 0.0 {
                self.waiting[line] = true;
                0.0
            } else if !one_shot {
                1.0
            } else if progress >= 1.0 {
                self.playing[line] = None;
                0.0
            } else {
                drop_envelope(progress as f32)
            };
            self.levels[line] = env * self.grid.lines[line].action;
        }
    }

    /// Every playing line, in order, on the engine's copies of the look
    /// and the master modifiers (call `frame` first). Lines add up.
    pub fn apply(&self, reg: &ControlRegistry, settings: &mut Settings, live: &mut LiveModifiers, c: &FxClock) {
        for line in 0..LINES {
            let share = self.levels[line];
            let Some(p) = self.playing[line] else { continue };
            let (Some(cell), Some((t0, b0))) = (self.cell(line, p.cell), p.start) else { continue };
            if share <= 0.0 {
                continue;
            }
            if let Some(m) = &cell.modifiers {
                overlay(live, m, share, c.bpm, c.beats_per_bar);
            }
            // A drop's waves start with it; a looping effect stays on the
            // tempo clock, in phase with the LFOs.
            let (t, since) = if cell.one_shot { (c.mod_t - t0, Some(c.mod_beat - b0)) } else { (c.mod_t, None) };
            lfo::modulate_grouped(&cell.modulators, reg, settings, live, t, &|g| c.group_beat(g, since), share);
        }
    }

    /// For `/api/state` and the pushed state: per line, the cell playing,
    /// whether it waits for its boundary, and its share now.
    pub fn view(&self) -> Value {
        let lines: Vec<Value> = (0..LINES)
            .map(|line| {
                let p = self.playing[line];
                json!({
                    "active": p.map(|p| p.cell),
                    "waiting": p.is_some_and(|p| p.start.is_none()) || self.waiting[line],
                    "level": self.levels[line],
                    "action": self.action(line),
                })
            })
            .collect();
        json!({ "lines": lines, "dest": self.grid.dest, "click": self.grid.click })
    }
}

/// A drop's rise and fall over its length, `p` in 0..1: up over the first
/// quarter, full, then down over the second half. Smooth at both ends, 0
/// at 0 and 1.
pub fn drop_envelope(p: f32) -> f32 {
    let smooth = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    smooth(p / 0.25) * (1.0 - smooth((p - 0.5) / 0.5))
}

/// A cell's modifier changes on the master copy, at `share` (0..1) of the
/// full effect. Only the fields that differ from neutral act:
/// - size, per-axis size, animation speed and brightness are multiplied
///   (brightness only ever dims), from ×1 at 0 to the cell's value at 1;
/// - position, rotation angles and rotation speeds are added (a synced
///   speed in turns per bar is converted to the master's unit);
/// - perspective moves linearly towards the cell's value;
/// - « Points visibles » and « Pointillés » only take light away;
/// - the strobe, mirror, prism and colour mode switch in at `SWITCH_AT`.
///
/// Everything ends within its range.
pub fn overlay(live: &mut LiveModifiers, fx: &LiveModifiers, share: f32, bpm: f64, beats_per_bar: u8) {
    let d = LiveModifiers::default();
    let k = share.clamp(0.0, 1.0);
    if k <= 0.0 {
        return;
    }
    let lerp = |from: f32, to: f32| from + (to - from) * k;
    let mul = |v: &mut f32, f: f32, lo: f32, hi: f32| *v = (*v * lerp(1.0, f)).clamp(lo, hi);
    if fx.brightness < 1.0 {
        live.brightness = (live.brightness * lerp(1.0, fx.brightness)).clamp(0.0, 1.0);
    }
    if fx.size != d.size {
        mul(&mut live.size, fx.size, 0.0, 2.0);
    }
    if fx.size_x != d.size_x {
        mul(&mut live.size_x, fx.size_x, -2.0, 2.0);
    }
    if fx.size_y != d.size_y {
        mul(&mut live.size_y, fx.size_y, -2.0, 2.0);
    }
    if fx.speed != d.speed {
        mul(&mut live.speed, fx.speed, 0.0, 4.0);
    }
    live.pos_x = (live.pos_x + fx.pos_x * k).clamp(-1.0, 1.0);
    live.pos_y = (live.pos_y + fx.pos_y * k).clamp(-1.0, 1.0);
    // Degrees per second in one turn per bar.
    let per_turn_per_bar = 360.0 * (bpm.max(1.0) as f32 / 60.0) / beats_per_bar.max(1) as f32;
    for axis in 0..3 {
        live.rot_angle[axis] += fx.rot_angle[axis] * k;
        if fx.rot_speed[axis] != 0.0 {
            let deg_s = if fx.rot_sync { fx.rot_speed[axis] * per_turn_per_bar } else { fx.rot_speed[axis] };
            let add = if live.rot_sync { deg_s / per_turn_per_bar } else { deg_s };
            live.rot_speed[axis] = (live.rot_speed[axis] + add * k).clamp(-720.0, 720.0);
        }
    }
    if fx.perspective != d.perspective {
        live.perspective = lerp(live.perspective, fx.perspective).clamp(0.0, 1.0);
    }
    if fx.trace < 1.0 {
        live.trace = live.trace.min(lerp(1.0, fx.trace));
    }
    if fx.dots > 0.0 {
        live.dots = live.dots.max(fx.dots * k);
    }
    if k >= SWITCH_AT {
        if fx.strobe.on {
            live.strobe = Strobe { on: true, hold: live.strobe.hold, rate: fx.strobe.rate(), duty: fx.strobe.duty() };
        }
        if fx.mirror != Mirror::None {
            live.mirror = fx.mirror;
        }
        if fx.prism > 1 {
            live.prism = fx.prism.clamp(1, MAX_PRISM);
        }
        if fx.color != ColorOverride::Normal {
            live.color = fx.color.clone();
            live.color_params = fx.color_params.clone();
        }
    }
    live.sanitize_effects();
}

fn lfo_on(target: &str, wave: Wave, beats: f32, depth: f32) -> Modulator {
    Modulator { target: target.into(), wave, rate: Rate::Beats(beats), depth, ..Default::default() }
}

fn looping(name: &str, modulators: Vec<Modulator>, modifiers: Option<LiveModifiers>) -> Option<FxCell> {
    Some(FxCell { name: name.into(), modulators, modifiers, ..Default::default() })
}

fn drop_fx(name: &str, beats: f32, modulators: Vec<Modulator>, modifiers: Option<LiveModifiers>) -> Option<FxCell> {
    Some(FxCell { name: name.into(), modulators, modifiers, length: Rate::Beats(beats), one_shot: true })
}

/// Our sixteen starter effects (names and settings are ours): a line of
/// movement, a line of colour and light effects, a line of drops, and an
/// empty line for the operator's own.
pub fn starter_grid() -> FxGrid {
    let m = LiveModifiers::default();
    let colour = |index: usize, params: ColorParams| LiveModifiers { color: params.build(index), color_params: params, ..m.clone() };
    let movement = vec![
        looping("Respiration", vec![lfo_on("master.size", Wave::Sine, 4.0, 0.15)], None),
        looping("Balancier", vec![lfo_on("master.rot_z.angle", Wave::Sine, 4.0, 0.12)], None),
        looping("Pulsation", vec![lfo_on("master.size", Wave::SawDown, 1.0, 0.12)], None),
        looping("Vague X", vec![lfo_on("master.pos_x", Wave::Sine, 2.0, 0.3)], None),
        looping("Vague Y", vec![Modulator { phase: 0.25, ..lfo_on("master.pos_y", Wave::Sine, 2.0, 0.25) }], None),
        looping("Tourbillon", Vec::new(), Some(LiveModifiers { rot_speed: [0.0, 0.0, 1.0], rot_sync: true, ..m.clone() })),
        looping("Zoom au temps", vec![lfo_on("master.size", Wave::Triangle, 2.0, 0.2)], None),
        looping("Bascule 3D", vec![lfo_on("master.rot_y.angle", Wave::Sine, 8.0, 0.25)], None),
    ];
    let effects = vec![
        looping("Arc-en-ciel", Vec::new(), Some(colour(4, ColorParams { spread: 1.0, rainbow_rate: Rate::Beats(4.0), ..Default::default() }))),
        looping("Chenillard rapide", Vec::new(), Some(colour(5, ColorParams { chase_step: Rate::Beats(0.25), ..Default::default() }))),
        looping("Strobe 1/2", Vec::new(), Some(LiveModifiers { strobe: Strobe { on: true, hold: false, rate: Rate::Beats(0.5), duty: 0.5 }, ..m.clone() })),
        looping("Tracé", vec![Modulator { offset: -1.0, ..lfo_on("master.trace", Wave::SawUp, 4.0, 1.0) }], None),
        looping("Prisme 4", Vec::new(), Some(LiveModifiers { prism: 4, ..m.clone() })),
        looping("Miroir XY", Vec::new(), Some(LiveModifiers { mirror: Mirror::Xy, ..m.clone() })),
        None,
        None,
    ];
    let drops = vec![
        drop_fx("Choc", 2.0, Vec::new(), Some(LiveModifiers { size: 1.5, ..m.clone() })),
        drop_fx("Plongée", 2.0, Vec::new(), Some(LiveModifiers { size: 0.3, ..m.clone() })),
        drop_fx("Tour complet", 4.0, vec![lfo_on("master.rot_z.angle", Wave::SawUp, 4.0, 1.0)], None),
        drop_fx("Éclair blanc", 1.0, Vec::new(), Some(colour(1, ColorParams { rgb: [255, 255, 255], ..Default::default() }))),
        None,
        None,
        None,
        None,
    ];
    let line = |cells: Vec<Option<FxCell>>| FxLine { cells, action: 1.0 };
    FxGrid { lines: vec![line(movement), line(effects), line(drops), FxLine::default()], ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::{self, LiveState};
    use crate::patterns::Point;
    use crate::presets;

    fn reg() -> ControlRegistry {
        ControlRegistry::build(&presets::catalog())
    }

    fn store() -> FxStore {
        FxStore::in_memory(&reg())
    }

    fn clock(beat: f64) -> FxClock {
        // 120 BPM: a beat is half a second.
        FxClock { t: beat / 2.0, beat, mod_t: beat / 2.0, mod_beat: beat, bpm: 120.0, beats_per_bar: 4, groups: None }
    }

    /// The master copy after the grid's frame at `beat`.
    fn master_at(st: &mut FxStore, beat: f64) -> (Settings, LiveModifiers) {
        let c = clock(beat);
        st.frame(&c);
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        st.apply(&reg(), &mut settings, &mut live, &c);
        (settings, live)
    }

    fn cell(name: &str, modifiers: LiveModifiers) -> Option<FxCell> {
        Some(FxCell { name: name.into(), modifiers: Some(modifiers), ..Default::default() })
    }

    #[test]
    fn the_starter_grid_has_sixteen_valid_effects_of_our_own() {
        let g = starter_grid();
        let names: Vec<&str> = g.lines.iter().flat_map(|l| l.cells.iter().flatten()).map(|c| c.name.as_str()).collect();
        assert!(names.len() >= 16, "at least 16 starter effects: {names:?}");
        validate(&g.cells(), &reg()).unwrap();
        assert_eq!(g.clone().sanitized(&reg()), g, "already in range");
        assert!(g.lines.iter().flat_map(|l| l.cells.iter().flatten()).all(|c| !c.is_empty()));
        assert!(g.lines[2].cells.iter().flatten().all(|c| c.one_shot), "line 3: drops");
    }

    #[test]
    fn one_effect_per_line_and_a_second_click_stops_it() {
        let mut st = store();
        assert!(st.press(0, 0, true, Quantize::Off));
        assert!(st.press(0, 0, false, Quantize::Off), "the release of a toggle is ignored");
        assert_eq!(st.active(0), Some(0));
        st.press(0, 3, true, Quantize::Off);
        assert_eq!(st.active(0), Some(3), "the other cell replaces it");
        st.press(0, 3, true, Quantize::Off);
        assert_eq!(st.active(0), None);
        assert!(!st.press(3, 0, true, Quantize::Off), "an empty cell does nothing");
        assert_eq!(st.active(3), None);
    }

    #[test]
    fn flash_plays_while_held() {
        let mut st = store();
        st.set_click(FxClick::Flash);
        st.press(0, 1, true, Quantize::Off);
        assert_eq!(st.active(0), Some(1));
        st.press(0, 1, false, Quantize::Off);
        assert_eq!(st.active(0), None);
    }

    #[test]
    fn action_zero_changes_nothing_and_one_is_the_full_effect() {
        let mut st = store();
        st.replace_cells(vec![vec![cell("x", LiveModifiers { size: 1.5, pos_x: 0.4, prism: 3, ..Default::default() })]], &reg());
        st.press(0, 0, true, Quantize::Off);
        st.set_action(0, 0.0);
        let (s0, m0) = master_at(&mut st, 1.0);
        assert_eq!((s0, m0), (Settings::default(), LiveModifiers::default()));
        st.set_action(0, 1.0);
        let (_, m1) = master_at(&mut st, 1.1);
        assert!((m1.size - 1.5).abs() < 1e-6 && (m1.pos_x - 0.4).abs() < 1e-6 && m1.prism == 3);
        st.set_action(0, 0.5);
        let (_, half) = master_at(&mut st, 1.2);
        assert!((half.size - 1.25).abs() < 1e-6, "linear blend: {}", half.size);
        assert!((half.pos_x - 0.2).abs() < 1e-6);
    }

    #[test]
    fn action_scales_the_modulators_linearly() {
        let mut st = store();
        st.press(0, 3, true, Quantize::Off); // Vague X: pos_x sine over 2 beats, depth 0.3
        let full = master_at(&mut st, 0.5).1.pos_x;
        assert!((full - 0.3).abs() < 1e-4, "{full}");
        st.set_action(0, 0.25);
        let quarter = master_at(&mut st, 0.5).1.pos_x;
        assert!((quarter - 0.075).abs() < 1e-4, "{quarter}");
    }

    #[test]
    fn a_two_beat_drop_ends_by_itself_after_two_beats() {
        let mut st = store();
        st.replace_cells(vec![vec![Some(FxCell { name: "d".into(), modifiers: Some(LiveModifiers { size: 2.0, ..Default::default() }), length: Rate::Beats(2.0), one_shot: true, ..Default::default() })]], &reg());
        let frame = 120.0 / 60.0 / 60.0; // beats per frame at 120 BPM, 60 fps
        let start = 10.3;
        st.press(0, 0, true, Quantize::Off);
        let mut beat = start;
        let mut peak: f32 = 1.0;
        while st.active(0).is_some() {
            let (_, m) = master_at(&mut st, beat);
            peak = peak.max(m.size);
            beat += frame;
            assert!(beat < start + 3.0, "never ended");
        }
        let lasted = beat - frame - start;
        assert!((lasted - 2.0).abs() <= frame + 1e-9, "lasted {lasted} beats");
        assert!(peak > 1.95, "reached the full effect: {peak}");
        assert_eq!(master_at(&mut st, beat).1, LiveModifiers::default());
    }

    #[test]
    fn the_drop_envelope_rises_and_falls() {
        assert_eq!(drop_envelope(0.0), 0.0);
        assert!(drop_envelope(1.0).abs() < 1e-6);
        assert!((drop_envelope(0.3) - 1.0).abs() < 1e-6);
        assert!(drop_envelope(0.1) < drop_envelope(0.2) && drop_envelope(0.8) < drop_envelope(0.6));
    }

    #[test]
    fn quantised_launches_wait_for_the_next_beat() {
        let mut st = store();
        st.press(0, 0, true, Quantize::Beat);
        let (_, waiting) = master_at(&mut st, 4.5);
        assert_eq!(waiting, LiveModifiers::default(), "waits for beat 5");
        assert_eq!(st.view()["lines"][0]["waiting"], true);
        let (_, on) = master_at(&mut st, 5.5);
        assert_ne!(on.size, 1.0);
    }

    #[test]
    fn two_lines_add_up() {
        let mut st = store();
        st.replace_cells(
            vec![vec![cell("a", LiveModifiers { pos_x: 0.2, ..Default::default() })], vec![cell("b", LiveModifiers { pos_x: 0.3, size: 0.5, ..Default::default() })]],
            &reg(),
        );
        st.press(0, 0, true, Quantize::Off);
        st.press(1, 0, true, Quantize::Off);
        let (_, m) = master_at(&mut st, 1.0);
        assert!((m.pos_x - 0.5).abs() < 1e-6 && (m.size - 0.5).abs() < 1e-6);
        st.stop_all();
        assert_eq!(master_at(&mut st, 1.1).1, LiveModifiers::default());
    }

    #[test]
    fn the_overlay_never_brightens_and_stays_in_range() {
        let fx = LiveModifiers { brightness: 0.5, size: 2.0, size_x: -2.0, pos_x: 1.0, rot_speed: [720.0; 3], speed: 4.0, trace: 0.2, dots: 1.0, ..Default::default() };
        let mut live = LiveModifiers { brightness: 0.8, size: 2.0, pos_x: 0.9, ..Default::default() };
        overlay(&mut live, &fx, 1.0, 120.0, 4);
        assert!((live.brightness - 0.4).abs() < 1e-6);
        assert!(live.size <= 2.0 && live.pos_x <= 1.0 && live.rot_speed.iter().all(|v| v.abs() <= 720.0) && live.speed <= 4.0);
        assert!((live.trace - 0.2).abs() < 1e-6 && live.dots == 1.0);
        // A synced turn per bar on a free master at 120 BPM, 4/4: 180 °/s.
        let mut free = LiveModifiers::default();
        overlay(&mut free, &LiveModifiers { rot_speed: [0.0, 0.0, 1.0], rot_sync: true, ..Default::default() }, 1.0, 120.0, 4);
        assert!((free.rot_speed[2] - 180.0).abs() < 1e-3, "{}", free.rot_speed[2]);
    }

    #[test]
    fn a_grid_strobe_only_cuts_light_like_the_live_strobe() {
        let mut st = store();
        st.press(1, 2, true, Quantize::Off); // Strobe 1/2
        let (_, m) = master_at(&mut st, 0.0);
        assert!(m.strobe.on, "the cell's strobe is on the master copy, before the limiter");
        let points: Vec<Point> = (0..50).map(|i| Point { x: i as f32 / 50.0 - 0.5, y: 0.0, r: 1.0, g: 0.5, b: 0.2 }).collect();
        let mut state = LiveState::default();
        for beat in [0.1, 0.3] {
            state.set_clock(beat / 2.0, beat);
            let out = live::apply(&points, &m, &state, &[]);
            assert!(out.iter().zip(&points).all(|(a, b)| a.r <= b.r && a.g <= b.g && a.b <= b.b));
        }
    }

    #[test]
    fn bad_numbers_and_unknown_targets_are_cleaned_and_the_momentary_ones_released() {
        let bad = FxCell {
            name: "x".repeat(100),
            modulators: vec![lfo_on("transport.arm", Wave::Square, 1.0, 1.0), Modulator { depth: f32::NAN, ..lfo_on("master.size", Wave::Sine, 1.0, 1.0) }],
            modifiers: Some(LiveModifiers { black: true, freeze: true, size: f32::INFINITY, prism: 200, ..Default::default() }),
            length: Rate::Beats(f32::NAN),
            one_shot: true,
        };
        assert!(validate(&vec![vec![Some(bad.clone())]], &reg()).is_err(), "transport.arm can't be modulated");
        let mut st = store();
        st.replace_cells(vec![vec![Some(bad)]], &reg());
        let c = st.cell(0, 0).unwrap();
        assert_eq!(c.name.chars().count(), MAX_NAME);
        assert_eq!(c.modulators.len(), 1);
        assert_eq!(c.modulators[0].target, "master.size");
        let m = c.modifiers.as_ref().unwrap();
        assert!(!m.black && !m.freeze && m.size == 1.0 && m.prism == MAX_PRISM);
        assert_eq!(c.length, Rate::Beats(4.0));
    }

    #[test]
    fn the_grid_survives_a_restart_and_old_files_load() {
        let dir = std::env::temp_dir().join(format!("laser-studio-fx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fx.json");
        let mut st = FxStore::load_or_create(path.clone(), &reg());
        assert_eq!(st.grid(), &starter_grid(), "a first start gets the starter effects");
        st.set_action(1, 0.4);
        st.set_dest(FxDest::Cue);
        st.set_cells(vec![vec![cell("Mien", LiveModifiers { size: 0.7, ..Default::default() })]], &reg()).unwrap();
        st.press(0, 0, true, Quantize::Off);
        let again = FxStore::load_or_create(path.clone(), &reg());
        assert_eq!(again.grid(), st.grid());
        assert_eq!(again.active(0), None, "nothing plays at start");
        std::fs::write(&path, r#"{"lines":[{"cells":[{"name":"Vieux"}]}]}"#).unwrap();
        let old = FxStore::load_or_create(path, &reg());
        assert_eq!(old.cell(0, 0).unwrap().name, "Vieux");
        assert_eq!(old.grid().lines.len(), LINES);
        assert!(old.grid().lines.iter().all(|l| l.cells.len() == CELLS && l.action == 1.0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
