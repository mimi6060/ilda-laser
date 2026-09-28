//! Festival sheets (T-106): flat scans that read as planes of light in
//! haze - ceilings, blades, curtains, falling lines, scanner bars, slats,
//! aurora and nets - as beat-synced line generators. Each is our own
//! maths, written from the look descriptions in
//! `docs/research/festival-looks.md` section C.
//!
//! All of them are lines (not `dots`), move with the tempo clock through
//! `ctx.beat_pos` (their motion is defined in beats), and stay at or above
//! the horizon (`fans::HORIZON`): they are meant to be seen over the
//! audience, never to scan it. Widths follow the look size (`ctx.scale`);
//! heights are frame units above the horizon. Parameters:
//!
//! - `ceiling` (liquid sky v2): a flat line at height `b` (0.05..0.6) with a
//!   ripple of amplitude `a` (0..0.03) travelling 0.25 cycle per beat. With
//!   `beat_sync`, the brightness also breathes 0.7..1 over 8 beats.
//! - `blade`: a line through the look's centre, raised to height `b`,
//!   tilting ±30° over `period_beats` (`easing`); what would dip below the
//!   horizon is cut off.
//! - `curtain`: `count` (≤ 5) vertical strokes 0.6 high, 0.35 apart at size
//!   0.7, from `b` above the horizon; `a` slides them sideways over the
//!   period.
//! - `waterfall`: short horizontal strokes that fall from 0.9 to `b` above
//!   the horizon in exactly 2 beats; a new one starts every step
//!   (`steps_per_beat`, 2 = every 1/2 beat), in `count` staggered lanes.
//! - `scanner`: a horizontal bar rising and falling (`a` < 0.5) or a
//!   vertical bar crossing the width (`a` ≥ 0.5), one pass per
//!   `period_beats`, back and forth or wrapping (`loop_mode`).
//! - `slats`: a line at height `b` cut into `count` (2..16) segments, half
//!   of them lit in runs of `a` segments; the pattern moves one segment per
//!   step (`steps_per_beat`, 2 = every 1/2 beat).
//! - `aurora`: a line whose height is a sum of three slow sines (0.03,
//!   0.05 and 0.08 cycle per beat), amplitude `a` (≤ 0.25) around `b`.
//! - `grid`: `count` horizontal and `a` vertical lines (≤ 6 each) in a box
//!   above the horizon, scrolling one line per beat.
//!
//! Brightness never flashes: every look keeps the same amount of lit line
//! from frame to frame (the slats move their gaps, the waterfall starts a
//! line as another one lands), and the ceiling's breath is 8 beats long.

use crate::fans::HORIZON;
use crate::generators::{BeamStyle, GenCtx, GenParams, Geometry, Stroke};
use std::f32::consts::TAU;

/// `ceiling`: height range above the horizon, largest ripple, ripple
/// speed (cycles per beat) and wavelengths across the width.
const CEILING_LOW: f32 = 0.05;
const CEILING_HIGH: f32 = 0.6;
const CEILING_RIPPLE: f32 = 0.03;
const CEILING_SPEED: f64 = 0.25;
const CEILING_WAVES: f32 = 2.0;
/// `ceiling` breath: brightness 0.7..1 over this many beats.
const BREATH_BEATS: f64 = 8.0;
const BREATH_DEPTH: f32 = 0.3;
/// `blade`: largest tilt either way.
const BLADE_TILT: f32 = 30.0 * std::f32::consts::PI / 180.0;
/// `curtain`: at most this many, this tall, this far apart at size 0.7.
const CURTAIN_MAX: usize = 5;
const CURTAIN_HEIGHT: f32 = 0.6;
const CURTAIN_SPACING: f32 = 0.35 / 0.7;
/// `waterfall`: lines start at this height and land after this many beats.
const FALL_TOP: f32 = 0.9;
const FALL_BEATS: f64 = 2.0;
/// `waterfall`: lanes across the width, and starts per beat (at least one
/// line is always falling).
const MAX_LANES: usize = 8;
const MIN_SPAWN_PER_BEAT: f32 = 0.5;
const MAX_SPAWN_PER_BEAT: f32 = 4.0;
/// `slats`: segments.
const MAX_SLATS: usize = 16;
/// `aurora`: component speeds (cycles per beat), wavelengths across the
/// width, weights (summing to 1, so the height stays within ±`a`) and
/// phases.
const AURORA_SPEEDS: [f64; 3] = [0.03, 0.05, 0.08];
const AURORA_WAVES: [f32; 3] = [1.0, 1.5, 2.3];
const AURORA_WEIGHTS: [f32; 3] = [0.5, 0.3, 0.2];
const AURORA_PHASES: [f32; 3] = [0.0, 1.3, 2.9];
const AURORA_MAX: f32 = 0.25;
/// `grid`: at most this many lines each way.
const GRID_MAX: usize = 6;
/// Samples along a curved line (ceiling, aurora).
const CURVE_SAMPLES: usize = 100;

