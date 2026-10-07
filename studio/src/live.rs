//! Master live modifiers: what the laserist changes constantly while a
//! cue plays - size, position, rotation (with speed presets and tempo
//! sync), animation speed, colour (fixed, hue, palette, rainbow, chase),
//! the live effects of T-142 (strobe, visible points, dotted lines,
//! mirror, prism, freeze, momentary black) and a master dimmer. Applied
//! to the rendered look, before calibration and safety, so a live move
//! can never push the beam past the calibration clamp or into a safety
//! zone, and the strobe limiter and static-beam guard still have the last
//! word on every effect.

use crate::patterns::Point;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveModifiers {
    /// Master dimmer 0..1.
    pub brightness: f32,
    /// Overall size 0..2.
    pub size: f32,
    /// Per-axis size -2..2 (negative flips).
    pub size_x: f32,
    pub size_y: f32,
    /// Offset -1..1.
    pub pos_x: f32,
    pub pos_y: f32,
    /// Fixed rotation per axis (X, Y, Z), degrees.
    pub rot_angle: [f32; 3],
    /// Rotation speed per axis: degrees per second, or turns per bar when
    /// `rot_sync` is on.
    pub rot_speed: [f32; 3],
    pub rot_sync: bool,
    /// Momentary: reverses rotation direction while held.
    pub rot_reverse: bool,
    /// Depth of the 3D effect for X/Y rotations, 0..1.
    pub perspective: f32,
    /// Animation speed multiplier for generators, 0..4 (0 freezes).
    pub speed: f32,
    /// Live recolouring of whatever the cue draws.
    pub color: ColorOverride,
    /// Every colour setting, including those of the modes not in use, so
    /// switching mode back and forth brings the operator's values back.
    pub color_params: ColorParams,
    /// Live strobe (T-142): latched (`on`) or held (`hold`).
    pub strobe: Strobe,
    /// « Points visibles »: share of the lit path drawn, from its start, 0..1.
    pub trace: f32,
    /// « Pointillés »: 0 = solid, 1 = widely spaced dots. Only removes light.
    pub dots: f32,
    /// Mirrored copies of the figure.
    pub mirror: Mirror,
    /// « Prisme »: copies turned around the centre, 1 (off) ..= `MAX_PRISM`.
    pub prism: u8,
    /// Momentary « Figer »: animation and modulation time stand still while
    /// held (the engine reads it; the static-beam guard still runs after).
    pub freeze: bool,
    /// Momentary « Noir »: a dark output while held. A dimmer, not a
    /// disarm: the arm state is never touched.
    pub black: bool,
    /// « Vitesse de balayage » (T-171): × 0.5..1.5 on the scan presets'
    /// steps (`scan::ScanPresetStore::resolve`), bounded by the output's
    /// scan speed range. Not geometry: the picture stays the same.
    /// Absent in older files: 100 %.
    pub scan: f32,
}

impl Default for LiveModifiers {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            size: 1.0,
            size_x: 1.0,
            size_y: 1.0,
            pos_x: 0.0,
            pos_y: 0.0,
            rot_angle: [0.0; 3],
            rot_speed: [0.0; 3],
            rot_sync: false,
            rot_reverse: false,
            perspective: 0.3,
            speed: 1.0,
            color: ColorOverride::Normal,
            color_params: ColorParams::default(),
            strobe: Strobe::default(),
            trace: 1.0,
            dots: 0.0,
            mirror: Mirror::None,
            prism: 1,
            freeze: false,
            black: false,
            scan: 1.0,
        }
    }
}

impl LiveModifiers {
    /// True when nothing but the colour and the effects differs from the
    /// defaults.
    fn geometry_is_identity(&self) -> bool {
        let d = Self::default();
        let geometry = Self {
            color: ColorOverride::Normal,
            color_params: d.color_params.clone(),
            strobe: d.strobe,
            trace: d.trace,
            dots: d.dots,
            mirror: d.mirror,
            prism: d.prism,
            freeze: d.freeze,
            black: d.black,
            scan: d.scan,
            ..self.clone()
        };
        geometry == d
    }

    /// Releases the momentary effects (« Figer », « Noir », strobe held):
    /// they never outlive the hand holding them - not in a saved file, a
    /// stored cue or a timeline event.
    pub fn release_momentary(&mut self) {
        self.freeze = false;
        self.black = false;
        self.strobe.hold = false;
    }

    /// The effects within their ranges (values from a client, an old file,
    /// a mangled event).
    pub fn sanitize_effects(&mut self) {
        let d = Self::default();
        self.trace = if self.trace.is_finite() { self.trace.clamp(0.0, 1.0) } else { d.trace };
        self.dots = if self.dots.is_finite() { self.dots.clamp(0.0, 1.0) } else { d.dots };
        self.prism = self.prism.clamp(1, MAX_PRISM);
        self.scan = if self.scan.is_finite() { self.scan.clamp(crate::scan::MIN_MASTER_SCAN, crate::scan::MAX_MASTER_SCAN) } else { d.scan };
        self.strobe.rate = self.strobe.rate();
        self.strobe.duty = self.strobe.duty();
    }
}

/// Live strobe (T-142): the output is lit for `duty` of every cycle of
/// `rate`, its phase on the clock (`Rate::cycles`), so a synced strobe
/// flashes exactly on the tempo's subdivisions. It only ever cuts light,
/// and runs before the strobe limiter (T-101), which still turns a strobe
/// kept too fast for too long into steady output.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Strobe {
    /// Latched (« Stroboscope »).
    pub on: bool,
    /// Momentary (« Flash strobe »), while held.
    pub hold: bool,
    /// Free (Hz, `STROBE_MIN_HZ..=STROBE_MAX_HZ`) or synced (beats per
    /// flash, 1/16..1).
    pub rate: Rate,
    /// Lit share of a cycle, `STROBE_MIN_DUTY..=STROBE_MAX_DUTY`.
    pub duty: f32,
}

pub const STROBE_MIN_HZ: f32 = 1.0;
pub const STROBE_MAX_HZ: f32 = 25.0;
pub const STROBE_MIN_DUTY: f32 = 0.1;
pub const STROBE_MAX_DUTY: f32 = 0.9;
/// Synced strobe choices: flashes per beat (each 1/1 .. 1/16 of a beat).
pub const STROBE_DIVISIONS: [f32; 5] = [1.0, 2.0, 4.0, 8.0, 16.0];
/// « Cadence » choices: free (the Hz fader), then the synced divisions.
pub const STROBE_SYNC_LABELS: [&str; 6] = ["Libre (Hz)", "1/1", "1/2", "1/4", "1/8", "1/16"];

impl Default for Strobe {
    fn default() -> Self {
        Self { on: false, hold: false, rate: Rate::Hz(8.0), duty: 0.5 }
    }
}

impl Strobe {
    pub fn active(&self) -> bool {
        self.on || self.hold
    }

    /// The rate within its range (a bad number = the default).
    pub fn rate(&self) -> Rate {
        match self.rate {
            Rate::Hz(hz) if hz.is_finite() => Rate::Hz(hz.clamp(STROBE_MIN_HZ, STROBE_MAX_HZ)),
            Rate::Beats(b) if b.is_finite() && b > 0.0 => Rate::Beats(b.clamp(1.0 / 16.0, 1.0)),
            Rate::Hz(_) => Strobe::default().rate,
            Rate::Beats(_) => Rate::Beats(0.25),
        }
    }

    pub fn duty(&self) -> f32 {
        if self.duty.is_finite() {
            self.duty.clamp(STROBE_MIN_DUTY, STROBE_MAX_DUTY)
        } else {
            Strobe::default().duty
        }
    }

    /// Whether the strobe lets light through at `t` seconds, `beat` beats.
    pub fn lit_at(&self, t: f64, beat: f64) -> bool {
        !self.active() || self.rate().cycles(t, beat).rem_euclid(1.0) < self.duty() as f64
    }

