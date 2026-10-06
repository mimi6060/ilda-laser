//! Abstracts built from stacked oscillators (T-440): CRÉATION › Abstraits.
//!
//! An abstract is a parametric curve: for `points` samples of `u` in
//! [0, 1[, `x(u)` is the sum of the X oscillators, each
//! `amp · wave(ratio · turns · u + phase + drift · beat)` (in cycles),
//! `y(u)` the sum of the Y ones. A sum whose amplitudes add up past 1 is
//! divided by that sum, so the curve always stays in -1..1. Colour
//! oscillators shift the look's hue along the trace (their sum × 180°).
//! The drift (turns per beat, on the tempo clock) animates the figure; 0
//! everywhere = a still figure, the same frame after frame.
//!
//! An abstract is content like any other look: drawn by the engine, then
//! densified (`engine::densify` or the optimiser), mixed within the
//! layer's point budget and passed through the whole safety stage. This
//! module only bounds what it is given: six oscillators at most, every
//! number finite and in range, and never more samples than the budget.
//!
//! Lissajous figures and sums of sines are public mathematics; the
//! catalogue below is ours (our parameters and names), nothing is taken
//! from another abstract editor.

use crate::engine::{Content, Settings};
use crate::lfo::Wave;
use serde::{Deserialize, Serialize};

/// Oscillators an abstract holds at most.
pub const MAX_OSCS: usize = 6;
/// Sample count bounds (before the layer's point budget).
pub const MIN_POINTS: u16 = 16;
pub const MAX_POINTS: u16 = 2000;
pub const DEFAULT_POINTS: u16 = 400;
/// Frequency ratio bounds.
pub const MIN_RATIO: f32 = 0.25;
pub const MAX_RATIO: f32 = 16.0;
/// Base periods bounds.
pub const MIN_TURNS: f32 = 0.25;
pub const MAX_TURNS: f32 = 64.0;
/// Drift bounds, in turns per beat.
pub const MAX_DRIFT: f32 = 4.0;
/// The catalogue page the shipped abstracts go on.
pub const CATEGORY: &str = "Abstraits";

/// What an oscillator drives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OscTarget {
    #[default]
    X,
    Y,
    /// The hue along the trace.
    Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Osc {
    pub target: OscTarget,
    /// Cycles per base period, 0.25..=16.
    pub ratio: f32,
    /// 0..=360 degrees.
    pub phase_deg: f32,
    /// 0..=1.
    pub amp: f32,
    /// The LFO waves, without « Aléatoire » (brought back to a sine).
    pub wave: Wave,
    /// Phase drift in turns per beat of the tempo clock, -4..=4; 0 = still.
    pub drift: f32,
    pub enabled: bool,
}

impl Default for Osc {
    fn default() -> Self {
        Self { target: OscTarget::X, ratio: 1.0, phase_deg: 0.0, amp: 1.0, wave: Wave::Sine, drift: 0.0, enabled: true }
    }
}

impl Osc {
    /// An oscillator on `target` (the others at their defaults).
    pub fn new(target: OscTarget, ratio: f32, phase_deg: f32, amp: f32) -> Self {
        Self { target, ratio, phase_deg, amp, ..Self::default() }
    }

    fn sanitized(&self) -> Self {
        let d = Self::default();
        let fin = |v: f32, def: f32| if v.is_finite() { v } else { def };
        Self {
            target: self.target,
            ratio: fin(self.ratio, d.ratio).clamp(MIN_RATIO, MAX_RATIO),
            phase_deg: fin(self.phase_deg, d.phase_deg).clamp(0.0, 360.0),
            amp: fin(self.amp, d.amp).clamp(0.0, 1.0),
            wave: if self.wave == Wave::Random { Wave::Sine } else { self.wave },
            drift: fin(self.drift, d.drift).clamp(-MAX_DRIFT, MAX_DRIFT),
            enabled: self.enabled,
        }
    }