/// Draw sheet generator `name`, or `None` if it isn't one.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    let w = ctx.scale.clamp(0.0, 1.0);
    let geo = match name {
        "ceiling" => ceiling(p, ctx, w),
        "blade" => Geometry::lines(vec![blade(p, ctx, w)]),
        "curtain" => Geometry::lines(curtain(p, ctx, w)),
        "waterfall" => Geometry::lines(waterfall(p, ctx, w)),
        "scanner" => Geometry::lines(vec![scanner(p, ctx, w)]),
        "slats" => slats(p, ctx, w),
        "aurora" => Geometry::lines(vec![aurora(p, ctx, w)]),
        "grid" => Geometry::lines(grid(p, ctx, w)),
        _ => return None,
    };
    let strokes = geo.strokes.into_iter().map(|s| s.into_iter().map(|(x, y)| (x.clamp(-1.0, 1.0), y.clamp(HORIZON, 1.0))).collect()).collect();
    Some(Geometry { strokes, ..geo })
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A box above the horizon for the travelling sheets: `b` is its bottom,
/// its height the look size.
fn sheet_box(b: f32, w: f32) -> (f32, f32) {
    let lo = HORIZON + b.clamp(0.02, 0.9);
    (lo, (lo + w).min(1.0))
}

fn sample(n: usize, f: impl Fn(f32) -> (f32, f32)) -> Stroke {
    (0..=n).map(|i| f(i as f32 / n as f32)).collect()
}

fn dir(p: &GenParams) -> f64 {
    if p.direction < 0 { -1.0 } else { 1.0 }
}

fn ceiling(p: &GenParams, ctx: &GenCtx, w: f32) -> Geometry {
    let y0 = HORIZON + p.b.clamp(CEILING_LOW, CEILING_HIGH);
    let amp = p.a.clamp(0.0, CEILING_RIPPLE);
    let travel = (dir(p) * ctx.beat_pos * CEILING_SPEED).rem_euclid(1.0) as f32;
    let line = sample(CURVE_SAMPLES, |u| (lerp(-w, w, u), y0 + amp * (TAU * (CEILING_WAVES * u - travel)).sin()));
    let geo = Geometry::lines(vec![line]);
    if !p.beat_sync {
        return geo;
    }
    let breath = (TAU as f64 * ctx.beat_pos / BREATH_BEATS).sin() as f32;
    geo.with_styles(vec![BeamStyle { intensity: 1.0 - BREATH_DEPTH * 0.5 * (1.0 - breath), ..Default::default() }])
}

fn blade(p: &GenParams, ctx: &GenCtx, w: f32) -> Stroke {
    let yc = HORIZON + p.b.clamp(0.05, 0.9);
    let theta = BLADE_TILT * p.easing.swing(ctx.cycle(p));
    let (s, c) = theta.sin_cos();
    let (a, b) = ((-w * c, yc - w * s), (w * c, yc + w * s));
    vec![above_horizon(a, b), above_horizon(b, a)]
}

/// End `p` of the segment `p`-`q`, moved up the segment to the horizon if
/// it is below it (the part below is cut off; the angle is kept).
fn above_horizon(p: (f32, f32), q: (f32, f32)) -> (f32, f32) {
    if p.1 >= HORIZON || q.1 <= p.1 {
        return p;
    }
    let t = ((HORIZON - p.1) / (q.1 - p.1)).min(1.0);
    (lerp(p.0, q.0, t), HORIZON)
}

