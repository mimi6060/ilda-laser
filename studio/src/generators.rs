//! Procedural effect generators: the families of looks professional laser
//! software ships as cues (abstracts, beams, tunnels, sweeps, spectrum,
//! clock...), written from scratch as maths. Each generator only produces
//! geometry - a list of strokes (polylines drawn with the beam on) plus a
//! "dot" flag for beam effects. Colour is applied afterwards by
//! `colorize`, so every generator works with every colour mode; a
//! generator may still set a per-beam intensity or colour (chases, tails,
//! odd/even) through `Geometry::styles`.
//!
//! Generators are pure functions of (`GenParams`, `GenCtx`). Musical
//! motion reads `ctx.beat_pos` (beats from the tempo clock, see
//! `beat.rs`), free-running motion reads `ctx.t`.

pub use crate::beat::GroupMode;
use crate::beat;
use crate::patterns::Point;
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};

pub type Stroke = Vec<(f32, f32)>;

/// Parameters shared by all generators. Each generator documents which
/// ones it reads; unused ones are ignored, so a preset only sets what
/// matters for its look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenParams {
    /// How many of something: beams, rings, petals, bars, lines...
    pub count: u32,
    /// Generator-specific shape parameters.
    pub a: f32,
    pub b: f32,
    /// Animation speed multiplier (0 = frozen).
    pub speed: f32,
    pub color_mode: ColorMode,
    /// Second colour for `Gradient` and `Alternate`.
    pub color2: [u8; 3],
    /// The look follows the tempo clock: `t` is derived from beats (one
    /// radian-cycle per `period_beats`) instead of seconds × `speed`, and
    /// `gate_beats` applies. Off = the free-running look of old.
    pub beat_sync: bool,
    /// Period of the main motion, in beats.
    pub period_beats: f32,
    /// Chase / position steps per beat (see `beat::step_index`).
    pub steps_per_beat: f32,
    /// Lit for this many beats after each beat, then dark. 0 = no gate.
    pub gate_beats: f32,
    /// 1 = forward, -1 = reversed motion (0 is left to generators that
    /// give it a meaning, e.g. ping-pong).
    pub direction: i8,
    /// Virtual heads: the beams split into 1..=4 contiguous groups.
    pub groups: u32,
    pub group_mode: GroupMode,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            count: 8,
            a: 3.0,
            b: 2.0,
            speed: 1.0,
            color_mode: ColorMode::Solid,
            color2: [0, 0, 255],
            beat_sync: false,
            period_beats: 4.0,
            steps_per_beat: 1.0,
            gate_beats: 0.0,
            direction: 1,
            groups: 1,
            group_mode: GroupMode::Unison,
        }
    }
}

/// Everything a generator may read besides its parameters, for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GenCtx {
    /// Animation time in seconds (already multiplied by `speed`), or
    /// derived from beats when the look is `beat_sync`.
    pub t: f32,
    /// Beats since the cue started, counted from the first beat of the bar
    /// it started in (so whole beats are the tempo clock's beats and a
    /// multiple of 4 is a "one"). Read from the one tempo clock.
    pub beat_pos: f64,
    pub bpm: f32,
    /// 0..1 audio features (0 when audio is off).
    pub level: f32,
    pub bass: f32,
    /// The look's half-extent.
    pub scale: f32,
}

// Beat helpers for the festival generators (T-102..T-108).
#[cfg_attr(not(test), allow(dead_code))]
impl GenCtx {
    /// A context for a free-running look (beat 0 at 120 BPM).
    pub fn at_time(t: f32, level: f32, bass: f32, scale: f32) -> Self {
        Self { t, beat_pos: 0.0, bpm: 120.0, level, bass, scale }
    }

    /// Position in the main motion's cycle, 0..1, running backwards when
    /// `direction` is negative.
    pub fn cycle(&self, p: &GenParams) -> f32 {
        let ph = beat::phase(self.beat_pos, p.period_beats);
        if p.direction < 0 { (1.0 - ph).rem_euclid(1.0) } else { ph }
    }

