//! Fixed frame rate for video capture (T-463): every frame the output
//! plays lasts exactly the same time, matched to the camera's, so a
//! filmed laser shows no rolling bands nor half-drawn frames.
//!
//! At a fixed rate of `fps`, a frame is `N = round(pps / fps)` points
//! (`target_points`), the output's own seam between two frames included
//! (`Output::fixed_seam`: what the DAC library inserts at a frame change,
//! made constant while the mode is on). The engine shapes each frame in
//! two steps around the safety stage:
//!
//! - **Before** it (`shape`): exactly `N - reserve` points. A frame that is
//!   too long is resampled along its lit path (corners, run ends and, when
//!   they fit, every dark point kept); one that is too short gets extra
//!   points spread evenly along its lit segments - never on a spot that
//!   doesn't move - and dark points for the rest. The lit points added are
//!   bounded so that neither the frame's mean brightness nor the most
//!   exposed cell of the static-beam guard's grid (T-256) get more than in
//!   the frame as it came: completing a frame never concentrates light.
//!   The reserve leaves room for the samples the zone mask inserts.
//! - **After** it (`top_up`): dark points only, held where the frame
//!   ends, up to `N`. Nothing is removed or lit again: the safety stage
//!   keeps the last word on what is lit. A frame still above `N` goes out
//!   as it is and is counted (`FixedRateStatus::overflow_frames`).
//!
//! Off (the default), nothing here runs and frames are what they were.
//! The rate is an output setting (`power::OutputScan::fixed_fps`), not a
//! control: no MIDI, DMX or timeline reaches it, and it is refused while
//! armed (web.rs). It changes nothing in arming, caps or zones.

use crate::patterns::Point;
use serde::Serialize;
use std::collections::VecDeque;

/// Free values the operator may type, frames per second.
pub const FIXED_FPS_RANGE: (f32, f32) = (20.0, 60.0);
/// The rates the menu offers (cinema, PAL, NTSC, and their doubles).
pub const FIXED_FPS_CHOICES: [f32; 5] = [24.0, 25.0, 30.0, 50.0, 60.0];
/// A lit point turning more than this is a corner, kept by `shape`.
const CORNER_DEG: f32 = 30.0;
/// How close two points must be to count as one position.
const SAME: f32 = 1e-6;
/// How far back and forward `is_corner` looks for a point that moved.
const CORNER_LOOK: usize = 64;
/// Dark travel's share of points per unit length against the lit path
/// when a frame is reduced (`optimize`'s default steps: 0.025 lit, 0.15
/// dark).
const DARK_SHARE: f32 = 1.0 / 6.0;
/// The measuring window of `FixedRateState`, seconds.
const MEASURE_S: f64 = 10.0;

/// The fixed rate of one output this frame (see `power::OutputScan::fixed_target`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedTarget {
    /// The rate asked for, frames per second (within `FIXED_FPS_RANGE`).
    pub fps: f32,
    /// The output's rate, points per second.
    pub pps: u32,
    /// Points of one whole frame, seam included (`target_points`).
    pub points: usize,
    /// Longest lit step the output allows (the optimiser's lit step,
    /// stretched as far as it may be, at the fastest scan the galvos
    /// take): a reduction that needs more warns.
    pub max_step: f32,
}

/// What `shape` did to one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct ShapeReport {
    /// Points in, points out (`N - reserve`).
    pub input: usize,
    pub output: usize,
    /// Too long: resampled down.
    pub reduced: bool,
    /// Too short: points added (lit and dark).
    pub padded: usize,
    /// The reduction needed a longer lit step than the output allows: the
    /// content does not fit the rate (the UI warns, with `output` as the
    /// point budget to aim for).
    pub over_step: bool,
}

/// The fixed rate as `/api/state` shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct FixedRateStatus {
    pub target_fps: f32,
    /// Mean over the last 10 s of `pps / points sent` (seam included).
    pub measured_fps: f32,
    /// Root mean square of the frames' deviation from the target, %.
    pub deviation_pct: f32,
    /// Frames that left the safety stage above `N` points (sent as they were).
    pub overflow_frames: u64,
    /// Points of one frame, seam included.
    pub points: usize,
    /// The last frame's shaping.
    pub report: ShapeReport,
}

