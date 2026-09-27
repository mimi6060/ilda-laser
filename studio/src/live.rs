//! Master live modifiers: what the laserist changes constantly while a
//! cue plays - size, position, rotation (with speed presets and tempo
//! sync), animation speed, colour (fixed, hue, palette, rainbow, chase)
//! and a master dimmer. Applied to the rendered look, before calibration
//! and safety, so a live move can never push the beam past the
//! calibration clamp or into a safety zone.

use crate::patterns::Point;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
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
        }
    }
}

impl LiveModifiers {
    /// True when nothing but the colour differs from the defaults.
    fn geometry_is_identity(&self) -> bool {
        let defaults = Self::default();
        Self { color: ColorOverride::Normal, color_params: defaults.color_params.clone(), ..self.clone() } == defaults
    }
}

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
    /// A missing, unreadable or invalid file starts an empty store.
    pub fn load_or_create(path: PathBuf) -> Self {
        let palettes: Vec<Palette> = crate::load_json(&path);
        let palettes = if validate_palettes(&palettes).is_ok() { palettes } else { Vec::new() };
        Self { path, palettes }
    }

    pub fn list(&self) -> &[Palette] {
        &self.palettes
    }

    /// Replaces every user palette and saves.
    pub fn set(&mut self, palettes: Vec<Palette>) -> Result<()> {
        validate_palettes(&palettes)?;
        let json = serde_json::to_string_pretty(&palettes).context("failed to serialize palettes")?;
        std::fs::write(&self.path, json).with_context(|| format!("failed to write {}", self.path.display()))?;
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
}

impl LiveState {
    /// Set the clock for this frame: `t` seconds, `beat` = `TempoClock::beat_at(t)`.
    pub fn set_clock(&mut self, t: f64, beat: f64) {
        self.time = t;
        self.beat = beat;
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

/// Geometry, then colour, then the master dimmer. `user` are the user
/// palettes (palette indexes past the built-ins point into it).
pub fn apply(points: &[Point], m: &LiveModifiers, st: &LiveState, user: &[Palette]) -> Vec<Point> {
    let mut out = if m.geometry_is_identity() && st.spin == [0.0; 3] { points.to_vec() } else { transform(points, m, st) };
    recolor(&mut out, &m.color, st, user);
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

fn transform(points: &[Point], m: &LiveModifiers, st: &LiveState) -> Vec<Point> {
    let [ax, ay, az] = st.angles(m).map(f32::to_radians);
    let (sx, cx) = ax.sin_cos();
    let (sy, cy) = ay.sin_cos();
    let (sz, cz) = az.sin_cos();
    let three_d = ax != 0.0 || ay != 0.0;

    points
        .iter()
        .map(|p| {
            let mut x = p.x * m.size * m.size_x;
            let mut y = p.y * m.size * m.size_y;
            if three_d {
                // Rotate around X then Y, then project with a simple perspective.
                let (y1, z1) = (y * cx, y * sx);
                let (x2, z2) = (x * cy + z1 * sy, -x * sy + z1 * cy);
                let depth = (1.0 + m.perspective.clamp(0.0, 1.0) * z2).max(0.2);
                x = x2 / depth;
                y = y1 / depth;
            }
            let (xr, yr) = (x * cz - y * sz, x * sz + y * cz);
            Point { x: xr + m.pos_x, y: yr + m.pos_y, ..*p }
        })
        .collect()
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
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
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
            let phase = rate.cycles(st.time, st.beat).rem_euclid(1.0) as f32;
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
            let now = step.cycles(st.time, st.beat).floor() as i64;
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
}