    /// Current chase / position step.
    pub fn step(&self, p: &GenParams) -> u64 {
        beat::step_index(self.beat_pos, p.steps_per_beat)
    }

    /// Position inside the current beat, 0..1.
    pub fn beat_phase(&self) -> f32 {
        self.beat_pos.rem_euclid(1.0) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// The look's main colour everywhere.
    Solid,
    /// Hue runs along the drawing and drifts over time.
    Rainbow,
    /// Main colour → `color2` along the drawing.
    Gradient,
    /// Strokes alternate between the main colour and `color2`.
    Alternate,
}

/// Names accepted by `generate`, in UI order.
pub const GENERATOR_NAMES: &[&str] = &[
    "lissajous", "spirograph", "rose", "tunnel", "polygon_tunnel", "spiral_arms", "starburst",
    "beam_fan", "beam_circle", "beam_wave", "sweep", "liquid_sky", "grid_scan", "sine_stack",
    "helix", "pulse_rings", "spectrum", "clock", "vortex", "flower",
];

pub struct Geometry {
    pub strokes: Vec<Stroke>,
    /// Beam looks: each stroke is a single point held for a while, so it
    /// reads as a bright beam in haze.
    pub dots: bool,
    /// Per-stroke (per-beam for `dots`) intensity and colour. Empty, or
    /// shorter than `strokes`, means the default style for the rest.
    pub styles: Vec<BeamStyle>,
}

impl Geometry {
    pub fn lines(strokes: Vec<Stroke>) -> Self {
        Self { strokes, dots: false, styles: Vec::new() }
    }

    pub fn dots(points: Vec<(f32, f32)>) -> Self {
        Self { strokes: points.into_iter().map(|pt| vec![pt]).collect(), dots: true, styles: Vec::new() }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_styles(self, styles: Vec<BeamStyle>) -> Self {
        Self { styles, ..self }
    }
}

/// How one stroke or beam is lit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeamStyle {
    /// 0..1 multiplier; 0 = dark (the galvo still visits it, blanked, so
    /// the geometry and its timing stay the same - chases rely on this).
    pub intensity: f32,
    pub tint: Tint,
}

impl Default for BeamStyle {
    fn default() -> Self {
        Self { intensity: 1.0, tint: Tint::Auto }
    }
}

/// Colour of a stroke or beam.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tint {
    /// Whatever the colour mode gives.
    #[default]
    Auto,
    /// The look's main colour.
    Primary,
    /// `color2`.
    Secondary,
    /// A fixed colour, 0..1 (e.g. a white flash).
    Rgb(f32, f32, f32),
}

