//! Festival beam fans (T-102): the backbone of mainstage laser work, as
//! beat-synced beam generators. Each is our own maths, written from the
//! look descriptions in `docs/research/festival-looks.md` section A.
//!
//! All of them are beams (`dots`), move with the tempo clock through
//! `ctx.beat_pos` (whatever `beat_sync` says: their motion is defined in
//! beats), and stay at or above the horizon, so they pass over the
//! audience. Widths follow the look size (`ctx.scale`); heights are frame
//! units above `HORIZON`. Parameters:
//!
//! - `fan`: `count` beams on the line `y = b`, half-width = size. `a` =
//!   pump depth (0 = static; 1 = the width snaps open on each beat and
//!   closes to `W_MIN` before the next).
//! - `fan_sweep`: a half-size fan whose centre swings `a`·swing(cycle)
//!   over `period_beats`, shaped by `easing`, per group (`group_mode`).
//! - `fan_tilt`: the fan rises from `HORIZON + 0.05` to `HORIZON + 0.7`
//!   over `period_beats` (ease-in), then snaps back down; `direction` -1
//!   comes down instead, 0 goes up and down (ping-pong).
//! - `fan_wave`: beam heights ride a sine one fan-width long that travels
//!   half a wavelength per beat; `a` = amplitude, `b` = mean height.
//! - `positions`: four positions, one per step (`steps_per_beat`): wide fan
//!   high, narrow fan tilted +15°, narrow fan tilted -15°, and a V.

use crate::beat;
use crate::generators::{GenCtx, GenParams, Geometry};
use std::f32::consts::TAU;

/// Height of the audience line in the look's frame. Fan beams never go
/// below it (before the user's rotation and calibration).
pub const HORIZON: f32 = 0.0;
/// Fans have at most this many beams (research: 6 to 16 per head), which
/// keeps every look well inside the point budget.
pub const MAX_BEAMS: usize = 32;
/// Half-width of a fully closed pumping fan: one thick beam.
const W_MIN: f32 = 0.02;
/// Pump envelope: opens over 1/16 beat, then closes with this time
/// constant (95 % closed after half a beat).
const PUMP_ATTACK: f32 = 1.0 / 16.0;
const PUMP_DECAY: f32 = 0.5 / 3.0;
/// `fan_tilt` travel, above the horizon.
const TILT_LOW: f32 = 0.05;
const TILT_HIGH: f32 = 0.7;
/// `fan_wave` travel speed: wavelengths per beat.
const WAVE_SPEED: f64 = 0.5;
/// `positions`: tilt of the narrow fans, and angle of the V's legs from
/// the vertical.
const POS_TILT: f32 = 15.0 * std::f32::consts::PI / 180.0;
const POS_V: f32 = 30.0 * std::f32::consts::PI / 180.0;

/// Draw fan generator `name`, or `None` if it isn't one.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    let n = p.count.clamp(1, MAX_BEAMS as u32) as usize;
    let w = ctx.scale.clamp(0.0, 1.0);
    let beams = match name {
        "fan" => fan(n, p, ctx, w),
        "fan_sweep" => fan_sweep(n, p, ctx, w),
        "fan_tilt" => fan_tilt(n, p, ctx, w),
        "fan_wave" => fan_wave(n, p, ctx, w),
        "positions" => positions(n, p, ctx, w),
        _ => return None,
    };
    Some(Geometry::dots(beams.into_iter().map(|(x, y)| (x.clamp(-1.0, 1.0), y.clamp(HORIZON, 1.0))).collect()))
}

