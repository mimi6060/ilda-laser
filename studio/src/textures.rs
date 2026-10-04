//! Textures (T-107, looks 30 to 32 of `docs/research/festival-looks.md`):
//! a starry sky for the tension before a drop, lightning for the impacts,
//! and fat beams faking a « beam brush » without special hardware. Our own
//! maths, written from the research's description in words.
//!
//! All of them move with the tempo clock (`ctx.beat_pos`) and stay at or
//! above the horizon. Random choices come from `beat::seeded_rand`, seeded
//! by the step (or strike) number and the beam's index: the same beat
//! always draws the same frame.
//!
//! - `starfield`: `count` stars (`STAR_MIN` to `STAR_MAX`) at random places
//!   above the horizon, visited by a short path, drawn again at every step (`steps_per_beat`). Each star
//!   lives 1/8 to 1/4 beat (kept between `STAR_MIN_LIFE_S` and
//!   `STAR_MAX_LIFE_S`) and is followed at once by the next one of its
//!   slot, so there are always `count` stars lit: the frame's light never
//!   flashes, whatever the tempo. A star twinkles (its brightness swells
//!   from `TWINKLE_FLOOR` and back over its life) and drifts slowly
//!   (`STAR_DRIFT`), so no point is ever held still; with at least
//!   `STAR_MIN` stars, each gets at most a quarter of the scan time for at
//!   most 0.15 s, well under the static-beam guard's caps (`dwell.rs`).
//!   `a` > 0: stars fall at `a` units per beat (rain). `b` = lowest height
//!   above the horizon; size = width of the sky.
//! - `lightning`: a jagged line of `count` segments (6 to 10) from the top
//!   down to a low point (`b` above the horizon), lit for 2 to 4 frames on
//!   every beat (`a` < 0.5) or every bar (`a` ≥ 0.5), dark otherwise. At a
//!   tempo where that would flash faster than the strobe limit
//!   (`safety::DEFAULT_MAX_HZ`), it strikes every other beat (or less):
//!   the strobe limiter downstream never has to cut it.
//! - `fat_beams`: `count` (3 to 8, 4 to 6 advised) fan positions, each
//!   drawn as a small closed circle of radius `a` (0.01 to 0.03) so the
//!   beam looks thicker in haze. The fan sways gently (`FAT_SWAY` each
//!   side over `period_beats`), so the circles never stay still. `b` =
//!   height, size = half-width of the fan.

use crate::beat;
use crate::fans::{line, HORIZON};
use crate::generators::{BeamStyle, ColorMode, GenCtx, GenParams, Geometry, Stroke, Tint};
use std::f32::consts::TAU;

/// `starfield`: fewest stars, so each one gets a small share of the scan
/// and the frame's light stays steady as stars come and go.
pub const STAR_MIN: usize = 4;
/// `starfield`: most stars (research: 4 to 10), so a frame of random
/// places and their travel stays inside the point budget.
pub const STAR_MAX: usize = 16;
/// `starfield`: a star's life, in beats, then in seconds.
const STAR_LIFE_BEATS: (f32, f32) = (1.0 / 8.0, 1.0 / 4.0);
pub const STAR_MIN_LIFE_S: f32 = 0.05;
pub const STAR_MAX_LIFE_S: f32 = 0.15;
/// `starfield`: dimmest a star gets at the start and end of its life.
pub const TWINKLE_FLOOR: f32 = 0.6;
/// `starfield`: drift speed of a star, in frame units per second.
const STAR_DRIFT: f32 = 0.15;
/// `starfield`: most stars a slot may go through in one step (a step at
/// 20 BPM with the shortest lives); a guard on the walk, never reached.
const STAR_WALK: u32 = 4096;
/// `lightning`: number of segments.
pub const BOLT_SEGMENTS: (u32, u32) = (6, 10);
/// `lightning`: lit for this many frames of the 60 fps engine.
pub const BOLT_FRAMES: (u32, u32) = (2, 4);
pub const FRAME_S: f32 = 1.0 / 60.0;
/// `lightning`: top of the bolt, and its sideways zig-zag (× size).
const BOLT_TOP: f32 = 0.95;
const BOLT_ZAG: f32 = 0.15;
/// `fat_beams`: radius limits, beams, and sway each side.
pub const FAT_R: (f32, f32) = (0.01, 0.03);
pub const FAT_BEAMS: (u32, u32) = (3, 8);
const FAT_SWAY: f32 = 0.1;
/// `fat_beams`: segments per circle (22.5° turns: not corners for densify,
/// so no extra dwell).
const FAT_SEGMENTS: usize = 16;