/// Draw generator `name` for one frame. Unknown names give `None`.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    let GenCtx { t, level, bass, scale, .. } = *ctx;
    let n = p.count.clamp(1, 64) as usize;
    let lines = |strokes: Vec<Stroke>| Some(Geometry::lines(strokes));
    let dots = |points: Vec<(f32, f32)>| Some(Geometry::dots(points));

    match name {
        // a:b frequency ratio; the phase drifts so the figure turns in 3D.
        "lissajous" => lines(vec![sample(240, |u| {
            let th = u * TAU;
            (scale * (p.a * th + t).sin(), scale * (p.b * th).sin())
        })]),

        // Hypotrochoid: a = ring/wheel ratio, b = pen offset.
        "spirograph" => {
            let k = p.a.max(1.1);
            let turns = spirograph_turns(k);
            lines(vec![sample(240 * turns, |u| {
                let th = u * TAU * turns as f32;
                let (r_big, r_small) = (1.0, 1.0 / k);
                let d = p.b.clamp(0.1, 2.0) * r_small;
                let x = (r_big - r_small) * th.cos() + d * ((r_big - r_small) / r_small * th).cos();
                let y = (r_big - r_small) * th.sin() - d * ((r_big - r_small) / r_small * th).sin();
                let norm = scale / (r_big - r_small + d);
                rotate((x * norm, y * norm), t * 0.3)
            })])
        }

        // Rhodonea curve with `count` petals, breathing with the bass.
        "rose" => {
            let k = n as f32;
            let pulse = 0.8 + 0.2 * (t * 2.0).sin() + 0.3 * bass;
            let full = if n % 2 == 1 { PI } else { TAU };
            lines(vec![sample(300, |u| {
                let th = u * full;
                let r = scale * pulse.min(1.0) * (k * th).cos();
                rotate((r * th.cos(), r * th.sin()), t * 0.4)
            })])
        }

        // Concentric circles flowing outward: the classic tunnel.
        "tunnel" => lines(
            (0..n)
                .map(|i| {
                    let r = scale * ((i as f32 / n as f32 + t * 0.25).fract()).max(0.05);
                    circle_stroke(r, 0.0, 0.0, 48)
                })
                .collect(),
        ),

        // Rotating nested polygons with `a` sides, each ring twisted a bit more.
        "polygon_tunnel" => {
            let sides = (p.a.round() as usize).clamp(3, 12);
            lines(
                (0..n)
                    .map(|i| {
                        let f = (i as f32 + 1.0) / n as f32;
                        polygon_stroke(sides, scale * f, t * 0.5 + f * p.b)
                    })
                    .collect(),
            )
        }

        // `count` curved arms spinning around the centre.
        "spiral_arms" => lines(
            (0..n)
                .map(|i| {
                    let base = i as f32 / n as f32 * TAU + t;
                    sample(60, |u| {
                        let r = scale * u;
                        let a = base + u * p.a.max(0.5) * PI;
                        (r * a.cos(), r * a.sin())
                    })
                })
                .collect(),
        ),

        // Radial rays whose length pulses with the music.
        "starburst" => lines(
            (0..n)
                .map(|i| {
                    let a = i as f32 / n as f32 * TAU + t * 0.5;
                    let inner = scale * 0.15;
                    let outer = scale * (0.55 + 0.45 * ((t * 3.0 + i as f32).sin() * 0.5 + 0.5).max(level));
                    vec![(inner * a.cos(), inner * a.sin()), (outer * a.cos(), outer * a.sin())]
                })
                .collect(),
        ),

        // A horizontal line of beams that opens and closes like a fan.
        "beam_fan" => {
            let spread = scale * (0.4 + 0.6 * (t.sin() * 0.5 + 0.5)).max(bass);
            dots((0..n).map(|i| (lerp(-spread, spread, frac(i, n)), p.b.clamp(-1.0, 1.0) * scale)).collect())
        }

        // Beams arranged on a rotating circle: a cone in haze.
        "beam_circle" => dots(
            (0..n)
                .map(|i| {
                    let a = frac_loop(i, n) * TAU + t;
                    (scale * a.cos(), scale * a.sin() * 0.35)
                })
                .collect(),
        ),

        // Beams riding a sine wave that travels sideways.
        "beam_wave" => dots(
            (0..n)
                .map(|i| {
                    let x = lerp(-scale, scale, frac(i, n));
                    (x, scale * 0.5 * (x * p.a + t * 2.0).sin())
                })
                .collect(),
        ),

        // One line sweeping left-right across the room.
        "sweep" => {
            let x = scale * (t * 1.5).sin();
            lines(vec![vec![(x, -scale), (x, scale)]])
        }

        // A flat, gently rippling line - reads as a "liquid sky" sheet in haze.
        "liquid_sky" => lines(vec![sample(120, |u| {
            let x = lerp(-scale, scale, u);
            (x, p.b.clamp(-1.0, 1.0) * scale + 0.03 * scale * (x * 8.0 + t * 3.0).sin())
        })]),

        // Scanning lines moving through a grid.
        "grid_scan" => {
            let mut strokes = Vec::new();
            for i in 0..n {
                let y = lerp(-scale, scale, ((frac_loop(i, n)) + t * 0.2).fract());
                strokes.push(vec![(-scale, y), (scale, y)]);
            }
            if p.a >= 1.0 {
                for i in 0..n {
                    let x = lerp(-scale, scale, ((frac_loop(i, n)) + t * 0.2).fract());
                    strokes.push(vec![(x, -scale), (x, scale)]);
                }
            }
            lines(strokes)
        }

        // Stacked sine waves, each a little out of phase: an "oscillator stack".
        "sine_stack" => lines(
            (0..n)
                .map(|i| {
                    let y0 = lerp(-scale * 0.7, scale * 0.7, frac(i, n));
                    let amp = scale * (0.12 + 0.3 * level);
                    sample(100, |u| {
                        let x = lerp(-scale, scale, u);
                        (x, y0 + amp * (x * p.a * 3.0 + t * 2.0 + i as f32 * p.b).sin())
                    })
                })
                .collect(),
        ),

        // Two intertwined strands: a DNA helix.
        "helix" => lines(
            (0..2)
                .map(|strand| {
                    let phase = strand as f32 * PI;
                    sample(140, |u| {
                        let x = lerp(-scale, scale, u);
                        (x, scale * 0.45 * (x * p.a * 2.0 + t * 2.0 + phase).sin())
                    })
                })
                .collect(),
        ),

        // Rings that pop out from the centre on each pulse.
        "pulse_rings" => lines(
            (0..n)
                .map(|i| {
                    let phase = (t * 0.8 + frac_loop(i, n)).fract();
                    circle_stroke(scale * (phase.max(0.03) * (1.0 + 0.3 * bass)).min(1.0), 0.0, 0.0, 60)
                })
                .collect(),
        ),

        // Audio bars. We only have level and bass, so each bar mixes them
        // with its own wobble - reads as a spectrum without an FFT.
        "spectrum" => lines(
            (0..n)
                .map(|i| {
                    let x = lerp(-scale, scale, frac(i, n));
                    let weight = 1.0 - frac(i, n);
                    let wobble = ((t * 7.0 + i as f32 * 1.7).sin() * 0.5 + 0.5) * 0.25;
                    let h = scale * (0.08 + 1.4 * (bass * weight + level * (1.0 - weight)) + wobble).min(1.9);
                    vec![(x, -scale), (x, -scale + h)]
                })
                .collect(),
        ),

        // A real analogue clock (local time from the system clock).
        "clock" => {
            let secs = clock_seconds();
            let mut strokes = vec![circle_stroke(scale, 0.0, 0.0, 90)];
            for h in 0..12 {
                let a = h as f32 / 12.0 * TAU;
                strokes.push(vec![(0.85 * scale * a.sin(), 0.85 * scale * a.cos()), (scale * a.sin(), scale * a.cos())]);
            }
            let hand = |frac: f32, len: f32| {
                let a = frac * TAU;
                vec![(0.0, 0.0), (len * scale * a.sin(), len * scale * a.cos())]
            };
            strokes.push(hand((secs / 43_200.0).fract(), 0.5));
            strokes.push(hand((secs / 3_600.0).fract(), 0.75));
            strokes.push(hand((secs / 60.0).fract(), 0.9));
            lines(strokes)
        }

        // A spiral that winds and unwinds.
        "vortex" => {
            let turns = 3.0 + 2.0 * (t * 0.5).sin();
            lines(vec![sample(300, |u| {
                let r = scale * u;
                let a = u * turns * TAU + t * 2.0;
                (r * a.cos(), r * a.sin())
            })])
        }

        // Overlapping circles around the centre (flower of life style).
        "flower" => lines(
            (0..n)
                .map(|i| {
                    let a = frac_loop(i, n) * TAU + t * 0.3;
                    let r = scale * 0.5;
                    circle_stroke(r, r * a.cos(), r * a.sin(), 60)
                })
                .collect(),
        ),

        _ => None,
    }
}