/// `b` as a height above the horizon, kept clear of it and of the top.
fn height(b: f32) -> f32 {
    HORIZON + b.clamp(0.05, 0.95)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// i / (n - 1): evenly spread from 0 to 1 inclusive (0.5 for one beam).
fn frac(i: usize, n: usize) -> f32 {
    if n <= 1 { 0.5 } else { i as f32 / (n - 1) as f32 }
}

/// A line of `n` beams from `x0` to `x1` at height `y`.
fn line(n: usize, x0: f32, x1: f32, y: f32) -> Vec<(f32, f32)> {
    (0..n).map(|i| (lerp(x0, x1, frac(i, n)), y)).collect()
}

/// Pump envelope over one beat, 0..1: a quick open, then a fast close.
fn pump(phase_in_beat: f32) -> f32 {
    let x = phase_in_beat.rem_euclid(1.0);
    if x < PUMP_ATTACK { x / PUMP_ATTACK } else { beat::env_stab(x - PUMP_ATTACK, 0.0, PUMP_DECAY) }
}

fn fan(n: usize, p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<(f32, f32)> {
    let depth = p.a.clamp(0.0, 1.0);
    let half = if depth > 0.0 { w - depth * (w - W_MIN).max(0.0) * (1.0 - pump(ctx.beat_phase())) } else { w };
    line(n, -half, half, height(p.b))
}

fn fan_sweep(n: usize, p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<(f32, f32)> {
    let amp = p.a.clamp(0.0, 1.0);
    let half = 0.5 * w;
    let y = height(p.b);
    let cycle = ctx.cycle(p);
    (0..n)
        .map(|i| {
            let (ph, sign) = p.group_mode.motion(beat::group_of(i, n, p.groups), p.groups, cycle);
            (sign * amp * p.easing.swing(ph) + lerp(-half, half, frac(i, n)), y)
        })
        .collect()
}

fn fan_tilt(n: usize, p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<(f32, f32)> {
    let forward = beat::phase(ctx.beat_pos, p.period_beats);
    (0..n)
        .map(|i| {
            let (ph, sign) = p.group_mode.motion(beat::group_of(i, n, p.groups), p.groups, forward);
            // 0 = low, 1 = high.
            let lift = match p.direction {
                0 => beat::ease_in(1.0 - (2.0 * ph - 1.0).abs()),
                d if d < 0 => 1.0 - beat::ease_in(ph),
                _ => beat::ease_in(ph),
            };
            // A mirrored group goes the other way: down while the others rise.
            let lift = if sign < 0.0 { 1.0 - lift } else { lift };
            (lerp(-w, w, frac(i, n)), HORIZON + lerp(TILT_LOW, TILT_HIGH, lift))
        })
        .collect()
}

fn fan_wave(n: usize, p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<(f32, f32)> {
    let amp = p.a.clamp(0.0, 0.45);
    // Keep the whole wave above the horizon instead of flattening it.
    let base = height(p.b).clamp(HORIZON + amp + 0.02, 1.0 - amp);
    let dir = if p.direction < 0 { -1.0 } else { 1.0 };
    let travel = (dir * ctx.beat_pos * WAVE_SPEED).rem_euclid(1.0) as f32;
    (0..n)
        .map(|i| {
            let u = frac(i, n);
            // x / λ with λ = the fan's width (2w) is u - 0.5.
            (lerp(-w, w, u), base + amp * (TAU * (u - 0.5 - travel)).sin())
        })
        .collect()
}

fn positions(n: usize, p: &GenParams, ctx: &GenCtx, w: f32) -> Vec<(f32, f32)> {
    let y = height(p.b);
    match ctx.step(p) % 4 {
        0 => line(n, -w, w, (y + 0.2).min(HORIZON + 0.95)),
        k @ (1 | 2) => {
            let half = 0.3 * w;
            let a = if k == 1 { POS_TILT } else { -POS_TILT };
            let (s, c) = a.sin_cos();
            let yc = y.max(HORIZON + half * s.abs() + 0.02);
            (0..n)
                .map(|i| {
                    let x = lerp(-half, half, frac(i, n));
                    (x * c, yc + x * s)
                })
                .collect()
        }
        _ => {
            // A V: the left leg top to bottom, then the right leg bottom to
            // top, so the galvo path stays short.
            let apex = HORIZON + 0.05;
            let (left, right) = (n / 2, n - n / 2);
            let (s, c) = POS_V.sin_cos();
            let leg = |j: usize, m: usize, side: f32| {
                let d = w * (j + 1) as f32 / m as f32;
                (side * d * s, apex + d * c)
            };
            (0..left).rev().map(|j| leg(j, left, -1.0)).chain((0..right).map(|j| leg(j, right, 1.0))).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beat::{Easing, GroupMode};
    use crate::engine::densify;
    use crate::generators::{colorize, ColorMode, GENERATOR_NAMES};

    const NAMES: [&str; 5] = ["fan", "fan_sweep", "fan_tilt", "fan_wave", "positions"];

    fn at(beat_pos: f64, scale: f32) -> GenCtx {
        GenCtx { beat_pos, ..GenCtx::at_time(0.0, 0.0, 0.0, scale) }
    }

    fn beams(name: &str, p: &GenParams, beat_pos: f64) -> Vec<(f32, f32)> {
        let geo = generate(name, p, &at(beat_pos, 0.7)).unwrap();
        assert!(geo.dots, "{name} is a beam look");
        geo.strokes.iter().map(|s| s[0]).collect()
    }

    fn close(a: &[(f32, f32)], b: &[(f32, f32)]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4)
    }

    fn centre(pts: &[(f32, f32)]) -> f32 {
        pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32
    }

    #[test]
    fn fans_are_listed_after_the_original_generators() {
        assert_eq!(&GENERATOR_NAMES[20..], &NAMES);
        assert!(generate("beam_fan", &GenParams::default(), &at(0.0, 0.5)).is_none());
    }

    #[test]
    fn every_fan_stays_in_bounds_above_the_horizon_with_n_beams() {
        let params = [
            GenParams::default(), // the UI's defaults: a = 3, b = 2
            GenParams { count: 1, a: 0.0, b: -1.0, ..Default::default() },
            GenParams { count: 12, a: 0.35, b: 0.25, groups: 2, group_mode: GroupMode::Mirror, easing: Easing::Trapezoid, ..Default::default() },
            GenParams { count: 64, a: 10.0, b: 3.0, direction: 0, groups: 4, group_mode: GroupMode::Offset, steps_per_beat: 8.0, ..Default::default() },
            GenParams { count: 7, a: 0.2, b: 0.1, direction: -1, period_beats: 0.0, ..Default::default() },
        ];
        for name in NAMES {
            for p in &params {
                for scale in [0.0, 0.3, 0.7, 1.0, 1.5] {
                    for k in 0..80 {
                        let beat_pos = k as f64 * 0.137 - 1.0;
                        let geo = generate(name, p, &at(beat_pos, scale)).unwrap();
                        let n = p.count.clamp(1, MAX_BEAMS as u32) as usize;
                        assert_eq!(geo.strokes.len(), n, "{name}: one beam per count");
                        assert!(geo.strokes.iter().all(|s| s.len() == 1));
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
    fn fan_is_static_at_a_0_and_pumps_on_the_beat_otherwise() {
        let p = GenParams { count: 8, a: 0.0, b: 0.25, ..Default::default() };
        let fixed = beams("fan", &p, 0.0);
        assert!(close(&fixed, &beams("fan", &p, 2.4)), "a = 0: no motion");
        assert!((fixed[0].0 + 0.7).abs() < 1e-6 && (fixed[7].0 - 0.7).abs() < 1e-6 && fixed.iter().all(|b| (b.1 - 0.25).abs() < 1e-6));
        let p = GenParams { a: 1.0, ..p };
        let width = |beat: f64| beams("fan", &p, beat)[7].0;
        assert!(width(3.0) < 0.03, "closed on the beat itself, before the attack");
        assert!((width(3.0 + 1.0 / 16.0) - 0.7).abs() < 1e-3, "fully open 1/16 beat later");
        assert!(width(3.5) < 0.1, "mostly closed half a beat later: {}", width(3.5));
        assert!(width(3.3) < width(3.2), "closing");
    }

    #[test]
    fn fan_sweep_repeats_every_period_and_swings_by_a() {
        let p = GenParams { count: 8, a: 0.35, b: 0.3, period_beats: 4.0, ..Default::default() };
        let start = beams("fan_sweep", &p, 0.0);
        assert!(close(&start, &beams("fan_sweep", &p, 4.0)) && close(&start, &beams("fan_sweep", &p, 8.0)), "same place at beats 0, 4, 8");
        assert!(centre(&start).abs() < 1e-5);
        assert!((centre(&beams("fan_sweep", &p, 1.0)) - 0.35).abs() < 1e-4, "full swing right a quarter period in");
        assert!((centre(&beams("fan_sweep", &p, 3.0)) + 0.35).abs() < 1e-4, "and left at three quarters");
        // Reversed direction swings left first.
        assert!(centre(&beams("fan_sweep", &GenParams { direction: -1, ..p.clone() }, 1.0)) < -0.3);
        // Trapezoid easing holds the right end; sine doesn't.
        let trap = GenParams { easing: Easing::Trapezoid, ..p.clone() };
        assert!(close(&beams("fan_sweep", &trap, 0.6), &beams("fan_sweep", &trap, 1.4)));
        assert!(centre(&beams("fan_sweep", &p, 0.6)) < centre(&beams("fan_sweep", &p, 1.0)));
    }

    #[test]
    fn mirrored_sweep_groups_swing_in_opposition() {
        let p = GenParams { count: 8, a: 0.35, b: 0.3, groups: 2, group_mode: GroupMode::Mirror, ..Default::default() };
        for beat in [0.5, 1.0, 2.7, 3.2] {
            let b = beams("fan_sweep", &p, beat);
            let rest = beams("fan_sweep", &p, 0.0);
            let shift = |r: std::ops::Range<usize>| r.clone().map(|i| b[i].0 - rest[i].0).sum::<f32>() / r.len() as f32;
            let (left, right) = (shift(0..4), shift(4..8));
            assert!(left.abs() > 0.05 && (left + right).abs() < 1e-4, "beat {beat}: {left} vs {right}");
        }
        // Unison: one fan, all beams move together.
        let u = beams("fan_sweep", &GenParams { group_mode: GroupMode::Unison, ..p }, 1.0);
        assert!(u.windows(2).all(|w| w[1].0 > w[0].0));
    }

    #[test]
    fn fan_tilt_rises_with_ease_in_then_snaps_back_or_ping_pongs() {
        let p = GenParams { count: 6, period_beats: 8.0, ..Default::default() };
        let y = |p: &GenParams, beat: f64| beams("fan_tilt", p, beat)[0].1;
        assert!((y(&p, 0.0) - (HORIZON + 0.05)).abs() < 1e-5);
        assert!(y(&p, 2.0) - y(&p, 0.0) < y(&p, 8.0 - 1e-3) - y(&p, 6.0), "ease-in: slow start, fast end");
        assert!((y(&p, 8.0 - 1e-4) - (HORIZON + 0.7)).abs() < 1e-3, "top at the end of the period");
        assert!((y(&p, 8.0) - (HORIZON + 0.05)).abs() < 1e-5, "instant return on the period");
        let pp = GenParams { direction: 0, ..p.clone() };
        assert!((y(&pp, 4.0) - (HORIZON + 0.7)).abs() < 1e-5, "ping-pong: top half-way");
        assert!((y(&pp, 2.0) - y(&pp, 6.0)).abs() < 1e-5, "and back down the same way");
        let down = GenParams { direction: -1, ..p.clone() };
        assert!((y(&down, 0.0) - (HORIZON + 0.7)).abs() < 1e-5 && y(&down, 7.9) < 0.1);
        // Beams stay a horizontal fan at every height.
        assert!(beams("fan_tilt", &p, 5.0).windows(2).all(|w| (w[0].1 - w[1].1).abs() < 1e-6));
    }

    #[test]
    fn fan_wave_travels_half_a_wavelength_per_beat() {
        let p = GenParams { count: 12, a: 0.25, b: 0.4, ..Default::default() };
        let w0 = beams("fan_wave", &p, 0.0);
        assert!(close(&w0, &beams("fan_wave", &p, 2.0)), "one wavelength every 2 beats");
        let half = beams("fan_wave", &p, 1.0);
        assert!(w0.iter().zip(&half).all(|(a, b)| ((a.1 - 0.4) + (b.1 - 0.4)).abs() < 1e-4), "half a wavelength later the wave is inverted");
        let ys: Vec<f32> = w0.iter().map(|b| b.1).collect();
        let (lo, hi) = (ys.iter().cloned().fold(1.0, f32::min), ys.iter().cloned().fold(-1.0, f32::max));
        assert!(hi - lo > 0.4 && hi <= 0.65 + 1e-5 && lo >= 0.15 - 1e-5, "amplitude a around b: {lo}..{hi}");
        // A low mean height is lifted so the trough clears the horizon.
        let low = beams("fan_wave", &GenParams { b: 0.0, ..p }, 0.3);
        assert!(low.iter().all(|b| b.1 >= HORIZON + 0.02 - 1e-5));
    }

    #[test]
    fn positions_change_exactly_on_the_whole_beat() {
        let p = GenParams { count: 8, b: 0.3, steps_per_beat: 1.0, ..Default::default() };
        let pos: Vec<_> = (0..4).map(|k| beams("positions", &p, k as f64)).collect();
        assert!(close(&beams("positions", &p, 0.999), &pos[0]) && close(&beams("positions", &p, 1.0), &pos[1]));
        assert!(close(&beams("positions", &p, 2.999), &pos[2]) && close(&beams("positions", &p, 3.0), &pos[3]));
        assert!(close(&beams("positions", &p, 4.0), &pos[0]), "four positions, then round again");
        for i in 0..4 {
            for j in 0..i {
                assert!(!close(&pos[i], &pos[j]), "positions {i} and {j} differ");
            }
        }
        // P0: wide and flat, above the tilted ones' centre.
        assert!(pos[0].windows(2).all(|w| w[0].1 == w[1].1) && pos[0][7].0 > 0.69);
        // P1/P2: narrow fans tilted +15° and -15°.
        let slope = |b: &[(f32, f32)]| (b[7].1 - b[0].1) / (b[7].0 - b[0].0);
        assert!((slope(&pos[1]) - 15f32.to_radians().tan()).abs() < 1e-4);
        assert!((slope(&pos[2]) + 15f32.to_radians().tan()).abs() < 1e-4);
        assert!(pos[1][7].0 - pos[1][0].0 < 0.5);
        // P3: a V, 4 beams per leg at ±30° from the vertical.
        let v = &pos[3];
        let (left, right) = (&v[..4], &v[4..]);
        assert!(left.iter().all(|b| b.0 < 0.0) && right.iter().all(|b| b.0 > 0.0));
        for b in v {
            let angle = (b.0 / (b.1 - (HORIZON + 0.05))).atan().to_degrees().abs();
            assert!((angle - 30.0).abs() < 0.01, "{angle}");
        }
        // Twice as fast with 2 steps per beat.
        let fast = GenParams { steps_per_beat: 2.0, ..p };
        assert!(close(&beams("positions", &fast, 0.5), &pos[1]));
    }

    /// A fan alone must fit the layer mixer's frame budget (750 points =
    /// 40 fps at 30 kpps, so above the 30 fps floor) after densify, so it
    /// is never decimated (the wrap-around jump is blanked by the output).
    #[test]
    fn fans_hold_30_fps_at_30_kpps() {
        let p = GenParams { count: 64, a: 0.35, b: 0.3, groups: 2, group_mode: GroupMode::Mirror, ..Default::default() };
        for name in NAMES {
            let mut worst = 0;
            for k in 0..64 {
                let geo = generate(name, &p, &at(k as f64 * 0.0625, 1.0)).unwrap();
                let pts = densify(&colorize(&geo, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0, 1.0));
                worst = worst.max(pts.len());
            }
            assert!(worst <= crate::layers::DEFAULT_POINT_BUDGET, "'{name}' makes {worst} points");
        }
    }

    #[test]
    fn same_params_and_beat_give_the_same_frame_at_any_tempo() {
        let p = GenParams { count: 10, a: 0.4, b: 0.3, groups: 2, group_mode: GroupMode::Offset, ..Default::default() };
        for name in NAMES {
            let slow = generate(name, &p, &GenCtx { bpm: 90.0, t: 1.0, ..at(5.3, 0.7) }).unwrap();
            let fast = generate(name, &p, &GenCtx { bpm: 174.0, t: 9.0, ..at(5.3, 0.7) }).unwrap();
            assert_eq!(slow.strokes, fast.strokes, "{name} depends only on the beat");
        }
    }
}
