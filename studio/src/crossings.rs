//! Crossings, the hot beam and convergences (T-104, looks 5, 11, 12 and
//! 29 of `docs/research/festival-looks.md`): the big moments of a
//! mainstage, approximated on one projector with virtual beam groups. Our
//! own maths, written from the research's description in words.
//!
//! All of them are beams (`dots`) that move with the tempo clock
//! (`ctx.beat_pos`) and stay at or above the horizon, like the fans.
//!
//! - `scissor`: two groups of `count / 2` beams. The left group's centre is
//!   at `-CROSS_C + a·sin φ`, the right one's at `+CROSS_C - a·sin φ`, with
//!   φ going round once per `period_beats` (2 by default), so with
//!   `a > CROSS_C` they cross twice per period. Each group is tilted ±15°
//!   so the crossing reads as an X. Left = main colour, right = `color2`
//!   (in « Uni » and « Alterné »); `cross_flash` flashes both white for
//!   1/8 beat around each crossing. `b` = height, size = group width.
//! - `hot_beam`: one beam panning `a` (0.3) each side over `period_beats`
//!   (16), at most `b` bright (0.6 when `b` is outside 0..1). It is never
//!   still: a small circle (half a turn per beat) is added to the pan,
//!   wide enough to outrun the pan by `HOT_MIN_SPEED`, so the beam always
//!   moves at that speed or more, whatever the tempo.
//! - `bundle_burst`: `count` beams bundled into one (half-width `W_MIN`)
//!   that burst open into a fan of half-width = size on the first beat of
//!   each period, in `a` beats (1/4 by default), hold, then collapse over
//!   the end of the period and wait bundled for the next burst.
//! - `beam_v`: a « V » (`b` < 0.5) or « Λ » (`b` ≥ 0.5) of two legs of
//!   `count / 2` beams at ±30° from the vertical, legs as long as the size.
//!   `a` > 0: the shape leans by `a` degrees (at most 45) one way then the
//!   other, one position per step (`steps_per_beat`): 0, +a, 0, -a.

use crate::beat;
use crate::fans::{line, HORIZON, MAX_BEAMS};
use crate::generators::{BeamStyle, ColorMode, GenCtx, GenParams, Geometry, Tint};
use std::f32::consts::TAU;

/// `scissor`: distance of each group's resting centre from the middle.
pub const CROSS_C: f32 = 0.3;
/// `scissor`: tilt of each group, so the crossing reads as an X.
const CROSS_TILT: f32 = 15.0 * std::f32::consts::PI / 180.0;
/// `scissor`: length of the white flash around a crossing, in beats.
const FLASH_BEATS: f32 = 1.0 / 8.0;
/// `hot_beam`: height of the pan above the horizon.
const HOT_HEIGHT: f32 = 0.45;
/// `hot_beam`: brightness cap when `b` doesn't give one.
pub const HOT_DEFAULT_CAP: f32 = 0.6;
/// `hot_beam`: slowest the beam may move, in frame units per second (the
/// field is 2 wide): about one exposure cell of the static-beam guard per
/// 1/4 s.
pub const HOT_MIN_SPEED: f32 = 0.2;
/// `hot_beam`: turns of the small circle per beat, and its radius limits.
const ORBIT_TURNS_PER_BEAT: f32 = 0.5;
const ORBIT_MIN: f32 = 0.03;
const ORBIT_MAX: f32 = 0.25;
/// `bundle_burst`: half-width of the bundle (one thick beam).
pub const W_MIN: f32 = 0.02;
/// `beam_v`: angle of each leg from the vertical, and the most it leans.
const V_ANGLE: f32 = 30.0;
const V_MAX_LEAN: f32 = 45.0;

/// Draw generator `name`, or `None` if it isn't one of these.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    let w = ctx.scale.clamp(0.0, 1.0);
    let geo = match name {
        "scissor" => scissor(p, ctx, w),
        "hot_beam" => hot_beam(p, ctx),
        "bundle_burst" => Geometry::dots(bundle_burst(p, ctx, w)),
        "beam_v" => Geometry::dots(beam_v(p, ctx, w)),
        _ => return None,
    };
    let strokes = geo.strokes.into_iter().map(|s| s.into_iter().map(|(x, y)| (x.clamp(-1.0, 1.0), y.clamp(HORIZON, 1.0))).collect()).collect();
    Some(Geometry { strokes, ..geo })
}