/// Turn geometry into laser points: colour each point, and join strokes
/// with blanked jumps. `hue_time` makes rainbow colours drift. `c1`/`c2`
/// arrive already dimmed; `gain` dims the colours made here (rainbow,
/// `Tint::Rgb`). Per-stroke styles set the intensity and tint.
pub fn colorize(
    geo: &Geometry,
    mode: ColorMode,
    c1: (f32, f32, f32),
    c2: (f32, f32, f32),
    hue_time: f32,
    gain: f32,
) -> Vec<Point> {
    const DOT_DWELL: usize = 12;
    let total: usize = geo.strokes.iter().map(|s| s.len()).sum::<usize>().max(1);
    let mut out = Vec::with_capacity(total * 2);
    let mut index = 0usize;

    for (si, stroke) in geo.strokes.iter().enumerate() {
        if let (Some(last), Some(&first)) = (out.last().copied(), stroke.first()) {
            let last: Point = last;
            out.push(Point::blanked(last.x, last.y));
            out.push(Point::blanked(first.0, first.1));
        }
        let style = geo.styles.get(si).copied().unwrap_or_default();
        for &(x, y) in stroke {
            let along = index as f32 / total as f32;
            let (r, g, b) = match (style.tint, mode) {
                (Tint::Primary, _) | (Tint::Auto, ColorMode::Solid) => c1,
                (Tint::Secondary, _) => c2,
                (Tint::Rgb(r, g, b), _) => (r * gain, g * gain, b * gain),
                (Tint::Auto, ColorMode::Rainbow) => {
                    let (r, g, b) = hsv((along + hue_time * 0.1).fract(), 1.0, 1.0);
                    (r * gain, g * gain, b * gain)
                }
                (Tint::Auto, ColorMode::Gradient) => mix(c1, c2, along),
                (Tint::Auto, ColorMode::Alternate) => {
                    if si % 2 == 0 { c1 } else { c2 }
                }
            };
            let k = style.intensity.clamp(0.0, 1.0);
            let point = if k <= 0.0 {
                Point::blanked(x, y)
            } else if k < 1.0 {
                Point::lit(x, y, r * k, g * k, b * k)
            } else {
                Point::lit(x, y, r, g, b)
            };
            let reps = if geo.dots { DOT_DWELL } else { 1 };
            for _ in 0..reps {
                out.push(point);
            }
            index += 1;
        }
    }
    out
}

