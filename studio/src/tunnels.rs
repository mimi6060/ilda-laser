//! Festival tunnels, cones and sun rays (T-105): the "flying through the
//! tunnel" moments of trance and big-room sets, as beat-synced generators.
//! Each is our own maths, written from the look descriptions in
//! `docs/research/festival-looks.md` section B.
//!
//! All of them move with the tempo clock through `ctx.beat_pos` (whatever
//! `beat_sync` says: their motion is defined in beats) and are pure
//! functions of (params, ctx). The tunnels are circles around
//! (0, `TUNNEL_CY`) with a radius of at most `MAX_RADIUS`, so the whole
//! cone stays above the horizon; the sun's rays fan out from
//! (0, `HORIZON`) into the upper half only. Rotating looks read `a` as
//! **turns per beat** (research §4.1: 1/64 glacial … 1/4 fast … 1 very
//! fast). Parameters:
//!
//! - `finger_tunnel`: `count` beams on a true circle of radius = size,
//!   turning `a` turns per beat (`direction` -1 turns the other way).
//! - `tunnel_pump`: one continuous circle. `b` < 0.5: its radius jumps to
//!   size × (1 + `a`) on each beat and is back to size half a beat later.
//!   `b` ≥ 0.5: a ramp from size down to `RAMP_END` over `period_beats`,
//!   then open again (`direction` -1 grows instead).
//! - `twin_tunnel`: two concentric circles (inner = 0.2/0.35 of the outer,
//!   outer = size) turning in opposite directions at `a` turns per beat,
//!   each with a 10 % gap so the rotation shows.
//! - `sunburst`: `count` rays at radius = size over the upper half-plane,
//!   turning `a` turns per beat; rays that leave the upper half are not
//!   drawn. `b` ≥ 0.5: odd and even rays swap between 100 % and 40 % on
//!   each beat.
//!
//! None of them flashes: brightness only changes for the sunburst's
//! odd/even swap, which keeps the total light constant and changes each
//! ray at most once per beat.

use crate::beat;
use crate::generators::{BeamStyle, GenCtx, GenParams, Geometry, Stroke};
use std::f32::consts::TAU;

/// Height of the audience line in the look's frame (same as the fans').
pub const HORIZON: f32 = crate::fans::HORIZON;
/// Centre height of the tunnels: with `MAX_RADIUS` the cone never dips
/// below the horizon (before the user's rotation and calibration).
pub const TUNNEL_CY: f32 = HORIZON + 0.5;
/// Largest tunnel radius (research: r_max = 0.45).
pub const MAX_RADIUS: f32 = 0.5;
/// Fastest rotation, in turns per beat (research §4.1 "very fast").
pub const MAX_TURNS: f32 = 1.0;
/// Beam counts (research §4.3: finger tunnel ≤ 16, sunburst ≤ 24).
pub const FINGER_MAX: usize = 16;
pub const SUN_MAX: usize = 24;
/// Segments of a full circle: a smooth outline that still redraws far
/// faster than the 40 Hz a cone needs to look solid.
const CIRCLE_SEGMENTS: usize = 72;
/// Pump envelope: instant jump on the beat, exponential fall with this
/// time constant (in beats), back to rest exactly at `PUMP_RELEASE`.
const PUMP_TAU: f32 = 0.1;
const PUMP_RELEASE: f32 = 0.5;
/// Where the pump ramp ends: almost a single beam.
const RAMP_END: f32 = 0.04;
/// Twin tunnel: inner radius / outer radius (research: 0.2 and 0.35),
/// and the gap left in each ring, as a fraction of a turn.
const TWIN_INNER: f32 = 0.2 / 0.35;
const TWIN_GAP: f32 = 0.1;
/// Sunburst odd/even: the dimmed group's intensity.
const SUN_DIM: f32 = 0.4;

/// Draw tunnel generator `name`, or `None` if it isn't one.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    let geo = match name {
        "finger_tunnel" => finger_tunnel(p, ctx),
        "tunnel_pump" => tunnel_pump(p, ctx),
        "twin_tunnel" => twin_tunnel(p, ctx),
        "sunburst" => sunburst(p, ctx),
        _ => return None,
    };
    Some(geo)
}

