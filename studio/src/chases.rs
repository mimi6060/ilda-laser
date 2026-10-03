//! Beam chasers (T-103, look 7 of `docs/research/festival-looks.md`): a
//! fan where only the chase index is lit. Our own maths, written from the
//! research's description in words.
//!
//! `chase_fan`: `count` beams on a fixed line `CHASE_HEIGHT` above the
//! horizon, half-width = the look size. The galvo visits every beam on
//! every frame - the geometry never moves - and only each beam's gate
//! (`Geometry::styles`) changes, one step per `steps_per_beat`.
//!
//! - `a` (rounded) = chase shape, see `ChaseShape`.
//! - `b` (rounded, 0..=3) = tail length: the beams lit by the previous
//!   steps stay on at `TAIL` intensities (40 %, 15 %, 6 %). Shapes that
//!   light groups (odd/even, fill) have no tail.
//! - Colour mode « Alterné »: the head in the main colour, the tail in
//!   `color2`.

use crate::beat;
use crate::fans::{self, HORIZON, MAX_BEAMS};
use crate::generators::{BeamStyle, ColorMode, GenCtx, GenParams, Geometry, Tint};

/// Height of the chase fan above the horizon.
pub const CHASE_HEIGHT: f32 = HORIZON + 0.3;
/// Intensity of the tail beams, newest first (research: 40 % and 15 %;
/// the third one continues the fade).
pub const TAIL: [f32; 3] = [0.4, 0.15, 0.06];
/// Seed of the random chase: the same steps pick the same beams on every
/// run, so frames and tests are reproducible.
const RANDOM_SEED: u64 = 0x0c4a_5e00;

/// The order in which a chase visits the beams (`a` of `chase_fan`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChaseShape {
    /// 0: left to right, then round again.
    Forward,
    /// 1: right to left.
    Backward,
    /// 2: there and back (ping-pong), without repeating the end beams.
    Bounce,
    /// 3: from the centre pair out to the edges.
    CentreOut,
    /// 4: from the edges in to the centre.
    OutsideIn,
    /// 5: even beams, then odd beams.
    OddEven,
    /// 6: a random beam per step (seeded by the step).
    Random,
    /// 7: beams light one by one until all are on, then go off one by one.
    Fill,
}

impl ChaseShape {
    pub fn from_a(a: f32) -> Self {
        match if a.is_finite() { a.round().clamp(0.0, 7.0) as u8 } else { 0 } {
            0 => Self::Forward,
            1 => Self::Backward,
            2 => Self::Bounce,
            3 => Self::CentreOut,
            4 => Self::OutsideIn,
            5 => Self::OddEven,
            6 => Self::Random,
            _ => Self::Fill,
        }
    }

    /// Whether the shape lights a moving head that can leave a tail.
    fn has_tail(self) -> bool {
        !matches!(self, Self::OddEven | Self::Fill)
    }

    /// Whether beam `i` of `n` is lit at chase step `s` (the head, or the
    /// lit group for odd/even and fill).
    pub fn lit(self, s: u64, n: usize, i: usize) -> bool {
        if n == 0 || i >= n {
            return false;
        }
        let n64 = n as u64;
        match self {
            Self::Forward => i as u64 == s % n64,
            Self::Backward => i as u64 == n64 - 1 - s % n64,
            Self::Bounce => {
                if n == 1 {
                    return true;
                }
                let period = 2 * n64 - 2;
                let k = s % period;
                i as u64 == if k < n64 { k } else { period - k }
            }
            Self::CentreOut => ring(i, n) as u64 == s % rings(n) as u64,
            Self::OutsideIn => ring(i, n) as u64 == rings(n) as u64 - 1 - s % rings(n) as u64,
            Self::OddEven => i as u64 % 2 == s % 2,
            Self::Random => i == ((beat::seeded_rand(RANDOM_SEED, s) * n as f32) as usize).min(n - 1),
            Self::Fill => {
                let k = s % (2 * n64);
                if k < n64 { i as u64 <= k } else { i as u64 > k - n64 }
            }
        }
    }
}

/// Distance of beam `i` from the centre, in beams: 0 for the middle beam
/// (odd `n`) or the middle pair (even `n`).
fn ring(i: usize, n: usize) -> usize {
    let (lo, hi) = ((n - 1) / 2, n / 2);
    if i <= lo { lo - i } else { i - hi }
}

/// Number of distinct `ring`s of `n` beams.
fn rings(n: usize) -> usize {
    n.div_ceil(2)
}

/// Intensity of beam `i` of `n` at step `s`: 1 for the head, a `TAIL`
/// value if a previous step lit it, 0 otherwise.
pub fn intensity(shape: ChaseShape, tail: usize, s: u64, n: usize, i: usize) -> f32 {
    if shape.lit(s, n, i) {
        return 1.0;
    }
    if !shape.has_tail() {
        return 0.0;
    }
    (1..=tail.min(TAIL.len()) as u64)
        .take_while(|&j| j <= s)
        .find(|&j| shape.lit(s - j, n, i))
        .map_or(0.0, |j| TAIL[j as usize - 1])
}