fn sample(n: usize, f: impl Fn(f32) -> (f32, f32)) -> Stroke {
    (0..=n).map(|i| f(i as f32 / n as f32)).collect()
}

fn circle_stroke(r: f32, cx: f32, cy: f32, n: usize) -> Stroke {
    sample(n, |u| (cx + r * (u * TAU).cos(), cy + r * (u * TAU).sin()))
}

fn polygon_stroke(sides: usize, r: f32, angle: f32) -> Stroke {
    (0..=sides)
        .map(|i| {
            let a = i as f32 / sides as f32 * TAU + angle;
            (r * a.cos(), r * a.sin())
        })
        .collect()
}

fn rotate((x, y): (f32, f32), a: f32) -> (f32, f32) {
    let (s, c) = a.sin_cos();
    (x * c - y * s, x * s + y * c)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// i / (n - 1): evenly spread from 0 to 1 inclusive.
fn frac(i: usize, n: usize) -> f32 {
    if n <= 1 { 0.5 } else { i as f32 / (n - 1) as f32 }
}

/// i / n: evenly spread around a loop (0 and 1 would coincide).
fn frac_loop(i: usize, n: usize) -> f32 {
    i as f32 / n as f32
}

/// Full turns needed for a hypotrochoid with ring/wheel ratio `k` to close
/// (capped so irrational-looking ratios don't explode the point count).
fn spirograph_turns(k: f32) -> usize {
    for q in 1..=6usize {
        let p = k * q as f32;
        if (p - p.round()).abs() < 0.02 {
            return q;
        }
    }
    6
}

fn mix(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t))
}

