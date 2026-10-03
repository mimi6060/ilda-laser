//! Point optimiser (T-002): the "pro" way to turn a look's raw points into
//! what the galvos draw, as an opt-in alternative to `engine::densify`.
//!
//! Built from public galvo knowledge (constant beam speed, dwell at sharp
//! corners, blanked and eased travel, nearest-neighbour path order), not
//! from any vendor's code or settings:
//!
//! - the look is split into **paths**: runs of lit points. Blanked points
//!   only ever mean "travel" and are rebuilt;
//! - paths are drawn in a **stable** order: the look's own order, unless a
//!   nearest-neighbour order (open paths may be drawn backwards) saves more
//!   than `REORDER_GAIN` of the travel. The order chosen is remembered, so
//!   an animated look doesn't flip between orders from frame to frame;
//! - lit stretches between corners are resampled at even arc-length steps
//!   of at most `lit_step`, so the beam moves at one speed and the
//!   brightness stays even, however the look sampled its curves;
//! - a vertex is held for `ceil(turn / corner_rad_per_point)` extra points
//!   when the path turns by more than `corner_min_deg` (capped), so a
//!   square gets crisp corners and a circle none; path ends get
//!   `endpoint_dwell`;
//! - a look that would need more than `max_points` gets a longer lit step
//!   (at most 2x), so it never drops under 30 frames/s on its own;
//! - travel between paths is **dark**: the beam goes off where it stands,
//!   moves with a smoothstep ease (slow at both ends, `blank_step` at most
//!   in the middle) and lights up only once it has arrived.
//!
//! The jump from a frame's end back to its start, the colour delay and the
//! startup blank stay laser-dac's job (its frame session does them); doing
//! them here too would only waste points. The safety stage (zones, strobe
//! limiter, static beam guard, power caps, gate) runs after, on the
//! finished frame, as for `densify`.

use crate::patterns::Point;
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// The optimiser's settings, per output frame (saved with the layer mixer).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OptimizeParams {
    /// Off = the classic `densify` (what every look used before T-002).
    pub enabled: bool,
    /// Longest step between two lit points (normalised units, full range 2).
    pub lit_step: f32,
    /// Longest step of blanked travel (the middle of the eased move).
    pub blank_step: f32,
    /// One extra corner point per this many radians of turn.
    pub corner_rad_per_point: f32,
    /// Turns gentler than this (degrees) are curves: no dwell.
    pub corner_min_deg: f32,
    /// Most extra points on one corner.
    pub max_corner_dwell: usize,
    /// Extra points on the first and last point of every path.
    pub endpoint_dwell: usize,
    /// Draw paths nearest-first (stable, see the module docs). Off = the
    /// look's own order ("preserve order").
    pub reorder: bool,
    /// Most points one look may take: 1000 = 30 frames/s at 30 kpps, the
    /// flicker floor. A look that needs more gets a longer lit step (up to
    /// `MAX_STEP_STRETCH` times) rather than a slower frame rate. The
    /// layer mixer's point budget still applies to the whole frame after.
    pub max_points: usize,
}

impl Default for OptimizeParams {
    fn default() -> Self {
        Self {
            enabled: false,
            lit_step: 0.025,
            blank_step: 0.15,
            corner_rad_per_point: 0.6,
            corner_min_deg: 30.0,
            max_corner_dwell: 8,
            endpoint_dwell: 3,
            reorder: true,
            max_points: 1000,
        }
    }
}

impl OptimizeParams {
    /// Every value in a range the galvos and the point budget can live
    /// with; non-finite values fall back to the defaults.
    pub fn sanitize(&mut self) {
        let d = Self::default();
        let fin = |v: f32, def: f32| if v.is_finite() { v } else { def };
        self.lit_step = fin(self.lit_step, d.lit_step).clamp(0.005, 0.2);
        self.blank_step = fin(self.blank_step, d.blank_step).clamp(self.lit_step, 1.0);
        self.corner_rad_per_point = fin(self.corner_rad_per_point, d.corner_rad_per_point).clamp(0.1, std::f32::consts::PI);
        self.corner_min_deg = fin(self.corner_min_deg, d.corner_min_deg).clamp(0.0, 180.0);
        self.max_corner_dwell = self.max_corner_dwell.min(32);
        self.endpoint_dwell = self.endpoint_dwell.min(32);
        self.max_points = self.max_points.clamp(crate::layers::MIN_POINT_BUDGET, crate::layers::MAX_POINT_BUDGET);
    }
}