/// Rotation reached at this beat, in turns (0..1): `a` turns per beat,
/// backwards when `direction` is negative. Computed in f64 so a long set
/// stays exact on the beat.
fn spin(p: &GenParams, ctx: &GenCtx) -> f32 {
    let turns = p.a.clamp(0.0, MAX_TURNS) as f64;
    let dir = if p.direction < 0 { -1.0 } else { 1.0 };
    (dir * turns * ctx.beat_pos).rem_euclid(1.0) as f32
}

fn radius(scale: f32) -> f32 {
    scale.clamp(0.0, MAX_RADIUS)
}

/// A point at `turns` around (0, `TUNNEL_CY`).
fn on_circle(r: f32, turns: f32) -> (f32, f32) {
    let (s, c) = (TAU * turns).sin_cos();
    (r * c, TUNNEL_CY + r * s)
}

/// An arc of `length` turns starting at `start`, around (0, `TUNNEL_CY`).
fn arc(r: f32, start: f32, length: f32) -> Stroke {
    let n = ((CIRCLE_SEGMENTS as f32 * length).ceil() as usize).max(2);
    (0..=n).map(|i| on_circle(r, start + length * i as f32 / n as f32)).collect()
}

fn finger_tunnel(p: &GenParams, ctx: &GenCtx) -> Geometry {
    let n = p.count.clamp(1, FINGER_MAX as u32) as usize;
    let (r, s) = (radius(ctx.scale), spin(p, ctx));
    Geometry::dots((0..n).map(|i| on_circle(r, i as f32 / n as f32 + s)).collect())
}

/// Radius of the pumping tunnel at this beat.
fn pump_radius(p: &GenParams, ctx: &GenCtx) -> f32 {
    if p.b >= 0.5 {
        // Ramp: tight through the build, then open again on the period.
        let start = radius(ctx.scale).max(RAMP_END);
        return start + (RAMP_END - start) * ctx.cycle(p);
    }
    let depth = p.a.clamp(0.0, 1.0);
    // The rest radius leaves room for the kick, so the peak stays <= MAX.
    let rest = ctx.scale.clamp(0.0, MAX_RADIUS / (1.0 + depth));
    let x = ctx.beat_phase();
    let env = if x < PUMP_RELEASE { beat::env_stab(x, 0.0, PUMP_TAU) } else { 0.0 };
    rest * (1.0 + depth * env)
}

fn tunnel_pump(p: &GenParams, ctx: &GenCtx) -> Geometry {
    Geometry::lines(vec![arc(pump_radius(p, ctx), 0.0, 1.0)])
}

fn twin_tunnel(p: &GenParams, ctx: &GenCtx) -> Geometry {
    let outer = radius(ctx.scale);
    let s = spin(p, ctx);
    Geometry::lines(vec![arc(outer * TWIN_INNER, -s, 1.0 - TWIN_GAP), arc(outer, s, 1.0 - TWIN_GAP)])
}