/// Draw generator `name`, or `None` if it isn't one of these.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    let w = ctx.scale.clamp(0.0, 1.0);
    let geo = match name {
        "starfield" => starfield(p, ctx, w),
        "lightning" => lightning(p, ctx, w),
        "fat_beams" => fat_beams(p, ctx, w),
        _ => return None,
    };
    let strokes = geo.strokes.into_iter().map(|s| s.into_iter().map(|(x, y)| (x.clamp(-1.0, 1.0), y.clamp(HORIZON, 1.0))).collect()).collect();
    Some(Geometry { strokes, ..geo })
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Beats per second at `bpm` (120 BPM when unset).
fn beats_per_s(bpm: f32) -> f32 {
    (if bpm.is_finite() && bpm > 0.0 { bpm } else { 120.0 }) / 60.0
}

/// `b` as the lowest height above the horizon.
fn low(b: f32) -> f32 {
    HORIZON + if b.is_finite() { b.clamp(0.05, 0.9) } else { 0.1 }
}

/// Deterministic 0..1 for (`step`, `index`, `j`, `what`): one stream per
/// step and beam, `what` picks the quantity (x, y, life...).
fn rand(step: u64, index: usize, j: u32, what: u64) -> f32 {
    let seed = step.wrapping_mul(0x0001_0000_0001).wrapping_add(index as u64);
    beat::seeded_rand(seed, (j as u64) << 8 | what)
}

// ---------- starfield ----------

/// One star: where it was born, its age and life (in beats).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Star {
    pub x: f32,
    pub y: f32,
    /// 0..1 through its life.
    pub age: f32,
    /// Seconds since it appeared.
    pub age_s: f32,
    /// Drift direction, radians.
    pub heading: f32,
}

/// `starfield`'s steps per beat (4 when unset), kept sane.
fn star_steps(p: &GenParams) -> f32 {
    if p.steps_per_beat.is_finite() && p.steps_per_beat > 0.0 { p.steps_per_beat.clamp(1.0 / 16.0, 16.0) } else { 4.0 }
}

/// The stars lit at `ctx.beat_pos`: `n` of them, slot by slot.
pub fn stars(p: &GenParams, ctx: &GenCtx, n: usize, w: f32) -> Vec<Star> {
    let spb = star_steps(p);
    let step = beat::step_index(ctx.beat_pos, spb);
    let into = (ctx.beat_pos - step as f64 / spb as f64).max(0.0) as f32;
    let bps = beats_per_s(ctx.bpm);
    let (lo, hi) = (STAR_MIN_LIFE_S * bps, STAR_MAX_LIFE_S * bps);
    let life = |i: usize, j: u32| lerp(STAR_LIFE_BEATS.0, STAR_LIFE_BEATS.1, rand(step, i, j, 0)).clamp(lo, hi);
    let width = w.max(0.1);
    let floor = low(p.b);
    let band = (BOLT_TOP - floor).max(0.0) * width;
    (0..n)
        .map(|i| {
            // Staggered: the step's first star of each slot is already part
            // way through its life, so slots don't all change together.
            let mut start = -rand(step, i, 0, 1) * life(i, 0);
            let mut j = 0;
            while j < STAR_WALK && into >= start + life(i, j) {
                start += life(i, j);
                j += 1;
            }
            let l = life(i, j);
            let age_beats = (into - start).max(0.0);
            Star {
                x: lerp(-width, width, rand(step, i, j, 2)),
                y: floor + band * rand(step, i, j, 3),
                age: (age_beats / l).clamp(0.0, 1.0),
                age_s: age_beats / bps,
                heading: TAU * rand(step, i, j, 4),
            }
        })
        .collect()
}