/// A new order must cut the travel by more than this fraction to replace
/// the current one (hysteresis against frame-to-frame flips).
const REORDER_GAIN: f32 = 0.2;

/// How far `max_points` may stretch the lit step.
const MAX_STEP_STRETCH: f32 = 2.0;

/// Most points of one blanked travel (whatever the settings say).
const MAX_TRAVEL_STEPS: usize = 600;

/// Most points between two anchors (a guard; lit_step ≥ 0.005 keeps
/// a full-range diagonal under 600).
const MAX_RESAMPLE: usize = 4096;

/// Points closer than this are the same point (a hold, not a segment).
const SAME: f32 = 1e-6;

/// The optimiser of one look: its settings plus the path order it drew
/// last frame. One per animator, so every look keeps its own order.
#[derive(Clone, Debug, Default)]
pub struct Optimizer {
    params: OptimizeParams,
    /// Last frame's order: (path index, drawn backwards).
    order: Vec<(usize, bool)>,
}

impl Optimizer {
    /// The settings are sanitised here: whatever reaches the optimiser
    /// (an API client, a saved file, a fuzzer) can't make it loop.
    pub fn new(params: OptimizeParams) -> Self {
        let mut o = Self { params: OptimizeParams::default(), order: Vec::new() };
        o.set_params(&params);
        o
    }

    pub fn set_params(&mut self, params: &OptimizeParams) {
        self.params = params.clone();
        self.params.sanitize();
    }

    /// One look's raw points → the points the galvos draw. A look with
    /// nothing lit (a closed gate, a dark strobe slot) keeps the classic
    /// `densify` path, so its frame stays exactly what it was.
    pub fn optimize(&mut self, points: &[Point]) -> Vec<Point> {
        let paths = lit_runs(points);
        if paths.is_empty() {
            return crate::engine::densify(points);
        }
        let p = &self.params;
        let order = choose_order(points, &paths, &self.order, p.reorder);
        let mut step = p.lit_step;
        let mut out = draw(points, &paths, &order, p, step);
        // Over the frame cap: stretch the lit step by what is missing (a
        // few tries, as dwell and travel don't shrink with it).
        let max_step = p.lit_step * MAX_STEP_STRETCH;
        while out.len() > p.max_points && step < max_step {
            step = (step * out.len() as f32 / p.max_points as f32).max(step * 1.05).min(max_step);
            out = draw(points, &paths, &order, p, step);
        }
        self.order = order;
        out
    }
}

fn draw(points: &[Point], paths: &[Range<usize>], order: &[(usize, bool)], p: &OptimizeParams, step: f32) -> Vec<Point> {
    let mut out = Vec::with_capacity(points.len() * 2);
    for &(i, rev) in order {
        let path = &points[paths[i].clone()];
        let (first, _) = ends(path, rev);
        if let Some(&last) = out.last() {
            travel(&mut out, last, first, p.blank_step);
        }
        draw_path(&mut out, path, rev, p, step);
    }
    out
}

/// Blanked travel from `from` to `to` (both blanked, then eased steps).
/// Also what joins two looks of a frame (`engine::join_looks`).
pub fn travel(out: &mut Vec<Point>, from: Point, to: Point, blank_step: f32) {
    // The beam goes dark where it stands...
    out.push(Point::blanked(from.x, from.y));
    // ...moves with smoothstep (peak speed 1.5x the mean, so 1.5x the
    // steps keeps the middle step within `blank_step`)...
    let d = dist(from, to);
    let step = if blank_step.is_finite() { blank_step.max(0.005) } else { OptimizeParams::default().blank_step };
    // Capped: a full-range jump needs ~30 steps at the default.
    let n = ((1.5 * d / step).ceil() as usize).clamp(1, MAX_TRAVEL_STEPS);
    for k in 1..=n {
        let u = k as f32 / n as f32;
        let e = u * u * (3.0 - 2.0 * u);
        out.push(Point::blanked(from.x + (to.x - from.x) * e, from.y + (to.y - from.y) * e));
    }
    // ...and arrives dark (k = n is `to` itself): the path lights up after.
}