/// The rate within `FIXED_FPS_RANGE`; not a number = off.
pub fn sanitize_fps(fps: f32) -> Option<f32> {
    fps.is_finite().then(|| fps.clamp(FIXED_FPS_RANGE.0, FIXED_FPS_RANGE.1))
}

/// Points of one frame at `fps` for an output at `pps`: `round(pps / fps)`,
/// at least 2. A rate that is not a number gives 0 (no fixed rate).
pub fn target_points(pps: u32, fps: f32) -> usize {
    match sanitize_fps(fps) {
        Some(fps) => ((pps as f64 / fps as f64).round() as usize).max(2),
        None => 0,
    }
}

fn brightness(p: &Point) -> f32 {
    p.r.max(p.g).max(p.b).clamp(0.0, 1.0)
}

fn dist(a: &Point, b: &Point) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

fn same(a: &Point, b: &Point) -> bool {
    (a.x - b.x).abs() <= SAME && (a.y - b.y).abs() <= SAME
}

fn lerp(a: &Point, b: &Point, t: f32) -> Point {
    let m = |u: f32, v: f32| u + (v - u) * t;
    Point { x: m(a.x, b.x), y: m(a.y, b.y), r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

/// A point within the field: a position that is not a number goes dark at
/// the centre, a colour that is not a number goes dark.
fn clean(p: &Point) -> Point {
    if !p.x.is_finite() || !p.y.is_finite() {
        return Point::blanked(0.0, 0.0);
    }
    let c = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    Point { x: p.x.clamp(-1.0, 1.0), y: p.y.clamp(-1.0, 1.0), r: c(p.r), g: c(p.g), b: c(p.b) }
}

/// Before the safety stage: exactly `n - reserve` points (at least 2),
/// see the module docs. `max_step` is the output's longest lit step,
/// `grid` the static-beam guard's cells per side.
pub fn shape(frame: &[Point], n: usize, reserve: usize, max_step: f32, grid: usize) -> (Vec<Point>, ShapeReport) {
    let m = n.saturating_sub(reserve).max(2);
    let f: Vec<Point> = frame.iter().map(clean).collect();
    let mut rep = ShapeReport { input: f.len(), output: m, ..Default::default() };
    let (out, squeezed) = if f.len() > m {
        rep.reduced = true;
        reduce(&f, m)
    } else {
        rep.padded = m - f.len();
        (pad(&f, m, grid.clamp(1, 256)), false)
    };
    rep.over_step = squeezed || (rep.reduced && longest_lit_step(&out) > longest_lit_step(&f).max(max_step) * 1.001);
    debug_assert_eq!(out.len(), m);
    (out, rep)
}

/// After the safety stage: dark points held where the frame ends, up to
/// `n`. Never removes a point, never lights one.
pub fn top_up(mut frame: Vec<Point>, n: usize) -> Vec<Point> {
    if frame.len() < n {
        let last = frame.last().map_or(Point::blanked(0.0, 0.0), |p| Point::blanked(p.x, p.y));
        frame.resize(n, last);
    }
    frame
}

/// Longest step between two consecutive lit points.
fn longest_lit_step(f: &[Point]) -> f32 {
    f.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()).map(|w| dist(&w[0], &w[1])).fold(0.0, f32::max)
}

/// A lit point where the path turns by more than `CORNER_DEG`, measured
/// between the nearest points before and after it that are elsewhere.
fn is_corner(f: &[Point], i: usize) -> bool {
    let p = f[i];
    let before = f[i.saturating_sub(CORNER_LOOK)..i].iter().rev().find(|q| !same(q, &p));
    let after = f[i + 1..(i + 1 + CORNER_LOOK).min(f.len())].iter().find(|q| !same(q, &p));
    let (Some(a), Some(b)) = (before, after) else {
        return false;
    };
    let (ux, uy, vx, vy) = (p.x - a.x, p.y - a.y, b.x - p.x, b.y - p.y);
    let cos = (ux * vx + uy * vy) / ((ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt());
    cos < CORNER_DEG.to_radians().cos()
}

/// Splits `total` into whole parts proportional to `weights` (largest
/// remainder): they always add up to `total` when some weight is positive.
fn apportion(total: usize, weights: &[f32]) -> Vec<usize> {
    let sum: f64 = weights.iter().map(|&w| w.max(0.0) as f64).sum();
    if sum <= 0.0 || weights.is_empty() {
        return vec![0; weights.len()];
    }
    let quotas: Vec<f64> = weights.iter().map(|&w| total as f64 * w.max(0.0) as f64 / sum).collect();
    let mut parts: Vec<usize> = quotas.iter().map(|q| q.floor() as usize).collect();
    let mut left = total.saturating_sub(parts.iter().sum());
    let mut order: Vec<usize> = (0..weights.len()).filter(|&i| weights[i] > 0.0).collect();
    order.sort_by(|&a, &b| (quotas[b] - quotas[b].floor()).total_cmp(&(quotas[a] - quotas[a].floor())).then(a.cmp(&b)));
    for &i in order.iter().cycle() {
        if left == 0 {
            break;
        }
        parts[i] += 1;
        left -= 1;
    }
    parts
}

/// Too long: down to `m` points. Kept as they are: the first and last
/// points, both sides of every lit/dark change, the corners, and, when
/// they fit, every dark point and the repeats on a kept spot (dwell).
/// Between two kept points the path is resampled evenly along its length.
/// If even the corners don't fit, evenly spaced points (`fit_frame`), and
/// `true`: the content does not fit at all.
fn reduce(f: &[Point], m: usize) -> (Vec<Point>, bool) {
    let len = f.len();
    let mut a = vec![false; len];
    a[0] = true;
    a[len - 1] = true;
    for i in 0..len {
        if i > 0 && f[i].is_lit() != f[i - 1].is_lit() {
            a[i] = true;
            a[i - 1] = true;
        }
        if f[i].is_lit() && is_corner(f, i) {
            a[i] = true;
        }
    }
    let mut b = a.clone();
    for (k, p) in b.iter_mut().zip(f) {
        *k |= !p.is_lit();
    }
    for i in 1..len {
        if b[i - 1] && same(&f[i - 1], &f[i]) {
            b[i] = true;
        }
    }
    for i in (0..len - 1).rev() {
        if b[i + 1] && same(&f[i], &f[i + 1]) {
            b[i] = true;
        }
    }
    let count = |k: &[bool]| k.iter().filter(|&&x| x).count();
    let kept = if count(&b) <= m {
        b
    } else if count(&a) <= m {
        a
    } else {
        return (crate::output::fit_frame(f, m).into_owned(), true);
    };
    (resample(f, &kept, m), false)
}

/// `m` points: the kept ones, and between each two of them points evenly
/// spaced along the original path, as many as its length's share.
fn resample(f: &[Point], kept: &[bool], m: usize) -> Vec<Point> {
    let idx: Vec<usize> = (0..f.len()).filter(|&i| kept[i]).collect();
    let gaps: Vec<(usize, usize)> = idx.windows(2).map(|w| (w[0], w[1])).collect();
    let lengths: Vec<f32> = gaps.iter().map(|&(i, j)| f[i..=j].windows(2).map(|w| dist(&w[0], &w[1])).sum()).collect();
    // The points go to the lit path; a dark travel keeps a few (as the
    // optimiser draws it, about six times sparser), and two kept points
    // next to each other with a lit/dark change or dark on both sides get
    // none (nothing to draw between them).
    let weight = |g: usize, len: f32| {
        let (i, j) = gaps[g];
        if f[i].is_lit() && f[j].is_lit() {
            len
        } else if !f[i].is_lit() && !f[j].is_lit() && j > i + 1 {
            len * DARK_SHARE
        } else {
            0.0
        }
    };
    let weights: Vec<f32> = lengths.iter().enumerate().map(|(g, &len)| weight(g, len)).collect();
    let weights: Vec<f32> = if weights.iter().sum::<f32>() > SAME { weights } else { gaps.iter().map(|&(i, j)| (j - i - 1) as f32).collect() };
    let parts = apportion(m - idx.len(), &weights);
    let mut out = Vec::with_capacity(m);
    out.push(f[idx[0]]);
    for (g, &(i, j)) in gaps.iter().enumerate() {
        let k = parts[g];
        let path = &f[i..=j];
        let len = lengths[g];
        if len <= SAME {
            // A path that doesn't move (only when nothing moves at all):
            // its points again, as many as it gets.
            out.extend((0..k).map(|q| path[1 + q % (path.len() - 2).max(1)]));
        } else {
            let (mut seg, mut walked) = (0, 0.0f32);
            for q in 1..=k {
                let s = len * q as f32 / (k + 1) as f32;
                while seg + 2 < path.len() && walked + dist(&path[seg], &path[seg + 1]) < s {
                    walked += dist(&path[seg], &path[seg + 1]);
                    seg += 1;
                }
                let d = dist(&path[seg], &path[seg + 1]);
                let t = if d > 0.0 { ((s - walked) / d).clamp(0.0, 1.0) } else { 1.0 };
                let (a, b) = (&path[seg], &path[seg + 1]);
                // Lit only between two lit points: a lit/dark edge is never lit along.
                let p = lerp(a, b, t);
                out.push(if a.is_lit() && b.is_lit() { p } else { Point::blanked(p.x, p.y) });
            }
        }
        out.push(f[j]);
    }
    out
}

/// Too short: up to `m` points. Lit points are added inside the lit
/// segments that move, evenly spaced, as many as each one's length share,
/// and only as many as keep both the frame's mean brightness and its
/// brightest guard cell at most what they were; the rest are dark points
/// held where the frame ends. The frame's own points all stay, in order.
fn pad(f: &[Point], m: usize, grid: usize) -> Vec<Point> {
    let Some(last) = f.last() else {
        return vec![Point::blanked(0.0, 0.0); m];
    };
    let extra = m - f.len();
    let segs: Vec<(usize, f32)> = f.windows(2).enumerate().filter(|(_, w)| w[0].is_lit() && w[1].is_lit()).map(|(i, w)| (i, dist(&w[0], &w[1]))).filter(|&(_, d)| d > SAME).collect();
    let weights: Vec<f32> = segs.iter().map(|s| s.1).collect();
    let cell = |p: &Point| {
        let idx = |v: f32| (((v.clamp(-1.0, 1.0) + 1.0) * 0.5 * grid as f32) as usize).min(grid - 1);
        idx(p.y) * grid + idx(p.x)
    };
    let mut base = vec![0.0f32; grid * grid];
    for p in f {
        base[cell(p)] += brightness(p);
    }
    let len = f.len() as f32;
    let max_cell = base.iter().fold(0.0f32, |a, &b| a.max(b)) / len;
    let mean = base.iter().sum::<f32>() / len;
    let inserted = |parts: &[usize], mut each: Box<dyn FnMut(Point) + '_>| {
        for (s, &k) in segs.iter().zip(parts) {
            for q in 1..=k {
                each(lerp(&f[s.0], &f[s.0 + 1], q as f32 / (k + 1) as f32));
            }
        }
    };
    let fits = |lit: usize| {
        let parts = apportion(lit, &weights);
        let mut cells = base.clone();
        inserted(&parts, Box::new(|p| cells[cell(&p)] += brightness(&p)));
        let total: f32 = cells.iter().sum();
        let top = cells.iter().fold(0.0f32, |a, &b| a.max(b));
        top / m as f32 <= max_cell * (1.0 + 1e-5) && total / m as f32 <= mean * (1.0 + 1e-5)
    };
    // The most lit points that fit (0 always does: dark points only dilute).
    let (mut lo, mut hi) = (0, if segs.is_empty() { 0 } else { extra });
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let parts = apportion(lo, &weights);
    let mut out = Vec::with_capacity(m);
    let mut next = segs.iter().zip(&parts).peekable();
    for (i, p) in f.iter().enumerate() {
        out.push(*p);
        if let Some(&(s, &k)) = next.peek() {
            if s.0 == i {
                out.extend((1..=k).map(|q| lerp(&f[i], &f[i + 1], q as f32 / (k + 1) as f32)));
                next.next();
            }
        }
    }
    out.resize(m, Point::blanked(last.x, last.y));
    out
}

/// One output's fixed rate between frames: the reserve left for the zone
/// mask, the last shaping, the overflow count and the 10 s measurement.
#[derive(Default)]
pub struct FixedRateState {
    /// Points the engine sends a frame (seam excluded), and the target's.
    points: usize,
    target_points: usize,
    target_fps: f32,
    reserve: usize,
    report: ShapeReport,
    overflow: u64,
    /// (time, frames/s of one sent frame) over the last 10 s.
    samples: VecDeque<(f64, f32)>,
    sum: f64,
    sum_sq: f64,
}

impl FixedRateState {
    /// Back to off: the next fixed rate starts a new measurement.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether a fixed rate ran on the last frame.
    pub fn is_on(&self) -> bool {
        self.points > 0
    }

    /// The reserve for a frame of `n` points (a new `n` starts over with
    /// a 1/32 reserve, at least 4 points).
    pub fn reserve(&mut self, target: &FixedTarget, n: usize) -> usize {
        if n != self.points || target.fps != self.target_fps || target.points != self.target_points {
            *self = Self { points: n, target_points: target.points, target_fps: target.fps, reserve: (n / 32).max(4).min(n / 4), ..Self::default() };
        }
        self.reserve
    }

    /// After the safety stage: `shaped` points went in, `safe` came out.
    /// The reserve follows what the zone mask added (up at once, down one
    /// point a frame), within a quarter of the frame; a frame above `n`
    /// is counted.
    pub fn after_safety(&mut self, report: ShapeReport, shaped: usize, safe: usize, n: usize) {
        self.report = report;
        let added = safe.saturating_sub(shaped);
        let want = added + 2;
        self.reserve = if want > self.reserve { want } else { self.reserve.saturating_sub(1).max(want) }.clamp(2, (n / 4).max(2));
        if safe > n {
            self.overflow += 1;
        }
    }

    /// One frame of `sent` points (seam included) went out at `t`.
    pub fn record(&mut self, t: f64, sent: usize, pps: u32) {
        let fps = crate::power::fps(pps, sent);
        let dev = if self.target_fps > 0.0 { ((fps - self.target_fps) / self.target_fps) as f64 } else { 0.0 };
        self.samples.push_back((t, fps));
        self.sum += fps as f64;
        self.sum_sq += dev * dev;
        while self.samples.front().is_some_and(|(at, _)| *at <= t - MEASURE_S) {
            let (_, old) = self.samples.pop_front().unwrap();
            let d = if self.target_fps > 0.0 { ((old - self.target_fps) / self.target_fps) as f64 } else { 0.0 };
            self.sum -= old as f64;
            self.sum_sq = (self.sum_sq - d * d).max(0.0);
        }
        if self.samples.len() == 1 {
            // Only this frame: wipe the rounding left by the ones dropped.
            self.sum = fps as f64;
            self.sum_sq = dev * dev;
        }
    }

    pub fn status(&self) -> FixedRateStatus {
        let n = self.samples.len().max(1) as f64;
        FixedRateStatus {
            target_fps: self.target_fps,
            measured_fps: if self.samples.is_empty() { 0.0 } else { (self.sum / n) as f32 },
            deviation_pct: ((self.sum_sq / n).sqrt() * 100.0) as f32,
            overflow_frames: self.overflow,
            points: self.target_points,
            report: self.report,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(x: f32, y: f32) -> Point {
        Point::lit(x, y, 1.0, 1.0, 1.0)
    }

    /// A closed square, densified to `per_side` points a side.
    fn square(per_side: usize) -> Vec<Point> {
        let c = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5), (-0.5, -0.5)];
        let mut out = vec![lit(-0.5, -0.5)];
        for w in c.windows(2) {
            for k in 1..=per_side {
                let t = k as f32 / per_side as f32;
                out.push(lit(w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
            }
        }
        out
    }

    fn lit_count(f: &[Point]) -> usize {
        f.iter().filter(|p| p.is_lit()).count()
    }

    /// Mean brightness of a frame, and its brightest cell's share, on `g`.
    fn exposure(f: &[Point], g: usize) -> (f32, f32) {
        let mut cells = vec![0.0f32; g * g];
        for p in f {
            let idx = |v: f32| (((v.clamp(-1.0, 1.0) + 1.0) * 0.5 * g as f32) as usize).min(g - 1);
            cells[idx(p.y) * g + idx(p.x)] += brightness(p);
        }
        let n = f.len() as f32;
        (cells.iter().sum::<f32>() / n, cells.iter().fold(0.0f32, |a, &b| a.max(b)) / n)
    }

    #[test]
    fn target_points_rounds_pps_over_fps() {
        assert_eq!(target_points(30_000, 25.0), 1_200);
        assert_eq!(target_points(30_000, 24.0), 1_250);
        assert_eq!(target_points(30_000, 60.0), 500);
        assert_eq!(target_points(30_000, 29.97), 1_001);
        // Clamped into 20..60, and not a number = off.
        assert_eq!(target_points(30_000, 5.0), 1_500);
        assert_eq!(target_points(30_000, 200.0), 500);
        assert_eq!(target_points(30_000, f32::NAN), 0);
        assert_eq!(target_points(30_000, f32::INFINITY), 0);
        assert_eq!(target_points(10, 60.0), 2);
        assert_eq!(sanitize_fps(f32::NAN), None);
        assert_eq!(sanitize_fps(25.0), Some(25.0));
    }

    #[test]
    fn a_short_frame_is_completed_along_its_lit_path() {
        let sq = square(10);
        let (out, rep) = shape(&sq, 1_200, 50, 0.05, 32);
        assert_eq!(out.len(), 1_150);
        assert_eq!((rep.input, rep.output, rep.reduced, rep.padded), (41, 1_150, false, 1_109));
        // Its own points are all there, in order.
        let mut it = out.iter();
        assert!(sq.iter().all(|p| it.any(|q| q == p)));
        // A lit square of evenly spaced points: nearly every point added is lit.
        assert!(lit_count(&out) > 1_100, "{} lit", lit_count(&out));
        // Evenly spread: every side gets the same share, within one point a segment.
        let steps: Vec<f32> = out.windows(2).filter(|w| w[0].is_lit() && w[1].is_lit()).map(|w| dist(&w[0], &w[1])).collect();
        let (lo, hi) = steps.iter().fold((f32::MAX, 0.0f32), |(a, b), &s| (a.min(s), b.max(s)));
        assert!(hi < lo * 1.2 && lo > 0.0, "steps {lo}..{hi}");
    }

    #[test]
    fn padding_never_lingers_on_a_lit_spot() {
        // A square with corner dwell: repeats on each corner.
        let mut f = Vec::new();
        for p in square(4) {
            f.push(p);
            if (p.x.abs() - 0.5).abs() < 1e-6 && (p.y.abs() - 0.5).abs() < 1e-6 {
                f.extend([p; 5]);
            }
        }
        let (out, _) = shape(&f, 900, 0, 0.05, 32);
        // Every lit point added moves from the one before it.
        let mut i = 0;
        for (k, p) in out.iter().enumerate() {
            if i < f.len() && *p == f[i] {
                i += 1;
            } else if p.is_lit() {
                assert!(dist(&out[k - 1], p) > 0.0, "an added lit point repeats {:?}", out[k - 1]);
            }
        }
        assert_eq!(i, f.len(), "the frame's own points stay");
    }

    #[test]
    fn a_lone_beam_and_a_dark_frame_are_completed_dark() {
        let beam = vec![lit(0.2, 0.3); 10];
        let (out, rep) = shape(&beam, 500, 0, 0.05, 32);
        assert_eq!((out.len(), rep.padded), (500, 490));
        assert_eq!(lit_count(&out), 10, "a beam is never held longer");
        assert!(out[10..].iter().all(|p| !p.is_lit() && p.x == 0.2 && p.y == 0.3));
        let dark: Vec<Point> = (0..50).map(|i| Point::blanked(i as f32 / 50.0, 0.0)).collect();
        let (out, _) = shape(&dark, 500, 0, 0.05, 32);
        assert!(out.len() == 500 && lit_count(&out) == 0);
        let (out, _) = shape(&[], 300, 10, 0.05, 32);
        assert!(out.len() == 290 && lit_count(&out) == 0);
        let (out, _) = shape(&[lit(0.0, 0.0)], 300, 0, 0.05, 32);
        assert_eq!((out.len(), lit_count(&out)), (300, 1));
    }

    #[test]
    fn padding_keeps_the_frame_dark_share() {
        // A line, then a long dark travel: completing it must not raise
        // the share of lit time (a cell would get more light).
        let mut f: Vec<Point> = (0..20).map(|i| lit(-0.9 + i as f32 * 0.01, 0.0)).collect();
        f.extend((0..80).map(|i| Point::blanked(-0.7 + i as f32 * 0.02, 0.5)));
        let before = exposure(&f, 32);
        let (out, _) = shape(&f, 1_000, 0, 0.05, 32);
        let after = exposure(&out, 32);
        assert!(after.0 <= before.0 * 1.0001 && after.1 <= before.1 * 1.0001, "{before:?} -> {after:?}");
        assert!(lit_count(&out) > 20, "the line still gets points");
    }

    #[test]
    fn a_long_frame_is_resampled_keeping_corners_and_travel() {
        // Two squares joined by a dark travel.
        let mut f = square(200);
        let end = *f.last().unwrap();
        f.extend((1..=10).map(|i| Point::blanked(end.x + i as f32 * 0.05, end.y)));
        let second: Vec<Point> = square(200).into_iter().map(|p| Point { x: p.x * 0.5 + 0.3, ..p }).collect();
        f.push(Point::blanked(second[0].x, second[0].y));
        f.extend(second);
        let (out, rep) = shape(&f, 1_000, 40, 0.05, 32);
        assert_eq!(out.len(), 960);
        assert!(rep.reduced && !rep.over_step, "{rep:?}");
        // Every corner and every dark point is still there.
        for p in f.iter().filter(|p| !p.is_lit() || ((p.x.abs() - 0.5).abs() < 1e-6 && (p.y.abs() - 0.5).abs() < 1e-6)) {
            assert!(out.contains(p), "{p:?} lost");
        }
        // Dark stays dark: as many dark points, none lit in between.
        assert_eq!(out.iter().filter(|p| !p.is_lit()).count(), f.iter().filter(|p| !p.is_lit()).count());
        assert!(out.iter().all(|p| (-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y)));
    }

    #[test]
    fn a_reduction_beyond_the_output_step_warns() {
        let f = square(500);
        let (out, rep) = shape(&f, 40, 0, 0.01, 32);
        assert_eq!(out.len(), 40);
        assert!(rep.reduced && rep.over_step, "{rep:?}");
        // The corners of a square survive even then.
        for c in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            assert!(out.iter().any(|p| (p.x - c.0).abs() < 1e-6 && (p.y - c.1).abs() < 1e-6), "corner {c:?} lost");
        }
        // Far too few points even for the corners: evenly spaced, never lighting travel.
        let mut zig: Vec<Point> = Vec::new();
        for i in 0..200 {
            zig.push(lit(-0.9 + i as f32 * 0.009, if i % 2 == 0 { 0.0 } else { 0.5 }));
            zig.push(Point::blanked(-0.9 + i as f32 * 0.009, 0.5));
        }
        let (out, rep) = shape(&zig, 50, 0, 0.05, 32);
        assert_eq!(out.len(), 50);
        assert!(rep.over_step);
    }

    #[test]
    fn top_up_only_adds_dark_points() {
        let f = square(10);
        let up = top_up(f.clone(), 100);
        assert_eq!(up.len(), 100);
        assert_eq!(&up[..f.len()], &f[..]);
        assert!(up[f.len()..].iter().all(|p| !p.is_lit() && p.x == f.last().unwrap().x));
        // Never removes anything.
        assert_eq!(top_up(f.clone(), 5), f);
        assert_eq!(top_up(Vec::new(), 3), vec![Point::blanked(0.0, 0.0); 3]);
    }

    #[test]
    fn values_that_are_not_numbers_go_dark() {
        let f = vec![lit(0.0, 0.0), Point::lit(f32::NAN, 0.0, 1.0, 1.0, 1.0), Point::lit(0.5, 0.5, f32::NAN, 1.0, 0.0), lit(2.0, -3.0)];
        let (out, _) = shape(&f, 200, 0, 0.05, 32);
        assert!(out.iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.r.is_finite() && (-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y)));
    }

    /// Random frames: the length is exact before (N - reserve) and after
    /// (N) the safety stage, nothing leaves the field, the frame's mean
    /// brightness and its brightest cell never rise when completed, and a
    /// reduction never lights a point between two dark ones.
    #[test]
    fn random_frames_keep_the_count_and_never_add_light() {
        let mut seed = 0x463u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for case in 0..400 {
            let len = (rnd() * 3_000.0) as usize;
            let mut f = Vec::with_capacity(len);
            let (mut x, mut y) = (rnd() as f32 * 2.0 - 1.0, rnd() as f32 * 2.0 - 1.0);
            for _ in 0..len {
                if rnd() < 0.8 {
                    x = (x + (rnd() as f32 - 0.5) * 0.2).clamp(-1.0, 1.0);
                    y = (y + (rnd() as f32 - 0.5) * 0.2).clamp(-1.0, 1.0);
                }
                f.push(if rnd() < 0.7 { Point::lit(x, y, rnd() as f32, rnd() as f32, 0.5) } else { Point::blanked(x, y) });
            }
            let n = 300 + (rnd() * 1_200.0) as usize;
            let reserve = (rnd() * 40.0) as usize;
            let grid = 16 + (rnd() * 48.0) as usize;
            let (out, rep) = shape(&f, n, reserve, 0.05, grid);
            assert_eq!(out.len(), n - reserve, "case {case}");
            assert!(out.iter().all(|p| (-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y)), "case {case}");
            if !f.is_empty() && !rep.reduced {
                let (b, a) = (exposure(&f, grid), exposure(&out, grid));
                assert!(a.0 <= b.0 * 1.0001 + 1e-6 && a.1 <= b.1 * 1.0001 + 1e-6, "case {case}: {b:?} -> {a:?}");
            }
            let up = top_up(out.clone(), n);
            assert_eq!(up.len(), n);
            assert_eq!(lit_count(&up), lit_count(&out));
        }
    }

    #[test]
    fn the_measure_follows_the_frames_sent() {
        let t = FixedTarget { fps: 25.0, pps: 30_000, points: 1_200, max_step: 0.05 };
        let mut st = FixedRateState::default();
        assert_eq!(st.reserve(&t, 1_200), 37);
        for i in 0..600 {
            st.record(i as f64 / 60.0, 1_200, 30_000);
        }
        let s = st.status();
        assert!((s.measured_fps - 25.0).abs() < 1e-3 && s.deviation_pct < 1e-3, "{s:?}");
        // One frame in ten at 1 300 points: about 2.4 % off.
        for i in 600..1_200 {
            st.record(i as f64 / 60.0, if i % 10 == 0 { 1_300 } else { 1_200 }, 30_000);
        }
        let s = st.status();
        assert!(s.deviation_pct > 1.0 && s.deviation_pct < 4.0, "{s:?}");
        // The reserve follows the zone mask, and an overflow is counted.
        st.after_safety(ShapeReport::default(), 1_163, 1_200, 1_200);
        assert_eq!(st.reserve(&t, 1_200), 39, "up at once");
        st.after_safety(ShapeReport::default(), 1_161, 1_165, 1_200);
        assert_eq!(st.reserve(&t, 1_200), 38, "down a point a frame");
        st.after_safety(ShapeReport::default(), 1_171, 1_210, 1_200);
        assert_eq!(st.status().overflow_frames, 1);
        // A new rate starts over.
        let t30 = FixedTarget { fps: 30.0, points: 1_000, ..t };
        st.reserve(&t30, 1_000);
        assert_eq!(st.status().overflow_frames, 0);
    }
}