fn starfield(p: &GenParams, ctx: &GenCtx, w: f32) -> Geometry {
    let n = (p.count as usize).clamp(STAR_MIN, STAR_MAX);
    let fall = if p.a.is_finite() { p.a.clamp(0.0, 4.0) } else { 0.0 };
    let bps = beats_per_s(ctx.bpm);
    let all = stars(p, ctx, n, w);
    let place = |s: &Star| {
        let (dy, dx) = s.heading.sin_cos();
        let drift = STAR_DRIFT * s.age_s;
        (s.x + dx * drift, s.y + dy * drift - fall * s.age_s * bps)
    };
    // In « Alterné », a star keeps its slot's colour while the drawing
    // order changes.
    let alternate = p.color_mode == ColorMode::Alternate;
    let style = |i: usize, s: &Star| BeamStyle {
        intensity: twinkle(s.age),
        tint: match (alternate, i % 2) {
            (false, _) => Tint::Auto,
            (true, 0) => Tint::Primary,
            (true, _) => Tint::Secondary,
        },
    };
    let order = short_path(&all.iter().map(place).collect::<Vec<_>>());
    let points = order.iter().map(|&i| place(&all[i])).collect();
    let styles = order.iter().map(|&i| style(i, &all[i])).collect();
    Geometry::dots(points).with_styles(styles)
}

/// A visiting order for `points`: from the leftmost, always to the
/// nearest one left, so the blanked travel between stars stays short.
fn short_path(points: &[(f32, f32)]) -> Vec<usize> {
    let mut left: Vec<usize> = (0..points.len()).collect();
    let mut order = Vec::with_capacity(points.len());
    let mut at = (-1.0f32, 0.0f32);
    while !left.is_empty() {
        let d = |i: usize| (points[i].0 - at.0).powi(2) + (points[i].1 - at.1).powi(2);
        let k = (0..left.len()).min_by(|&a, &b| d(left[a]).total_cmp(&d(left[b]))).unwrap();
        let i = left.swap_remove(k);
        at = points[i];
        order.push(i);
    }
    order
}

/// A star's brightness `age` (0..1) through its life.
pub fn twinkle(age: f32) -> f32 {
    TWINKLE_FLOOR + (1.0 - TWINKLE_FLOOR) * (std::f32::consts::PI * age.clamp(0.0, 1.0)).sin()
}

// ---------- lightning ----------

/// Beats between two strikes: one beat (`a` < 0.5) or one bar, doubled
/// until the strikes come no faster than the strobe limit.
pub fn strike_interval(p: &GenParams, bpm: f32) -> f32 {
    let mut every = if p.a.is_finite() && p.a >= 0.5 { 4.0 } else { 1.0 };
    let bps = beats_per_s(bpm);
    while every / bps < 1.0 / crate::safety::DEFAULT_MAX_HZ - 1e-6 {
        every *= 2.0;
    }
    every
}

/// The strike in progress at `beat_pos` (its number), or `None` between
/// strikes.
pub fn strike(p: &GenParams, ctx: &GenCtx) -> Option<i64> {
    let every = strike_interval(p, ctx.bpm) as f64;
    // A hair of tolerance so a strike lands exactly on its beat.
    let k = ((ctx.beat_pos + 1e-9) / every).floor();
    let age_s = (ctx.beat_pos - k * every).max(0.0) as f32 / beats_per_s(ctx.bpm);
    let k = k as i64;
    let frames = BOLT_FRAMES.0 + (rand(k as u64, 0, 0, 0) * (BOLT_FRAMES.1 - BOLT_FRAMES.0 + 1) as f32) as u32;
    (age_s < frames.min(BOLT_FRAMES.1) as f32 * FRAME_S).then_some(k)
}

/// Strike `k`'s bolt: `segments` + 1 points from the top down.
pub fn bolt(k: i64, segments: usize, floor: f32, w: f32) -> Stroke {
    let k = k as u64;
    let width = w.max(0.1);
    let top_x = lerp(-0.6, 0.6, rand(k, 1, 0, 0)) * width;
    let bottom_x = top_x + lerp(-0.4, 0.4, rand(k, 1, 0, 1)) * width;
    (0..=segments)
        .map(|j| {
            let v = j as f32 / segments as f32;
            let zag = if j == 0 || j == segments {
                0.0
            } else {
                let side = if j % 2 == 0 { 1.0 } else { -1.0 };
                side * BOLT_ZAG * width * lerp(0.3, 1.0, rand(k, 2, j as u32, 0))
            };
            (lerp(top_x, bottom_x, v) + zag, lerp(BOLT_TOP, floor, v))
        })
        .collect()
}