    /// Index in `STROBE_SYNC_LABELS` (0 = free).
    pub fn sync_index(&self) -> usize {
        match self.rate() {
            Rate::Hz(_) => 0,
            Rate::Beats(b) => STROBE_DIVISIONS.iter().position(|d| (1.0 / d - b).abs() < 1e-4).map_or(0, |i| i + 1),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mirror {
    #[default]
    None,
    /// A copy flipped left-right.
    X,
    /// A copy flipped top-bottom.
    Y,
    /// Both, and the copy flipped both ways: four in all.
    Xy,
}

pub const MIRROR_LABELS: [&str; 4] = ["Aucun", "X", "Y", "XY"];
pub const MIRRORS: [Mirror; 4] = [Mirror::None, Mirror::X, Mirror::Y, Mirror::Xy];
pub const MAX_PRISM: u8 = 8;
/// « Pointillés » at 1: dark points after each lit pair.
const MAX_DOT_GAP: f32 = 12.0;
/// Fewest points a mirror or prism copy keeps when the copies share the
/// figure's point budget.
const MIN_COPY_POINTS: usize = 48;

/// A speed that is either free-running or locked to the tempo clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rate {
    /// Cycles (or steps) per second.
    Hz(f32),
    /// Beats per cycle (or step), read from the tempo clock.
    Beats(f32),
}

impl Rate {
    /// Cycles elapsed at `t` seconds, `beat` beats. A pure function of the
    /// clock, so synced effects land exactly on the beat and never drift.
    pub fn cycles(self, t: f64, beat: f64) -> f64 {
        match self {
            Rate::Hz(hz) => t * hz.max(0.0) as f64,
            Rate::Beats(b) if b > 0.0 => beat / b as f64,
            Rate::Beats(_) => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteMode {
    /// Each colour becomes the closest palette colour.
    #[default]
    Nearest,
    /// The next palette colour at every colour change along the path.
    Step,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChaseSpread {
    /// The whole frame is one colour.
    Whole,
    /// Each stroke (run of lit points) takes the next colour.
    #[default]
    Stroke,
    /// Each lit point takes the next colour.
    Point,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColorOverride {
    /// The cue's own colours.
    #[default]
    Normal,
    /// One colour for every lit point, keeping each point's intensity.
    Fixed { rgb: [u8; 3] },
    /// Keeps saturation and value, replaces the hue (degrees).
    Hue { hue: f32 },
    /// `palette` indexes the built-in palettes first, then the user ones.
    Palette { palette: usize, mode: PaletteMode, offset: usize },
    /// Hue along the path (`spread` cycles over the frame), scrolling at `rate`.
    Rainbow { spread: f32, rate: Rate },
    /// Palette colours advancing one step every `step`.
    Chase { palette: usize, step: Rate, spread: ChaseSpread },
}

pub const COLOR_MODE_LABELS: [&str; 6] = ["Normal", "Fixe", "Teinte", "Palette", "Arc-en-ciel", "Chenillard"];

impl ColorOverride {
    /// Index in `COLOR_MODE_LABELS`.
    pub fn mode_index(&self) -> usize {
        match self {
            ColorOverride::Normal => 0,
            ColorOverride::Fixed { .. } => 1,
            ColorOverride::Hue { .. } => 2,
            ColorOverride::Palette { .. } => 3,
            ColorOverride::Rainbow { .. } => 4,
            ColorOverride::Chase { .. } => 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorParams {
    pub rgb: [u8; 3],
    /// Degrees 0..360.
    pub hue: f32,
    pub palette: usize,
    pub palette_mode: PaletteMode,
    pub offset: usize,
    /// Rainbow cycles over the frame, 0..4.
    pub spread: f32,
    pub rainbow_rate: Rate,
    pub chase_step: Rate,
    pub chase_spread: ChaseSpread,
}

impl Default for ColorParams {
    fn default() -> Self {
        Self {
            rgb: [255, 0, 0],
            hue: 0.0,
            palette: 0,
            palette_mode: PaletteMode::Nearest,
            offset: 0,
            spread: 1.0,
            rainbow_rate: Rate::Beats(4.0),
            chase_step: Rate::Beats(1.0),
            chase_spread: ChaseSpread::Stroke,
        }
    }
}

impl ColorParams {
    /// The override for mode `index` (see `COLOR_MODE_LABELS`) with these settings.
    pub fn build(&self, index: usize) -> ColorOverride {
        match index {
            1 => ColorOverride::Fixed { rgb: self.rgb },
            2 => ColorOverride::Hue { hue: self.hue },
            3 => ColorOverride::Palette { palette: self.palette, mode: self.palette_mode, offset: self.offset },
            4 => ColorOverride::Rainbow { spread: self.spread, rate: self.rainbow_rate },
            5 => ColorOverride::Chase { palette: self.palette, step: self.chase_step, spread: self.chase_spread },
            _ => ColorOverride::Normal,
        }
    }
}

/// « Pas » choices for chases and synced rainbows, in beats.
pub const COLOR_STEPS_BEATS: [f32; 6] = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0];
pub const COLOR_STEP_LABELS: [&str; 6] = ["1/8", "1/4", "1/2", "1", "2", "4"];

/// A named set of 1..=16 colours. The one palette type of the app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub colors: Vec<[u8; 3]>,
}

pub const MAX_PALETTE_COLORS: usize = 16;
pub const MAX_USER_PALETTES: usize = 8;

/// Our own built-in palettes (names and colours chosen by us).
const BUILTIN: [(&str, &[[u8; 3]]); 8] = [
    ("Froid", &[[0, 255, 255], [0, 64, 255], [128, 0, 255], [160, 220, 255]]),
    ("Chaud", &[[255, 0, 0], [255, 96, 0], [255, 180, 0], [255, 0, 96]]),
    ("Feu", &[[255, 0, 0], [255, 64, 0], [255, 140, 0], [255, 220, 0]]),
    ("Océan", &[[0, 40, 255], [0, 160, 255], [0, 255, 200], [0, 255, 120]]),
    ("Néon", &[[255, 0, 255], [0, 255, 255], [0, 255, 0], [255, 255, 0]]),
    ("Forêt", &[[0, 255, 0], [80, 200, 0], [0, 160, 60], [180, 255, 0]]),
    ("Tricolore", &[[0, 0, 255], [255, 255, 255], [255, 0, 0]]),
    ("Blanc pur", &[[255, 255, 255]]),
];

/// Labels of the palette choice control: the built-ins, then the user slots.
pub const PALETTE_LABELS: [&str; 16] = [
    "Froid", "Chaud", "Feu", "Océan", "Néon", "Forêt", "Tricolore", "Blanc pur", "Perso 1", "Perso 2", "Perso 3", "Perso 4",
    "Perso 5", "Perso 6", "Perso 7", "Perso 8",
];

pub fn builtin_palettes() -> Vec<Palette> {
    BUILTIN.iter().map(|(name, colors)| Palette { name: name.to_string(), colors: colors.to_vec() }).collect()
}

/// Colours of palette `index` (built-ins first, then user palettes).
fn palette_colors(index: usize, user: &[Palette]) -> Option<&[[u8; 3]]> {
    let colors = match index.checked_sub(BUILTIN.len()) {
        None => BUILTIN[index].1,
        Some(i) => user.get(i)?.colors.as_slice(),
    };
    (!colors.is_empty()).then_some(colors)
}

/// User palettes, saved to `palettes.json` on every change.
pub struct PaletteStore {
    path: PathBuf,
    palettes: Vec<Palette>,
}

impl PaletteStore {
    /// A missing file starts an empty store; a damaged or invalid one too,
    /// once moved to the quarantine folder (T-402).
    pub fn load_or_create(path: PathBuf) -> Self {
        let palettes: Vec<Palette> = crate::load_json(&path);
        let palettes = match validate_palettes(&palettes) {
            Ok(()) => palettes,
            Err(e) => {
                crate::datafile::set_aside(&path, &crate::datafile::beside(&path), &format!("palettes invalides : {e}"), false);
                Vec::new()
            }
        };
        Self { path, palettes }
    }

    pub fn list(&self) -> &[Palette] {
        &self.palettes
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Replaces every user palette in memory only (already validated); the
    /// caller saves the file (project open, T-286).
    pub fn replace_in_memory(&mut self, palettes: Vec<Palette>) {
        self.palettes = palettes;
    }

    /// Replaces every user palette and saves.
    pub fn set(&mut self, palettes: Vec<Palette>) -> Result<()> {
        validate_palettes(&palettes)?;
        let json = serde_json::to_string_pretty(&palettes).context("failed to serialize palettes")?;
        crate::datafile::replace(&self.path, json.as_bytes()).with_context(|| format!("failed to write {}", self.path.display()))?;
        self.palettes = palettes;
        Ok(())
    }
}

pub fn validate_palettes(palettes: &[Palette]) -> Result<()> {
    if palettes.len() > MAX_USER_PALETTES {
        bail!("{MAX_USER_PALETTES} palettes utilisateur au maximum");
    }
    for p in palettes {
        if p.name.trim().is_empty() || p.name.chars().count() > 40 {
            bail!("nom de palette invalide : « {} »", p.name);
        }
        if !(1..=MAX_PALETTE_COLORS).contains(&p.colors.len()) {
            bail!("la palette « {} » doit avoir de 1 à {MAX_PALETTE_COLORS} couleurs", p.name);
        }
    }
    Ok(())
}

/// Rotation speed presets (Stop, Lent, Moyen, Rapide): degrees per second
/// in free mode, turns per bar in tempo-sync mode.
pub const ROT_PRESETS_FREE: [f32; 4] = [0.0, 30.0, 90.0, 270.0];
pub const ROT_PRESETS_SYNC: [f32; 4] = [0.0, 0.25, 1.0, 2.0];
pub const ROT_PRESET_LABELS: [&str; 4] = ["Stop", "Lent", "Moyen", "Rapide"];

/// Time-based state: accumulated rotation per axis, in degrees, and the
/// clock that colour effects read.
#[derive(Default)]
pub struct LiveState {
    spin: [f32; 3],
    /// Seconds and tempo-clock beats of the frame being drawn.
    time: f64,
    beat: f64,
    /// While « Figer » is held: the clock colour effects stay on.
    frozen: Option<(f64, f64)>,
}

impl LiveState {
    /// Set the clock for this frame: `t` seconds, `beat` = `TempoClock::beat_at(t)`.
    pub fn set_clock(&mut self, t: f64, beat: f64) {
        self.time = t;
        self.beat = beat;
    }

    /// « Figer » (T-142): colour effects read `clock` (seconds, beats)
    /// instead of the frame's own while it is `Some`. The strobe keeps the
    /// real clock.
    pub fn freeze_colours(&mut self, clock: Option<(f64, f64)>) {
        self.frozen = clock;
    }

    fn colour_clock(&self) -> (f64, f64) {
        self.frozen.unwrap_or((self.time, self.beat))
    }

    /// Advance the rotations by `dt` seconds. `bpm` and `beats_per_bar`
    /// turn tempo-synced speeds (turns per bar) into degrees per second.
    pub fn advance(&mut self, m: &LiveModifiers, dt: f32, bpm: f64, beats_per_bar: u8) {
        let direction = if m.rot_reverse { -1.0 } else { 1.0 };
        for axis in 0..3 {
            let deg_per_s = if m.rot_sync {
                m.rot_speed[axis] * 360.0 * (bpm as f32 / 60.0) / beats_per_bar.max(1) as f32
            } else {
                m.rot_speed[axis]
            };
            self.spin[axis] = (self.spin[axis] + direction * deg_per_s * dt).rem_euclid(360.0);
        }
    }

    pub fn angles(&self, m: &LiveModifiers) -> [f32; 3] {
        [0, 1, 2].map(|a| m.rot_angle[a] + self.spin[a])
    }
}

/// Geometry, mirror and prism copies, colour, then the effects that only
/// remove light (visible points, dotted lines, strobe, black) and the
/// master dimmer. `user` are the user palettes (palette indexes past the
/// built-ins point into it).
pub fn apply(points: &[Point], m: &LiveModifiers, st: &LiveState, user: &[Palette]) -> Vec<Point> {
    let out = if m.geometry_is_identity() && st.spin == [0.0; 3] { points.to_vec() } else { transform(points, m, st) };
    let mut out = copies(out, m.mirror, m.prism);
    recolor(&mut out, &m.color, st, user);
    trace(&mut out, m.trace);
    dots(&mut out, m.dots);
    if m.black || !m.strobe.lit_at(st.time, st.beat) {
        black_out(&mut out);
    }
    if m.brightness != 1.0 {
        let gain = m.brightness.clamp(0.0, 1.0);
        for p in &mut out {
            p.r *= gain;
            p.g *= gain;
            p.b *= gain;
        }
    }
    out
}

/// Every point dark, positions kept (« Noir », a strobe's off phase).
pub fn black_out(points: &mut [Point]) {
    for p in points {
        (p.r, p.g, p.b) = (0.0, 0.0, 0.0);
    }
}

/// Mirror and prism copies, around the centre of the frame, one after the
/// other. Each copy starts with blanked travel from where the previous
/// one ended (the beam goes dark before it moves), so no copy is ever
/// joined by a lit line. The copies share the figure's point budget.
fn copies(points: Vec<Point>, mirror: Mirror, prism: u8) -> Vec<Point> {
    let flips: &[(f32, f32)] = match mirror {
        Mirror::None => &[(1.0, 1.0)],
        Mirror::X => &[(1.0, 1.0), (-1.0, 1.0)],
        Mirror::Y => &[(1.0, 1.0), (1.0, -1.0)],
        Mirror::Xy => &[(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)],
    };
    let turns = prism.clamp(1, MAX_PRISM) as usize;
    let n = flips.len() * turns;
    if n == 1 || points.is_empty() {
        return points;
    }
    let src = crate::output::fit_frame(&points, (points.len() / n).max(MIN_COPY_POINTS));
    let blank_step = crate::optimize::OptimizeParams::default().blank_step;
    let mut out: Vec<Point> = Vec::with_capacity(src.len() * n + n * 32);
    for k in 0..turns {
        let (s, c) = (std::f32::consts::TAU * k as f32 / turns as f32).sin_cos();
        for &(fx, fy) in flips {
            let place = |p: &Point| {
                let (x, y) = (p.x * fx, p.y * fy);
                Point { x: x * c - y * s, y: x * s + y * c, ..*p }
            };
            if let (Some(&last), Some(first)) = (out.last(), src.first()) {
                crate::optimize::travel(&mut out, last, place(first), blank_step);
            }
            out.extend(src.iter().map(place));
        }
    }
    out
}

/// « Points visibles »: only the first `share` of the lit points stay lit.
fn trace(points: &mut [Point], share: f32) {
    let share = if share.is_finite() { share.clamp(0.0, 1.0) } else { 1.0 };
    if share >= 1.0 {
        return;
    }
    let keep = (points.iter().filter(|p| p.is_lit()).count() as f32 * share).round() as usize;
    for (i, p) in points.iter_mut().filter(|p| p.is_lit()).enumerate() {
        if i >= keep {
            (p.r, p.g, p.b) = (0.0, 0.0, 0.0);
        }
    }
}

/// « Pointillés »: along each stroke, two lit points then a gap of 1 (just
/// above 0) to `MAX_DOT_GAP` (at 1) dark ones. 0 = solid.
fn dots(points: &mut [Point], amount: f32) {
    if !(amount.is_finite() && amount > 0.0) {
        return;
    }
    let period = 3 + (amount.min(1.0) * (MAX_DOT_GAP - 1.0)).round() as usize;
    let mut i = 0;
    for p in points.iter_mut() {
        if !p.is_lit() {
            i = 0;
            continue;
        }
        if i % period >= 2 {
            (p.r, p.g, p.b) = (0.0, 0.0, 0.0);
        }
        i += 1;
    }
}

fn transform(points: &[Point], m: &LiveModifiers, st: &LiveState) -> Vec<Point> {
    let f = Transform::new(m, st);
    points
        .iter()
        .map(|p| {
            let (x, y) = f.apply(p.x, p.y);
            Point { x, y, ..*p }
        })
        .collect()
}

/// The geometry of a `LiveModifiers` (size, rotations, perspective, then
/// position), around the origin.
struct Transform {
    sx: f32,
    sy: f32,
    rot_x: (f32, f32),
    rot_y: (f32, f32),
    rot_z: (f32, f32),
    three_d: bool,
    perspective: f32,
    pos: (f32, f32),
}

impl Transform {
    fn new(m: &LiveModifiers, st: &LiveState) -> Self {
        let [ax, ay, az] = st.angles(m).map(f32::to_radians);
        Self {
            sx: m.size * m.size_x,
            sy: m.size * m.size_y,
            rot_x: ax.sin_cos(),
            rot_y: ay.sin_cos(),
            rot_z: az.sin_cos(),
            three_d: ax != 0.0 || ay != 0.0,
            perspective: m.perspective.clamp(0.0, 1.0),
            pos: (m.pos_x, m.pos_y),
        }
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let (mut x, mut y) = (x * self.sx, y * self.sy);
        if self.three_d {
            // Rotate around X then Y, then project with a simple perspective.
            let ((sx, cx), (sy, cy)) = (self.rot_x, self.rot_y);
            let (y1, z1) = (y * cx, y * sx);
            let (x2, z2) = (x * cy + z1 * sy, -x * sy + z1 * cy);
            let depth = (1.0 + self.perspective * z2).max(0.2);
            x = x2 / depth;
            y = y1 / depth;
        }
        let (sz, cz) = self.rot_z;
        let (xr, yr) = (x * cz - y * sz, x * sz + y * cz);
        (xr + self.pos.0, yr + self.pos.1)
    }
}

/// Per-part modifiers (T-441), before the cue's own: `parts[i]` is the
/// part of `points[i]` (`Figure::frame_points_tagged`), `per_part` the
/// modifiers of each part (0, « no part », is never a target) and
/// `states` each part's rotation and clock. Only the points of a part
/// with modifiers change; every other sample is returned bit for bit.
///
/// A part keeps its points: no mirror, prism or copies (they would add
/// points between the strokes). Its geometry turns and scales around the
/// centre of the part's bounding box (a wheel turns on itself), then
/// moves; the result is clamped to -1..1. Colour, visible points, dots,
/// strobe and brightness (0..1) only change or remove the light of lit
/// points: a blanked point (the travel to and from every stroke) stays
/// blanked. Lengths that don't match: the points unchanged.
pub fn apply_parts(points: &[Point], parts: &[u8], per_part: &BTreeMap<u8, LiveModifiers>, states: &BTreeMap<u8, LiveState>, user: &[Palette]) -> Vec<Point> {
    let mut out = points.to_vec();
    if parts.len() != points.len() {
        return out;
    }
    let neutral = LiveState::default();
    for (&part, m) in per_part.iter().filter(|(&id, _)| id != 0) {
        let idx: Vec<usize> = (0..parts.len()).filter(|&i| parts[i] == part).collect();
        if idx.is_empty() {
            continue;
        }
        let st = states.get(&part).unwrap_or(&neutral);
        let mut own: Vec<Point> = idx.iter().map(|&i| out[i]).collect();
        if !(m.geometry_is_identity() && st.spin == [0.0; 3]) {
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for p in &own {
                lo = [lo[0].min(p.x), lo[1].min(p.y)];
                hi = [hi[0].max(p.x), hi[1].max(p.y)];
            }
            let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
            let f = Transform::new(m, st);
            for p in &mut own {
                let (x, y) = f.apply(p.x - cx, p.y - cy);
                let (x, y) = (x + cx, y + cy);
                // A non-finite value (a mangled modifier) leaves the point where it was.
                if x.is_finite() && y.is_finite() {
                    (p.x, p.y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
                }
            }
        }
        recolor(&mut own, &m.color, st, user);
        trace(&mut own, m.trace);
        dots(&mut own, m.dots);
        if m.black || !m.strobe.lit_at(st.time, st.beat) {
            black_out(&mut own);
        }
        let gain = if m.brightness.is_finite() { m.brightness.clamp(0.0, 1.0) } else { 1.0 };
        for p in &mut own {
            if gain != 1.0 {
                (p.r, p.g, p.b) = (p.r * gain, p.g * gain, p.b * gain);
            }
            if !(p.r.is_finite() && p.g.is_finite() && p.b.is_finite()) {
                (p.r, p.g, p.b) = (0.0, 0.0, 0.0);
            }
        }
        for (k, &i) in idx.iter().enumerate() {
            out[i] = own[k];
        }
    }
    out
}

/// Brightness of a point (HSV value). Blanked points are 0 and stay blanked.
fn intensity(p: &Point) -> f32 {
    p.r.max(p.g).max(p.b)
}

/// Give a lit point colour `c` (0..1 per channel) at intensity `v`.
fn paint(p: &mut Point, c: [f32; 3], v: f32) {
    p.r = c[0] * v;
    p.g = c[1] * v;
    p.b = c[2] * v;
}

fn unit_rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| v as f32 / 255.0)
}

/// RGB 0..1 → (hue 0..1, saturation, value).
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    if d <= 0.0 {
        return (0.0, 0.0, max);
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, d / max, max)
}

/// Hue 0..1 (wraps), saturation, value → RGB 0..1.
pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = h6 - h6.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match h6 as u32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Index of the palette colour closest to `c` (RGB distance).
fn nearest(colors: &[[u8; 3]], c: [f32; 3]) -> usize {
    let dist = |p: &[u8; 3]| unit_rgb(*p).iter().zip(c).map(|(a, b)| (a - b) * (a - b)).sum::<f32>();
    (0..colors.len()).min_by(|&a, &b| dist(&colors[a]).total_cmp(&dist(&colors[b]))).unwrap_or(0)
}

/// Recolour lit points in place; blanked points are never touched.
fn recolor(points: &mut [Point], c: &ColorOverride, st: &LiveState, user: &[Palette]) {
    match *c {
        ColorOverride::Normal => {}
        ColorOverride::Fixed { rgb } => {
            let c = unit_rgb(rgb);
            for p in points.iter_mut() {
                let v = intensity(p);
                if v > 0.0 {
                    paint(p, c, v);
                }
            }
        }
        ColorOverride::Hue { hue } => {
            for p in points.iter_mut() {
                let (_, s, v) = rgb_to_hsv(p.r, p.g, p.b);
                if v > 0.0 {
                    paint(p, hsv_to_rgb(hue / 360.0, s, 1.0), v);
                }
            }
        }
        ColorOverride::Palette { palette, mode, offset } => {
            let Some(colors) = palette_colors(palette, user) else { return };
            let mut step = 0;
            let mut previous: Option<[f32; 3]> = None;
            for p in points.iter_mut() {
                let v = intensity(p);
                if v <= 0.0 {
                    continue;
                }
                // The point's colour at full intensity, so a dim green is still green.
                let own = [p.r / v, p.g / v, p.b / v];
                let index = match mode {
                    PaletteMode::Nearest => nearest(colors, own),
                    PaletteMode::Step => {
                        if previous.is_some_and(|q| q.iter().zip(own).any(|(a, b)| (a - b).abs() > 1e-3)) {
                            step += 1;
                        }
                        previous = Some(own);
                        step
                    }
                };
                paint(p, unit_rgb(colors[(index + offset) % colors.len()]), v);
            }
        }
        ColorOverride::Rainbow { spread, rate } => {
            let (time, beat) = st.colour_clock();
            let phase = rate.cycles(time, beat).rem_euclid(1.0) as f32;
            let (spread, n) = (spread.clamp(0.0, 4.0), points.len().max(1) as f32);
            for (i, p) in points.iter_mut().enumerate() {
                let v = intensity(p);
                if v > 0.0 {
                    paint(p, hsv_to_rgb(phase + spread * i as f32 / n, 1.0, 1.0), v);
                }
            }
        }
        ColorOverride::Chase { palette, step, spread } => {
            let Some(colors) = palette_colors(palette, user) else { return };
            let len = colors.len() as i64;
            let (time, beat) = st.colour_clock();
            let now = step.cycles(time, beat).floor() as i64;
            let (mut stroke, mut lit_points, mut was_lit) = (0i64, 0i64, false);
            for p in points.iter_mut() {
                let v = intensity(p);
                if v <= 0.0 {
                    was_lit = false;
                    continue;
                }
                if !was_lit && lit_points > 0 {
                    stroke += 1;
                }
                was_lit = true;
                let k = match spread {
                    ChaseSpread::Whole => 0,
                    ChaseSpread::Stroke => stroke,
                    ChaseSpread::Point => lit_points,
                };
                lit_points += 1;
                // Element k shows what element k-1 showed one step earlier:
                // the colours travel forward along the path.
                paint(p, unit_rgb(colors[(now - k).rem_euclid(len) as usize]), v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, AudioFeatures, BeatClock};
    use crate::presets::catalog;

    /// Two strokes: a lit square around (0.5, 0.5) in part 2, a lit line
    /// in part 1, each with its blanked arrival.
    fn two_parts() -> (Vec<Point>, Vec<u8>) {
        let mut pts = vec![Point::blanked(-0.8, -0.8), Point::lit(-0.8, -0.8, 1.0, 0.0, 0.0), Point::lit(-0.2, -0.8, 1.0, 0.0, 0.0)];
        let mut parts = vec![1, 1, 1];
        let square = [[0.4, 0.4], [0.6, 0.4], [0.6, 0.6], [0.4, 0.6], [0.4, 0.4]];
        pts.push(Point::blanked(0.4, 0.4));
        pts.extend(square.iter().map(|q| Point::lit(q[0], q[1], 0.0, 1.0, 0.0)));
        parts.extend([2; 6]);
        (pts, parts)
    }

    fn bits(p: &Point) -> [u32; 5] {
        [p.x, p.y, p.r, p.g, p.b].map(f32::to_bits)
    }

    #[test]
    fn a_part_effect_leaves_every_other_sample_bit_for_bit() {
        let (pts, parts) = two_parts();
        let m = LiveModifiers { brightness: 0.3, size: 1.7, pos_x: 0.2, rot_angle: [10.0, 20.0, 33.0], color: ColorOverride::Fixed { rgb: [0, 0, 255] }, ..Default::default() };
        let out = apply_parts(&pts, &parts, &BTreeMap::from([(2, m)]), &BTreeMap::new(), &[]);
        for i in 0..3 {
            assert_eq!(bits(&out[i]), bits(&pts[i]), "part 1, point {i}");
        }
        assert!(out[3..].iter().zip(&pts[3..]).any(|(a, b)| bits(a) != bits(b)), "part 2 moved");
        assert!(!out[3].is_lit(), "its blanked arrival stays blanked");
        assert!(out[4..].iter().all(|p| p.b > 0.0 && p.b <= 0.3 + 1e-6 && p.g == 0.0));
        // Part 0 is never a target; lengths that don't match change nothing.
        let none = apply_parts(&pts, &[0; 9], &BTreeMap::from([(0, LiveModifiers { brightness: 0.0, ..Default::default() })]), &BTreeMap::new(), &[]);
        assert_eq!(none, pts);
        assert_eq!(apply_parts(&pts, &parts[..3], &BTreeMap::from([(2, LiveModifiers { brightness: 0.0, ..Default::default() })]), &BTreeMap::new(), &[]), pts);
    }

    #[test]
    fn a_part_turns_and_scales_around_its_own_centre() {
        let (pts, parts) = two_parts();
        let m = LiveModifiers { rot_angle: [0.0, 0.0, 90.0], ..Default::default() };
        let out = apply_parts(&pts, &parts, &BTreeMap::from([(2, m)]), &BTreeMap::new(), &[]);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        // (0.4, 0.4) turns a quarter around (0.5, 0.5): to (0.6, 0.4).
        assert!(near(out[4].x, 0.6) && near(out[4].y, 0.4), "{:?}", out[4]);
        assert!(out[4..].iter().all(|p| (0.39..=0.61).contains(&p.x) && (0.39..=0.61).contains(&p.y)), "not around the figure's centre");
        // Twice the size around the same centre, clamped to the frame.
        let big = apply_parts(&pts, &parts, &BTreeMap::from([(2, LiveModifiers { size: 2.0, ..Default::default() })]), &BTreeMap::new(), &[]);
        assert!(near(big[4].x, 0.3) && near(big[4].y, 0.3), "{:?}", big[4]);
        let far = apply_parts(&pts, &parts, &BTreeMap::from([(2, LiveModifiers { pos_x: 1.0, ..Default::default() })]), &BTreeMap::new(), &[]);
        assert!(far.iter().all(|p| p.x <= 1.0));
        // A spinning part: its own state turns it.
        let spin = LiveModifiers { rot_speed: [0.0, 0.0, 90.0], ..Default::default() };
        let mut st = LiveState::default();
        st.advance(&spin, 1.0, 120.0, 4);
        let turned = apply_parts(&pts, &parts, &BTreeMap::from([(2, spin)]), &BTreeMap::from([(2, st)]), &[]);
        assert!(near(turned[4].x, 0.6) && near(turned[4].y, 0.4), "{:?}", turned[4]);
    }

    #[test]
    fn a_part_can_only_dim() {
        let (pts, parts) = two_parts();
        for brightness in [1.0, 2.0, f32::INFINITY, f32::NAN] {
            let out = apply_parts(&pts, &parts, &BTreeMap::from([(2, LiveModifiers { brightness, ..Default::default() })]), &BTreeMap::new(), &[]);
            assert!(out.iter().zip(&pts).all(|(a, b)| a.g <= b.g && a.r <= b.r), "{brightness}");
        }
        // Then the cue, layer and master stages still multiply down.
        let part = apply_parts(&pts, &parts, &BTreeMap::from([(2, LiveModifiers { brightness: 0.8, ..Default::default() })]), &BTreeMap::new(), &[]);
        let master = apply(&part, &LiveModifiers { brightness: 0.5, ..Default::default() }, &LiveState::default(), &[]);
        assert!(master.iter().all(|p| p.g <= 0.4 + 1e-6 && p.r <= 0.5 + 1e-6));
    }

    #[test]
    fn default_modifiers_change_nothing() {
        let cues = catalog();
        for p in cues.iter().step_by(40).take(5) {
            let frame = Animator::default().render(&p.settings, AudioFeatures::default(), 1.0 / 60.0, &BeatClock::default());
            assert_eq!(apply(&frame, &LiveModifiers::default(), &LiveState::default(), &[]), frame, "cue {}", p.id);
        }
    }

    #[test]
    fn size_doubles_and_negative_x_flips() {
        let pts = vec![Point::lit(0.25, 0.1, 1.0, 1.0, 1.0)];
        let big = apply(&pts, &LiveModifiers { size: 2.0, ..Default::default() }, &LiveState::default(), &[]);
        assert_eq!((big[0].x, big[0].y), (0.5, 0.2));
        let flipped = apply(&pts, &LiveModifiers { size_x: -1.0, ..Default::default() }, &LiveState::default(), &[]);
        assert_eq!((flipped[0].x, flipped[0].y), (-0.25, 0.1));
    }

    #[test]
    fn z_rotation_speed_and_reverse() {
        let mut m = LiveModifiers { rot_speed: [0.0, 0.0, 90.0], ..Default::default() };
        let mut st = LiveState::default();
        for _ in 0..60 {
            st.advance(&m, 1.0 / 60.0, 120.0, 4);
        }
        assert!((st.angles(&m)[2] - 90.0).abs() < 0.5);
        m.rot_reverse = true;
        for _ in 0..60 {
            st.advance(&m, 1.0 / 60.0, 120.0, 4);
        }
        let a = st.angles(&m)[2];
        assert!(a < 0.5 || a > 359.5, "angle {a}");
    }

    #[test]
    fn synced_rotation_follows_the_tempo() {
        // 1 turn per bar at 120 BPM in 4/4 = 1 turn per 2 s.
        let m = LiveModifiers { rot_speed: [0.0, 0.0, 1.0], rot_sync: true, ..Default::default() };
        let mut st = LiveState::default();
        for _ in 0..60 {
            st.advance(&m, 1.0 / 60.0, 120.0, 4);
        }
        assert!((st.angles(&m)[2] - 180.0).abs() < 0.5);
    }

    #[test]
    fn rotation_by_90_degrees_moves_x_to_y() {
        let pts = vec![Point::lit(0.5, 0.0, 1.0, 1.0, 1.0)];
        let m = LiveModifiers { rot_angle: [0.0, 0.0, 90.0], ..Default::default() };
        let out = apply(&pts, &m, &LiveState::default(), &[]);
        assert!(out[0].x.abs() < 1e-6 && (out[0].y - 0.5).abs() < 1e-6);
    }

    #[test]
    fn position_offsets_and_brightness_dims() {
        let pts = vec![Point::lit(0.0, 0.0, 1.0, 0.5, 0.0)];
        let out = apply(&pts, &LiveModifiers { pos_x: 0.3, pos_y: -0.2, brightness: 0.5, ..Default::default() }, &LiveState::default(), &[]);
        assert_eq!((out[0].x, out[0].y, out[0].r, out[0].g), (0.3, -0.2, 0.5, 0.25));
    }

    #[test]
    fn y_rotation_by_90_collapses_to_a_vertical_line() {
        let pts = vec![Point::lit(0.5, 0.2, 1.0, 1.0, 1.0)];
        let out = apply(&pts, &LiveModifiers { rot_angle: [0.0, 90.0, 0.0], ..Default::default() }, &LiveState::default(), &[]);
        assert!(out[0].x.abs() < 0.01, "x {}", out[0].x);
    }

    #[test]
    fn two_thousand_points_cost_well_under_a_millisecond() {
        let pts: Vec<Point> = (0..2000).map(|i| Point::lit((i as f32 / 2000.0) - 0.5, 0.1, 1.0, 1.0, 1.0)).collect();
        let m = LiveModifiers { size: 1.3, rot_angle: [20.0, 30.0, 40.0], pos_x: 0.1, brightness: 0.8, ..Default::default() };
        let st = LiveState::default();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(apply(&pts, &m, &st, &[]));
        }
        let per_frame = start.elapsed() / 100;
        // Debug builds are much slower than release; this bound holds in both.
        assert!(per_frame.as_micros() < 5_000, "{per_frame:?} per frame");
    }

    fn colored(color: ColorOverride) -> LiveModifiers {
        LiveModifiers { color, ..Default::default() }
    }

    fn rgb(p: &Point) -> [f32; 3] {
        [p.r, p.g, p.b]
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    /// Two strokes of two lit points each, with a blanked jump between.
    fn two_strokes() -> Vec<Point> {
        vec![
            Point::lit(0.0, 0.0, 0.0, 1.0, 0.0),
            Point::lit(0.1, 0.0, 0.0, 0.5, 0.0),
            Point::blanked(0.2, 0.0),
            Point::lit(0.3, 0.0, 0.0, 0.0, 1.0),
            Point::lit(0.4, 0.0, 0.0, 0.0, 1.0),
        ]
    }

    #[test]
    fn normal_colour_changes_nothing_even_with_colour_settings_changed() {
        let m = LiveModifiers {
            color_params: ColorParams { hue: 200.0, palette: 4, rgb: [1, 2, 3], ..Default::default() },
            ..Default::default()
        };
        for p in catalog().iter().step_by(40).take(5) {
            let frame = Animator::default().render(&p.settings, AudioFeatures::default(), 1.0 / 60.0, &BeatClock::default());
            let out = apply(&frame, &m, &LiveState::default(), &[]);
            let bits = |f: &[Point]| f.iter().flat_map(|p| [p.x, p.y, p.r, p.g, p.b].map(f32::to_bits)).collect::<Vec<_>>();
            assert_eq!(bits(&out), bits(&frame), "cue {}", p.id);
        }
    }

    #[test]
    fn fixed_red_keeps_intensity_and_blanking() {
        let out = apply(&two_strokes(), &colored(ColorOverride::Fixed { rgb: [255, 0, 0] }), &LiveState::default(), &[]);
        assert_eq!(rgb(&out[0]), [1.0, 0.0, 0.0]);
        assert_eq!(rgb(&out[1]), [0.5, 0.0, 0.0]);
        assert_eq!(rgb(&out[2]), [0.0, 0.0, 0.0]);
        assert_eq!(rgb(&out[3]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn master_brightness_applies_after_the_colour() {
        let m = LiveModifiers { brightness: 0.5, ..colored(ColorOverride::Fixed { rgb: [255, 255, 255] }) };
        let out = apply(&[Point::lit(0.0, 0.0, 0.0, 0.0, 1.0)], &m, &LiveState::default(), &[]);
        assert_eq!(rgb(&out[0]), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn hue_keeps_saturation_and_value() {
        // Half-bright pastel red (s = 0.5) turned to pure green hue.
        let pts = [Point::lit(0.0, 0.0, 0.5, 0.25, 0.25), Point::lit(0.0, 0.0, 0.8, 0.8, 0.8)];
        let out = apply(&pts, &colored(ColorOverride::Hue { hue: 120.0 }), &LiveState::default(), &[]);
        assert!(close(rgb(&out[0]), [0.25, 0.5, 0.25]), "{:?}", rgb(&out[0]));
        // Grey has no hue to change.
        assert!(close(rgb(&out[1]), [0.8, 0.8, 0.8]));
    }

    #[test]
    fn hsv_round_trips() {
        for c in [[1.0, 0.0, 0.0], [0.2, 0.7, 0.4], [0.9, 0.1, 0.8], [0.3, 0.3, 0.9]] {
            let (h, s, v) = rgb_to_hsv(c[0], c[1], c[2]);
            assert!(close(hsv_to_rgb(h, s, v), c), "{c:?}");
        }
    }

    #[test]
    fn palette_nearest_maps_green_to_the_closest_colour() {
        // « Chaud » = red, orange, amber, pink: amber is closest to green.
        let m = colored(ColorOverride::Palette { palette: 1, mode: PaletteMode::Nearest, offset: 0 });
        let out = apply(&two_strokes(), &m, &LiveState::default(), &[]);
        let amber = unit_rgb([255, 180, 0]);
        assert!(close(rgb(&out[0]), amber));
        assert!(close(rgb(&out[1]), amber.map(|c| c * 0.5)), "dim green keeps its intensity");
        assert_eq!(rgb(&out[2]), [0.0; 3]);
        // The offset turns the palette: amber → pink.
        let m = colored(ColorOverride::Palette { palette: 1, mode: PaletteMode::Nearest, offset: 1 });
        assert!(close(rgb(&apply(&two_strokes(), &m, &LiveState::default(), &[])[0]), unit_rgb([255, 0, 96])));
    }

    #[test]
    fn palette_step_moves_on_at_each_colour_change() {
        // Green, green (dimmer), blue, blue: one change → colours 0, 0, 1, 1.
        let m = colored(ColorOverride::Palette { palette: 6, mode: PaletteMode::Step, offset: 0 });
        let out = apply(&two_strokes(), &m, &LiveState::default(), &[]);
        assert_eq!(rgb(&out[0]), [0.0, 0.0, 1.0]);
        assert_eq!(rgb(&out[1]), [0.0, 0.0, 0.5]);
        assert_eq!(rgb(&out[3]), [1.0, 1.0, 1.0]);
        assert_eq!(rgb(&out[4]), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn rainbow_spreads_along_the_path_and_scrolls_with_the_beat() {
        let pts: Vec<Point> = (0..4).map(|i| Point::lit(i as f32 * 0.1, 0.0, 1.0, 1.0, 1.0)).collect();
        let m = colored(ColorOverride::Rainbow { spread: 1.0, rate: Rate::Beats(4.0) });
        let mut st = LiveState::default();
        let out = apply(&pts, &m, &st, &[]);
        // Hues 0, 90, 180, 270 degrees.
        assert!(close(rgb(&out[0]), [1.0, 0.0, 0.0]));
        assert!(close(rgb(&out[2]), [0.0, 1.0, 1.0]));
        // One beat later (a quarter cycle) point 0 shows what point 1 showed.
        st.set_clock(0.5, 1.0);
        assert!(close(rgb(&apply(&pts, &m, &st, &[])[0]), rgb(&out[1])));
        // In Hz, time drives it: 0.25 Hz for 1 s = a quarter cycle too.
        let hz = colored(ColorOverride::Rainbow { spread: 1.0, rate: Rate::Hz(0.25) });
        st.set_clock(1.0, 99.0);
        assert!(close(rgb(&apply(&pts, &hz, &st, &[])[0]), rgb(&out[1])));
    }

    #[test]
    fn chase_changes_colour_exactly_on_the_beat() {
        use crate::tempo::TempoClock;
        let mut clock = TempoClock::default();
        clock.set_bpm_manual(128.0, 0.0);
        clock.nudge(0.37); // beats that don't fall on round seconds
        let m = colored(ColorOverride::Chase { palette: 6, step: Rate::Beats(1.0), spread: ChaseSpread::Whole });
        let pts = [Point::lit(0.0, 0.0, 1.0, 1.0, 1.0)];
        let mut st = LiveState::default();
        let mut previous: Option<([f32; 3], f64)> = None;
        let mut changes = 0;
        for frame in 0..600 {
            let t = frame as f64 / 60.0;
            st.set_clock(t, clock.beat_at(t));
            let c = rgb(&apply(&pts, &m, &st, &[])[0]);
            let beat = clock.beat_at(t);
            if let Some((pc, pbeat)) = previous {
                let crossed = beat.floor() != pbeat.floor();
                assert_eq!(c != pc, crossed, "frame {frame}: beat {pbeat} → {beat}");
                changes += crossed as u32;
            }
            previous = Some((c, beat));
        }
        assert_eq!(changes, 21, "128 BPM over 10 s");
    }

    #[test]
    fn chase_spreads_over_strokes_and_points() {
        let st = LiveState::default(); // step 0
        let by_stroke = colored(ColorOverride::Chase { palette: 6, step: Rate::Beats(1.0), spread: ChaseSpread::Stroke });
        let out = apply(&two_strokes(), &by_stroke, &st, &[]);
        // Tricolore = blue, white, red. Stroke 0 → blue, stroke 1 → (0 - 1) mod 3 = red.
        assert_eq!(rgb(&out[0]), [0.0, 0.0, 1.0]);
        assert_eq!(rgb(&out[1]), [0.0, 0.0, 0.5]);
        assert_eq!(rgb(&out[3]), [1.0, 0.0, 0.0]);
        let by_point = colored(ColorOverride::Chase { palette: 6, step: Rate::Beats(1.0), spread: ChaseSpread::Point });
        let out = apply(&two_strokes(), &by_point, &st, &[]);
        // Lit points k = 0, 1, 2, 3 → colours 0, 2, 1, 0.
        assert_eq!([rgb(&out[0]), rgb(&out[3]), rgb(&out[4])], [[0.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 0.0, 1.0]]);
    }

    #[test]
    fn user_palettes_follow_the_built_ins_and_missing_ones_change_nothing() {
        let user = [Palette { name: "Mienne".into(), colors: vec![[10, 20, 30]] }];
        let m = colored(ColorOverride::Palette { palette: BUILTIN.len(), mode: PaletteMode::Nearest, offset: 0 });
        let out = apply(&two_strokes(), &m, &LiveState::default(), &user);
        assert!(close(rgb(&out[0]), unit_rgb([10, 20, 30])));
        let missing = colored(ColorOverride::Palette { palette: BUILTIN.len() + 1, mode: PaletteMode::Nearest, offset: 0 });
        assert_eq!(apply(&two_strokes(), &missing, &LiveState::default(), &user), two_strokes());
    }

    #[test]
    fn builtin_palettes_are_valid() {
        let all = builtin_palettes();
        assert_eq!(all.len(), 8);
        assert!(all.iter().all(|p| (1..=MAX_PALETTE_COLORS).contains(&p.colors.len())));
        assert_eq!(all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), PALETTE_LABELS[..8]);
    }

    #[test]
    fn user_palettes_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("laser-studio-palettes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("palettes.json");
        let mine = vec![Palette { name: "Scène".into(), colors: vec![[255, 0, 0], [0, 0, 255]] }];
        let mut store = PaletteStore::load_or_create(path.clone());
        store.set(mine.clone()).unwrap();
        assert_eq!(PaletteStore::load_or_create(path.clone()).list(), mine.as_slice());
        // Invalid lists are refused and leave the saved ones alone.
        assert!(store.set(vec![Palette { name: "Vide".into(), colors: vec![] }]).is_err());
        assert!(store.set(vec![Palette { name: "Trop".into(), colors: vec![[0, 0, 0]; 17] }]).is_err());
        assert_eq!(PaletteStore::load_or_create(path).list(), mine.as_slice());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn colour_settings_round_trip_and_old_files_still_load() {
        let m = colored(ColorOverride::Chase { palette: 2, step: Rate::Beats(0.5), spread: ChaseSpread::Point });
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""kind":"chase""#), "{json}");
        assert_eq!(serde_json::from_str::<LiveModifiers>(&json).unwrap(), m);
        let old: LiveModifiers = serde_json::from_str(r#"{"size": 1.5}"#).unwrap();
        assert_eq!(old.color, ColorOverride::Normal);
    }

    #[test]
    fn damaged_or_invalid_palettes_are_quarantined_and_never_overwritten() {
        let (dir, path) = crate::datafile::tests::with_damaged("palettes", "palettes.json");
        let mut store = PaletteStore::load_or_create(path.clone());
        assert!(store.list().is_empty());
        store.set(Vec::new()).unwrap();
        crate::datafile::tests::check_kept(&dir, "palettes.json", false);
        // Valid JSON that fails the checks is set aside too.
        let dir = crate::datafile::tests::temp_dir("palettes-invalid");
        let path = dir.join("palettes.json");
        std::fs::write(&path, r#"[{"name":"","colors":[]}]"#).unwrap();
        assert!(PaletteStore::load_or_create(path.clone()).list().is_empty());
        assert!(!path.exists());
        assert_eq!(crate::datafile::take_pending(&dir.join(crate::datafile::QUARANTINE)).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------- live effects (T-142) ----------

    /// A lit horizontal line of `n` points, white.
    fn line(n: usize) -> Vec<Point> {
        (0..n).map(|i| Point::lit(-0.5 + i as f32 / n as f32, 0.2, 1.0, 1.0, 1.0)).collect()
    }

    fn lit_count(points: &[Point]) -> usize {
        points.iter().filter(|p| p.is_lit()).count()
    }

    fn strobe(rate: Rate, duty: f32) -> LiveModifiers {
        LiveModifiers { strobe: Strobe { on: true, rate, duty, ..Default::default() }, ..Default::default() }
    }

    #[test]
    fn strobe_10_hz_half_duty_lights_30_frames_a_second() {
        let m = strobe(Rate::Hz(10.0), 0.5);
        let mut st = LiveState::default();
        let lit_frames = (0..60)
            .filter(|&f| {
                st.set_clock(5.0 + f as f64 / 60.0, 0.0);
                lit_count(&apply(&line(10), &m, &st, &[])) > 0
            })
            .count();
        assert!((29..=31).contains(&lit_frames), "{lit_frames} frames lit");
    }

    #[test]
    fn strobe_off_or_released_changes_nothing() {
        let mut m = strobe(Rate::Hz(10.0), 0.1);
        m.strobe.on = false;
        let mut st = LiveState::default();
        for f in 0..60 {
            st.set_clock(f as f64 / 60.0, 0.0);
            assert_eq!(apply(&line(10), &m, &st, &[]), line(10));
        }
        // Held: the same strobe as latched.
        m.strobe.hold = true;
        st.set_clock(0.95, 0.0);
        assert_eq!(lit_count(&apply(&line(10), &m, &st, &[])), 0);
    }

    #[test]
    fn synced_strobe_edges_land_on_the_sixteenths() {
        use crate::tempo::TempoClock;
        let mut clock = TempoClock::default();
        clock.set_bpm_manual(128.0, 0.0);
        clock.nudge(0.37);
        // 1/4: four flashes a beat, a rising edge on every sixteenth note.
        let m = strobe(Rate::Beats(0.25), 0.5);
        assert_eq!(m.strobe.sync_index(), 3);
        let mut st = LiveState::default();
        let mut was_lit = None;
        let mut edges = 0;
        for frame in 0..600 {
            let t = frame as f64 / 60.0;
            let beat = clock.beat_at(t);
            st.set_clock(t, beat);
            let lit = lit_count(&apply(&line(4), &m, &st, &[])) > 0;
            if was_lit == Some(false) && lit {
                edges += 1;
                // The sixteenth this edge belongs to started at most one frame ago.
                let sixteenth = (beat * 4.0).floor() / 4.0;
                let since = (beat - sixteenth) * 60.0 / clock.bpm;
                assert!(since < 1.0 / 60.0 + 1e-9, "frame {frame}: edge {since:.4} s after the sixteenth");
            }
            was_lit = Some(lit);
        }
        assert!(edges > 60, "{edges} rising edges in 10 s at 128 BPM");
    }

    #[test]
    fn strobe_never_brightens_and_bad_values_stay_in_range() {
        let m = LiveModifiers {
            strobe: Strobe { on: true, rate: Rate::Hz(f32::NAN), duty: f32::INFINITY, ..Default::default() },
            trace: f32::NAN,
            dots: -4.0,
            prism: 200,
            ..Default::default()
        };
        let mut st = LiveState::default();
        let src = line(50);
        for f in 0..120 {
            st.set_clock(f as f64 / 60.0, f as f64 / 30.0);
            for p in apply(&src, &m, &st, &[]) {
                assert!(p.r <= 1.0 && p.g <= 1.0 && p.b <= 1.0 && p.x.is_finite() && p.y.is_finite());
            }
        }
        let mut s = m.clone();
        s.sanitize_effects();
        assert_eq!((s.strobe.rate, s.strobe.duty, s.trace, s.dots, s.prism), (Rate::Hz(8.0), 0.5, 1.0, 0.0, MAX_PRISM));
        assert_eq!(strobe(Rate::Hz(99.0), 0.0).strobe.rate(), Rate::Hz(STROBE_MAX_HZ));
        assert_eq!(strobe(Rate::Beats(9.0), 0.0).strobe.duty(), STROBE_MIN_DUTY);
    }

    #[test]
    fn black_darkens_everything_and_keeps_the_path() {
        let m = LiveModifiers { black: true, ..Default::default() };
        let out = apply(&two_strokes(), &m, &LiveState::default(), &[]);
        assert_eq!(lit_count(&out), 0);
        assert_eq!(out.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(), two_strokes().iter().map(|p| (p.x, p.y)).collect::<Vec<_>>());
    }

    #[test]
    fn half_the_visible_points_are_the_start_of_the_path() {
        let src = line(100);
        let out = apply(&src, &LiveModifiers { trace: 0.5, ..Default::default() }, &LiveState::default(), &[]);
        assert_eq!(lit_count(&out), 50);
        assert!(out[..50].iter().all(Point::is_lit) && !out[50..].iter().any(Point::is_lit));
        assert_eq!(out.len(), src.len(), "positions kept: travel unchanged");
        assert_eq!(lit_count(&apply(&src, &LiveModifiers { trace: 0.0, ..Default::default() }, &LiveState::default(), &[])), 0);
    }

    #[test]
    fn dots_only_remove_light_and_space_out_with_the_amount() {
        let src = line(120);
        let solid = apply(&src, &LiveModifiers { dots: 0.0, ..Default::default() }, &LiveState::default(), &[]);
        assert_eq!(lit_count(&solid), 120);
        let mut previous = 120;
        for amount in [0.05, 0.3, 0.6, 1.0] {
            let out = apply(&src, &LiveModifiers { dots: amount, ..Default::default() }, &LiveState::default(), &[]);
            let n = lit_count(&out);
            assert!(n > 0 && n < previous, "dots {amount}: {n} lit");
            previous = n;
            for (a, b) in src.iter().zip(&out) {
                assert_eq!((a.x, a.y), (b.x, b.y));
                assert!(b.r <= a.r && b.g <= a.g && b.b <= a.b);
            }
        }
        // Dots restart on each stroke: its first point stays lit.
        let out = apply(&two_strokes(), &LiveModifiers { dots: 1.0, ..Default::default() }, &LiveState::default(), &[]);
        assert!(out[0].is_lit() && out[3].is_lit());
    }

    /// Every lit segment of `out` is a lit segment of `src` under one of
    /// `maps` (the copies), so no copy is joined to the next by light.
    type Place<'a> = &'a dyn Fn(&Point) -> (f32, f32);

    fn lit_segments_come_from_the_source(src: &[Point], out: &[Point], maps: &[Place]) -> bool {
        let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4;
        out.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()).all(|w| {
            maps.iter().any(|f| {
                src.windows(2).any(|s| s[0].is_lit() && s[1].is_lit() && close(f(&s[0]), (w[0].x, w[0].y)) && close(f(&s[1]), (w[1].x, w[1].y)))
                    || (close((w[0].x, w[0].y), (w[1].x, w[1].y)) && src.iter().any(|s| s.is_lit() && close(f(s), (w[0].x, w[0].y))))
            })
        })
    }

    #[test]
    fn prism_4_draws_four_copies_each_after_blanked_travel() {
        let src = line(40);
        let out = apply(&src, &LiveModifiers { prism: 4, ..Default::default() }, &LiveState::default(), &[]);
        // Four lit runs (one per copy), each preceded by dark travel.
        let runs: Vec<usize> = (0..out.len()).filter(|&i| out[i].is_lit() && (i == 0 || !out[i - 1].is_lit())).collect();
        assert_eq!(runs.len(), 4, "{runs:?}");
        for &start in &runs[1..] {
            assert!(!out[start - 1].is_lit() && !out[start - 2].is_lit(), "copy at {start} starts after blanked travel");
            assert!((out[start - 1].x - out[start].x).abs() < 1e-5, "travel arrives where the copy starts");
        }
        // The copies are the figure turned by 0, 90, 180, 270 degrees.
        let turn = |k: f32| move |p: &Point| {
            let (s, c) = (std::f32::consts::FRAC_PI_2 * k).sin_cos();
            (p.x * c - p.y * s, p.x * s + p.y * c)
        };
        let (a, b, c, d) = (turn(0.0), turn(1.0), turn(2.0), turn(3.0));
        assert!(lit_segments_come_from_the_source(&src, &out, &[&a, &b, &c, &d]));
        // Within the figure's point budget (plus the travel).
        assert!(out.len() < src.len() + 4 * MIN_COPY_POINTS + 4 * 40, "{} points", out.len());
    }

    #[test]
    fn mirrors_add_flipped_copies_joined_in_the_dark() {
        let src = two_strokes();
        for (mirror, copies) in [(Mirror::None, 1), (Mirror::X, 2), (Mirror::Y, 2), (Mirror::Xy, 4)] {
            let out = apply(&src, &LiveModifiers { mirror, ..Default::default() }, &LiveState::default(), &[]);
            let runs = (0..out.len()).filter(|&i| out[i].is_lit() && (i == 0 || !out[i - 1].is_lit())).count();
            assert_eq!(runs, 2 * copies, "{mirror:?}");
            let id = |p: &Point| (p.x, p.y);
            let fx = |p: &Point| (-p.x, p.y);
            let fy = |p: &Point| (p.x, -p.y);
            let fxy = |p: &Point| (-p.x, -p.y);
            assert!(lit_segments_come_from_the_source(&src, &out, &[&id, &fx, &fy, &fxy]), "{mirror:?}");
        }
        // Mirror X of a point on the right puts its twin on the left.
        let out = apply(&[Point::lit(0.6, 0.1, 1.0, 0.0, 0.0)], &LiveModifiers { mirror: Mirror::X, ..Default::default() }, &LiveState::default(), &[]);
        assert!(out.iter().any(|p| p.is_lit() && (p.x + 0.6).abs() < 1e-6 && (p.y - 0.1).abs() < 1e-6));
    }

    #[test]
    fn frozen_colours_stand_still_and_the_strobe_does_not() {
        let pts: Vec<Point> = (0..4).map(|i| Point::lit(i as f32 * 0.1, 0.0, 1.0, 1.0, 1.0)).collect();
        let m = colored(ColorOverride::Rainbow { spread: 1.0, rate: Rate::Hz(0.25) });
        let mut st = LiveState::default();
        st.freeze_colours(Some((0.0, 0.0)));
        st.set_clock(1.0, 2.0);
        let frozen = apply(&pts, &m, &st, &[]);
        assert_eq!(frozen, apply(&pts, &m, &LiveState::default(), &[]));
        st.freeze_colours(None);
        assert_ne!(apply(&pts, &m, &st, &[]), frozen);
        // The strobe reads the real clock even while frozen.
        let s = LiveModifiers { strobe: Strobe { on: true, rate: Rate::Hz(1.0), duty: 0.5, hold: false }, ..m };
        st.freeze_colours(Some((0.0, 0.0)));
        st.set_clock(0.75, 0.0);
        assert_eq!(lit_count(&apply(&pts, &s, &st, &[])), 0);
    }

    #[test]
    fn momentary_effects_are_released_and_old_files_load() {
        let mut m = LiveModifiers { freeze: true, black: true, strobe: Strobe { hold: true, on: true, ..Default::default() }, ..Default::default() };
        m.release_momentary();
        assert!(!m.freeze && !m.black && !m.strobe.hold && m.strobe.on, "the latched strobe stays");
        let old: LiveModifiers = serde_json::from_str(r#"{"size": 1.5}"#).unwrap();
        assert_eq!((old.trace, old.dots, old.prism, old.mirror, old.strobe), (1.0, 0.0, 1, Mirror::None, Strobe::default()));
        let json = serde_json::to_string(&LiveModifiers { mirror: Mirror::Xy, strobe: strobe(Rate::Beats(0.5), 0.3).strobe, ..Default::default() }).unwrap();
        assert!(json.contains(r#""mirror":"xy""#), "{json}");
        let back: LiveModifiers = serde_json::from_str(&json).unwrap();
        assert_eq!((back.mirror, back.strobe.rate), (Mirror::Xy, Rate::Beats(0.5)));
    }
}