/// `b` as a height above the horizon, kept clear of it and of the top.
fn height(b: f32) -> f32 {
    HORIZON + if b.is_finite() { b.clamp(0.05, 0.95) } else { 0.3 }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn frac(i: usize, n: usize) -> f32 {
    if n <= 1 { 0.5 } else { i as f32 / (n - 1) as f32 }
}

/// Period of the main motion, in beats (4 when unset).
fn period(p: &GenParams) -> f32 {
    if p.period_beats > 0.0 && p.period_beats.is_finite() { p.period_beats } else { 4.0 }
}

/// `scissor`'s swing: `a`, kept so the groups stay in the field.
fn scissor_amp(p: &GenParams) -> f32 {
    if p.a.is_finite() { p.a.clamp(0.0, 0.9 - CROSS_C) } else { 0.0 }
}

/// `scissor`'s group centres at cycle position `ph` (0..1).
pub fn scissor_centres(amp: f32, ph: f32) -> (f32, f32) {
    let s = amp * (TAU * ph).sin();
    (-CROSS_C + s, CROSS_C - s)
}

/// Beats from cycle position `ph` to the nearest crossing (`None` when the
/// groups never cross).
fn beats_to_crossing(amp: f32, ph: f32, period: f32) -> Option<f32> {
    if amp <= CROSS_C {
        return None;
    }
    let f1 = (CROSS_C / amp).asin() / TAU;
    [f1, 0.5 - f1]
        .into_iter()
        .map(|f| {
            let d = (ph - f).rem_euclid(1.0);
            d.min(1.0 - d) * period
        })
        .reduce(f32::min)
}

fn scissor(p: &GenParams, ctx: &GenCtx, w: f32) -> Geometry {
    let n = p.count.clamp(2, MAX_BEAMS as u32) as usize;
    let (nl, nr) = (n / 2, n - n / 2);
    let amp = scissor_amp(p);
    let ph = ctx.cycle(p);
    let (cl, cr) = scissor_centres(amp, ph);
    let half = 0.25 * w;
    let (s, c) = CROSS_TILT.sin_cos();
    let y = height(p.b).max(HORIZON + half * s + 0.02);
    let group = |m: usize, centre: f32, sign: f32| {
        (0..m).map(move |i| {
            let u = lerp(-half, half, frac(i, m));
            (centre + u * c, y + sign * u * s)
        })
    };
    let beams: Vec<(f32, f32)> = group(nl, cl, 1.0).chain(group(nr, cr, -1.0)).collect();
    let flash = p.cross_flash && beats_to_crossing(amp, ph, period(p)).is_some_and(|d| d < FLASH_BEATS / 2.0);
    let split = matches!(p.color_mode, ColorMode::Solid | ColorMode::Alternate);
    let styles = (0..n)
        .map(|i| {
            let tint = if flash {
                Tint::Rgb(1.0, 1.0, 1.0)
            } else if !split {
                Tint::Auto
            } else if i < nl {
                Tint::Primary
            } else {
                Tint::Secondary
            };
            BeamStyle { intensity: 1.0, tint }
        })
        .collect();
    Geometry::dots(beams).with_styles(styles)
}

/// Radius of `hot_beam`'s circle at `bpm` for a pan of `amp` over
/// `period` beats: the circle alone outruns the fastest pan by
/// `HOT_MIN_SPEED`, so the two can never cancel out.
fn orbit_radius(bpm: f32, amp: f32, period: f32) -> f32 {
    let bpm = if bpm.is_finite() && bpm > 0.0 { bpm } else { 120.0 };
    let beats_per_s = bpm / 60.0;
    let pan_max = amp * TAU * beats_per_s / period;
    let rad_per_s = TAU * ORBIT_TURNS_PER_BEAT * beats_per_s;
    ((HOT_MIN_SPEED + pan_max) / rad_per_s).clamp(ORBIT_MIN, ORBIT_MAX)
}

/// `hot_beam`'s position.
pub fn hot_beam_at(p: &GenParams, ctx: &GenCtx) -> (f32, f32) {
    let amp = if p.a.is_finite() { p.a.clamp(0.0, 0.8) } else { 0.0 };
    let pan = amp * (TAU * ctx.cycle(p)).sin();
    let r = orbit_radius(ctx.bpm, amp, period(p));
    let th = TAU * (ctx.beat_pos * ORBIT_TURNS_PER_BEAT as f64).rem_euclid(1.0) as f32;
    (pan + r * th.cos(), HORIZON + HOT_HEIGHT + r * th.sin())
}

/// `hot_beam`'s brightness cap.
pub fn hot_cap(p: &GenParams) -> f32 {
    if p.b > 0.0 && p.b <= 1.0 { p.b } else { HOT_DEFAULT_CAP }
}

fn hot_beam(p: &GenParams, ctx: &GenCtx) -> Geometry {
    Geometry::dots(vec![hot_beam_at(p, ctx)]).with_styles(vec![BeamStyle { intensity: hot_cap(p), tint: Tint::Auto }])
}

/// `bundle_burst`'s half-width `x` beats into a period of `period` beats,
/// opening in `open` beats, for a full half-width `w`.
pub fn burst_width(x: f32, period: f32, open: f32, w: f32) -> f32 {
    let wide = w.max(W_MIN);
    // A period under 1/16 beat (a mangled one) must not panic the clamp.
    let open = open.clamp(1.0 / 32.0, (period / 2.0).max(1.0 / 32.0));
    let collapse = (period / 4.0).min(0.5);
    let hold = (period / 8.0).min(0.25);
    let shut = period - hold - collapse;
    if x < open {
        let t = x / open;
        lerp(W_MIN, wide, 1.0 - (1.0 - t) * (1.0 - t))
    } else if x < shut {
        wide
    } else if x < period - hold {
        lerp(wide, W_MIN, beat::ease_sine((x - shut) / collapse))
    } else {
        W_MIN
    }
}

fn bundle_burst(p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<(f32, f32)> {
    let n = p.count.clamp(1, MAX_BEAMS as u32) as usize;
    let per = period(p);
    let x = beat::phase(ctx.beat_pos, per) * per;
    let open = if p.a.is_finite() { p.a } else { 0.25 };
    let half = burst_width(x, per, open, w);
    line(n, -half, half, height(p.b))
}

fn beam_v(p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<(f32, f32)> {
    let n = p.count.clamp(2, MAX_BEAMS as u32) as usize;
    let (left, right) = (n / 2, n - n / 2);
    let len = w.min(0.9);
    let lean_max = if p.a.is_finite() { p.a.clamp(0.0, V_MAX_LEAN) } else { 0.0 };
    let dir = if p.direction < 0 { -1.0 } else { 1.0 };
    let lean = dir * lean_max * [0.0, 1.0, 0.0, -1.0][(ctx.step(p) % 4) as usize];
    let up = p.b >= 0.5;
    let base = HORIZON + 0.05;
    // V: the apex at the bottom; Λ: at the top, legs coming down.
    let (apex, sign) = if up { (base + len, -1.0) } else { (base, 1.0) };
    let leg = |j: usize, m: usize, angle: f32| {
        let d = len * (j + 1) as f32 / m as f32;
        let (s, c) = angle.to_radians().sin_cos();
        (d * s, apex + sign * d * c)
    };
    // Left leg from its far end in, then the right leg out: a short path.
    (0..left).rev().map(|j| leg(j, left, lean - V_ANGLE)).chain((0..right).map(|j| leg(j, right, lean + V_ANGLE))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::densify;
    use crate::generators::{colorize, GENERATOR_NAMES};

    /// A period shorter than 1/16 beat (found by the safety fuzz) used to
    /// panic `clamp` (min > max); it now just opens at once.
    #[test]
    fn a_tiny_burst_period_does_not_panic() {
        for period in [0.04, 0.001, 0.0] {
            let w = burst_width(0.01, period, 0.25, 0.5);
            assert!(w.is_finite() && w >= 0.0, "{period}: {w}");
        }
    }

    const NAMES: [&str; 4] = ["scissor", "hot_beam", "bundle_burst", "beam_v"];

    fn at(beat_pos: f64, scale: f32) -> GenCtx {
        GenCtx { beat_pos, ..GenCtx::at_time(0.0, 0.0, 0.0, scale) }
    }

    fn geo(name: &str, p: &GenParams, beat_pos: f64) -> Geometry {
        generate(name, p, &at(beat_pos, 0.7)).unwrap()
    }

    fn beams(name: &str, p: &GenParams, beat_pos: f64) -> Vec<(f32, f32)> {
        let g = geo(name, p, beat_pos);
        assert!(g.dots, "{name} is a beam look");
        g.strokes.iter().map(|s| s[0]).collect()
    }

    fn mean_x(pts: &[(f32, f32)]) -> f32 {
        pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32
    }

    fn scissor_params() -> GenParams {
        GenParams { count: 8, a: 0.5, b: 0.3, period_beats: 2.0, color_mode: ColorMode::Alternate, ..Default::default() }
    }

    #[test]
    fn crossings_are_listed_after_the_chaser() {
        let pos = |name: &str| GENERATOR_NAMES.iter().position(|n| *n == name).unwrap();
        for name in NAMES {
            assert!(pos(name) > pos("chase_fan"), "{name}: appended, saved looks refer to generators by name");
        }
        assert!(generate("fan", &GenParams::default(), &at(0.0, 0.5)).is_none());
    }

    #[test]
    fn every_crossing_look_stays_in_bounds_above_the_horizon() {
        let params = [
            GenParams::default(),
            GenParams { count: 1, a: 0.0, b: -1.0, ..Default::default() },
            GenParams { count: 64, a: 10.0, b: 3.0, period_beats: 0.0, direction: -1, steps_per_beat: 8.0, cross_flash: true, ..Default::default() },
            GenParams { count: 10, a: 45.0, b: 1.0, period_beats: 0.25, ..Default::default() },
            GenParams { a: f32::NAN, b: f32::NAN, ..Default::default() },
        ];
        for name in NAMES {
            for p in &params {
                for scale in [0.0, 0.3, 0.7, 1.0, 1.5] {
                    for k in 0..120 {
                        let beat_pos = k as f64 * 0.093 - 1.0;
                        for bpm in [20.0, 128.0, 300.0] {
                            let g = generate(name, p, &GenCtx { bpm, ..at(beat_pos, scale) }).unwrap();
                            assert!(g.dots && !g.strokes.is_empty() && g.strokes.len() <= MAX_BEAMS);
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
    fn scissor_groups_cross_in_the_centre_twice_per_period() {
        let p = scissor_params();
        let gap = |beat: f64| {
            let b = beams("scissor", &p, beat);
            mean_x(&b[4..]) - mean_x(&b[..4])
        };
        // At rest the groups sit at ±CROSS_C.
        let b0 = beams("scissor", &p, 0.0);
        assert!((mean_x(&b0[..4]) + CROSS_C).abs() < 1e-4 && (mean_x(&b0[4..]) - CROSS_C).abs() < 1e-4);
        for start in [0.0, 2.0, 6.0] {
            let mut crossings = Vec::new();
            let steps = 2000;
            for k in 0..steps {
                let (a, b) = (start + 2.0 * k as f64 / steps as f64, start + 2.0 * (k + 1) as f64 / steps as f64);
                if gap(a).signum() != gap(b).signum() {
                    crossings.push(a);
                }
            }
            assert_eq!(crossings.len(), 2, "two crossings per 2-beat period from beat {start}: {crossings:?}");
            for c in crossings {
                let bm = beams("scissor", &p, c);
                assert!(mean_x(&bm[..4]).abs() < 0.01 && mean_x(&bm[4..]).abs() < 0.01, "they cross in the centre");
            }
        }
        // Too small a swing never crosses.
        let small = GenParams { a: 0.2, ..p.clone() };
        assert!((0..100).all(|k| {
            let b = beams("scissor", &small, k as f64 * 0.02);
            mean_x(&b[..4]) < mean_x(&b[4..])
        }));
    }

    #[test]
    fn scissor_groups_are_tilted_into_an_x_and_coloured_apart() {
        let p = scissor_params();
        let b = beams("scissor", &p, 0.0);
        let slope = |g: &[(f32, f32)]| (g[3].1 - g[0].1) / (g[3].0 - g[0].0);
        let tan = 15f32.to_radians().tan();
        assert!((slope(&b[..4]) - tan).abs() < 1e-4 && (slope(&b[4..]) + tan).abs() < 1e-4, "±15°: an X where they meet");
        let g = geo("scissor", &p, 0.0);
        assert!(g.styles[..4].iter().all(|s| s.tint == Tint::Primary) && g.styles[4..].iter().all(|s| s.tint == Tint::Secondary));
        // Rainbow: the colour mode decides.
        let g = geo("scissor", &GenParams { color_mode: ColorMode::Rainbow, ..p }, 0.0);
        assert!(g.styles.iter().all(|s| s.tint == Tint::Auto));
    }

    #[test]
    fn scissor_flashes_white_for_an_eighth_of_a_beat_on_each_crossing() {
        let p = GenParams { cross_flash: true, ..scissor_params() };
        // First crossing: sin φ = c / a, φ = 2π·beat / 2.
        let first = (CROSS_C / 0.5).asin() / TAU * 2.0;
        let white = |beat: f32| geo("scissor", &p, beat as f64).styles.iter().all(|s| s.tint == Tint::Rgb(1.0, 1.0, 1.0));
        for crossing in [first, 1.0 - first, 2.0 + first, 3.0 - first] {
            assert!(white(crossing) && white(crossing - 0.06) && white(crossing + 0.06), "white around beat {crossing}");
            assert!(!white(crossing - 0.07) && !white(crossing + 0.07), "only 1/8 beat around {crossing}");
        }
        assert!(!white(0.0) && !white(1.5));
        // Off by default.
        let plain = scissor_params();
        assert!(!geo("scissor", &plain, first as f64).styles.iter().any(|s| matches!(s.tint, Tint::Rgb(..))));
    }

    #[test]
    fn hot_beam_is_one_capped_beam_panning_a_over_the_period() {
        let p = GenParams { count: 5, a: 0.3, b: 0.6, period_beats: 16.0, ..Default::default() };
        let g = geo("hot_beam", &p, 0.0);
        assert_eq!(g.strokes.len(), 1, "one beam whatever the count");
        assert_eq!(g.styles[0].intensity, 0.6);
        assert_eq!(geo("hot_beam", &GenParams { b: 2.0, ..p.clone() }, 0.0).styles[0].intensity, HOT_DEFAULT_CAP);
        assert_eq!(geo("hot_beam", &GenParams { b: 0.3, ..p.clone() }, 0.0).styles[0].intensity, 0.3);
        let r = orbit_radius(120.0, 0.3, 16.0);
        let x = |beat: f64| beams("hot_beam", &p, beat)[0].0;
        // A quarter period in: right by a (± the small circle).
        assert!((x(4.0) - 0.3).abs() <= r + 1e-4 && (x(12.0) + 0.3).abs() <= r + 1e-4);
        // Same place every 16 beats (the circle turns once per 2 beats).
        assert!((x(1.0) - x(17.0)).abs() < 1e-4);
    }

    #[test]
    fn hot_beam_never_stays_still_for_100_ms() {
        let cases = [
            GenParams { a: 0.3, period_beats: 16.0, ..Default::default() },
            GenParams { a: 0.0, period_beats: 16.0, ..Default::default() },
            GenParams { a: 0.8, period_beats: 1.0, direction: -1, ..Default::default() },
        ];
        for p in &cases {
            for bpm in [20.0f32, 60.0, 128.0, 174.0, 300.0] {
                let pos = |s: f64| hot_beam_at(p, &GenCtx { bpm, ..at(s * bpm as f64 / 60.0, 0.7) });
                // Every 100 ms window over 20 s: the beam moves at least
                // HOT_MIN_SPEED · 0.1 s · (chord of the circle) somewhere in it.
                for k in 0..2000 {
                    let t0 = k as f64 * 0.01;
                    let a = pos(t0);
                    let moved = (1..=10).map(|j| pos(t0 + j as f64 * 0.01)).map(|b| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()).fold(0.0, f32::max);
                    assert!(moved > 0.25 * HOT_MIN_SPEED * 0.1, "still at {t0} s, {bpm} BPM: moved {moved}");
                }
                // And its speed alone (no pan) is HOT_MIN_SPEED or more.
                let still = GenParams { a: 0.0, ..p.clone() };
                let q = |s: f64| hot_beam_at(&still, &GenCtx { bpm, ..at(s * bpm as f64 / 60.0, 0.7) });
                let (a, b) = (q(1.0), q(1.001));
                let v = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() / 0.001;
                assert!(v >= HOT_MIN_SPEED * 0.99, "{bpm} BPM: {v} units/s");
            }
        }
    }

    #[test]
    fn bundle_burst_is_bundled_before_the_beat_and_wide_a_quarter_beat_after() {
        let p = GenParams { count: 10, a: 0.25, b: 0.35, period_beats: 4.0, ..Default::default() };
        let width = |beat: f64| {
            let b = beams("bundle_burst", &p, beat);
            b.iter().map(|q| q.0.abs()).fold(0.0, f32::max)
        };
        for bar in [0.0, 4.0, 8.0, 40.0] {
            assert!(width(bar) <= W_MIN + 1e-6, "bundled on the beat");
            if bar > 0.0 {
                assert!(width(bar - 0.01) <= W_MIN + 1e-6 && width(bar - 0.2) <= W_MIN + 1e-6, "bundled just before the beat");
            }
            assert!(width(bar + 0.1) > 0.3, "bursting");
            assert!((width(bar + 0.25) - 0.7).abs() < 1e-4, "fully open 1/4 beat after");
            assert!((width(bar + 2.5) - 0.7).abs() < 1e-4, "held open");
            assert!(width(bar + 3.5) < 0.7 && width(bar + 3.5) > W_MIN, "collapsing");
        }
        // `a` = opening time: half a beat.
        let slow = GenParams { a: 0.5, ..p.clone() };
        assert!(beams("bundle_burst", &slow, 0.25)[9].0 < 0.69 && (beams("bundle_burst", &slow, 0.5)[9].0 - 0.7).abs() < 1e-4);
        // A flat line of beams at height b.
        assert!(beams("bundle_burst", &p, 1.0).iter().all(|q| (q.1 - 0.35).abs() < 1e-6));
    }

    #[test]
    fn beam_v_has_two_legs_of_five_at_30_degrees() {
        let p = GenParams { count: 10, a: 0.0, b: 0.0, ..Default::default() };
        let v = beams("beam_v", &p, 0.0);
        assert_eq!(v.len(), 10);
        let base = HORIZON + 0.05;
        assert!(v[..5].iter().all(|q| q.0 < 0.0) && v[5..].iter().all(|q| q.0 > 0.0));
        for q in &v {
            let angle = (q.0 / (q.1 - base)).atan().to_degrees().abs();
            assert!((angle - 30.0).abs() < 0.01 && q.1 > base, "V: {angle}");
        }
        // Λ: the apex on top, legs coming down at the same angle.
        let up = beams("beam_v", &GenParams { b: 1.0, ..p.clone() }, 0.0);
        let top = base + 0.7;
        for q in &up {
            let angle = (q.0 / (top - q.1)).atan().to_degrees().abs();
            assert!((angle - 30.0).abs() < 0.01 && q.1 < top && q.1 >= base - 1e-5, "Λ: {angle}");
        }
        // No lean: still at every step.
        assert_eq!(v, beams("beam_v", &p, 3.0));
    }

    #[test]
    fn beam_v_leans_one_way_then_the_other_by_steps() {
        let p = GenParams { count: 10, a: 15.0, b: 0.0, steps_per_beat: 1.0, ..Default::default() };
        let lean = |beat: f64| mean_x(&beams("beam_v", &p, beat));
        assert!(lean(0.5).abs() < 1e-4 && lean(1.5) > 0.05 && lean(2.5).abs() < 1e-4 && lean(3.5) < -0.05);
        assert!((lean(1.5) - lean(5.5)).abs() < 1e-6, "four steps, then round again");
        // Right leg at 30 + 15 = 45° from the vertical while leaning right.
        let b = beams("beam_v", &p, 1.0);
        let base = HORIZON + 0.05;
        assert!((b[9].0 / (b[9].1 - base) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn crossings_hold_30_fps_at_30_kpps() {
        let p = GenParams { count: 64, a: 0.5, b: 0.3, cross_flash: true, ..Default::default() };
        for name in NAMES {
            let mut worst = 0;
            for k in 0..64 {
                let g = generate(name, &p, &at(k as f64 * 0.0625, 1.0)).unwrap();
                let pts = densify(&colorize(&g, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 1.0), 0.0, 1.0));
                worst = worst.max(pts.len());
            }
            assert!(worst <= crate::layers::DEFAULT_POINT_BUDGET, "'{name}' makes {worst} points");
        }
    }

    #[test]
    fn old_looks_load_without_cross_flash() {
        assert!(!serde_json::from_str::<GenParams>(r#"{"count":5}"#).unwrap().cross_flash);
    }
}