fn sunburst(p: &GenParams, ctx: &GenCtx) -> Geometry {
    let n = p.count.clamp(1, SUN_MAX as u32) as usize;
    let r = ctx.scale.clamp(0.0, 1.0);
    let s = spin(p, ctx);
    // 2n rays around the whole circle, half a spacing off the horizon at
    // rest, so exactly n are in the upper half (n - 1 while one crosses).
    let mut rays: Vec<(usize, f32)> = (0..2 * n)
        .map(|k| (k, TAU * ((k as f32 + 0.5) / (2 * n) as f32 + s)))
        .filter(|&(_, angle)| angle.sin() > 0.0)
        .collect();
    // Right to left, so the galvo sweeps once across.
    rays.sort_by(|a, b| b.1.cos().total_cmp(&a.1.cos()));
    let points = rays.iter().map(|&(_, angle)| (r * angle.cos(), (HORIZON + r * angle.sin()).clamp(HORIZON, 1.0))).collect();
    let geo = Geometry::dots(points);
    if p.b < 0.5 {
        return geo;
    }
    // Odd/even: the groups swap on each whole beat (never faster, whatever
    // `steps_per_beat` says), and the total light stays the same.
    let beat = ctx.beat_pos.floor() as i64;
    let styles = rays
        .iter()
        .map(|&(k, _)| BeamStyle { intensity: if (k as i64 + beat).rem_euclid(2) == 0 { 1.0 } else { SUN_DIM }, ..Default::default() })
        .collect();
    geo.with_styles(styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::densify;
    use crate::generators::{colorize, generate as any_generator, ColorMode, GENERATOR_NAMES};
    use crate::safety::{level, SafetySettings, StrobeLimiter};

    const NAMES: [&str; 4] = ["finger_tunnel", "tunnel_pump", "twin_tunnel", "sunburst"];

    fn at(beat_pos: f64, scale: f32) -> GenCtx {
        GenCtx { beat_pos, ..GenCtx::at_time(0.0, 0.0, 0.0, scale) }
    }

    fn geo(name: &str, p: &GenParams, beat_pos: f64, scale: f32) -> Geometry {
        generate(name, p, &at(beat_pos, scale)).unwrap()
    }

    fn beams(name: &str, p: &GenParams, beat_pos: f64) -> Vec<(f32, f32)> {
        let g = geo(name, p, beat_pos, 0.35);
        assert!(g.dots, "{name} is a beam look");
        g.strokes.iter().map(|s| s[0]).collect()
    }

    /// Angle of a point around the tunnel centre, in turns (0..1).
    fn turns_of((x, y): (f32, f32)) -> f32 {
        ((y - TUNNEL_CY).atan2(x) / TAU).rem_euclid(1.0)
    }

    fn dist(a: f32, b: f32) -> f32 {
        let d = (a - b).rem_euclid(1.0);
        d.min(1.0 - d)
    }

    fn radii(g: &Geometry) -> Vec<f32> {
        g.strokes.iter().flatten().map(|&(x, y)| x.hypot(y - TUNNEL_CY)).collect()
    }

    fn render(name: &str, p: &GenParams, beat_pos: f64, scale: f32) -> Vec<crate::patterns::Point> {
        let g = any_generator(name, p, &at(beat_pos, scale)).unwrap();
        densify(&colorize(&g, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0, 1.0))
    }

    #[test]
    fn tunnels_are_listed_after_the_fans() {
        assert_eq!(&GENERATOR_NAMES[25..29], &NAMES);
        assert!(generate("tunnel", &GenParams::default(), &at(0.0, 0.5)).is_none());
        assert!(generate("fan", &GenParams::default(), &at(0.0, 0.5)).is_none());
    }

    #[test]
    fn every_tunnel_stays_in_bounds_and_above_the_horizon() {
        let params = [
            GenParams::default(), // the UI's defaults: a = 3, b = 2
            GenParams { count: 1, a: 0.0, b: -1.0, ..Default::default() },
            GenParams { count: 12, a: 0.25, b: 0.0, direction: -1, ..Default::default() },
            GenParams { count: 64, a: 10.0, b: 3.0, direction: 0, period_beats: 0.0, ..Default::default() },
            GenParams { count: 16, a: 1.0 / 64.0, b: 1.0, period_beats: 16.0, ..Default::default() },
        ];
        for name in NAMES {
            for p in &params {
                for scale in [0.0, 0.2, 0.35, 0.5, 0.85, 1.0, 1.5] {
                    for k in 0..80 {
                        let beat_pos = k as f64 * 0.137 - 1.0;
                        let g = geo(name, p, beat_pos, scale);
                        assert!(!g.strokes.is_empty(), "{name} draws something");
                        for &(x, y) in g.strokes.iter().flatten() {
                            assert!(x.is_finite() && y.is_finite() && x.abs() <= 1.0 && y.abs() <= 1.0, "{name} ({x}, {y})");
                            assert!(y >= HORIZON - 1e-6, "{name} below the horizon: y={y} at beat {beat_pos}");
                        }
                        if name != "sunburst" {
                            assert!(radii(&g).iter().all(|&r| r <= MAX_RADIUS + 1e-5), "{name} wider than MAX_RADIUS");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn finger_tunnel_beams_sit_on_a_true_circle() {
        for scale in [0.25, 0.35, 0.4, 0.5] {
            for beat in [0.0, 0.3, 2.7, 13.1] {
                let g = geo("finger_tunnel", &GenParams { count: 12, a: 1.0 / 16.0, ..Default::default() }, beat, scale);
                assert_eq!(g.strokes.len(), 12);
                assert!(radii(&g).iter().all(|&r| (r - scale).abs() <= 0.01 * scale), "radius {scale} ± 1 %: {:?}", radii(&g));
            }
        }
        // Evenly spread: a spoke every 1/n turn.
        let b = beams("finger_tunnel", &GenParams { count: 8, a: 0.0, ..Default::default() }, 0.0);
        assert_eq!(b.len(), 8);
        for (i, &beam) in b.iter().enumerate() {
            assert!(dist(turns_of(beam), i as f32 / 8.0) < 1e-4);
        }
    }

    #[test]
    fn a_quarter_turn_per_beat_is_home_again_after_four_beats() {
        let p = GenParams { count: 8, a: 0.25, ..Default::default() };
        let angle = |p: &GenParams, beat: f64| turns_of(beams("finger_tunnel", p, beat)[0]);
        let start = angle(&p, 0.0);
        assert!(dist(angle(&p, 4.0), start) < 1e-4 && dist(angle(&p, 400.0), start) < 1e-4);
        assert!(dist(angle(&p, 1.0), start + 0.25) < 1e-4, "a quarter turn after one beat");
        assert!(dist(angle(&p, 2.5), start + 0.625) < 1e-4);
        let rev = GenParams { direction: -1, ..p.clone() };
        assert!(dist(angle(&rev, 1.0), start - 0.25) < 1e-4, "reversed");
        // The UI's speeds: 1/16 turn per beat = one turn per 4 bars.
        let medium = GenParams { a: 1.0 / 16.0, ..p.clone() };
        assert!(dist(angle(&medium, 16.0), start) < 1e-4 && dist(angle(&medium, 8.0), start + 0.5) < 1e-4);
        // Frozen at a = 0, and the same frame at any tempo.
        let still = GenParams { a: 0.0, ..p.clone() };
        assert_eq!(beams("finger_tunnel", &still, 0.0), beams("finger_tunnel", &still, 3.3));
        for name in NAMES {
            let slow = generate(name, &p, &GenCtx { bpm: 90.0, t: 1.0, ..at(5.3, 0.35) }).unwrap();
            let fast = generate(name, &p, &GenCtx { bpm: 174.0, t: 9.0, ..at(5.3, 0.35) }).unwrap();
            assert_eq!(slow.strokes, fast.strokes, "{name} depends only on the beat");
        }
    }

    #[test]
    fn tunnel_pump_kicks_on_the_beat_and_rests_half_a_beat_later() {
        let p = GenParams { a: 0.3, b: 0.0, ..Default::default() };
        let r = |beat: f64| {
            let g = geo("tunnel_pump", &p, beat, 0.3);
            assert!(!g.dots && g.strokes.len() == 1);
            let rs = radii(&g);
            assert!(rs.iter().all(|&x| (x - rs[0]).abs() < 1e-4), "a circle");
            rs[0]
        };
        assert!((r(3.0) - 0.39).abs() < 1e-4, "biggest right on the beat: {}", r(3.0));
        assert!(r(3.01) > r(3.1) && r(3.1) > r(3.3) && r(3.3) > 0.3, "then shrinks");
        for beat in [3.5, 3.7, 3.99] {
            assert!((r(beat) - 0.3).abs() < 1e-6, "back to rest at half a beat ({beat}: {})", r(beat));
        }
        assert!((r(3.5 - 1e-3) - 0.3).abs() < 0.003, "no visible jump at the release");
        // Depth 0: no pump. A big size leaves room for the kick.
        assert!((geo("tunnel_pump", &GenParams { a: 0.0, ..p.clone() }, 3.0, 0.3).strokes[0][0].0 - 0.3).abs() < 1e-6);
        assert!(radii(&geo("tunnel_pump", &p, 3.0, 1.0)).iter().all(|&x| (x - MAX_RADIUS).abs() < 1e-4));
    }

    #[test]
    fn tunnel_pump_ramp_closes_over_the_period_then_opens() {
        let p = GenParams { b: 1.0, period_beats: 16.0, ..Default::default() };
        let r = |p: &GenParams, beat: f64| radii(&geo("tunnel_pump", p, beat, 0.35))[0];
        assert!((r(&p, 0.0) - 0.35).abs() < 1e-5);
        assert!((r(&p, 8.0) - (0.35 + 0.04) / 2.0).abs() < 1e-4, "linear");
        assert!((r(&p, 16.0 - 1e-3) - 0.04).abs() < 1e-3, "almost a single beam at the end");
        assert!((r(&p, 16.0) - 0.35).abs() < 1e-5, "open again on the period");
        let grow = GenParams { direction: -1, ..p.clone() };
        assert!(r(&grow, 1.0) < r(&grow, 15.0), "reversed: grows");
    }

    #[test]
    fn twin_tunnel_rings_turn_against_each_other_with_a_visible_gap() {
        let p = GenParams { a: 1.0 / 8.0, ..Default::default() };
        let rings = |beat: f64| {
            let g = geo("twin_tunnel", &p, beat, 0.35);
            assert!(!g.dots && g.strokes.len() == 2);
            g.strokes
        };
        let r0 = rings(0.0);
        let ring_r = |s: &Stroke| s[0].0.hypot(s[0].1 - TUNNEL_CY);
        assert!((ring_r(&r0[0]) - 0.2).abs() < 1e-4 && (ring_r(&r0[1]) - 0.35).abs() < 1e-4);
        // Each ring covers 90 % of a turn: the gap is the marker.
        for s in &r0 {
            let span = dist(turns_of(s[0]), turns_of(*s.last().unwrap()));
            assert!((span - TWIN_GAP).abs() < 1e-3, "gap {span}");
        }
        // 1/8 turn per beat: opposite ways, home after 8 beats (2 bars).
        let r2 = rings(2.0);
        assert!(dist(turns_of(r2[1][0]), turns_of(r0[1][0]) + 0.25) < 1e-4, "outer forwards");
        assert!(dist(turns_of(r2[0][0]), turns_of(r0[0][0]) - 0.25) < 1e-4, "inner backwards");
        let r8 = rings(8.0);
        assert!(r8.iter().zip(&r0).all(|(a, b)| a.iter().zip(b).all(|(p, q)| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4)));
    }

    #[test]
    fn sunburst_never_lights_a_ray_below_the_horizon() {
        for n in [1, 12, 18, 24, 64] {
            for dir in [1, -1] {
                let p = GenParams { count: n, a: 0.25, b: 1.0, direction: dir, ..Default::default() };
                for k in 0..200 {
                    let beat = k as f64 * 0.0173;
                    let pts = render("sunburst", &p, beat, 0.85);
                    let lit: Vec<_> = pts.iter().filter(|q| q.is_lit()).collect();
                    assert!(!lit.is_empty() && lit.iter().all(|q| q.y > HORIZON), "n={n} beat {beat}");
                    let g = geo("sunburst", &p, beat, 0.85);
                    let want = (n as usize).min(SUN_MAX);
                    assert!(g.strokes.len() == want || g.strokes.len() + 1 == want, "n rays in the upper half");
                    assert!(g.strokes.iter().all(|s| (s[0].0.hypot(s[0].1 - HORIZON) - 0.85).abs() < 1e-4), "radius 0.85");
                }
            }
        }
        // At rest the rays fill the upper half evenly, off the horizon.
        let rest = beams("sunburst", &GenParams { count: 12, a: 0.0, b: 0.0, ..Default::default() }, 0.0);
        assert_eq!(rest.len(), 12);
        assert!(rest.iter().all(|b| b.1 > 0.03), "none sits on the horizon");
    }

    #[test]
    fn sunburst_odd_and_even_rays_swap_on_each_beat() {
        let p = GenParams { count: 12, a: 0.0, b: 1.0, ..Default::default() };
        let styles = |beat: f64| geo("sunburst", &p, beat, 0.85).styles.iter().map(|s| s.intensity).collect::<Vec<_>>();
        let (s0, s1) = (styles(0.2), styles(1.2));
        assert_eq!(s0, styles(0.9), "steady within the beat");
        assert!(s0.iter().zip(&s1).all(|(a, b)| (a + b - 1.4).abs() < 1e-6), "100 % and 40 % swap");
        assert_eq!(s0.iter().filter(|&&i| i == 1.0).count(), 6, "half bright, half dim");
        assert!(s0.windows(2).all(|w| w[0] != w[1]), "neighbours alternate");
        assert_eq!(styles(2.2), s0);
        // Steps per beat never make it faster.
        let fast = GenParams { steps_per_beat: 8.0, ..p.clone() };
        assert_eq!(geo("sunburst", &fast, 0.6, 0.85).styles.iter().map(|s| s.intensity).collect::<Vec<_>>(), s0);
        // b < 0.5: all rays full.
        assert!(geo("sunburst", &GenParams { b: 0.0, ..p }, 1.2, 0.85).styles.is_empty());
    }

    /// No tunnel flashes: through the real strobe limiter at the fastest
    /// tempo (250 BPM, 60 fps), none is ever seen as flashing, and the
    /// frame's light never drops by half.
    #[test]
    fn no_tunnel_flashes_even_at_250_bpm() {
        let cfg = SafetySettings::default();
        let params = [
            GenParams { count: 24, a: 1.0, b: 1.0, ..Default::default() },
            GenParams { count: 13, a: 0.25, b: 1.0, direction: -1, ..Default::default() },
            GenParams { count: 16, a: 1.0, b: 0.0, period_beats: 1.0, ..Default::default() },
            GenParams { count: 16, a: 1.0, b: 1.0, period_beats: 1.0, ..Default::default() },
        ];
        for name in NAMES {
            for p in &params {
                let mut limiter = StrobeLimiter::default();
                let (mut lo, mut hi) = (f32::MAX, 0.0f32);
                for f in 0..600 {
                    let t = f as f64 / 60.0;
                    let frame = render(name, p, t * 250.0 / 60.0, 0.4);
                    let e = level(&frame);
                    (lo, hi) = (lo.min(e), hi.max(e));
                    limiter.process(frame, t, &cfg);
                    let st = limiter.status();
                    assert!(!st.fast && !st.active, "{name} flashes at {t:.2} s ({:.1} Hz)", st.rate_hz);
                }
                assert!(lo > 0.5 * hi, "{name}: light drops from {hi} to {lo}");
            }
        }
    }

    /// Each look alone fits the layer mixer's frame budget after densify.
    #[test]
    fn tunnels_hold_30_fps_at_30_kpps() {
        let params = [
            GenParams { count: 64, a: 0.25, b: 1.0, ..Default::default() },
            GenParams { count: 64, a: 1.0, b: 0.0, ..Default::default() },
        ];
        for name in NAMES {
            let mut worst = 0;
            for p in &params {
                for scale in [0.35, 0.5, 1.0] {
                    for k in 0..64 {
                        worst = worst.max(render(name, p, k as f64 * 0.0625, scale).len());
                    }
                }
            }
            assert!(worst <= crate::layers::DEFAULT_POINT_BUDGET, "'{name}' makes {worst} points");
        }
        // A snapping square tunnel of 4 rings at full size (snap only rotates:
        // same points as the smooth one).
        let snap = GenParams { count: 4, a: 4.0, snap: true, ..Default::default() };
        let worst = (0..16).map(|k| render("polygon_tunnel", &snap, k as f64 * 0.25, 1.0).len()).max().unwrap();
        assert!(worst <= crate::layers::DEFAULT_POINT_BUDGET, "polygon_tunnel with snap makes {worst} points");
    }
}