/// Draw chase generator `name`, or `None` if it isn't one.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    if name != "chase_fan" {
        return None;
    }
    let n = p.count.clamp(1, MAX_BEAMS as u32) as usize;
    let w = ctx.scale.clamp(0.0, 1.0);
    let shape = ChaseShape::from_a(p.a);
    let tail = if p.b.is_finite() { p.b.round().clamp(0.0, TAIL.len() as f32) as usize } else { 0 };
    let s = ctx.step(p);
    let split = p.color_mode == ColorMode::Alternate && shape.has_tail();
    let styles = (0..n)
        .map(|i| {
            let k = intensity(shape, tail, s, n, i);
            let tint = match (split, k >= 1.0) {
                (false, _) => Tint::Auto,
                (true, true) => Tint::Primary,
                (true, false) => Tint::Secondary,
            };
            BeamStyle { intensity: k, tint }
        })
        .collect();
    Some(Geometry::dots(fans::line(n, -w, w, CHASE_HEIGHT)).with_styles(styles))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::densify;
    use crate::generators::{colorize, GENERATOR_NAMES};

    fn at(beat_pos: f64) -> GenCtx {
        GenCtx { beat_pos, ..GenCtx::at_time(0.0, 0.0, 0.0, 0.7) }
    }

    fn params(shape: f32, tail: f32) -> GenParams {
        GenParams { count: 8, a: shape, b: tail, steps_per_beat: 1.0, ..Default::default() }
    }

    /// Intensity of each beam at `beat_pos`.
    fn gates(p: &GenParams, beat_pos: f64) -> Vec<f32> {
        generate("chase_fan", p, &at(beat_pos)).unwrap().styles.iter().map(|s| s.intensity).collect()
    }

    /// The lit beams (heads) at step `s` of 8 beams.
    fn heads(shape: ChaseShape, s: u64) -> Vec<usize> {
        (0..8).filter(|&i| shape.lit(s, 8, i)).collect()
    }

    #[test]
    fn chase_fan_is_listed_after_the_sheets() {
        let pos = |name: &str| GENERATOR_NAMES.iter().position(|n| *n == name).unwrap();
        assert!(pos("chase_fan") > pos("grid"), "appended: saved looks refer to generators by name");
        assert!(generate("fan", &GenParams::default(), &at(0.0)).is_none());
    }

    #[test]
    fn forward_chase_lights_beam_k_on_beat_k_mod_8() {
        let p = params(0.0, 0.0);
        for beat in 0..20u64 {
            for frac in [0.0, 0.5, 0.99] {
                let g = gates(&p, beat as f64 + frac);
                let lit: Vec<usize> = (0..8).filter(|&i| g[i] > 0.0).collect();
                assert_eq!(lit, vec![(beat % 8) as usize], "beat {beat}+{frac}");
            }
        }
        // Two steps per beat: beam 3 half-way through beat 1.
        assert_eq!(gates(&GenParams { steps_per_beat: 2.0, ..p }, 1.5)[3], 1.0);
    }

    #[test]
    fn every_shape_visits_beams_in_its_order() {
        use ChaseShape::*;
        let seq = |shape: ChaseShape, steps: u64| (0..steps).map(|s| heads(shape, s)).collect::<Vec<_>>();
        let one = |v: &[usize]| v.iter().map(|&i| vec![i]).collect::<Vec<_>>();
        assert_eq!(seq(Backward, 9), one(&[7, 6, 5, 4, 3, 2, 1, 0, 7]));
        assert_eq!(seq(Bounce, 16), one(&[0, 1, 2, 3, 4, 5, 6, 7, 6, 5, 4, 3, 2, 1, 0, 1]));
        assert_eq!(seq(CentreOut, 5), vec![vec![3, 4], vec![2, 5], vec![1, 6], vec![0, 7], vec![3, 4]]);
        assert_eq!(seq(OutsideIn, 5), vec![vec![0, 7], vec![1, 6], vec![2, 5], vec![3, 4], vec![0, 7]]);
        assert_eq!(seq(OddEven, 3), vec![vec![0, 2, 4, 6], vec![1, 3, 5, 7], vec![0, 2, 4, 6]]);
        let fill = seq(Fill, 17);
        assert_eq!(fill[0], [0]);
        assert_eq!(fill[3], [0, 1, 2, 3]);
        assert_eq!(fill[7], (0..8).collect::<Vec<_>>(), "full after n steps");
        assert_eq!(fill[8], (1..8).collect::<Vec<_>>(), "then empties from the first");
        assert!(fill[15].is_empty() && fill[16] == [0], "one dark step, then again");
        // Odd beam counts: a single centre beam.
        assert!(CentreOut.lit(0, 5, 2) && !CentreOut.lit(0, 5, 1) && CentreOut.lit(1, 5, 1) && CentreOut.lit(1, 5, 3));
        // Random: one beam per step, reproducible, and it does move around.
        let r = seq(Random, 64);
        assert!(r.iter().all(|h| h.len() == 1));
        assert_eq!(r, seq(Random, 64));
        let distinct: std::collections::HashSet<_> = r.iter().map(|h| h[0]).collect();
        assert!(distinct.len() >= 6, "{distinct:?}");
        // Out-of-range shapes clamp; a lone beam is always the head.
        assert_eq!(ChaseShape::from_a(-3.0), Forward);
        assert_eq!(ChaseShape::from_a(42.0), Fill);
        assert_eq!(ChaseShape::from_a(f32::NAN), Forward);
        for a in 0..8 {
            let shape = ChaseShape::from_a(a as f32);
            if !matches!(shape, Fill | OddEven) {
                assert!((0..20).all(|s| shape.lit(s, 1, 0)), "{shape:?} with one beam");
            }
        }
    }

    #[test]
    fn tail_fades_1_then_0_4_then_0_15() {
        let g = gates(&params(0.0, 2.0), 5.0);
        assert_eq!(g, vec![0.0, 0.0, 0.0, 0.15, 0.4, 1.0, 0.0, 0.0]);
        // Wraps around the end of the fan.
        assert_eq!(gates(&params(0.0, 2.0), 9.0), vec![0.4, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15]);
        // b = 3 adds a faint third beam; b = 0 is the head alone.
        assert_eq!(gates(&params(0.0, 3.0), 5.0)[2], 0.06);
        assert_eq!(gates(&params(0.0, 0.0), 5.0).iter().filter(|&&k| k > 0.0).count(), 1);
        // No tail before the chase has made those steps.
        assert_eq!(gates(&params(0.0, 2.0), 0.0), vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // A bounce's tail follows it back.
        assert_eq!(gates(&params(2.0, 2.0), 9.0)[5..], [1.0, 0.4, 0.15]);
        // Group shapes have no tail.
        assert_eq!(gates(&params(5.0, 3.0), 1.0), vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn beam_positions_never_change_only_the_gates_do() {
        for shape in 0..8 {
            let p = GenParams { count: 12, a: shape as f32, b: 2.0, ..Default::default() };
            let first = generate("chase_fan", &p, &at(0.0)).unwrap();
            assert!(first.dots);
            let mut gate_sets = std::collections::HashSet::new();
            for k in 0..40 {
                let geo = generate("chase_fan", &p, &at(k as f64 * 0.37)).unwrap();
                assert_eq!(geo.strokes, first.strokes, "shape {shape}: the geometry moved");
                assert!(geo.strokes.iter().all(|s| (s[0].1 - CHASE_HEIGHT).abs() < 1e-6 && s[0].0.abs() <= 0.7 + 1e-6));
                gate_sets.insert(geo.styles.iter().map(|s| (s.intensity * 100.0) as u32).collect::<Vec<_>>());
            }
            assert!(gate_sets.len() > 1, "shape {shape} never changes");
        }
    }

    #[test]
    fn alternate_colours_the_tail_with_color2() {
        let p = GenParams { color_mode: ColorMode::Alternate, ..params(0.0, 2.0) };
        let geo = generate("chase_fan", &p, &at(5.0)).unwrap();
        assert_eq!(geo.styles[5].tint, Tint::Primary);
        assert_eq!((geo.styles[4].tint, geo.styles[3].tint), (Tint::Secondary, Tint::Secondary));
        let pts = colorize(&geo, p.color_mode, (1.0, 1.0, 1.0), (0.0, 0.0, 1.0), 0.0, 1.0);
        let lit_at = |x: f32| pts.iter().filter(|q| q.is_lit() && (q.x - x).abs() < 1e-6).copied().collect::<Vec<_>>();
        assert!(lit_at(geo.strokes[5][0].0).iter().all(|q| (q.r, q.g, q.b) == (1.0, 1.0, 1.0)), "white head");
        assert!(lit_at(geo.strokes[4][0].0).iter().all(|q| (q.r, q.g, q.b) == (0.0, 0.0, 0.4)), "blue tail at 40 %");
        // Other modes leave the colour to the mode.
        assert!(generate("chase_fan", &params(0.0, 2.0), &at(5.0)).unwrap().styles.iter().all(|s| s.tint == Tint::Auto));
    }

    #[test]
    fn chase_fan_fits_the_point_budget_and_bounds() {
        for count in [1, 8, 16, 64] {
            for shape in 0..8 {
                let p = GenParams { count, a: shape as f32, b: 3.0, ..Default::default() };
                for scale in [0.0, 0.7, 1.0, 3.0] {
                    let ctx = GenCtx { beat_pos: 13.7, ..GenCtx::at_time(0.0, 0.0, 0.0, scale) };
                    let geo = generate("chase_fan", &p, &ctx).unwrap();
                    assert_eq!(geo.strokes.len(), (count as usize).min(MAX_BEAMS));
                    let pts = densify(&colorize(&geo, ColorMode::Solid, (1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0, 1.0));
                    assert!(pts.len() <= crate::layers::DEFAULT_POINT_BUDGET, "{count} beams: {} points", pts.len());
                    assert!(pts.iter().all(|q| q.x.abs() <= 1.0 && (HORIZON..=1.0).contains(&q.y)));
                }
            }
        }
    }
}