    /// The wave at `cycles` (any finite number), -1..1.
    fn at(&self, cycles: f64) -> f64 {
        crate::lfo::shape(self.wave, cycles.rem_euclid(1.0))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AbstractShape {
    pub oscs: Vec<Osc>,
    /// Samples asked for, 16..=2000, then bounded by the layer's budget.
    pub points: u16,
    /// Base periods the trace runs through, 0.25..=64.
    pub turns: f32,
    /// `turns` = the fewest whole periods that close the curve, when the
    /// ratios allow it (else `turns` as set).
    pub auto_turns: bool,
    /// A curve that closes repeats its first point (closed outlines rule).
    pub closed: bool,
}

impl Default for AbstractShape {
    /// A circle: X and Y at ratio 1, X a quarter turn ahead.
    fn default() -> Self {
        Self {
            oscs: vec![Osc::new(OscTarget::X, 1.0, 90.0, 1.0), Osc::new(OscTarget::Y, 1.0, 0.0, 1.0)],
            points: DEFAULT_POINTS,
            turns: 1.0,
            auto_turns: true,
            closed: true,
        }
    }
}

/// Whether `v` is a whole number (to a rounding error).
fn whole(v: f64) -> bool {
    (v - v.round()).abs() < 1e-3
}

impl AbstractShape {
    /// Everything within bounds: non-finite numbers back to their
    /// defaults, six oscillators at most, no « Aléatoire » wave.
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        Self {
            oscs: self.oscs.iter().take(MAX_OSCS).map(Osc::sanitized).collect(),
            points: self.points.clamp(MIN_POINTS, MAX_POINTS),
            turns: if self.turns.is_finite() { self.turns } else { d.turns }.clamp(MIN_TURNS, MAX_TURNS),
            auto_turns: self.auto_turns,
            closed: self.closed,
        }
    }

    fn active(&self) -> impl Iterator<Item = &Osc> {
        self.oscs.iter().filter(|o| o.enabled && o.amp > 0.0)
    }

    /// The base periods the trace runs through: with « Tours
    /// automatiques », the fewest whole periods (1..=64) after which every
    /// drawing oscillator is back where it started; `turns` otherwise, or
    /// when no such number exists. Call on a sanitized shape.
    pub fn effective_turns(&self) -> f32 {
        if self.auto_turns {
            let ratios: Vec<f64> = self.active().filter(|o| o.target != OscTarget::Color).map(|o| o.ratio as f64).collect();
            if let Some(t) = (1..=MAX_TURNS as u32).find(|&t| ratios.iter().all(|r| whole(r * t as f64))) {
                return t as f32;
            }
        }
        self.turns
    }

    /// Whether the trace ends where it started (every drawing oscillator
    /// runs whole cycles), so repeating the first point closes it.
    pub fn closes(&self) -> bool {
        let turns = self.effective_turns() as f64;
        self.active().filter(|o| o.target != OscTarget::Color).all(|o| whole(o.ratio as f64 * turns))
    }

    /// Samples drawn with a layer budget of `budget` points: the
    /// requested count, bounded by the budget (one kept for the closing
    /// point). Call on a sanitized shape.
    pub fn sample_count(&self, budget: usize) -> usize {
        let room = budget.saturating_sub(usize::from(self.closed && self.closes())).max(2);
        (self.points as usize).min(room)
    }
}

/// The curve at tempo-clock beat `beat` within a budget of `budget`
/// points: (x, y, hue offset in turns 0..1) per point, x and y in -1..1,
/// never more than `budget` points (2 at the least), every value finite.
pub fn points(shape: &AbstractShape, beat: f64, budget: usize) -> Vec<[f32; 3]> {
    let shape = shape.sanitized();
    let beat = if beat.is_finite() { beat } else { 0.0 };
    let turns = shape.effective_turns() as f64;
    let n = shape.sample_count(budget.max(2));
    // Each oscillator's start (phase + drift), in cycles.
    let oscs: Vec<(&Osc, f64)> = shape.active().map(|o| (o, (o.phase_deg as f64 / 360.0 + (o.drift as f64 * beat).rem_euclid(1.0)).rem_euclid(1.0))).collect();
    let norm = |t: OscTarget| {
        let sum: f64 = oscs.iter().filter(|(o, _)| o.target == t).map(|(o, _)| o.amp as f64).sum();
        if sum > 1.0 { 1.0 / sum } else { 1.0 }
    };
    let (kx, ky) = (norm(OscTarget::X), norm(OscTarget::Y));
    let sample = |u: f64| {
        let (mut x, mut y, mut h) = (0.0, 0.0, 0.0);
        for (o, start) in &oscs {
            let v = o.amp as f64 * o.at(o.ratio as f64 * turns * u + start);
            match o.target {
                OscTarget::X => x += v,
                OscTarget::Y => y += v,
                OscTarget::Color => h += v,
            }
        }
        let fin = |v: f64| if v.is_finite() { v as f32 } else { 0.0 };
        [fin(x * kx).clamp(-1.0, 1.0), fin(y * ky).clamp(-1.0, 1.0), fin((h * 0.5).rem_euclid(1.0)).clamp(0.0, 1.0)]
    };
    let mut out: Vec<[f32; 3]> = (0..n).map(|i| sample(i as f64 / n as f64)).collect();
    if shape.closed && shape.closes() {
        out.push(out[0]);
    }
    out
}

/// The editor's live preview (CRÉATION › Abstraits): the look drawn by
/// the engine on its own animator (what plays is never touched), one
/// picture every `PREVIEW_STEP` beats over `PREVIEW_BEATS` (one picture
/// for a still figure), and how many samples the budget lets through.
/// Display only: it never reaches the output.
#[derive(Clone, Debug, Serialize)]
pub struct Preview {
    pub step_beats: f64,
    pub frames: Vec<String>,
    /// Samples drawn (the request, bounded by the budget).
    pub samples: usize,
    /// Samples asked for.
    pub requested: usize,
    /// The layer's point budget.
    pub budget: usize,
    /// The request was cut by the budget.
    pub limited: bool,
    /// Points of the first picture as the engine drew it (densified).
    pub points: usize,
}

pub const PREVIEW_BEATS: f64 = 4.0;
pub const PREVIEW_STEP: f64 = 0.125;
/// Points kept per preview picture (plenty for the editor's canvas).
const PREVIEW_POINTS: usize = 600;

pub fn preview(look: &Settings, bpm: f64, beats_per_bar: u8, budget: usize) -> Preview {
    use crate::engine::{Animator, AudioFeatures, BeatClock};
    let Content::Abstract(shape) = &look.content else {
        return Preview { step_beats: PREVIEW_STEP, frames: Vec::new(), samples: 0, requested: 0, budget, limited: false, points: 0 };
    };
    let clean = shape.sanitized();
    let samples = clean.sample_count(budget);
    let still = clean.active().all(|o| o.drift == 0.0);
    let count = if still { 1 } else { (PREVIEW_BEATS / PREVIEW_STEP) as usize };
    let bpm = if bpm.is_finite() { bpm.clamp(20.0, 400.0) } else { 120.0 };
    let dt = (PREVIEW_STEP * 60.0 / bpm) as f32;
    let mut animator = Animator::starting_at(0.0);
    animator.set_point_budget(budget);
    let mut points = 0;
    let frames = (0..count)
        .map(|i| {
            let clock = BeatClock { beat: i as f64 * PREVIEW_STEP, bpm, beats_per_bar };
            let frame = animator.render(look, AudioFeatures::default(), if i == 0 { 0.0 } else { dt }, &clock);
            if i == 0 {
                points = frame.len();
            }
            crate::presets::pack_max(&frame, PREVIEW_POINTS)
        })
        .collect();
    Preview { step_beats: PREVIEW_STEP, frames, samples, requested: clean.points as usize, budget, limited: samples < clean.points as usize, points }
}

/// A look as it may be stored or previewed: an abstract's numbers within
/// bounds. Anything else is not an abstract (`None`).
pub fn clean_look(look: &Settings) -> Option<Settings> {
    let Content::Abstract(shape) = &look.content else { return None };
    Some(Settings { content: Content::Abstract(shape.sanitized()), ..look.clone() })
}

/// A shipped abstract: its name, colour and oscillators.
fn shipped(name: &str, color: [u8; 3], oscs: Vec<Osc>) -> (String, Settings) {
    let shape = AbstractShape { oscs, ..AbstractShape::default() };
    (name.to_string(), Settings { content: Content::Abstract(shape), color, scale: 0.6, ..Settings::default() })
}

fn drifting(mut o: Osc, drift: f32) -> Osc {
    o.drift = drift;
    o
}

fn waved(mut o: Osc, wave: Wave) -> Osc {
    o.wave = wave;
    o
}

/// The abstracts on the « Abstraits » page: parameters chosen by us.
pub fn catalog() -> Vec<(String, Settings)> {
    use OscTarget::{Color, X, Y};
    const GREEN: [u8; 3] = [0, 255, 0];
    const CYAN: [u8; 3] = [0, 255, 255];
    const RED: [u8; 3] = [255, 0, 0];
    const MAGENTA: [u8; 3] = [255, 0, 255];
    const BLUE: [u8; 3] = [0, 80, 255];
    const YELLOW: [u8; 3] = [255, 220, 0];
    vec![
        shipped("Lissajous 3:2", GREEN, vec![Osc::new(X, 3.0, 90.0, 1.0), Osc::new(Y, 2.0, 0.0, 1.0)]),
        // r = cos 5θ, written as two pairs of circles.
        shipped("Rosace 5", RED, vec![
            Osc::new(X, 6.0, 90.0, 0.5), Osc::new(X, 4.0, 90.0, 0.5),
            Osc::new(Y, 6.0, 0.0, 0.5), Osc::new(Y, 4.0, 180.0, 0.5),
        ]),
        shipped("Huit lent", CYAN, vec![drifting(Osc::new(X, 1.0, 0.0, 1.0), 0.125), Osc::new(Y, 2.0, 0.0, 0.8)]),
        shipped("Trèfle", GREEN, vec![
            Osc::new(X, 1.0, 90.0, 0.6), Osc::new(X, 2.0, 90.0, 0.4),
            Osc::new(Y, 1.0, 0.0, 0.6), Osc::new(Y, 2.0, 180.0, 0.4),
        ]),
        shipped("Étoile filante 5", YELLOW, vec![
            Osc::new(X, 1.0, 90.0, 0.6), Osc::new(X, 4.0, 90.0, 0.4),
            Osc::new(Y, 1.0, 0.0, 0.6), Osc::new(Y, 4.0, 180.0, 0.4),
            drifting(Osc::new(Color, 1.0, 0.0, 0.3), 0.25),
        ]),
        shipped("Lissajous 5:4 en dérive", BLUE, vec![drifting(Osc::new(X, 5.0, 0.0, 1.0), 0.0625), Osc::new(Y, 4.0, 0.0, 1.0)]),
        shipped("Triangles croisés 3:4", MAGENTA, vec![waved(Osc::new(X, 3.0, 45.0, 1.0), Wave::Triangle), waved(Osc::new(Y, 4.0, 0.0, 1.0), Wave::Triangle)]),
        shipped("Ruban arc-en-ciel 2:3", RED, vec![Osc::new(X, 2.0, 90.0, 1.0), Osc::new(Y, 3.0, 0.0, 1.0), Osc::new(Color, 1.0, 0.0, 1.0)]),
        shipped("Cercle ondulé 7", CYAN, vec![
            Osc::new(X, 1.0, 90.0, 0.75), Osc::new(X, 8.0, 90.0, 0.25),
            Osc::new(Y, 1.0, 0.0, 0.75), Osc::new(Y, 8.0, 0.0, 0.25),
            drifting(Osc::new(Color, 2.0, 0.0, 0.4), 0.5),
        ]),
        shipped("Nœud 4:3 tournant", GREEN, vec![
            drifting(Osc::new(X, 4.0, 45.0, 1.0), 0.03125), Osc::new(Y, 3.0, 0.0, 1.0),
            drifting(Osc::new(Color, 1.0, 0.0, 0.5), 0.125),
        ]),
        shipped("Spirale 3:2 à demi-rapports", YELLOW, vec![Osc::new(X, 1.5, 90.0, 1.0), Osc::new(Y, 1.0, 0.0, 1.0)]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety_invariants::Rng;
    use std::f64::consts::TAU;

    fn lissajous(phase: f32) -> AbstractShape {
        AbstractShape { oscs: vec![Osc::new(OscTarget::X, 3.0, phase, 1.0), Osc::new(OscTarget::Y, 2.0, 0.0, 1.0)], ..Default::default() }
    }

    /// Local maxima of a closed sequence (each point against its neighbours).
    fn maxima(v: &[f32]) -> usize {
        let n = v.len();
        (0..n).filter(|&i| v[i] > v[(i + n - 1) % n] && v[i] >= v[(i + 1) % n]).count()
    }

    #[test]
    fn lissajous_3_2_follows_the_formula_and_has_3_by_2_lobes() {
        let phase = 30.0;
        let pts = points(&lissajous(phase), 0.0, 750);
        assert_eq!(pts.len(), 401, "400 samples and the closing point");
        for (i, p) in pts[..400].iter().enumerate() {
            let u = i as f64 / 400.0;
            assert!((p[0] as f64 - (TAU * 3.0 * u + phase as f64 / 360.0 * TAU).sin()).abs() < 1e-4, "x at {u}");
            assert!((p[1] as f64 - (TAU * 2.0 * u).sin()).abs() < 1e-4, "y at {u}");
        }
        let open = &pts[..400];
        assert_eq!(maxima(&open.iter().map(|p| p[0]).collect::<Vec<_>>()), 3, "3 horizontal lobes");
        assert_eq!(maxima(&open.iter().map(|p| p[1]).collect::<Vec<_>>()), 2, "2 vertical lobes");
        assert_eq!(pts[400], pts[0], "closed: the first point repeated");
    }

    #[test]
    fn the_default_is_a_circle() {
        for p in points(&AbstractShape::default(), 0.0, 750) {
            assert!(((p[0] * p[0] + p[1] * p[1]).sqrt() - 1.0).abs() < 1e-4);
            assert_eq!(p[2], 0.0, "no colour oscillator: the look's colour");
        }
    }

    #[test]
    fn amplitudes_past_one_are_normalised() {
        let shape = AbstractShape {
            oscs: vec![Osc::new(OscTarget::X, 1.0, 90.0, 1.0), Osc::new(OscTarget::X, 1.0, 90.0, 1.0), Osc::new(OscTarget::Y, 1.0, 0.0, 0.5)],
            ..Default::default()
        };
        let pts = points(&shape, 0.0, 750);
        let max_x = pts.iter().map(|p| p[0].abs()).fold(0.0, f32::max);
        let max_y = pts.iter().map(|p| p[1].abs()).fold(0.0, f32::max);
        assert!((max_x - 1.0).abs() < 1e-4, "two full X oscillators in phase: divided by 2 ({max_x})");
        assert!((max_y - 0.5).abs() < 1e-3, "a sum under 1 is left alone ({max_y})");
    }

    #[test]
    fn the_budget_bounds_the_points() {
        let shape = AbstractShape { points: MAX_POINTS, ..Default::default() };
        assert_eq!(points(&shape, 0.0, 750).len(), 750);
        assert_eq!(shape.sample_count(750), 749);
        assert_eq!(points(&shape, 0.0, 100).len(), 100);
        assert_eq!(points(&shape, 0.0, 0).len(), 3, "never empty: 2 samples and the closing point");
        assert_eq!(points(&AbstractShape { points: 100, ..shape }, 0.0, 750).len(), 101);
    }

    #[test]
    fn the_engine_fits_the_densified_curve_in_the_budget() {
        use crate::engine::{Animator, AudioFeatures, BeatClock};
        let shapes = [
            AbstractShape { points: MAX_POINTS, ..Default::default() },
            AbstractShape { points: MAX_POINTS, oscs: vec![Osc::new(OscTarget::X, 9.0, 90.0, 1.0), Osc::new(OscTarget::Y, 8.0, 0.0, 1.0)], ..Default::default() },
        ];
        for (shape, budget) in shapes.into_iter().zip([750, 1000]) {
            let look = Settings { content: Content::Abstract(shape), scale: 0.6, brightness: 1.0, ..Settings::default() };
            let mut a = Animator::default();
            a.set_point_budget(budget);
            let frame = a.render(&look, AudioFeatures::default(), 0.0, &BeatClock::default());
            assert!(frame.len() <= budget && frame.len() > budget / 2, "{} points for {budget}", frame.len());
        }
    }

    #[test]
    fn sanitized_bounds_everything() {
        let wild = Osc { ratio: f32::NAN, phase_deg: f32::INFINITY, amp: -3.0, wave: Wave::Random, drift: 99.0, ..Osc::default() };
        let shape = AbstractShape { oscs: vec![wild; 7], points: 9, turns: f32::NAN, ..Default::default() }.sanitized();
        assert_eq!(shape.oscs.len(), MAX_OSCS);
        let o = &shape.oscs[0];
        assert_eq!((o.ratio, o.phase_deg, o.amp, o.wave, o.drift), (1.0, 0.0, 0.0, Wave::Sine, MAX_DRIFT));
        assert_eq!((shape.points, shape.turns), (MIN_POINTS, 1.0));
        let pts = points(&AbstractShape { points: 60000, ..shape }, f64::NAN, 750);
        assert!(pts.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn auto_turns_close_the_curve() {
        assert_eq!(lissajous(0.0).effective_turns(), 1.0);
        let half = AbstractShape { oscs: vec![Osc::new(OscTarget::X, 1.5, 90.0, 1.0), Osc::new(OscTarget::Y, 1.0, 0.0, 1.0)], ..Default::default() };
        assert_eq!(half.effective_turns(), 2.0);
        assert!(half.closes());
        let quarter = AbstractShape { oscs: vec![Osc::new(OscTarget::X, 0.25, 0.0, 1.0)], ..Default::default() };
        assert_eq!(quarter.effective_turns(), 4.0);
        let manual = AbstractShape { auto_turns: false, turns: 0.5, ..Default::default() };
        assert_eq!(manual.effective_turns(), 0.5);
        assert!(!manual.closes());
        assert_eq!(points(&manual, 0.0, 750).len(), 400, "an open curve doesn't repeat its first point");
        // A ratio with no small common period keeps `turns`.
        let odd = AbstractShape { oscs: vec![Osc::new(OscTarget::X, 1.0137, 0.0, 1.0)], turns: 3.0, ..Default::default() };
        assert_eq!(odd.effective_turns(), 3.0);
    }

    #[test]
    fn still_without_drift_moving_with_it() {
        let still = lissajous(0.0);
        assert_eq!(points(&still, 0.0, 750), points(&still, 13.37, 750));
        let mut moving = still.clone();
        moving.oscs[0].drift = 0.25;
        assert_ne!(points(&moving, 0.0, 750), points(&moving, 1.0, 750));
        // A quarter turn per beat: four beats later, the same figure.
        let (a, b) = (points(&moving, 0.5, 750), points(&moving, 4.5, 750));
        assert!(a.iter().zip(&b).all(|(p, q)| (p[0] - q[0]).abs() < 1e-4 && (p[1] - q[1]).abs() < 1e-4));
    }

    #[test]
    fn colour_oscillators_shift_the_hue() {
        let shape = AbstractShape { oscs: vec![Osc::new(OscTarget::X, 1.0, 90.0, 1.0), Osc::new(OscTarget::Color, 1.0, 0.0, 1.0)], ..Default::default() };
        let pts = points(&shape, 0.0, 750);
        let hues: Vec<f32> = pts.iter().map(|p| p[2]).collect();
        assert!(hues.iter().any(|&h| h > 0.2) && hues.iter().all(|&h| (0.0..=1.0).contains(&h)));
    }

    #[test]
    fn serde_round_trip_and_defaults() {
        let shape = lissajous(45.0);
        let json = serde_json::to_string(&Content::Abstract(shape.clone())).unwrap();
        assert!(json.contains(r#""kind":"abstract""#), "{json}");
        assert_eq!(serde_json::from_str::<Content>(&json).unwrap(), Content::Abstract(shape));
        let bare: Content = serde_json::from_str(r#"{"kind":"abstract","oscs":[{"target":"y","wave":"saw_down"}]}"#).unwrap();
        let Content::Abstract(a) = bare else { panic!() };
        assert_eq!(a.oscs[0], Osc { target: OscTarget::Y, wave: Wave::SawDown, ..Osc::default() });
        assert_eq!((a.points, a.turns, a.auto_turns, a.closed), (DEFAULT_POINTS, 1.0, true, true));
        assert!(Content::Abstract(a).same_drawing(&Content::Abstract(AbstractShape::default())), "editing the oscillators is the same drawing");
    }

    /// A saved abstract (a user cue, a project) reloads and plays the same
    /// frames; an older look without one loads unchanged.
    #[test]
    fn saved_abstracts_replay_identically_and_old_looks_still_load() {
        use crate::engine::{Animator, AudioFeatures, BeatClock};
        let (_, mut look) = catalog().into_iter().find(|(n, _)| n == "Nœud 4:3 tournant").unwrap();
        look.brightness = 1.0;
        let reloaded: Settings = serde_json::from_str(&serde_json::to_string(&look).unwrap()).unwrap();
        assert_eq!(reloaded, look);
        let (mut a, mut b) = (Animator::starting_at(0.0), Animator::starting_at(0.0));
        for i in 0..8 {
            let clock = BeatClock { beat: i as f64 * 0.25, ..BeatClock::default() };
            assert_eq!(a.render(&look, AudioFeatures::default(), 0.01, &clock), b.render(&reloaded, AudioFeatures::default(), 0.01, &clock));
        }
        let old = r#"{"content":{"kind":"generator","generator":"lissajous","params":{"count":1,"a":3,"b":2}},"color":[0,255,0],"scale":0.6}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(matches!(s.content, Content::Generator { .. }));
    }

    #[test]
    fn every_shipped_abstract_stays_in_bounds() {
        let list = catalog();
        assert!(list.len() >= 10);
        for (name, look) in &list {
            let Content::Abstract(shape) = &look.content else { panic!("{name}") };
            assert_eq!(shape, &shape.sanitized(), "{name}: parameters within bounds");
            assert!(shape.oscs.len() <= MAX_OSCS);
            for beat in [0.0, 1.3, 7.9] {
                let pts = points(shape, beat, crate::layers::DEFAULT_POINT_BUDGET);
                assert!(pts.len() <= crate::layers::DEFAULT_POINT_BUDGET, "{name}");
                assert!(pts.iter().all(|p| p[0].abs() <= 1.0 && p[1].abs() <= 1.0 && p.iter().all(|v| v.is_finite())), "{name}");
            }
        }
    }

    fn random_shape(rng: &mut Rng) -> AbstractShape {
        let waves = [Wave::Sine, Wave::Triangle, Wave::Square, Wave::SawUp, Wave::SawDown, Wave::Random];
        let wild = |rng: &mut Rng, lo: f32, hi: f32| match rng.below(12) {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => -f32::INFINITY,
            3 => rng.range(-1e6, 1e6),
            _ => rng.range(lo, hi),
        };
        let n = rng.below(10);
        AbstractShape {
            oscs: (0..n)
                .map(|_| Osc {
                    target: *rng.pick(&[OscTarget::X, OscTarget::Y, OscTarget::Color]),
                    ratio: wild(rng, -1.0, 20.0),
                    phase_deg: wild(rng, -90.0, 450.0),
                    amp: wild(rng, -0.5, 1.5),
                    wave: *rng.pick(&waves),
                    drift: wild(rng, -6.0, 6.0),
                    enabled: rng.chance(0.85),
                })
                .collect(),
            points: rng.below(70000) as u16,
            turns: wild(rng, -1.0, 80.0),
            auto_turns: rng.chance(0.5),
            closed: rng.chance(0.7),
        }
    }

    /// Property: random oscillator stacks (mangled numbers included) always
    /// give finite points within -1..1 and within the point budget, raw
    /// and drawn by the engine.
    #[test]
    fn random_stacks_stay_finite_in_bounds_and_in_budget() {
        use crate::engine::{Animator, AudioFeatures, BeatClock};
        for seed in crate::safety_invariants::seeds(440, 300) {
            let mut rng = Rng::new(seed);
            let shape = random_shape(&mut rng);
            let budget = *rng.pick(&[2usize, 100, 300, 750, 2000, 10_000]);
            let beat = rng.range(-50.0, 5000.0) as f64;
            let pts = points(&shape, beat, budget);
            assert!(!pts.is_empty() && pts.len() <= budget.max(3), "seed {seed}: {} points for a budget of {budget}", pts.len());
            for p in &pts {
                assert!(p.iter().all(|v| v.is_finite()), "seed {seed}: {p:?}");
                assert!(p[0].abs() <= 1.0 && p[1].abs() <= 1.0 && (0.0..=1.0).contains(&p[2]), "seed {seed}: {p:?}");
            }
            // Through the engine and the mixer: the frame stays in budget.
            let look = Settings { content: Content::Abstract(shape), brightness: 1.0, scale: 1.0, ..Settings::default() };
            let mut a = Animator::default();
            let budget = budget.clamp(crate::layers::MIN_POINT_BUDGET, crate::layers::MAX_POINT_BUDGET);
            a.set_point_budget(budget);
            let frame = a.render(&look, AudioFeatures::default(), 1.0 / 60.0, &BeatClock { beat, ..BeatClock::default() });
            assert!(frame.iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 1.0 && p.y.abs() <= 1.0), "seed {seed}");
            let mixer = crate::layers::Mixer { point_budget: budget, ..Default::default() };
            let (mixed, _) = crate::layers::mix(vec![(1, frame)], &mixer);
            // A smooth curve short enough to draw in half the budget at
            // the densify step always fits: the sampling never overshoots.
            let Content::Abstract(shape) = &look.content else { unreachable!() };
            let clean = shape.sanitized();
            let turns = clean.effective_turns();
            let length: f32 = clean.active().filter(|o| o.target != OscTarget::Color).map(|o| 4.0 * o.amp * o.ratio * turns).sum();
            if clean.active().all(|o| o.wave == Wave::Sine) && length / 0.03 <= budget as f32 / 2.0 {
                assert!(mixed.len() <= budget, "seed {seed}: {} points mixed for a budget of {budget}", mixed.len());
            }
        }
    }
}