pub fn hsv(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = h6 - h6.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match h6.floor() as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// Seconds since local midnight-ish (UTC offset is not known without a
/// timezone crate, so this is UTC; good enough for a spinning clock).
fn clock_seconds() -> f32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    (now % 86_400.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_generator_produces_points_in_bounds() {
        let p = GenParams::default();
        for name in GENERATOR_NAMES {
            for t in [0.0, 1.3, 7.9] {
                let geo = generate(name, &p, &GenCtx::at_time(t, 0.5, 0.5, 0.8)).unwrap_or_else(|| panic!("'{name}' not handled"));
                let pts = colorize(&geo, ColorMode::Rainbow, (1.0, 0.0, 0.0), (0.0, 0.0, 1.0), t, 1.0);
                assert!(!pts.is_empty(), "'{name}' is empty");
                assert!(
                    pts.iter().all(|q| q.x.abs() <= 1.0001 && q.y.abs() <= 1.0001 && q.x.is_finite() && q.y.is_finite()),
                    "'{name}' leaves the -1..1 range at t={t}"
                );
            }
        }
    }

    #[test]
    fn unknown_generator_is_none() {
        assert!(generate("nope", &GenParams::default(), &GenCtx::at_time(0.0, 0.0, 0.0, 0.5)).is_none());
    }

    #[test]
    fn strokes_are_joined_by_blanked_jumps() {
        let geo = Geometry { strokes: vec![vec![(0.0, 0.0), (0.1, 0.0)], vec![(0.5, 0.5), (0.6, 0.5)]], dots: false, styles: Vec::new() };
        let pts = colorize(&geo, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0, 1.0);
        assert_eq!(pts.len(), 6);
        assert!(!pts[2].is_lit() && !pts[3].is_lit());
        assert_eq!((pts[3].x, pts[3].y), (0.5, 0.5));
    }

    #[test]
    fn dots_are_held_so_they_read_as_beams() {
        let geo = generate("beam_fan", &GenParams { count: 4, ..Default::default() }, &GenCtx::at_time(0.0, 0.0, 0.0, 0.5)).unwrap();
        assert!(geo.dots);
        let pts = colorize(&geo, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0, 1.0);
        assert!(pts.iter().filter(|p| p.is_lit()).count() >= 4 * 10);
    }

    #[test]
    fn alternate_mode_switches_colour_per_stroke() {
        let geo = generate("tunnel", &GenParams { count: 2, ..Default::default() }, &GenCtx::at_time(0.0, 0.0, 0.0, 0.5)).unwrap();
        let pts = colorize(&geo, ColorMode::Alternate, (1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.0, 1.0);
        let lit: Vec<_> = pts.iter().filter(|p| p.is_lit()).collect();
        assert_eq!(lit.first().unwrap().r, 1.0);
        assert_eq!(lit.last().unwrap().b, 1.0);
    }

    #[test]
    fn gradient_goes_from_first_to_second_colour() {
        let geo = generate("sweep", &GenParams::default(), &GenCtx::at_time(0.0, 0.0, 0.0, 0.5)).unwrap();
        let pts = colorize(&geo, ColorMode::Gradient, (1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.0, 1.0);
        assert_eq!((pts[0].r, pts[0].b), (1.0, 0.0));
        assert!(pts[1].b > 0.0);
    }

    #[test]
    fn spirograph_turn_count_closes_simple_ratios() {
        assert_eq!(spirograph_turns(3.0), 1);
        assert_eq!(spirograph_turns(2.5), 2);
        assert_eq!(spirograph_turns(std::f32::consts::E), 6);
    }

    /// Geometry of every generator (clock excluded: system time) for a few
    /// parameter sets and times, rounded to 1e-3 and hashed (FNV-1a).
    fn geometry_digest(render: impl Fn(&str, &GenParams, f32) -> Geometry) -> u64 {
        let params = [
            GenParams::default(),
            GenParams { count: 5, a: 2.5, b: 0.4, ..Default::default() },
            GenParams { count: 13, a: 0.7, b: -0.3, ..Default::default() },
        ];
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for name in GENERATOR_NAMES.iter().filter(|n| **n != "clock") {
            for p in &params {
                for t in [0.0, 0.37, 2.9, 11.2] {
                    let geo = render(name, p, t);
                    for v in geo.strokes.iter().flatten().flat_map(|&(x, y)| [x, y]).chain([geo.dots as u8 as f32]) {
                        h = (h ^ (v * 1000.0).round() as i64 as u64).wrapping_mul(0x0100_0000_01b3);
                    }
                }
            }
        }
        h
    }

    #[test]
    fn existing_generators_draw_exactly_what_they_did() {
        let digest = geometry_digest(|name, p, t| generate(name, p, &GenCtx::at_time(t, 0.4, 0.3, 0.7)).unwrap());
        assert_eq!(digest, 16_525_639_321_235_901_176, "a generator changed its drawing");
    }

    #[test]
    fn beam_styles_set_intensity_and_colour_per_beam() {
        let geo = Geometry::dots(vec![(0.0, 0.0), (0.1, 0.0), (0.2, 0.0), (0.3, 0.0)]).with_styles(vec![
            BeamStyle { intensity: 0.0, tint: Tint::Auto },
            BeamStyle { intensity: 0.4, tint: Tint::Secondary },
            BeamStyle { intensity: 1.0, tint: Tint::Rgb(1.0, 1.0, 1.0) },
        ]);
        let pts = colorize(&geo, ColorMode::Solid, (1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.0, 0.5);
        let at = |x: f32| pts.iter().filter(|p| (p.x - x).abs() < 1e-6 && p.y == 0.0).copied().collect::<Vec<_>>();
        assert!(at(0.0).iter().all(|p| !p.is_lit()), "intensity 0 is dark");
        assert!(at(0.0).len() > 12, "a dark beam keeps its dwell (plus travel), so the timing stays stable");
        // Lit points only: the blanked travel between beams also passes there.
        let lit = |x: f32| at(x).into_iter().filter(|p| p.is_lit()).collect::<Vec<_>>();
        assert!(lit(0.1).len() == 12 && lit(0.1).iter().all(|p| (p.b - 0.4).abs() < 1e-6 && p.r == 0.0), "40 % of color2");
        assert!(lit(0.2).len() == 12 && lit(0.2).iter().all(|p| (p.r, p.g, p.b) == (0.5, 0.5, 0.5)), "fixed colours are dimmed by gain");
        assert!(lit(0.3).len() == 12 && lit(0.3).iter().all(|p| (p.r, p.g, p.b) == (1.0, 0.0, 0.0)), "missing styles fall back to the colour mode");
        // Primary overrides the colour mode (Alternate would give color2 here).
        let geo = Geometry::dots(vec![(0.0, 0.0), (0.5, 0.0)]).with_styles(vec![BeamStyle::default(), BeamStyle { intensity: 1.0, tint: Tint::Primary }]);
        let pts = colorize(&geo, ColorMode::Alternate, (1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.0, 1.0);
        assert!(pts.iter().filter(|p| p.is_lit()).all(|p| p.r == 1.0));
    }

    #[test]
    fn old_generator_params_load_with_beat_defaults() {
        let p: GenParams = serde_json::from_str(r#"{"count":5,"a":1.5,"b":0.2,"speed":2.0,"color_mode":"rainbow","color2":[1,2,3]}"#).unwrap();
        assert_eq!((p.count, p.speed, p.color_mode), (5, 2.0, ColorMode::Rainbow));
        assert!(!p.beat_sync);
        assert_eq!((p.period_beats, p.steps_per_beat, p.gate_beats, p.direction), (4.0, 1.0, 0.0, 1));
        assert_eq!((p.groups, p.group_mode), (1, GroupMode::Unison));
        let json = serde_json::to_string(&GenParams { beat_sync: true, group_mode: GroupMode::Mirror, ..Default::default() }).unwrap();
        assert!(json.contains(r#""group_mode":"mirror""#), "{json}");
        assert_eq!(serde_json::from_str::<GenParams>(&json).unwrap().group_mode, GroupMode::Mirror);
    }

    #[test]
    fn ctx_reads_cycle_steps_and_beat_phase_from_beats() {
        let p = GenParams { period_beats: 4.0, steps_per_beat: 2.0, ..Default::default() };
        let ctx = GenCtx { beat_pos: 5.25, ..GenCtx::at_time(0.0, 0.0, 0.0, 0.5) };
        assert!((ctx.cycle(&p) - 0.3125).abs() < 1e-6);
        assert!((ctx.cycle(&GenParams { direction: -1, ..p.clone() }) - 0.6875).abs() < 1e-6, "reversed");
        assert_eq!(ctx.step(&p), 10);
        assert!((ctx.beat_phase() - 0.25).abs() < 1e-6);
    }

    /// A tiny beat-synced look built only from the shared toolkit, the way
    /// the festival generators (T-102..T-108) will be: a fan whose groups
    /// sweep with the group mode, plus a one-beam-per-step chase.
    fn test_fan(p: &GenParams, ctx: &GenCtx) -> Geometry {
        let n = p.count as usize;
        let cycle = ctx.cycle(p);
        let points = (0..n)
            .map(|i| {
                let g = beat::group_of(i, n, p.groups);
                let (ph, sign) = p.group_mode.motion(g, p.groups, cycle);
                let x = lerp(-0.2, 0.2, frac(i, n)) + sign * 0.3 * (ph * TAU).sin();
                (x, 0.3)
            })
            .collect();
        let lit = (ctx.step(p) % n as u64) as usize;
        let styles = (0..n).map(|i| BeamStyle { intensity: if i == lit { 1.0 } else { 0.0 }, ..Default::default() }).collect();
        Geometry::dots(points).with_styles(styles)
    }

    #[test]
    fn toolkit_expresses_mirrored_groups_and_a_beat_chase() {
        let p = GenParams { count: 4, groups: 2, group_mode: GroupMode::Mirror, beat_sync: true, ..Default::default() };
        let ctx = GenCtx { beat_pos: 1.0, ..GenCtx::at_time(0.0, 0.0, 0.0, 0.5) }; // a quarter cycle: full swing
        let geo = test_fan(&p, &ctx);
        let centre = |g: std::ops::Range<usize>| g.clone().map(|i| geo.strokes[i][0].0).sum::<f32>() / g.len() as f32;
        let (left, right) = (centre(0..2), centre(2..4));
        assert!(left > 0.1 && (left + right).abs() < 1e-5, "mirrored groups: {left} vs {right}");
        // Step 1 of the chase lights beam 1 only.
        let pts = colorize(&geo, ColorMode::Solid, (0.0, 1.0, 0.0), (0.0, 0.0, 0.0), 0.0, 1.0);
        let lit_x: Vec<f32> = pts.iter().filter(|q| q.is_lit()).map(|q| q.x).collect();
        assert!(lit_x.iter().all(|&x| x == geo.strokes[1][0].0) && lit_x.len() == 12);
        // Unison: both groups move together.
        let geo = test_fan(&GenParams { group_mode: GroupMode::Unison, ..p }, &ctx);
        assert!((geo.strokes[0][0].0 - (-0.2 + 0.3)).abs() < 1e-5 && geo.strokes[3][0].0 > 0.4);
    }

    #[test]
    fn every_generator_stays_within_a_sane_point_budget() {
        let p = GenParams { count: 16, ..Default::default() };
        for name in GENERATOR_NAMES {
            let geo = generate(name, &p, &GenCtx::at_time(0.5, 0.5, 0.5, 0.8)).unwrap();
            let n = colorize(&geo, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0, 1.0).len();
            assert!(n <= 3000, "'{name}' makes {n} points");
        }
    }
}