fn lightning(p: &GenParams, ctx: &GenCtx, w: f32) -> Geometry {
    let segments = p.count.clamp(BOLT_SEGMENTS.0, BOLT_SEGMENTS.1) as usize;
    match strike(p, ctx) {
        Some(k) => Geometry::lines(vec![bolt(k, segments, low(p.b), w)]),
        // Dark: one blanked point at the top, never an empty frame (a DAC
        // may repeat the last frame it got).
        None => Geometry::lines(vec![vec![(0.0, BOLT_TOP)]]).with_styles(vec![BeamStyle { intensity: 0.0, tint: Tint::Auto }]),
    }
}

// ---------- fat beams ----------

/// `fat_beams`' circle radius.
pub fn fat_radius(a: f32) -> f32 {
    if a.is_finite() { a.clamp(FAT_R.0, FAT_R.1) } else { 0.02 }
}

/// A closed circle (its first point repeated exactly at the end).
fn circle(cx: f32, cy: f32, r: f32) -> Stroke {
    let mut s: Stroke = (0..FAT_SEGMENTS)
        .map(|i| {
            let th = TAU * i as f32 / FAT_SEGMENTS as f32;
            (cx + r * th.cos(), cy + r * th.sin())
        })
        .collect();
    s.push(s[0]);
    s
}