fn curtain(p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<Stroke> {
    let n = (p.count as usize).clamp(1, CURTAIN_MAX);
    let bottom = HORIZON + p.b.clamp(0.0, 1.0 - CURTAIN_HEIGHT);
    let spacing = CURTAIN_SPACING * w;
    let slide = p.a.clamp(0.0, 0.3) * p.easing.swing(ctx.cycle(p));
    (0..n)
        .map(|i| {
            let x = (i as f32 - (n - 1) as f32 / 2.0) * spacing + slide;
            // Up, then down: the galvo never crosses the frame blanked.
            let (y0, y1) = if i % 2 == 0 { (bottom, bottom + CURTAIN_HEIGHT) } else { (bottom + CURTAIN_HEIGHT, bottom) };
            vec![(x, y0), (x, y1)]
        })
        .collect()
}

/// Waterfall lane of the `m`-th line to start: even lanes first, then odd
/// ones, so consecutive lines land apart (staggered).
fn lane(m: i64, lanes: usize) -> usize {
    let k = m.rem_euclid(lanes as i64) as usize;
    let evens = lanes.div_ceil(2);
    if k < evens { 2 * k } else { 2 * (k - evens) + 1 }
}

fn waterfall(p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<Stroke> {
    let lanes = (p.count as usize).clamp(1, MAX_LANES);
    let spawn = p.steps_per_beat.clamp(MIN_SPAWN_PER_BEAT, MAX_SPAWN_PER_BEAT) as f64;
    let bottom = HORIZON + p.b.clamp(0.0, 0.5);
    let half = (0.8 * w / lanes as f32).max(0.03);
    let now = (ctx.beat_pos * spawn + 1e-9).floor() as i64;
    // Lines alive: started less than FALL_BEATS ago (one lands as the next starts).
    let alive = (FALL_BEATS * spawn).ceil() as i64;
    let mut strokes: Vec<Stroke> = (0..alive)
        .filter_map(|j| {
            let m = now - j;
            // (m <= now, so a negative age is only rounding noise.)
            let age = (ctx.beat_pos - m as f64 / spawn).max(0.0);
            if age >= FALL_BEATS {
                return None;
            }
            let y = lerp(FALL_TOP, bottom, (age / FALL_BEATS) as f32);
            let x = if lanes == 1 { 0.0 } else { lerp(-w, w, lane(m, lanes) as f32 / (lanes - 1) as f32) };
            Some(vec![(x - half, y), (x + half, y)])
        })
        .collect();
    // Top to bottom, each drawn the other way from the last: short travel.
    strokes.sort_by(|a, b| b[0].1.total_cmp(&a[0].1));
    for (i, s) in strokes.iter_mut().enumerate() {
        if i % 2 == 1 {
            s.reverse();
        }
    }
    strokes
}

fn scanner(p: &GenParams, ctx: &GenCtx, w: f32) -> Stroke {
    let (lo, hi) = sheet_box(p.b, w);
    let period = if p.period_beats > 0.0 { p.period_beats as f64 } else { 4.0 };
    let mut u = p.loop_mode.travel(ctx.beat_pos / period);
    if p.direction < 0 {
        u = 1.0 - u;
    }
    if p.a >= 0.5 {
        let x = lerp(-w, w, u);
        vec![(x, lo), (x, hi)]
    } else {
        let y = lerp(lo, hi, u);
        vec![(-w, y), (w, y)]
    }
}

fn slats(p: &GenParams, ctx: &GenCtx, w: f32) -> Geometry {
    let k = (p.count as usize).clamp(2, MAX_SLATS);
    let run = (p.a.round().max(1.0) as usize).min(k / 2);
    let y = HORIZON + p.b.clamp(0.05, 0.95);
    let shift = ctx.step(p) as i64 * if p.direction < 0 { -1 } else { 1 };
    let strokes = (0..k).map(|i| vec![(lerp(-w, w, i as f32 / k as f32), y), (lerp(-w, w, (i + 1) as f32 / k as f32), y)]).collect();
    let styles = (0..k)
        .map(|i| {
            let lit = (i as i64 - shift).rem_euclid(2 * run as i64) < run as i64;
            BeamStyle { intensity: if lit { 1.0 } else { 0.0 }, ..Default::default() }
        })
        .collect();
    Geometry::lines(strokes).with_styles(styles)
}

fn aurora(p: &GenParams, ctx: &GenCtx, w: f32) -> Stroke {
    let amp = p.a.clamp(0.0, AURORA_MAX);
    // The trough stays clear of the horizon, the crest of the top.
    let base = (HORIZON + p.b.clamp(0.05, 0.95)).clamp(HORIZON + amp + 0.02, 1.0 - amp);
    let phases: Vec<f32> = AURORA_SPEEDS.iter().map(|s| (dir(p) * ctx.beat_pos * s).rem_euclid(1.0) as f32).collect();
    sample(CURVE_SAMPLES, |u| {
        let h: f32 = (0..3).map(|k| AURORA_WEIGHTS[k] * (TAU * (AURORA_WAVES[k] * u - phases[k]) + AURORA_PHASES[k]).sin()).sum();
        (lerp(-w, w, u), base + amp * h)
    })
}

fn grid(p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<Stroke> {
    let (lo, hi) = sheet_box(p.b, w);
    let rows = (p.count as usize).clamp(1, GRID_MAX);
    let cols = (p.a.round().max(0.0) as usize).min(GRID_MAX);
    let scroll = dir(p) * ctx.beat_pos;
    // Line i of n, scrolled by one line per beat and wrapped, as 0..1.
    let at = |i: usize, n: usize| ((i as f64 + scroll) / n as f64).rem_euclid(1.0) as f32;
    let mut ys: Vec<f32> = (0..rows).map(|i| lerp(lo, hi, at(i, rows))).collect();
    let mut xs: Vec<f32> = (0..cols).map(|j| lerp(-w, w, at(j, cols))).collect();
    ys.sort_by(f32::total_cmp);
    xs.sort_by(f32::total_cmp);
    // Serpentine: each line drawn the other way from the one before.
    // Rows bottom to top, then columns from the side and the top where the
    // last row ended.
    let rows = ys.iter().enumerate().map(|(i, &y)| if i % 2 == 0 { vec![(-w, y), (w, y)] } else { vec![(w, y), (-w, y)] });
    if ys.len() % 2 == 1 {
        xs.reverse();
    }
    let cols = xs.iter().enumerate().map(|(j, &x)| if j % 2 == 0 { vec![(x, hi), (x, lo)] } else { vec![(x, lo), (x, hi)] });
    rows.chain(cols).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beat::{Easing, LoopMode};
    use crate::engine::densify;
    use crate::generators::{colorize, ColorMode, GENERATOR_NAMES};
    use crate::safety::{SafetySettings, StrobeLimiter};

    const NAMES: [&str; 8] = ["ceiling", "blade", "curtain", "waterfall", "scanner", "slats", "aurora", "grid"];

    fn at(beat_pos: f64, scale: f32) -> GenCtx {
        GenCtx { beat_pos, ..GenCtx::at_time(0.0, 0.0, 0.0, scale) }
    }

    fn strokes(name: &str, p: &GenParams, beat_pos: f64) -> Vec<Stroke> {
        let geo = generate(name, p, &at(beat_pos, 0.7)).unwrap();
        assert!(!geo.dots, "{name} is a line look");
        geo.strokes
    }

    fn close(a: &[(f32, f32)], b: &[(f32, f32)]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4)
    }

    fn lit_points(name: &str, p: &GenParams, ctx: &GenCtx) -> Vec<crate::patterns::Point> {
        let geo = generate(name, p, ctx).unwrap();
        densify(&colorize(&geo, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0, 1.0))
    }

    #[test]
    fn sheets_are_listed_after_the_fans() {
        let first = GENERATOR_NAMES.iter().position(|n| *n == "ceiling").unwrap();
        assert_eq!(first, 29, "after the fans and the tunnels");
        assert!(first > GENERATOR_NAMES.iter().position(|n| *n == "positions").unwrap());
        assert_eq!(&GENERATOR_NAMES[first..first + NAMES.len()], &NAMES);
        assert!(generate("liquid_sky", &GenParams::default(), &at(0.0, 0.5)).is_none(), "the old sheet stays in generators.rs");
    }

    #[test]
    fn every_sheet_stays_in_bounds_above_the_horizon() {
        let params = [
            GenParams::default(), // the UI's defaults: a = 3, b = 2
            GenParams { count: 1, a: 0.0, b: -1.0, beat_sync: true, ..Default::default() },
            GenParams { count: 5, a: 0.02, b: 0.1, easing: Easing::Trapezoid, loop_mode: LoopMode::Wrap, steps_per_beat: 2.0, ..Default::default() },
            GenParams { count: 64, a: 10.0, b: 3.0, direction: -1, steps_per_beat: 8.0, period_beats: 16.0, ..Default::default() },
            GenParams { count: 8, a: 0.6, b: 0.0, direction: 0, period_beats: 0.0, steps_per_beat: 0.0, ..Default::default() },
        ];
        for name in NAMES {
            for p in &params {
                for scale in [0.0, 0.3, 0.7, 1.0, 1.5] {
                    for k in 0..80 {
                        let beat_pos = k as f64 * 0.137 - 1.0;
                        let geo = generate(name, p, &at(beat_pos, scale)).unwrap();
                        assert!(!geo.dots && !geo.strokes.is_empty(), "{name}: lines");
                        for &(x, y) in geo.strokes.iter().flatten() {
                            assert!(x.is_finite() && y.is_finite() && x.abs() <= 1.0, "{name} x={x}");
                            assert!((HORIZON..=1.0).contains(&y), "{name} below the horizon or off the top: y={y} at beat {beat_pos}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn ceiling_sits_at_b_ripples_by_a_and_breathes_only_in_tempo() {
        let p = GenParams { a: 0.02, b: 0.15, ..Default::default() };
        let line = &strokes("ceiling", &p, 0.0)[0];
        let (lo, hi) = line.iter().fold((1.0f32, -1.0f32), |(lo, hi), q| (lo.min(q.1), hi.max(q.1)));
        assert!((lo - 0.13).abs() < 2e-3 && (hi - 0.17).abs() < 2e-3, "ripple of ±a around b: {lo}..{hi}");
        assert!((line[0].0 + 0.7).abs() < 1e-6 && (line.last().unwrap().0 - 0.7).abs() < 1e-6);
        // 0.25 cycle per beat: back in place after 4 beats, inverted after 2.
        assert!(close(line, &strokes("ceiling", &p, 4.0)[0]));
        assert!(line.iter().zip(&strokes("ceiling", &p, 2.0)[0]).all(|(a, b)| ((a.1 - 0.15) + (b.1 - 0.15)).abs() < 1e-4));
        // Height and ripple are held to their ranges.
        let wild = &strokes("ceiling", &GenParams { a: 3.0, b: 2.0, ..Default::default() }, 1.0)[0];
        assert!(wild.iter().all(|q| q.1 <= 0.6 + 0.03 + 1e-5 && q.1 >= 0.6 - 0.03 - 1e-5));
        let low = &strokes("ceiling", &GenParams { a: 3.0, b: -1.0, ..Default::default() }, 1.0)[0];
        assert!(low.iter().all(|q| q.1 >= 0.02 - 1e-5));
        // Breath: none without the tempo; 0.7..1 over 8 beats with it.
        let k = |p: &GenParams, beat: f64| generate("ceiling", p, &at(beat, 0.7)).unwrap().styles.first().map_or(1.0, |s| s.intensity);
        assert!((0..16).all(|i| k(&p, i as f64 * 0.5) == 1.0));
        let synced = GenParams { beat_sync: true, ..p };
        assert!((k(&synced, 0.0) - 0.85).abs() < 1e-5 && (k(&synced, 2.0) - 1.0).abs() < 1e-5 && (k(&synced, 6.0) - 0.7).abs() < 1e-5);
        assert!((k(&synced, 8.0) - k(&synced, 0.0)).abs() < 1e-5);
    }

    #[test]
    fn blade_tilts_thirty_degrees_either_way_over_its_period() {
        let p = GenParams { b: 0.5, period_beats: 16.0, ..Default::default() };
        let angle = |beat: f64| {
            let s = &strokes("blade", &p, beat)[0];
            ((s[1].1 - s[0].1) / (s[1].0 - s[0].0)).atan().to_degrees()
        };
        assert!(angle(0.0).abs() < 1e-3 && (angle(4.0) - 30.0).abs() < 1e-3 && angle(8.0).abs() < 1e-3 && (angle(12.0) + 30.0).abs() < 1e-3);
        assert!(angle(2.0) > 5.0 && angle(2.0) < 30.0);
        // It turns about the raised centre.
        let s = &strokes("blade", &p, 3.0)[0];
        assert!(((s[0].0 + s[1].0) / 2.0).abs() < 1e-4 && ((s[0].1 + s[1].1) / 2.0 - 0.5).abs() < 1e-4);
        // Low down, the part below the horizon is cut, the angle kept.
        let low = GenParams { b: 0.1, ..p };
        let s = &strokes("blade", &low, 4.0)[0];
        assert!((s[0].1 - HORIZON).abs() < 1e-6 && s[0].0 > -0.7 + 0.1);
        assert!((((s[1].1 - s[0].1) / (s[1].0 - s[0].0)).atan().to_degrees() - 30.0).abs() < 1e-3);
    }

    #[test]
    fn curtains_are_at_most_five_vertical_strokes_0_35_apart() {
        let p = GenParams { count: 9, a: 0.0, b: 0.0, ..Default::default() };
        let s = strokes("curtain", &p, 1.3);
        assert_eq!(s.len(), 5);
        for (i, c) in s.iter().enumerate() {
            assert!((c[0].0 - c[1].0).abs() < 1e-6, "vertical");
            assert!((c[0].0 - (i as f32 - 2.0) * 0.35).abs() < 1e-5, "0.35 apart, centred");
            let (lo, hi) = (c[0].1.min(c[1].1), c[0].1.max(c[1].1));
            assert!((lo - HORIZON).abs() < 1e-6 && (hi - (HORIZON + 0.6)).abs() < 1e-6, "from y_h to y_h + 0.6");
        }
        assert!(close(&s[0], &strokes("curtain", &p, 3.7)[0]), "static at a = 0");
        let three = strokes("curtain", &GenParams { count: 3, ..p.clone() }, 0.0);
        assert!((three[0][0].0 + 0.35).abs() < 1e-5 && three[1][0].0.abs() < 1e-5);
        // `a` slides them sideways over the period.
        let slide = GenParams { a: 0.2, ..p };
        assert!((strokes("curtain", &slide, 1.0)[2][0].0 - 0.2).abs() < 1e-4);
    }

    #[test]
    fn a_waterfall_line_takes_exactly_two_beats_to_fall() {
        let p = GenParams { count: 6, b: 0.0, steps_per_beat: 2.0, ..Default::default() };
        // The line started at beat 3 (spawn index 6) is in its lane at 0.9 on
        // its start, half-way at beat 4, and lands on the horizon at beat 5.
        let x = {
            let s = strokes("waterfall", &p, 3.0);
            let top = s.iter().find(|s| (s[0].1 - FALL_TOP).abs() < 1e-5).expect("a new line at the top");
            (top[0].0 + top[1].0) / 2.0
        };
        let height_at = |beat: f64| {
            strokes("waterfall", &p, beat).into_iter().find(|s| ((s[0].0 + s[1].0) / 2.0 - x).abs() < 1e-5).map(|s| s[0].1)
        };
        assert!((height_at(4.0).unwrap() - 0.45).abs() < 1e-4, "half-way after one beat, linear");
        assert!(height_at(5.0 - 1e-4).unwrap() < 1e-3, "on the horizon just before 2 beats");
        // Lane 6 of 6 is next used 6 spawns later (beat 6): at beat 5 exactly it is gone.
        assert!(height_at(5.0).is_none(), "gone at exactly 2 beats");
        // One starts every half beat and one lands: always 4 lines alive.
        for k in 0..40 {
            assert_eq!(strokes("waterfall", &p, k as f64 * 0.113).len(), 4);
        }
        // Consecutive lines start in different, staggered lanes.
        let lanes: Vec<usize> = (0..6).map(|m| lane(m, 6)).collect();
        assert_eq!(lanes, vec![0, 2, 4, 1, 3, 5]);
    }

    #[test]
    fn scanner_makes_one_pass_per_period_back_and_forth_or_wrapping() {
        let p = GenParams { a: 0.0, b: 0.1, period_beats: 4.0, ..Default::default() };
        let y = |p: &GenParams, beat: f64| strokes("scanner", p, beat)[0][0].1;
        let (lo, hi) = (0.1, 0.8);
        assert!((y(&p, 0.0) - lo).abs() < 1e-5 && (y(&p, 2.0) - 0.45).abs() < 1e-5 && (y(&p, 4.0) - hi).abs() < 1e-5, "linear, one pass per bar");
        assert!((y(&p, 6.0) - 0.45).abs() < 1e-5 && (y(&p, 8.0) - lo).abs() < 1e-5, "and back (ping-pong)");
        let wrap = GenParams { loop_mode: LoopMode::Wrap, ..p.clone() };
        assert!((y(&wrap, 4.0 - 1e-4) - hi).abs() < 1e-3 && (y(&wrap, 4.0) - lo).abs() < 1e-5, "wraps to the bottom");
        let down = GenParams { direction: -1, ..wrap };
        assert!((y(&down, 0.0) - hi).abs() < 1e-5 && (y(&down, 1.0) - (hi - 0.175)).abs() < 1e-5);
        // a >= 0.5: a vertical bar crossing the width.
        let v = GenParams { a: 1.0, ..p };
        let bar = |beat: f64| strokes("scanner", &v, beat)[0].clone();
        assert!((bar(0.0)[0].0 + 0.7).abs() < 1e-5 && bar(2.0)[0].0.abs() < 1e-5 && (bar(4.0)[0].0 - 0.7).abs() < 1e-5);
        assert!(bar(1.0)[0].0 == bar(1.0)[1].0 && (bar(1.0)[0].1 - lo).abs() < 1e-5 && (bar(1.0)[1].1 - hi).abs() < 1e-5);
    }

    #[test]
    fn slats_pattern_moves_one_segment_every_half_beat() {
        let p = GenParams { count: 8, a: 2.0, b: 0.3, steps_per_beat: 2.0, ..Default::default() };
        let pattern = |beat: f64| -> Vec<bool> {
            let geo = generate("slats", &p, &at(beat, 0.7)).unwrap();
            assert_eq!(geo.strokes.len(), 8);
            geo.styles.iter().map(|s| s.intensity > 0.0).collect()
        };
        let p0 = pattern(0.0);
        assert_eq!(p0, vec![true, true, false, false, true, true, false, false]);
        assert_eq!(p0.iter().filter(|l| **l).count(), 4, "50 % lit");
        assert_eq!(pattern(0.499), p0, "still until the half beat");
        let rot = |v: &[bool], k: usize| (0..v.len()).map(|i| v[(i + v.len() - k) % v.len()]).collect::<Vec<_>>();
        assert_eq!(pattern(0.5), rot(&p0, 1), "one segment on at the half beat");
        assert_eq!(pattern(1.0), rot(&p0, 2));
        assert_eq!(pattern(2.0), p0, "a run of 2 on and 2 off comes back after 4 steps");
        // The segments tile the line; the geometry itself never moves.
        let geo = generate("slats", &p, &at(0.0, 0.7)).unwrap();
        assert!((geo.strokes[0][0].0 + 0.7).abs() < 1e-6 && (geo.strokes[7][1].0 - 0.7).abs() < 1e-6);
        assert!(geo.strokes.windows(2).all(|w| (w[0][1].0 - w[1][0].0).abs() < 1e-6));
    }

    #[test]
    fn aurora_bends_slowly_within_its_amplitude() {
        let p = GenParams { a: 0.2, b: 0.5, ..Default::default() };
        let line = |beat: f64| strokes("aurora", &p, beat)[0].clone();
        let l0 = line(0.0);
        assert!(l0.iter().all(|q| (q.1 - 0.5).abs() <= 0.2 + 1e-5));
        for beat in (0..100).step_by(7) {
            let l = line(beat as f64);
            let spread = l.iter().map(|q| q.1).fold(0.0f32, f32::max) - l.iter().map(|q| q.1).fold(1.0f32, f32::min);
            assert!(spread > 0.12, "it always bends: {spread} at beat {beat}");
        }
        // Slow: a beat moves it by little, the whole shape repeats after 100 beats.
        let step = l0.iter().zip(&line(1.0)).map(|(a, b)| (a.1 - b.1).abs()).fold(0.0f32, f32::max);
        assert!(step > 1e-3 && step < 0.1, "{step}");
        assert!(close(&l0, &line(100.0)));
        // Amplitude is capped and the trough lifted clear of the horizon.
        let low = strokes("aurora", &GenParams { a: 1.0, b: 0.0, ..Default::default() }, 7.0)[0].clone();
        assert!(low.iter().all(|q| q.1 >= HORIZON + 0.02 - 1e-5 && q.1 <= HORIZON + 0.02 + 0.5 + 1e-5));
    }

    #[test]
    fn grid_scrolls_one_line_per_beat_with_up_to_six_plus_six_lines() {
        let p = GenParams { count: 4, a: 4.0, b: 0.1, ..Default::default() };
        let s = strokes("grid", &p, 0.3);
        assert_eq!(s.len(), 8);
        let rows = |s: &[Stroke]| s.iter().filter(|l| l[0].1 == l[1].1).map(|l| l[0].1).collect::<Vec<_>>();
        let cols = |s: &[Stroke]| s.iter().filter(|l| l[0].0 == l[1].0).map(|l| l[0].0).collect::<Vec<_>>();
        assert_eq!((rows(&s).len(), cols(&s).len()), (4, 4));
        // One beat later every line has moved to the next one's place.
        let later = strokes("grid", &p, 1.3);
        assert!(close(&rows(&s).into_iter().map(|y| (0.0, y)).collect::<Vec<_>>(), &rows(&later).into_iter().map(|y| (0.0, y)).collect::<Vec<_>>()));
        assert!(!close(&s[0], &strokes("grid", &p, 0.8)[0]), "and moves in between");
        // A quarter of the spacing per quarter beat.
        let (r0, r1) = (rows(&s), rows(&strokes("grid", &p, 0.55)));
        assert!((r1[1] - r0[1] - 0.7 / 4.0 / 4.0).abs() < 1e-4, "{r0:?} {r1:?}");
        // At most 6 + 6, and no vertical lines at a = 0.
        assert_eq!(strokes("grid", &GenParams { count: 20, a: 20.0, ..p.clone() }, 0.0).len(), 12);
        assert_eq!(strokes("grid", &GenParams { a: 0.0, ..p }, 0.0).len(), 4);
    }

    /// Each sheet alone fits the layer mixer's frame budget (750 points = 40
    /// fps at 30 kpps) after densify, at full size with the most lines.
    #[test]
    fn sheets_hold_30_fps_at_30_kpps() {
        let params = [
            GenParams { count: 64, a: 10.0, b: 0.02, steps_per_beat: 8.0, beat_sync: true, ..Default::default() },
            GenParams { count: 6, a: 6.0, b: 0.1, steps_per_beat: 2.0, ..Default::default() },
            GenParams { count: 8, a: 0.2, b: 0.4, steps_per_beat: 4.0, loop_mode: LoopMode::Wrap, ..Default::default() },
        ];
        for name in NAMES {
            let mut worst = 0;
            for p in &params {
                for k in 0..64 {
                    worst = worst.max(lit_points(name, p, &at(k as f64 * 0.0625, 1.0)).len());
                }
            }
            assert!(worst <= crate::layers::DEFAULT_POINT_BUDGET, "'{name}' makes {worst} points");
        }
    }

    /// No sheet flashes: fed to the strobe limiter at 60 fps and 180 BPM for
    /// 12 s, none is ever seen flashing, so none is ever held.
    #[test]
    fn sheets_never_trip_the_strobe_limiter() {
        let cfg = SafetySettings::default();
        let params = [
            GenParams { count: 8, a: 2.0, b: 0.3, steps_per_beat: 2.0, beat_sync: true, period_beats: 4.0, ..Default::default() },
            GenParams { count: 12, a: 1.0, b: 0.3, steps_per_beat: 4.0, beat_sync: true, period_beats: 1.0, loop_mode: LoopMode::Wrap, ..Default::default() },
        ];
        for name in NAMES {
            for p in &params {
                let mut limiter = StrobeLimiter::default();
                for f in 0..720 {
                    let t = f as f64 / 60.0;
                    let frame = lit_points(name, p, &at(t * 3.0, 0.7));
                    let out = limiter.process(frame.clone(), t, &cfg);
                    assert!(!limiter.status().fast && out == frame, "'{name}' flashes at {t:.2} s");
                }
            }
        }
    }

    #[test]
    fn loop_mode_defaults_to_ping_pong_and_old_looks_load() {
        let old: GenParams = serde_json::from_str(r#"{"count":5,"a":1.5,"b":0.2,"easing":"triangle"}"#).unwrap();
        assert_eq!(old.loop_mode, LoopMode::PingPong);
        let json = serde_json::to_string(&GenParams { loop_mode: LoopMode::Wrap, ..Default::default() }).unwrap();
        assert!(json.contains(r#""loop_mode":"wrap""#), "{json}");
        assert!(serde_json::to_string(&GenParams::default()).unwrap().contains(r#""loop_mode":"ping_pong""#));
    }

    #[test]
    fn same_params_and_beat_give_the_same_frame_at_any_tempo() {
        let p = GenParams { count: 6, a: 0.2, b: 0.3, steps_per_beat: 2.0, beat_sync: true, ..Default::default() };
        for name in NAMES {
            let slow = generate(name, &p, &GenCtx { bpm: 90.0, t: 1.0, ..at(5.3, 0.7) }).unwrap();
            let fast = generate(name, &p, &GenCtx { bpm: 174.0, t: 9.0, ..at(5.3, 0.7) }).unwrap();
            assert_eq!(slow.strokes, fast.strokes, "{name} depends only on the beat");
            assert_eq!(slow.styles, fast.styles);
        }
    }
}