/// Index ranges of the lit runs of `points`.
fn lit_runs(points: &[Point]) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut start = None;
    for (i, p) in points.iter().enumerate() {
        match (p.is_lit(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push(s..points.len());
    }
    runs
}

fn dist(a: Point, b: Point) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

/// Galvo travel cost: the axes move independently, so the longer axis sets
/// the time (L∞).
fn travel_cost(a: Point, b: Point) -> f32 {
    (b.x - a.x).abs().max((b.y - a.y).abs())
}

/// (first, last) point of a path as drawn.
fn ends(path: &[Point], rev: bool) -> (Point, Point) {
    let (a, b) = (path[0], path[path.len() - 1]);
    if rev { (b, a) } else { (a, b) }
}

/// Closed outlines repeat their first point: reversing them changes nothing.
fn is_closed(path: &[Point]) -> bool {
    dist(path[0], path[path.len() - 1]) < SAME
}

fn order_cost(points: &[Point], paths: &[Range<usize>], order: &[(usize, bool)]) -> f32 {
    order
        .windows(2)
        .map(|w| {
            let (_, a) = ends(&points[paths[w[0].0].clone()], w[0].1);
            let (b, _) = ends(&points[paths[w[1].0].clone()], w[1].1);
            travel_cost(a, b)
        })
        .sum()
}

/// The order to draw `paths` in: last frame's (or the look's own) unless
/// the greedy nearest-neighbour order is clearly shorter. Deterministic:
/// the same points and memory always give the same order.
fn choose_order(points: &[Point], paths: &[Range<usize>], previous: &[(usize, bool)], reorder: bool) -> Vec<(usize, bool)> {
    let natural: Vec<(usize, bool)> = (0..paths.len()).map(|i| (i, false)).collect();
    if !reorder || paths.len() < 2 {
        return natural;
    }
    // Last frame's order still fits if it names the same number of paths.
    let current = if previous.len() == paths.len() { previous.to_vec() } else { natural };
    let greedy = nearest_neighbour(points, paths);
    if order_cost(points, paths, &greedy) < (1.0 - REORDER_GAIN) * order_cost(points, paths, &current) {
        greedy
    } else {
        current
    }
}

/// Greedy nearest neighbour from the look's first path (kept first, so the
/// frame starts where the look does). Open paths may be drawn backwards.
/// Ties go to the lowest index, forwards.
fn nearest_neighbour(points: &[Point], paths: &[Range<usize>]) -> Vec<(usize, bool)> {
    let mut left: Vec<usize> = (1..paths.len()).collect();
    let mut order = vec![(0, false)];
    let mut at = points[paths[0].end - 1];
    while !left.is_empty() {
        let mut best = (f32::INFINITY, 0usize, false);
        for (slot, &i) in left.iter().enumerate() {
            let path = &points[paths[i].clone()];
            let fwd = travel_cost(at, path[0]);
            if fwd < best.0 {
                best = (fwd, slot, false);
            }
            if !is_closed(path) {
                let back = travel_cost(at, path[path.len() - 1]);
                if back < best.0 {
                    best = (back, slot, true);
                }
            }
        }
        let i = left.remove(best.1);
        order.push((i, best.2));
        at = ends(&points[paths[i].clone()], best.2).1;
    }
    order
}

/// Extra points for a turn of `angle` radians.
fn corner_dwell(angle: f32, p: &OptimizeParams) -> usize {
    if angle.to_degrees() <= p.corner_min_deg {
        return 0;
    }
    ((angle / p.corner_rad_per_point).ceil() as usize).min(p.max_corner_dwell)
}

/// Turn at `b` going a → b → c, in radians (0 = straight on).
fn turn(a: Point, b: Point, c: Point) -> f32 {
    let (d1x, d1y, d2x, d2y) = (b.x - a.x, b.y - a.y, c.x - b.x, c.y - b.y);
    let (l1, l2) = ((d1x * d1x + d1y * d1y).sqrt(), (d2x * d2x + d2y * d2y).sqrt());
    if l1 < SAME || l2 < SAME {
        return 0.0;
    }
    ((d1x * d2x + d1y * d2y) / (l1 * l2)).clamp(-1.0, 1.0).acos()
}

/// One lit path. Its vertices (a repeat is a hold) are split into
/// **anchors** - the ends, corners and held vertices - and the smooth
/// stretches between them, which are resampled at even arc-length steps of
/// at most `step`: the beam keeps one speed along curves however densely
/// (or coarsely) the look sampled them. Anchors are drawn exactly, held for
/// their dwell (or the look's own hold when longer).
fn draw_path(out: &mut Vec<Point>, path: &[Point], rev: bool, p: &OptimizeParams, step: f32) {
    // (vertex, how many times the look repeats it), in drawing order.
    let mut verts: Vec<(Point, usize)> = Vec::with_capacity(path.len());
    let mut push = |q: Point| match verts.last_mut() {
        Some((v, n)) if dist(*v, q) < SAME => *n += 1,
        _ => verts.push((q, 1)),
    };
    if rev {
        path.iter().rev().for_each(|&q| push(q));
    } else {
        path.iter().for_each(|&q| push(q));
    }
    let last = verts.len() - 1;
    let extra = |i: usize| {
        if i == 0 || i == last { p.endpoint_dwell } else { corner_dwell(turn(verts[i - 1].0, verts[i].0, verts[i + 1].0), p) }
    };
    let hold = |out: &mut Vec<Point>, i: usize, extra: usize| {
        let (v, n) = verts[i];
        out.extend(std::iter::repeat_n(v, n.max(1 + extra)));
    };
    hold(out, 0, extra(0));
    let mut from = 0;
    for i in 1..=last {
        let e = extra(i);
        if i < last && e == 0 && verts[i].1 == 1 {
            continue; // a smooth vertex: resampled through
        }
        resample(out, &verts[from..=i], step);
        hold(out, i, e);
        from = i;
    }
}

/// The points strictly between the first and last of `verts`, evenly
/// spaced along the polyline at most `step` apart. A point takes the
/// colour of the vertex that ends its segment.
fn resample(out: &mut Vec<Point>, verts: &[(Point, usize)], step: f32) {
    let length: f32 = verts.windows(2).map(|w| dist(w[0].0, w[1].0)).sum();
    if !length.is_finite() {
        return;
    }
    let n = ((length / step).ceil() as usize).min(MAX_RESAMPLE);
    let mut seg = 1;
    let mut seg_start = 0.0; // arc length at verts[seg - 1]
    for k in 1..n {
        let s = length * k as f32 / n as f32;
        while seg < verts.len() - 1 && seg_start + dist(verts[seg - 1].0, verts[seg].0) < s {
            seg_start += dist(verts[seg - 1].0, verts[seg].0);
            seg += 1;
        }
        let (a, b) = (verts[seg - 1].0, verts[seg].0);
        let d = dist(a, b);
        let t = if d > 0.0 { ((s - seg_start) / d).clamp(0.0, 1.0) } else { 1.0 };
        out.push(Point { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, ..b });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patterns;

    fn on() -> OptimizeParams {
        OptimizeParams { enabled: true, ..Default::default() }
    }

    fn opt(points: &[Point]) -> Vec<Point> {
        Optimizer::new(on()).optimize(points)
    }

    fn step(a: &Point, b: &Point) -> f32 {
        dist(*a, *b)
    }

    fn line(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Point> {
        vec![Point::lit(x0, y0, 1.0, 0.0, 0.0), Point::lit(x1, y1, 1.0, 0.0, 0.0)]
    }

    /// Strokes joined the way generators join them (two blanked points).
    fn strokes(list: &[Vec<Point>]) -> Vec<Point> {
        let mut out: Vec<Point> = Vec::new();
        for s in list {
            if let Some(&last) = out.last() {
                out.push(Point::blanked(last.x, last.y));
                out.push(Point::blanked(s[0].x, s[0].y));
            }
            out.extend(s);
        }
        out
    }

    /// Runs of identical consecutive points: (point, length).
    fn runs(points: &[Point]) -> Vec<(Point, usize)> {
        let mut r: Vec<(Point, usize)> = Vec::new();
        for &q in points {
            match r.last_mut() {
                Some((p, n)) if *p == q => *n += 1,
                _ => r.push((q, 1)),
            }
        }
        r
    }

    #[test]
    fn lit_steps_never_exceed_lit_step() {
        let star = patterns::by_name("star", 0.9, 1.0, 1.0, 1.0).unwrap();
        for pts in [patterns::square(0.9, 1.0, 1.0, 1.0), star, strokes(&[line(-1.0, -1.0, 1.0, 1.0), line(1.0, -1.0, -1.0, 1.0)])] {
            let out = opt(&pts);
            for w in out.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()) {
                assert!(step(&w[0], &w[1]) <= on().lit_step + 1e-5, "{w:?}");
            }
        }
    }

    #[test]
    fn travel_is_dark_eased_and_within_blank_step() {
        let pts = strokes(&[line(-0.9, 0.0, -0.8, 0.0), line(0.8, 0.0, 0.9, 0.0)]);
        let out = opt(&pts);
        // Never a lit point that moves more than a lit step: jumps are dark.
        for w in out.windows(2) {
            if step(&w[0], &w[1]) > on().lit_step + 1e-5 {
                assert!(!w[0].is_lit() && !w[1].is_lit(), "lit jump {w:?}");
            }
        }
        let dark: Vec<&Point> = out.iter().filter(|p| !p.is_lit()).collect();
        let steps: Vec<f32> = dark.windows(2).map(|w| step(w[0], w[1])).collect();
        assert!(steps.len() >= 4);
        let mid = steps[steps.len() / 2];
        assert!(steps[0] < mid && steps[steps.len() - 1] < mid, "{steps:?}");
        assert!(steps.iter().all(|&s| s <= on().blank_step + 1e-5), "{steps:?}");
        // The beam goes dark where it stands and lights up where it lands.
        assert_eq!((dark[0].x, dark[0].y), (-0.8, 0.0));
        assert_eq!((dark[dark.len() - 1].x, dark[dark.len() - 1].y), (0.8, 0.0));
        // Fewer points than densify's even 0.03 steps over the same jump.
        assert!(dark.len() < (1.6 / 0.03) as usize / 2);
    }

    #[test]
    fn square_corners_hold_three_points_and_a_circle_none() {
        let out = opt(&patterns::square(0.5, 1.0, 1.0, 1.0));
        let r = runs(&out);
        // Interior corners: 1 + 3 points; the two ends: 1 + endpoint_dwell.
        let held: Vec<usize> = r.iter().map(|x| x.1).filter(|&n| n > 1).collect();
        assert_eq!(held, vec![4, 4, 4, 4, 4], "{r:?}");
        let corner = |x: f32, y: f32| out.iter().filter(|p| (p.x - x).abs() < 1e-6 && (p.y - y).abs() < 1e-6).count();
        assert_eq!(corner(0.5, -0.5), 4);
        assert_eq!(corner(0.5, 0.5), 4);
        assert_eq!(corner(-0.5, 0.5), 4);

        let circle = opt(&patterns::circle(0.5, 1.0, 1.0, 1.0));
        let r = runs(&circle);
        assert!(r[1..r.len() - 1].iter().all(|x| x.1 == 1), "a circle has no corners");
    }

    #[test]
    fn curves_are_resampled_at_even_steps_whatever_their_sampling() {
        // A circle sampled far too densely (2000 points) and one sampled
        // coarsely (16 points, 22.5° turns: still a curve) both come out
        // as about circumference / lit_step evenly spaced points.
        let ring = |n: usize| -> Vec<Point> {
            (0..=n).map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Point::lit(0.5 * a.cos(), 0.5 * a.sin(), 1.0, 1.0, 1.0)
            }).collect()
        };
        let p = on();
        for n in [2000, 16] {
            let out = opt(&ring(n));
            let lit = out.len() - 2 * p.endpoint_dwell;
            let ideal = (n as f32 * 2.0 * 0.5 * (std::f32::consts::PI / n as f32).sin() / p.lit_step).ceil() as usize + 1;
            assert!(lit.abs_diff(ideal) <= 1, "{n} samples: {lit} points, {ideal} expected");
            let steps: Vec<f32> = out.windows(2).map(|w| step(&w[0], &w[1])).filter(|&d| d > 0.0).collect();
            let (lo, hi) = steps.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &d| (lo.min(d), hi.max(d)));
            assert!(hi <= p.lit_step + 1e-5 && lo > 0.8 * hi, "{n} samples: steps {lo}..{hi}");
        }
    }

    #[test]
    fn a_look_over_the_frame_cap_gets_a_longer_lit_step() {
        // 15 long diagonals: about 1500 points at the normal lit step.
        let many: Vec<Vec<Point>> = (0..15).map(|i| {
            let y = -0.9 + i as f32 * 0.12;
            line(-1.0, y, 1.0, -y)
        }).collect();
        let pts = strokes(&many);
        let free = Optimizer::new(OptimizeParams { max_points: 10_000, ..on() }).optimize(&pts);
        assert!(free.len() > 1300, "{}", free.len());
        let capped = opt(&pts);
        assert!(capped.len() <= on().max_points, "{}", capped.len());
        let longest = capped.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()).map(|w| step(&w[0], &w[1])).fold(0.0, f32::max);
        assert!(longest > on().lit_step && longest <= 2.0 * on().lit_step + 1e-5, "{longest}");
    }

    #[test]
    fn corner_dwell_grows_with_the_angle_and_is_capped() {
        let p = on();
        assert_eq!(corner_dwell(20f32.to_radians(), &p), 0);
        assert_eq!(corner_dwell(45f32.to_radians(), &p), 2);
        assert_eq!(corner_dwell(90f32.to_radians(), &p), 3);
        assert_eq!(corner_dwell(180f32.to_radians(), &p), 6);
        assert_eq!(corner_dwell(180f32.to_radians(), &OptimizeParams { max_corner_dwell: 4, ..on() }), 4);
    }

    #[test]
    fn beams_keep_their_own_hold() {
        let beam = vec![Point::lit(0.2, 0.3, 0.0, 1.0, 0.0); 12];
        let out = opt(&beam);
        assert_eq!(out, beam);
        assert_eq!(opt(&beam[..1]).len(), 1 + on().endpoint_dwell);
    }

    #[test]
    fn paths_are_drawn_nearest_first_and_open_ones_may_turn_round() {
        // Left, right, middle: the middle stroke is nearest to the left
        // one, then the right one is best drawn from its inner end.
        let a = line(-0.9, 0.0, -0.6, 0.0);
        let b = line(0.9, 0.0, 0.6, 0.0);
        let c = line(-0.5, 0.0, 0.5, 0.0);
        let pts = strokes(&[a, b, c]);
        let mut o = Optimizer::new(on());
        let out = o.optimize(&pts);
        assert_eq!(o.order, vec![(0, false), (2, false), (1, true)]);
        let lit_x: Vec<f32> = out.iter().filter(|p| p.is_lit()).map(|p| p.x).collect();
        assert!(lit_x.windows(2).all(|w| w[1] >= w[0] - 1e-6), "left to right");
        // Preserve order: the look's own order.
        let mut keep = Optimizer::new(OptimizeParams { reorder: false, ..on() });
        keep.optimize(&pts);
        assert_eq!(keep.order, vec![(0, false), (1, false), (2, false)]);
    }

    #[test]
    fn the_order_is_deterministic_and_sticks_unless_clearly_better() {
        let pts = strokes(&[line(-0.9, 0.0, -0.6, 0.0), line(0.9, 0.0, 0.6, 0.0), line(-0.5, 0.0, 0.5, 0.0)]);
        assert_eq!(opt(&pts), opt(&pts));
        let mut o = Optimizer::new(on());
        let first = o.optimize(&pts);
        assert_eq!(o.optimize(&pts), first, "same frame, same order");
        // A small improvement doesn't reshuffle: last frame's order stays.
        // Closed loops (never reversed) from the origin: (0,1,2) travels
        // 1.05, (0,2,1) 1.1 - less than 20 % apart.
        let lp = |x: f32, y: f32| vec![Point::lit(x, y, 1.0, 1.0, 1.0), Point::lit(x + 0.05, y, 1.0, 1.0, 1.0), Point::lit(x, y + 0.05, 1.0, 1.0, 1.0), Point::lit(x, y, 1.0, 1.0, 1.0)];
        let near = strokes(&[line(-0.2, 0.0, 0.0, 0.0), lp(0.5, 0.0), lp(0.0, 0.55)]);
        assert_eq!(nearest_neighbour(&near, &lit_runs(&near)), vec![(0, false), (1, false), (2, false)]);
        let mut o = Optimizer::new(on());
        o.order = vec![(0, false), (2, false), (1, false)];
        o.optimize(&near);
        assert_eq!(o.order, vec![(0, false), (2, false), (1, false)]);
        // A fresh optimiser keeps the look's own order for the same reason.
        let mut fresh = Optimizer::new(on());
        fresh.optimize(&near);
        assert_eq!(fresh.order, vec![(0, false), (1, false), (2, false)]);
    }

    #[test]
    fn a_dark_look_keeps_the_classic_path() {
        let dark: Vec<Point> = patterns::square(0.5, 0.0, 0.0, 0.0);
        assert_eq!(opt(&dark), crate::engine::densify(&dark));
        assert!(opt(&[]).is_empty());
    }

    #[test]
    fn output_stays_in_range_and_lit_points_are_the_looks_colours() {
        let pts = strokes(&[patterns::square(1.0, 1.0, 0.0, 0.0), patterns::circle(1.0, 0.0, 1.0, 0.0), line(-1.0, 1.0, 1.0, -1.0)]);
        let out = opt(&pts);
        assert!(out.iter().all(|p| (-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y)));
        let colours = |v: &[Point]| v.iter().filter(|p| p.is_lit()).map(|p| (p.r.to_bits(), p.g.to_bits(), p.b.to_bits())).collect::<std::collections::HashSet<_>>().len();
        assert_eq!(colours(&out), colours(&pts));
    }

    #[test]
    fn unsanitised_settings_cannot_hang_or_flood_the_frame() {
        let pts = strokes(&[patterns::square(0.8, 1.0, 1.0, 1.0), line(-1.0, 1.0, 1.0, -1.0)]);
        for (lit_step, blank_step) in [(0.0, 0.0), (-1.0, -1.0), (f32::NAN, f32::INFINITY), (1e-9, 1e-9)] {
            let p = OptimizeParams { lit_step, blank_step, corner_rad_per_point: 0.0, max_points: 0, ..on() };
            let out = Optimizer::new(p.clone()).optimize(&pts);
            assert!(!out.is_empty() && out.len() < 5000, "{lit_step}/{blank_step}: {}", out.len());
            let mut joined = Vec::new();
            travel(&mut joined, pts[0], Point::lit(-1.0, -1.0, 1.0, 1.0, 1.0), blank_step);
            assert!(joined.len() <= MAX_TRAVEL_STEPS + 1);
        }
    }

    #[test]
    fn params_load_with_defaults_and_sanitize() {
        let p: OptimizeParams = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(p, on());
        let mut bad = OptimizeParams { lit_step: f32::NAN, blank_step: 0.0, corner_rad_per_point: -1.0, max_corner_dwell: 999, max_points: 0, ..on() };
        bad.sanitize();
        assert_eq!(bad.max_points, crate::layers::MIN_POINT_BUDGET);
        assert_eq!(bad.lit_step, 0.025);
        assert_eq!(bad.blank_step, bad.lit_step);
        assert_eq!(bad.corner_rad_per_point, 0.1);
        assert_eq!(bad.max_corner_dwell, 32);
    }
}