fn fat_beams(p: &GenParams, ctx: &GenCtx, w: f32) -> Geometry {
    let n = p.count.clamp(FAT_BEAMS.0, FAT_BEAMS.1) as usize;
    let r = fat_radius(p.a);
    let sway = FAT_SWAY * (TAU * ctx.cycle(p)).sin();
    let y = (HORIZON + if p.b.is_finite() { p.b.clamp(0.05, 0.9) } else { 0.3 }).max(HORIZON + r + 0.02);
    let half = w.min(1.0 - FAT_SWAY - FAT_R.1);
    Geometry::lines(line(n, -half + sway, half + sway, y).into_iter().map(|(x, y)| circle(x, y, r)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dwell::{DwellGuard, DwellState, StrictArea};
    use crate::engine::densify;
    use crate::generators::{colorize, GENERATOR_NAMES};
    use crate::patterns::Point;
    use crate::safety::{level, SafetySettings, StrobeLimiter};

    const NAMES: [&str; 3] = ["starfield", "lightning", "fat_beams"];

    fn at(beat_pos: f64, scale: f32) -> GenCtx {
        GenCtx { beat_pos, ..GenCtx::at_time(0.0, 0.0, 0.0, scale) }
    }

    fn at_bpm(beat_pos: f64, bpm: f32) -> GenCtx {
        GenCtx { bpm, ..at(beat_pos, 0.7) }
    }

    fn geo(name: &str, p: &GenParams, ctx: &GenCtx) -> Geometry {
        generate(name, p, ctx).unwrap()
    }

    /// The frame as it would leave the generator stage (no optimiser).
    fn frame(name: &str, p: &GenParams, ctx: &GenCtx) -> Vec<Point> {
        densify(&colorize(&geo(name, p, ctx), p.color_mode, (1.0, 1.0, 1.0), (0.0, 0.0, 1.0), 0.0, 1.0))
    }

    fn lit(name: &str, p: &GenParams, ctx: &GenCtx) -> bool {
        frame(name, p, ctx).iter().any(|q| q.is_lit())
    }

    fn star_params() -> GenParams {
        GenParams { count: 8, a: 0.0, b: 0.1, steps_per_beat: 4.0, beat_sync: true, ..Default::default() }
    }

    fn bolt_params(a: f32) -> GenParams {
        GenParams { count: 8, a, b: 0.1, beat_sync: true, ..Default::default() }
    }

    fn fat_params() -> GenParams {
        GenParams { count: 5, a: 0.02, b: 0.3, period_beats: 8.0, beat_sync: true, ..Default::default() }
    }

    #[test]
    fn textures_are_listed_after_the_crossings() {
        let pos = |name: &str| GENERATOR_NAMES.iter().position(|n| *n == name).unwrap();
        for name in NAMES {
            assert!(pos(name) > pos("beam_v"), "{name}: appended, saved looks refer to generators by name");
        }
        assert!(generate("fan", &GenParams::default(), &at(0.0, 0.5)).is_none());
    }

    #[test]
    fn every_texture_stays_in_bounds_above_the_horizon_and_never_empty() {
        let params = [
            GenParams::default(),
            GenParams { count: 0, a: 0.0, b: -1.0, steps_per_beat: 0.0, ..Default::default() },
            GenParams { count: 64, a: 10.0, b: 3.0, period_beats: 0.0, direction: -1, steps_per_beat: 64.0, ..Default::default() },
            GenParams { count: 6, a: 1.0, b: 0.9, period_beats: 0.25, steps_per_beat: 0.01, ..Default::default() },
            GenParams { a: f32::NAN, b: f32::NAN, steps_per_beat: f32::NAN, ..Default::default() },
        ];
        for name in NAMES {
            for p in &params {
                for scale in [0.0, 0.3, 0.7, 1.0, 1.5] {
                    for k in 0..120 {
                        let beat_pos = k as f64 * 0.093 - 1.0;
                        for bpm in [20.0, 128.0, 300.0, 0.0] {
                            let g = generate(name, p, &GenCtx { bpm, ..at(beat_pos, scale) }).unwrap();
                            assert!(!g.strokes.is_empty() && g.strokes.iter().all(|s| !s.is_empty()), "{name} is never empty");
                            for &(x, y) in g.strokes.iter().flatten() {
                                assert!(x.is_finite() && y.is_finite() && x.abs() <= 1.0, "{name} x={x}");
                                assert!((HORIZON..=1.0).contains(&y), "{name} below the horizon or off the top: y={y} at beat {beat_pos}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn starfield_is_deterministic_and_drawn_again_at_every_step() {
        let p = star_params();
        // Same step, same moment: same stars, at any later call.
        for beat in [0.0, 0.3, 5.6, 101.13] {
            assert_eq!(geo("starfield", &p, &at(beat, 0.7)).strokes, geo("starfield", &p, &at(beat, 0.7)).strokes);
            assert_eq!(stars(&p, &at(beat, 0.7), 8, 0.7), stars(&p, &at(beat, 0.7), 8, 0.7));
        }
        // A new step (1/4 beat): every star is new.
        let birth = |beat: f64| stars(&p, &at(beat, 0.7), 8, 0.7).iter().map(|s| (s.x, s.y)).collect::<Vec<_>>();
        for step in 0..32 {
            let (a, b) = (birth(step as f64 * 0.25 + 0.01), birth(step as f64 * 0.25 + 0.26));
            assert!(a.iter().zip(&b).all(|(p, q)| p != q), "step {step}: all redrawn");
        }
        // The seed is the step: the step's first star of each slot is the
        // same whatever the moment inside the step it is asked at.
        let first = stars(&p, &at(8.0, 0.7), 8, 0.7);
        let later = stars(&p, &at(8.0 + 1e-4, 0.7), 8, 0.7);
        assert!(first.iter().zip(&later).all(|(a, b)| (a.x, a.y) == (b.x, b.y)));
    }

    #[test]
    fn stars_live_an_eighth_to_a_quarter_beat_and_stay_in_the_sky() {
        let p = GenParams { steps_per_beat: 0.25, ..star_params() };
        // At 120 BPM, 1/8..1/4 beat is inside the 0.05..0.15 s bounds.
        let mut changes = 0;
        let mut last = stars(&p, &at(0.0, 0.7), 8, 0.7);
        let dt = 1.0 / 256.0;
        let mut since = [0.0f64; 8];
        let mut seen = [false; 8];
        let mut whole = Vec::new();
        for k in 1..(4 * 256) {
            let now = stars(&p, &at(k as f64 * dt, 0.7), 8, 0.7);
            for i in 0..8 {
                since[i] += dt;
                if (now[i].x, now[i].y) != (last[i].x, last[i].y) {
                    changes += 1;
                    // Whole lives only (each slot's first is cut by the stagger).
                    if seen[i] {
                        whole.push(since[i]);
                    }
                    seen[i] = true;
                    since[i] = 0.0;
                }
            }
            last = now;
        }
        assert!(changes > 8 * 4 * 4, "about one star every 1/8..1/4 beat per slot: {changes}");
        assert!(whole.iter().all(|&l| (0.125 - 2.0 * dt..=0.25 + 2.0 * dt).contains(&l)), "{whole:?}");
        // In the band above the horizon, at most the size each side.
        for s in stars(&p, &at(3.3, 0.7), 8, 0.7) {
            assert!(s.x.abs() <= 0.7 && s.y >= HORIZON + 0.1 && s.y <= BOLT_TOP);
        }
    }

    #[test]
    fn stars_twinkle_and_drift_so_none_is_held_still() {
        let p = star_params();
        for bpm in [20.0f32, 60.0, 128.0, 200.0] {
            for k in 0..400 {
                let beat = k as f64 * 0.0371;
                let a = geo("starfield", &p, &at_bpm(beat, bpm));
                let b = geo("starfield", &p, &at_bpm(beat + bpm as f64 / 60.0 / 60.0, bpm));
                // One frame later, every star has moved (drifted or been replaced).
                for s in &a.strokes {
                    assert!(b.strokes.iter().all(|t| t[0] != s[0]), "{bpm} BPM, beat {beat}: a star stood still");
                }
                assert!(a.styles.iter().all(|s| (TWINKLE_FLOOR..=1.0).contains(&s.intensity)));
            }
            // A life lasts at most STAR_MAX_LIFE_S whatever the tempo.
            let slow = GenParams { steps_per_beat: 0.25, ..p.clone() };
            for k in 0..200 {
                let s = stars(&slow, &at_bpm(k as f64 * 0.05, bpm), 8, 0.7);
                assert!(s.iter().all(|s| s.age_s <= STAR_MAX_LIFE_S + 1e-3), "{bpm} BPM");
            }
        }
        assert_eq!(twinkle(0.0), TWINKLE_FLOOR);
        assert_eq!(twinkle(0.5), 1.0);
    }

    #[test]
    fn starfield_rain_falls_and_keeps_at_least_four_stars() {
        let rain = GenParams { a: 1.0, ..star_params() };
        let s0 = geo("starfield", &rain, &at(2.01, 0.7));
        let s1 = geo("starfield", &rain, &at(2.03, 0.7));
        let fell = s1.strokes.iter().filter(|b| s0.strokes.iter().any(|a| (b[0].0 - a[0].0).abs() < 0.01 && (0.005..0.05).contains(&(a[0].1 - b[0].1)))).count();
        assert!(fell >= 6, "most stars fall: {fell}");
        assert_eq!(geo("starfield", &GenParams { count: 1, ..star_params() }, &at(0.0, 0.7)).strokes.len(), STAR_MIN);
    }

    #[test]
    fn lightning_is_lit_for_2_to_4_frames_on_each_beat_then_dark() {
        let p = bolt_params(0.0);
        for bpm in [60.0f32, 120.0, 174.0] {
            let fps = 60.0;
            let mut runs = Vec::new();
            let mut run = 0;
            for f in 0..(fps as usize * 8) {
                let beat = f as f64 / fps * bpm as f64 / 60.0;
                if lit("lightning", &p, &at_bpm(beat, bpm)) {
                    run += 1;
                } else if run > 0 {
                    runs.push(run);
                    run = 0;
                }
            }
            assert!(runs.iter().all(|r| (2..=4).contains(r)), "{bpm} BPM: {runs:?}");
            assert!(runs.len() as f32 >= 8.0 * bpm / 60.0 - 1.0, "{bpm} BPM: a strike per beat, {}", runs.len());
            // Lit on the beat, dark half a beat later and just before the next.
            for beat in [0.0, 1.0, 7.0] {
                assert!(lit("lightning", &p, &at_bpm(beat, bpm)), "{bpm} BPM: strike on beat {beat}");
                assert!(!lit("lightning", &p, &at_bpm(beat + 0.5, bpm)) && !lit("lightning", &p, &at_bpm(beat + 0.99, bpm)));
            }
        }
        // `a` ≥ 0.5: once a bar.
        let bar = bolt_params(1.0);
        assert!(lit("lightning", &bar, &at(4.0, 0.7)) && !lit("lightning", &bar, &at(5.0, 0.7)) && lit("lightning", &bar, &at(8.0, 0.7)));
        // Each strike its own bolt, the same each time it is asked for.
        assert_ne!(geo("lightning", &p, &at(1.0, 0.7)).strokes, geo("lightning", &p, &at(2.0, 0.7)).strokes);
        assert_eq!(geo("lightning", &p, &at(3.0, 0.7)).strokes, geo("lightning", &p, &at(3.0, 0.7)).strokes);
    }

    #[test]
    fn lightning_zig_zags_from_the_top_down_in_6_to_10_segments() {
        for count in [1, 6, 8, 10, 40] {
            let g = geo("lightning", &GenParams { count, ..bolt_params(0.0) }, &at(2.0, 0.7));
            assert_eq!(g.strokes.len(), 1);
            let s = &g.strokes[0];
            assert_eq!(s.len() as u32 - 1, count.clamp(6, 10));
            assert!((s[0].1 - BOLT_TOP).abs() < 1e-6 && (s.last().unwrap().1 - 0.1).abs() < 1e-6);
            assert!(s.windows(2).all(|w| w[1].1 < w[0].1), "always going down");
            // Zig-zag: the sideways steps change direction.
            let turns = s.windows(3).filter(|w| (w[1].0 - w[0].0).signum() != (w[2].0 - w[1].0).signum()).count();
            assert!(turns >= 2, "{s:?}");
        }
    }

    #[test]
    fn lightning_never_strikes_faster_than_the_strobe_limit() {
        assert_eq!(strike_interval(&bolt_params(0.0), 120.0), 1.0);
        assert_eq!(strike_interval(&bolt_params(0.0), 240.0), 1.0);
        assert_eq!(strike_interval(&bolt_params(0.0), 300.0), 2.0);
        assert_eq!(strike_interval(&bolt_params(1.0), 300.0), 4.0);
        let cfg = SafetySettings::default();
        for bpm in [120.0f32, 200.0, 240.0, 300.0, 400.0] {
            let p = bolt_params(0.0);
            let mut limiter = StrobeLimiter::default();
            for f in 0..900 {
                let t = f as f64 / 60.0;
                let frame = frame("lightning", &p, &at_bpm(t * bpm as f64 / 60.0, bpm));
                let out = limiter.process(frame.clone(), t, &cfg);
                assert!(!limiter.status().active && out == frame, "{bpm} BPM: cut by the limiter at {t:.2} s");
            }
        }
    }

    #[test]
    fn stars_and_fat_beams_never_flash() {
        let cfg = SafetySettings::default();
        for (name, p) in [("starfield", star_params()), ("starfield", GenParams { count: 4, steps_per_beat: 1.0, ..star_params() }), ("fat_beams", fat_params())] {
            for bpm in [20.0f32, 90.0, 128.0, 250.0] {
                let mut limiter = StrobeLimiter::default();
                let (mut lo, mut hi) = (f32::MAX, 0.0f32);
                for f in 0..600 {
                    let t = f as f64 / 60.0;
                    let frame = frame(name, &p, &at_bpm(t * bpm as f64 / 60.0, bpm));
                    let e = level(&frame);
                    (lo, hi) = (lo.min(e), hi.max(e));
                    limiter.process(frame, t, &cfg);
                    assert!(!limiter.status().fast, "{name} flashes at {t:.2} s, {bpm} BPM");
                }
                assert!(lo > 0.35 * hi, "{name}: light drops from {hi} to {lo} at {bpm} BPM");
            }
        }
    }

    /// Through the static-beam guard (default « Faisceaux » profile, armed,
    /// horizon at 0) at 60 fps for 10 s: nothing is ever dimmed.
    #[test]
    fn textures_pass_the_static_beam_guard() {
        let guard = DwellGuard::default();
        let area = StrictArea::below(HORIZON);
        let cases = [
            ("starfield", star_params()),
            ("starfield", GenParams { count: 4, steps_per_beat: 1.0, ..star_params() }),
            ("starfield", GenParams { count: 4, steps_per_beat: 0.25, a: 0.5, ..star_params() }),
            ("lightning", bolt_params(0.0)),
            ("fat_beams", fat_params()),
            ("fat_beams", GenParams { count: 3, a: 0.03, ..fat_params() }),
        ];
        for (name, p) in &cases {
            for bpm in [20.0f32, 60.0, 128.0, 200.0] {
                let mut dwell = DwellState::default();
                let mut worst = 0.0f32;
                for f in 0..600 {
                    let t = f as f64 / 60.0;
                    let frame = frame(name, p, &at_bpm(t * bpm as f64 / 60.0, bpm));
                    dwell.apply(frame, t, &guard, &area, true);
                    worst = worst.max(dwell.status().max_dose);
                    assert_eq!(dwell.status().points_dimmed, 0, "{name} dimmed at {t:.2} s, {bpm} BPM");
                }
                assert!(worst < 0.8 * guard.max_cell_dose_beams, "{name} at {bpm} BPM: dose {worst}");
            }
        }
    }

    #[test]
    fn fat_beams_are_closed_circles_of_the_asked_radius() {
        for (a, r) in [(0.01, 0.01), (0.02, 0.02), (0.03, 0.03), (0.5, 0.03), (0.0, 0.01)] {
            for count in [4, 5, 6] {
                let g = geo("fat_beams", &GenParams { count, a, ..fat_params() }, &at(1.3, 0.7));
                assert!(!g.dots);
                assert_eq!(g.strokes.len(), count as usize);
                for s in &g.strokes {
                    assert_eq!(s.first(), s.last(), "closed");
                    let n = (s.len() - 1) as f32;
                    let (cx, cy) = (s[..s.len() - 1].iter().map(|q| q.0).sum::<f32>() / n, s[..s.len() - 1].iter().map(|q| q.1).sum::<f32>() / n);
                    assert!(s.iter().all(|q| (((q.0 - cx).powi(2) + (q.1 - cy).powi(2)).sqrt() - r).abs() < 1e-5), "radius {r}");
                    assert!((cy - 0.3).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn fat_beams_sway_as_a_fan() {
        let p = fat_params();
        let centres = |beat: f64| geo("fat_beams", &p, &at(beat, 0.7)).strokes.iter().map(|s| s[..FAT_SEGMENTS].iter().map(|q| q.0).sum::<f32>() / FAT_SEGMENTS as f32).collect::<Vec<_>>();
        let c0 = centres(0.0);
        assert!((c0[0] + 0.7).abs() < 1e-4 && (c0[4] - 0.7).abs() < 1e-4, "spread over the size: {c0:?}");
        assert!(c0.windows(2).all(|w| (w[1] - w[0] - 0.35).abs() < 1e-4), "evenly");
        let c2 = centres(2.0);
        assert!(c2.iter().zip(&c0).all(|(a, b)| (a - b - FAT_SWAY).abs() < 1e-4), "a quarter period in: swayed right");
    }

    #[test]
    fn textures_hold_30_fps_at_30_kpps() {
        let params = [
            GenParams { count: 64, a: 0.03, b: 0.05, steps_per_beat: 16.0, ..Default::default() },
            GenParams { count: 10, a: 0.0, b: 0.9, ..Default::default() },
        ];
        for name in NAMES {
            let mut worst = 0;
            for p in &params {
                for k in 0..64 {
                    worst = worst.max(frame(name, p, &at(k as f64 * 0.0625, 1.0)).len());
                }
            }
            assert!(worst <= crate::layers::DEFAULT_POINT_BUDGET, "'{name}' makes {worst} points");
        }
    }

    #[test]
    fn colours_follow_the_colour_mode() {
        let g = geo("fat_beams", &GenParams { color_mode: ColorMode::Alternate, ..fat_params() }, &at(0.0, 0.7));
        let pts = colorize(&g, ColorMode::Alternate, (1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.0, 1.0);
        assert!(pts.iter().any(|q| q.r > 0.0) && pts.iter().any(|q| q.b > 0.0), "circles alternate colours");
    }
}
