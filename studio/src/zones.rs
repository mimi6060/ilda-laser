//! Projection safety zones, the horizon and colour calibration (T-003).
//!
//! Applied in the output safety stage (`safety::apply`): after the layers,
//! the live stage and calibration, before the strobe limiter and the
//! output gate. Coordinates are output coordinates (-1..1, y up), i.e.
//! where the light really goes, which is also what the preview draws.
//!
//! **Zones.** Up to `MAX_ZONES` polygons (concave allowed, even-odd rule).
//! A `Blank` zone keeps nothing lit inside; a `Dim` zone scales the colour
//! by its level. Overlapping attenuations multiply.
//!
//! **Horizon.** Beams below `y` are always blanked (the T-101 protection,
//! `safety::blank_low_beams`). With `lines` on, *everything* below `y` -
//! lines, text, sheets, figures - is scaled to `level` (0 = blank), with a
//! linear ramp from `level` at `y` up to full brightness at `y + ramp`. The
//! ramp sits above `y`, so below the horizon the light is never above
//! `level`.
//!
//! **Segment splitting.** A DAC draws a straight line between two samples,
//! so blanking the samples inside a zone is not enough: a lit segment
//! between two outside samples can cross a zone. Every segment with a lit
//! end is cut where it crosses a zone edge (or the horizon). Around each
//! crossing we insert samples `EDGE_GAP` either side of the edge: the
//! outside one keeps its colour, and the short hop across the edge is
//! drawn at the *lower* of the two sides' levels, with duplicates so that
//! the result is right whether a DAC colours a segment with its start or
//! its end sample. Every emitted sample is also scaled by the attenuation
//! at its own position, so no lit sample is ever inside a Blank zone.
//!
//! **Edges** (T-351). A zone is a closed set: a point on an edge (within
//! `ON_EDGE`) is inside. Besides proper crossings, a segment is cut where
//! it enters or leaves that thin band along an edge and at an end lying on
//! an edge, and a crossing whose own point is darker than both sides (a
//! touched corner) is hopped over dark. So a sample on an edge is dark, a
//! line along an edge is dark, and nothing lit starts or stops on one.
//!
//! **Colour calibration.** A per-colour gain (0..1, can only dim) and a
//! minimum diode level: analogue diodes emit nothing below a threshold, so
//! a lit channel `v` is sent as `min + v·(1 − min)`; a dark channel stays 0.

use crate::patterns::Point;
use serde::{Deserialize, Serialize};

pub const MAX_ZONES: usize = 8;
pub const MAX_VERTICES: usize = 24;
/// Inserted samples sit this far (normalised units) along the segment on
/// each side of an edge (0.05 % of the field width).
const EDGE_GAP: f32 = 1e-3;
/// A point this close (normalised units) to a zone edge is on it.
const ON_EDGE: f32 = 1e-6;
/// Below this a channel counts as dark for the minimum diode level.
const DARK: f32 = 1e-3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneKind {
    /// Nothing lit inside.
    #[default]
    Blank,
    /// Colour scaled by the zone's level inside.
    Dim,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Zone {
    /// Stable id (assigned by the store), to tell an edit from a removal.
    pub id: u32,
    pub name: String,
    pub kind: ZoneKind,
    /// Dim zones: the brightness kept inside, 0..1. Ignored by Blank.
    pub level: f32,
    /// Polygon vertices in output coordinates, not closed.
    pub points: Vec<[f32; 2]>,
}

impl Default for Zone {
    fn default() -> Self {
        Self { id: 0, name: "Zone".into(), kind: ZoneKind::Blank, level: 0.3, points: Vec::new() }
    }
}

impl Zone {
    /// Brightness factor inside the zone.
    pub fn factor(&self) -> f32 {
        match self.kind {
            ZoneKind::Blank => 0.0,
            ZoneKind::Dim => {
                if self.level.is_finite() {
                    self.level.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Horizon {
    /// Height of the horizon, -1..1 (0 = centre). Beams below are blanked.
    pub y: f32,
    /// Also scale lines, text, sheets and figures below `y` to `level`.
    pub lines: bool,
    /// Height of the fade above `y` (0..0.5).
    pub ramp: f32,
    /// Brightness kept below the horizon when `lines` is on (0 = blank).
    pub level: f32,
}

impl Default for Horizon {
    fn default() -> Self {
        Self { y: 0.0, lines: false, ramp: 0.1, level: 0.0 }
    }
}

impl Horizon {
    /// Brightness factor at height `y` (1 when `lines` is off).
    pub fn factor(&self, y: f32) -> f32 {
        if !self.lines {
            return 1.0;
        }
        let level = self.level.clamp(0.0, 1.0);
        if y < self.y {
            level
        } else if self.ramp > 0.0 && y < self.y + self.ramp {
            level + (1.0 - level) * (y - self.y) / self.ramp
        } else {
            1.0
        }
    }
}

struct Poly {
    factor: f32,
    v: Vec<(f32, f32)>,
    min: (f32, f32),
    max: (f32, f32),
}

impl Poly {
    fn new(zone: &Zone) -> Option<Self> {
        if zone.points.len() < 3 {
            return None;
        }
        let v: Vec<(f32, f32)> = zone.points.iter().map(|p| (p[0], p[1])).collect();
        let min = v.iter().fold((f32::INFINITY, f32::INFINITY), |m, p| (m.0.min(p.0), m.1.min(p.1)));
        let max = v.iter().fold((f32::NEG_INFINITY, f32::NEG_INFINITY), |m, p| (m.0.max(p.0), m.1.max(p.1)));
        Some(Self { factor: zone.factor(), v, min, max })
    }

    /// Even-odd point in polygon.
    fn contains(&self, x: f32, y: f32) -> bool {
        if x < self.min.0 || x > self.max.0 || y < self.min.1 || y > self.max.1 {
            return false;
        }
        let mut inside = false;
        let mut j = self.v.len() - 1;
        for i in 0..self.v.len() {
            let (xi, yi) = self.v[i];
            let (xj, yj) = self.v[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    /// True when (x, y) is on an edge, within `ON_EDGE`.
    fn on_edge(&self, x: f32, y: f32) -> bool {
        if x < self.min.0 - ON_EDGE || x > self.max.0 + ON_EDGE || y < self.min.1 - ON_EDGE || y > self.max.1 + ON_EDGE {
            return false;
        }
        let mut j = self.v.len() - 1;
        for i in 0..self.v.len() {
            let (p, q) = (self.v[j], self.v[i]);
            let e = (q.0 - p.0, q.1 - p.1);
            let w = (x - p.0, y - p.1);
            let len2 = e.0 * e.0 + e.1 * e.1;
            let s = if len2 > 0.0 { ((w.0 * e.0 + w.1 * e.1) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (dx, dy) = (w.0 - s * e.0, w.1 - s * e.1);
            if dx * dx + dy * dy <= ON_EDGE * ON_EDGE {
                return true;
            }
            j = i;
        }
        false
    }

    /// Inside or on an edge: the zone as a closed set. `contains` alone
    /// puts a point on an edge in or out depending on the edge's side.
    fn covers(&self, x: f32, y: f32) -> bool {
        self.contains(x, y) || self.on_edge(x, y)
    }

    /// Pushes the parameters t in (0, 1) where a→b crosses an edge.
    fn crossings(&self, a: (f32, f32), b: (f32, f32), out: &mut Vec<f32>) {
        let (min, max) = ((self.min.0 - ON_EDGE, self.min.1 - ON_EDGE), (self.max.0 + ON_EDGE, self.max.1 + ON_EDGE));
        if a.0.max(b.0) < min.0 || a.0.min(b.0) > max.0 || a.1.max(b.1) < min.1 || a.1.min(b.1) > max.1 {
            return;
        }
        let d = (b.0 - a.0, b.1 - a.1);
        let mut j = self.v.len() - 1;
        let (lo, hi) = ((a.0.min(b.0) - ON_EDGE, a.1.min(b.1) - ON_EDGE), (a.0.max(b.0) + ON_EDGE, a.1.max(b.1) + ON_EDGE));
        for i in 0..self.v.len() {
            let (p, q) = (self.v[j], self.v[i]);
            // No crossing nor band unless the boxes overlap.
            if p.0.max(q.0) < lo.0 || p.0.min(q.0) > hi.0 || p.1.max(q.1) < lo.1 || p.1.min(q.1) > hi.1 {
                j = i;
                continue;
            }
            let e = (q.0 - p.0, q.1 - p.1);
            let denom = d.0 * e.1 - d.1 * e.0;
            if denom.abs() > 1e-12 {
                let w = (p.0 - a.0, p.1 - a.1);
                let t = (w.0 * e.1 - w.1 * e.0) / denom;
                let u = (w.0 * d.1 - w.1 * d.0) / denom;
                if t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&u) {
                    out.push(t);
                }
            }
            Self::band(a, b, p, self.v[i], out);
            j = i;
        }
    }

    /// Pushes the parameters t in (0, 1) where a→b enters or leaves the
    /// band of points `on_edge` of p→q (a stadium of radius `ON_EDGE`), so
    /// that a segment running along an edge, or touching a corner, gets
    /// breakpoints too (T-351). The stadium is convex: its intersection
    /// with the line is one interval, the hull of the line's intersections
    /// with the rectangle and the two end discs. In f64: ON_EDGE² is tiny.
    fn band(a: (f32, f32), b: (f32, f32), p: (f32, f32), q: (f32, f32), out: &mut Vec<f32>) {
        let eps = ON_EDGE as f64;
        let (ax, ay, px, py) = (a.0 as f64, a.1 as f64, p.0 as f64, p.1 as f64);
        let d = (b.0 as f64 - ax, b.1 as f64 - ay);
        let e = (q.0 as f64 - px, q.1 as f64 - py);
        let len = e.0.hypot(e.1);
        if len == 0.0 {
            return;
        }
        let (u, n) = ((e.0 / len, e.1 / len), (-e.1 / len, e.0 / len));
        let w = (ax - px, ay - py);
        let dot = |x: (f64, f64), y: (f64, f64)| x.0 * y.0 + x.1 * y.1;
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        // The rectangle: |n·(P - p)| <= eps and 0 <= u·(P - p) <= len.
        let mut range = Some((f64::NEG_INFINITY, f64::INFINITY));
        for (c0, c1, min, max) in [(dot(n, w), dot(n, d), -eps, eps), (dot(u, w), dot(u, d), 0.0, len)] {
            range = range.and_then(|(r0, r1)| {
                if c1 == 0.0 {
                    (min..=max).contains(&c0).then_some((r0, r1))
                } else {
                    let (t0, t1) = ((min - c0) / c1, (max - c0) / c1);
                    let (t0, t1) = (r0.max(t0.min(t1)), r1.min(t0.max(t1)));
                    (t0 <= t1).then_some((t0, t1))
                }
            });
        }
        if let Some((t0, t1)) = range {
            (lo, hi) = (t0, t1);
        }
        // The discs at both ends: |w + t·d|² = eps².
        let dd = dot(d, d);
        for v in [(px, py), (q.0 as f64, q.1 as f64)] {
            let w = (ax - v.0, ay - v.1);
            let (bh, c) = (dot(w, d), dot(w, w) - eps * eps);
            let disc = bh * bh - dd * c;
            if dd > 0.0 && disc >= 0.0 {
                let s = disc.sqrt();
                lo = lo.min((-bh - s) / dd);
                hi = hi.max((-bh + s) / dd);
            }
        }
        if lo <= hi {
            for t in [lo, hi] {
                if t > 0.0 && t < 1.0 {
                    out.push(t as f32);
                }
            }
        }
    }
}

/// The compiled zones, horizon and colour calibration for one frame.
pub struct Mask {
    polys: Vec<Poly>,
    horizon: Horizon,
    gain: [f32; 3],
    min_level: f32,
}

fn unit(v: f32, default: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        default
    }
}

impl Mask {
    pub fn new(zones: &[Zone], horizon: Horizon, gain: [f32; 3], min_level: f32) -> Self {
        Self {
            polys: zones.iter().filter_map(Poly::new).collect(),
            horizon,
            gain: gain.map(|g| unit(g, 1.0)),
            min_level: unit(min_level, 0.0).min(0.5),
        }
    }

    /// True when the mask changes nothing.
    pub fn is_identity(&self) -> bool {
        self.polys.is_empty() && !self.horizon.lines && self.gain == [1.0; 3] && self.min_level == 0.0
    }

    /// Brightness factor at (x, y): horizon × every zone containing it.
    /// A point on a zone's edge counts as inside (T-351).
    pub fn factor(&self, x: f32, y: f32) -> f32 {
        let mut f = self.horizon.factor(y);
        for p in &self.polys {
            if f == 0.0 {
                break;
            }
            if p.covers(x, y) {
                f *= p.factor;
            }
        }
        f
    }

    fn drive(&self, v: f32, gain: f32, f: f32) -> f32 {
        let v = if v.is_finite() { (v * gain * f).clamp(0.0, 1.0) } else { 0.0 };
        if self.min_level > 0.0 {
            if v < DARK {
                0.0
            } else {
                self.min_level + v * (1.0 - self.min_level)
            }
        } else {
            v
        }
    }

    /// A sample at (x, y) with `colour`'s colour scaled by `f`.
    fn sample(&self, x: f32, y: f32, colour: &Point, f: f32) -> Point {
        Point {
            x,
            y,
            r: self.drive(colour.r, self.gain[0], f),
            g: self.drive(colour.g, self.gain[1], f),
            b: self.drive(colour.b, self.gain[2], f),
        }
    }

    /// Applies the mask to a frame. Returns the new frame and how many lit
    /// input samples were darkened by a zone or the horizon.
    pub fn apply(&self, frame: &[Point]) -> (Vec<Point>, usize) {
        if self.is_identity() {
            return (frame.to_vec(), 0);
        }
        let mut out = Vec::with_capacity(frame.len() + 16);
        let mut ts = Vec::new();
        let mut masked = 0;
        for (i, b) in frame.iter().enumerate() {
            let fb = self.factor(b.x, b.y);
            if fb < 1.0 && b.is_lit() {
                masked += 1;
            }
            if i > 0 {
                let a = &frame[i - 1];
                if a.is_lit() || b.is_lit() {
                    self.crossings(a, b, &mut ts);
                    if !ts.is_empty() {
                        self.split(a, b, &ts, &mut out);
                    }
                }
            }
            out.push(self.sample(b.x, b.y, b, fb));
        }
        (out, masked)
    }

    fn crossings(&self, a: &Point, b: &Point, ts: &mut Vec<f32>) {
        ts.clear();
        for p in &self.polys {
            p.crossings((a.x, a.y), (b.x, b.y), ts);
        }
        // An end on an edge is a crossing too (at t = 0 or 1): the segment
        // must not start or stop lit on the edge.
        if self.polys.iter().any(|p| p.on_edge(a.x, a.y)) {
            ts.push(0.0);
        }
        if self.polys.iter().any(|p| p.on_edge(b.x, b.y)) {
            ts.push(1.0);
        }
        let h = self.horizon;
        if h.lines && (a.y - h.y) * (b.y - h.y) < 0.0 {
            ts.push((h.y - a.y) / (b.y - a.y));
        }
        ts.sort_by(|x, y| x.total_cmp(y));
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
    }

    /// Inserts the samples around each crossing of a→b (the segment takes
    /// b's colour; a is already emitted, b is emitted by the caller).
    fn split(&self, a: &Point, b: &Point, ts: &[f32], out: &mut Vec<Point>) {
        let at = |t: f32| (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let gap = if len > 0.0 { EDGE_GAP / len } else { 0.0 };
        // Interval k runs from bounds[k] to bounds[k+1]; its level is taken
        // at its middle (no edge inside an interval, so it is constant). A
        // crossing may be at t = 0 or 1, making an empty first/last interval
        // whose level is the end's own.
        let bound = |k: usize| if k == 0 { 0.0 } else if k > ts.len() { 1.0 } else { ts[k - 1] };
        let level = |k: usize| {
            let (x, y) = at((bound(k) + bound(k + 1)) / 2.0);
            self.factor(x, y)
        };
        let mut before = level(0);
        for (k, &t) in ts.iter().enumerate() {
            let after = level(k + 1);
            // The edge itself can be darker than both sides: a segment that
            // only touches a zone (a corner, or a vertex of a concave one).
            let (x, y) = at(t);
            let low = before.min(after).min(self.factor(x, y));
            if before > low || after > low {
                let lo = at(t - gap.min((t - bound(k)) / 2.0));
                let hi = at(t + gap.min((bound(k + 2) - t) / 2.0));
                let mut emit = |(x, y): (f32, f32), f: f32| out.push(self.sample(x, y, b, f.min(self.factor(x, y))));
                emit(lo, before);
                if before > low {
                    emit(lo, low);
                }
                if after > low {
                    emit(hi, low);
                }
                emit(hi, after);
            }
            before = after;
        }
    }
}

/// Test helper: lit samples inside or on the edge of a Blank zone, plus
/// lit segments that pass through or touch one (a segment counts as lit if either end is, whichever
/// end the DAC takes its colour from), checked at 19 points each.
#[cfg(test)]
pub fn blank_violations(frame: &[Point], zones: &[Zone]) -> usize {
    let polys: Vec<Poly> = zones.iter().filter(|z| z.kind == ZoneKind::Blank).filter_map(Poly::new).collect();
    let inside = |x: f32, y: f32| polys.iter().any(|z| z.covers(x, y));
    let samples = frame.iter().filter(|p| p.is_lit() && inside(p.x, p.y)).count();
    let segments = frame
        .windows(2)
        .filter(|w| w[0].is_lit() || w[1].is_lit())
        .filter(|w| (1..20).any(|s| {
            let t = s as f32 / 20.0;
            inside(w[0].x + (w[1].x - w[0].x) * t, w[0].y + (w[1].y - w[0].y) * t)
        }))
        .count();
    samples + segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32, kind: ZoneKind) -> Zone {
        Zone { points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], kind, ..Default::default() }
    }

    fn mask(zones: &[Zone]) -> Mask {
        Mask::new(zones, Horizon::default(), [1.0; 3], 0.0)
    }

    fn white(x: f32, y: f32) -> Point {
        Point::lit(x, y, 1.0, 1.0, 1.0)
    }

    /// Lit samples inside or on the edge of any Blank zone.
    fn lit_inside(frame: &[Point], zones: &[Zone]) -> Vec<Point> {
        let polys: Vec<Poly> = zones.iter().filter(|z| z.kind == ZoneKind::Blank).filter_map(Poly::new).collect();
        frame.iter().copied().filter(|p| p.is_lit() && polys.iter().any(|z| z.covers(p.x, p.y))).collect()
    }

    /// The parts of lit segments (under either colouring convention) that
    /// pass through a Blank zone, found by sampling each drawn segment.
    fn lit_segments_inside(frame: &[Point], zones: &[Zone]) -> usize {
        let polys: Vec<Poly> = zones.iter().filter(|z| z.kind == ZoneKind::Blank).filter_map(Poly::new).collect();
        let mut bad = 0;
        for w in frame.windows(2) {
            // The DAC may use the start or the end colour: check both.
            if !w[0].is_lit() && !w[1].is_lit() {
                continue;
            }
            for lit_end in [0, 1] {
                if !w[lit_end].is_lit() {
                    continue;
                }
                for s in 1..20 {
                    let t = s as f32 / 20.0;
                    let (x, y) = (w[0].x + (w[1].x - w[0].x) * t, w[0].y + (w[1].y - w[0].y) * t);
                    if polys.iter().any(|z| z.covers(x, y)) {
                        bad += 1;
                        break;
                    }
                }
            }
        }
        bad
    }

    #[test]
    fn point_in_polygon_handles_concave_shapes() {
        // An L shape.
        let l = Zone { points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.3], [0.3, 0.3], [0.3, 1.0], [0.0, 1.0]], ..Default::default() };
        let p = Poly::new(&l).unwrap();
        assert!(p.contains(0.1, 0.1));
        assert!(p.contains(0.8, 0.1));
        assert!(p.contains(0.1, 0.8));
        assert!(!p.contains(0.8, 0.8), "the notch is outside");
        assert!(!p.contains(-0.1, 0.1));
        assert!(Poly::new(&Zone { points: vec![[0.0, 0.0], [1.0, 1.0]], ..Default::default() }).is_none());
    }

    #[test]
    fn a_segment_crossing_a_zone_is_split_at_its_edges() {
        let zones = [rect(-0.2, -0.2, 0.2, 0.2, ZoneKind::Blank)];
        // One lit line from x = -1 to x = 1 through the zone, no sample inside.
        let frame = vec![Point::blanked(-1.0, 0.0), white(-1.0, 0.0), white(1.0, 0.0)];
        let (out, masked) = mask(&zones).apply(&frame);
        assert_eq!(masked, 0, "no input sample was inside");
        assert!(out.len() > frame.len());
        assert!(lit_inside(&out, &zones).is_empty());
        assert_eq!(lit_segments_inside(&out, &zones), 0, "{out:?}");
        // Lit up to the edge on both sides.
        let lit_x: Vec<f32> = out.iter().filter(|p| p.is_lit()).map(|p| p.x).collect();
        assert!(lit_x.iter().any(|&x| (x + 0.2).abs() < 2e-3 && x < -0.2), "{lit_x:?}");
        assert!(lit_x.iter().any(|&x| (x - 0.2).abs() < 2e-3 && x > 0.2), "{lit_x:?}");
    }

    #[test]
    fn a_segment_clipping_a_corner_is_cut() {
        // Crosses a tiny corner: both crossings are closer than 2 × EDGE_GAP.
        let zones = [rect(0.0, 0.0, 0.5, 0.5, ZoneKind::Blank)];
        let frame = vec![white(-0.1, 0.0005), white(0.0005, 0.1)];
        let (out, _) = mask(&zones).apply(&frame);
        assert_eq!(lit_segments_inside(&out, &zones), 0, "{out:?}");
    }

    /// T-351: a lit sample exactly on a Blank edge used to stay lit (the
    /// even-odd test puts right and top edges outside) and the segment from
    /// it into the zone was not cut (a crossing at t = 0 is not one).
    #[test]
    fn a_lit_sample_on_a_blank_edge_does_not_leak_in() {
        let zones = [rect(-0.25, -0.25, 0.25, 0.25, ZoneKind::Blank)];
        let m = mask(&zones);
        // Lines with a step of 0.05 that land exactly on each edge and each
        // corner, then go on into the zone; and the same lines leaving it.
        let dirs = [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (1.0, -1.0)];
        for (dx, dy) in dirs {
            let line: Vec<Point> = (0..10).map(|k| white(-dx * (0.5 - 0.05 * k as f32), -dy * (0.5 - 0.05 * k as f32))).collect();
            assert!(line.iter().any(|p| m.polys[0].on_edge(p.x, p.y)), "a sample is on the edge");
            for frame in [line.clone(), line.iter().rev().copied().collect()] {
                let (out, _) = m.apply(&frame);
                assert!(lit_inside(&out, &zones).is_empty(), "({dx}, {dy}): {out:?}");
                assert_eq!(lit_segments_inside(&out, &zones), 0, "({dx}, {dy}): {out:?}");
                assert_eq!(blank_violations(&out, &zones), 0, "({dx}, {dy})");
                // Still lit up to about the edge, not cut early.
                let off = |p: &Point| (p.x.abs().max(p.y.abs()) - 0.25).abs();
                assert!(out.iter().any(|p| p.is_lit() && off(p) < 2e-3), "({dx}, {dy}): {out:?}");
            }
        }
    }

    /// A lit segment that only touches a Blank zone - at a corner or along
    /// an edge - is dark where it touches.
    #[test]
    fn a_segment_touching_a_blank_zone_is_dark_where_it_touches() {
        let zones = [rect(0.0, 0.0, 0.5, 0.5, ZoneKind::Blank)];
        let m = mask(&zones);
        // Is (x, y) on a lit drawn segment (under either colouring)?
        let lit_through = |out: &[Point], x: f32, y: f32| {
            out.windows(2).any(|w| {
                let (ex, ey) = (w[1].x - w[0].x, w[1].y - w[0].y);
                let (px, py) = (x - w[0].x, y - w[0].y);
                let len2 = ex * ex + ey * ey;
                let s = if len2 > 0.0 { ((px * ex + py * ey) / len2).clamp(0.0, 1.0) } else { 0.0 };
                (w[0].is_lit() || w[1].is_lit()) && (px - s * ex).hypot(py - s * ey) < 1e-6
            })
        };
        // Through the corner (0.5, 0.5) only.
        let (out, _) = m.apply(&[white(0.0, 1.0), white(1.0, 0.0)]);
        assert!(!lit_through(&out, 0.5, 0.5), "{out:?}");
        assert!(lit_inside(&out, &zones).is_empty(), "{out:?}");
        assert!(lit_through(&out, 0.25, 0.75), "lit away from the corner: {out:?}");
        // Along the top edge, from outside to outside.
        let (out, _) = m.apply(&[white(-0.5, 0.5), white(1.0, 0.5)]);
        assert!(!lit_through(&out, 0.25, 0.5), "{out:?}");
        assert!(lit_through(&out, -0.25, 0.5) && lit_through(&out, 0.75, 0.5), "lit outside: {out:?}");
        // A vertical line one f32 ulp right of the right edge (what a
        // calibrated x = 0.5 can turn into), coming down past the corner.
        let x = 0.500_000_06;
        let frame: Vec<Point> = (0..30).map(|k| white(x, 0.8 - 0.03 * k as f32)).collect();
        let (out, _) = m.apply(&frame);
        assert!(lit_inside(&out, &zones).is_empty(), "{out:?}");
        assert_eq!(lit_segments_inside(&out, &zones), 0, "{out:?}");
        assert!(lit_through(&out, x, 0.6), "lit above the corner: {out:?}");
    }

    #[test]
    fn samples_inside_are_blanked_and_dim_zones_scale() {
        let blank = [rect(-0.5, -0.5, 0.5, 0.5, ZoneKind::Blank)];
        let (out, masked) = mask(&blank).apply(&[white(0.0, 0.0), white(0.1, 0.0), white(0.9, 0.9)]);
        assert_eq!(masked, 2);
        assert!(!out[0].is_lit() && !out[1].is_lit());
        assert!(out.last().unwrap().is_lit());
        let dim = [Zone { level: 0.25, ..rect(-0.5, -0.5, 0.5, 0.5, ZoneKind::Dim) }];
        let (out, _) = mask(&dim).apply(&[white(0.0, 0.0)]);
        assert!((out[0].r - 0.25).abs() < 1e-6);
        // Overlapping dim zones multiply; a blank one wins.
        let both = [dim[0].clone(), Zone { level: 0.5, ..rect(-0.1, -0.1, 0.1, 0.1, ZoneKind::Dim) }];
        assert!((mask(&both).factor(0.0, 0.0) - 0.125).abs() < 1e-6);
        let with_blank = [dim[0].clone(), rect(-0.1, -0.1, 0.1, 0.1, ZoneKind::Blank)];
        assert_eq!(mask(&with_blank).factor(0.0, 0.0), 0.0);
    }

    #[test]
    fn a_dim_edge_is_crossed_at_the_lower_level() {
        let zones = [Zone { level: 0.4, ..rect(0.0, -1.0, 1.0, 1.0, ZoneKind::Dim) }];
        let (out, _) = mask(&zones).apply(&[white(-0.5, 0.0), white(0.5, 0.0)]);
        // Every sample right of the edge is at 0.4 at most.
        assert!(out.iter().filter(|p| p.x > 0.0).all(|p| p.r <= 0.4 + 1e-6), "{out:?}");
        // The hop across the edge (samples within the gap) is at 0.4 both ends.
        let near: Vec<&Point> = out.iter().filter(|p| p.x.abs() < 2e-3).collect();
        assert!(near.iter().any(|p| p.x < 0.0 && (p.r - 1.0).abs() < 1e-6), "the outside keeps its colour");
        assert!(near.iter().any(|p| p.x < 0.0 && (p.r - 0.4).abs() < 1e-6), "a duplicate at the lower level");
    }

    #[test]
    fn blanked_travel_is_left_alone() {
        let zones = [rect(-0.2, -0.2, 0.2, 0.2, ZoneKind::Blank)];
        let frame = vec![Point::blanked(-1.0, 0.0), Point::blanked(1.0, 0.0)];
        let (out, _) = mask(&zones).apply(&frame);
        assert_eq!(out, frame, "no light: no extra samples");
    }

    #[test]
    fn a_segment_into_a_dark_sample_is_still_cut() {
        // Lit start, dark end: a DAC that colours with the start sample
        // would draw light through the zone - it must stop at the edge.
        let zones = [rect(-0.2, -0.2, 0.2, 0.2, ZoneKind::Blank)];
        let (out, _) = mask(&zones).apply(&[white(-1.0, 0.0), Point::blanked(1.0, 0.0)]);
        assert_eq!(lit_segments_inside(&out, &zones), 0, "{out:?}");
    }

    #[test]
    fn the_horizon_covers_lines_with_a_ramp() {
        let h = Horizon { y: 0.0, lines: true, ramp: 0.2, level: 0.0 };
        assert_eq!(h.factor(-0.01), 0.0);
        assert!((h.factor(0.1) - 0.5).abs() < 1e-6);
        assert_eq!(h.factor(0.25), 1.0);
        let dimmed = Horizon { level: 0.2, ..h };
        assert!((dimmed.factor(-0.5) - 0.2).abs() < 1e-6);
        assert!((dimmed.factor(0.1) - 0.6).abs() < 1e-6);
        assert_eq!(Horizon { lines: false, ..h }.factor(-1.0), 1.0, "off: beams only (safety.rs)");
        // A vertical line from -1 to 1 in one segment: nothing lit below 0.
        let m = Mask::new(&[], h, [1.0; 3], 0.0);
        let (out, masked) = m.apply(&[white(0.0, 1.0), white(0.0, -1.0), white(0.0, 1.0)]);
        assert_eq!(masked, 1);
        assert!(out.iter().filter(|p| p.y < 0.0).all(|p| !p.is_lit()), "{out:?}");
        for w in out.windows(2) {
            if w[0].y < -EDGE_GAP * 2.0 || w[1].y < -EDGE_GAP * 2.0 {
                assert!(!(w[0].is_lit() && w[1].is_lit()), "no lit segment below the horizon: {w:?}");
            }
        }
    }

    #[test]
    fn colour_gain_and_minimum_diode_level() {
        let m = Mask::new(&[], Horizon::default(), [0.5, 1.0, 0.0], 0.0);
        let (out, _) = m.apply(&[white(0.0, 0.0)]);
        assert_eq!((out[0].r, out[0].g, out[0].b), (0.5, 1.0, 0.0));
        let m = Mask::new(&[], Horizon::default(), [1.0; 3], 0.1);
        let (out, _) = m.apply(&[Point::lit(0.0, 0.0, 0.5, 0.0, 1.0)]);
        assert!((out[0].r - 0.55).abs() < 1e-6);
        assert_eq!(out[0].g, 0.0, "a dark channel stays dark");
        assert_eq!(out[0].b, 1.0);
        // A blank zone stays blank whatever the minimum level.
        let m = Mask::new(&[rect(-1.0, -1.0, 1.0, 1.0, ZoneKind::Blank)], Horizon::default(), [1.0; 3], 0.3);
        assert!(!m.apply(&[white(0.0, 0.0)]).0[0].is_lit());
        // Out-of-range values are clamped (gain can only dim).
        let m = Mask::new(&[], Horizon::default(), [3.0, f32::NAN, -1.0], 2.0);
        assert_eq!(m.gain, [1.0, 1.0, 0.0]);
        assert_eq!(m.min_level, 0.5);
    }

    #[test]
    fn nothing_configured_is_the_identity() {
        let m = Mask::new(&[], Horizon::default(), [1.0; 3], 0.0);
        assert!(m.is_identity());
        let frame = vec![white(0.1, -0.9), Point::blanked(0.0, 0.0)];
        assert_eq!(m.apply(&frame), (frame, 0));
    }

    #[test]
    fn random_zones_never_leave_light_inside() {
        let r = |seed: u64, i: u64| crate::beat::seeded_rand(seed, i) * 2.0 - 1.0;
        for seed in 0..200u64 {
            let zones: Vec<Zone> = (0..3)
                .map(|z| {
                    let n = 3 + (seed + z) as usize % 6;
                    let (cx, cy, s) = (r(seed, z * 100), r(seed, z * 100 + 1), 0.2 + 0.3 * r(seed, z * 100 + 2).abs());
                    // A star-ish (often concave) polygon around (cx, cy).
                    let points = (0..n)
                        .map(|k| {
                            let a = k as f32 / n as f32 * std::f32::consts::TAU;
                            let rad = s * (0.4 + 0.6 * r(seed, z * 100 + 10 + k as u64).abs());
                            [(cx + rad * a.cos()).clamp(-1.0, 1.0), (cy + rad * a.sin()).clamp(-1.0, 1.0)]
                        })
                        .collect();
                    Zone { points, ..Default::default() }
                })
                .collect();
            // Random polyline with long lit segments and some dark samples.
            let frame: Vec<Point> = (0..60u64)
                .map(|i| {
                    let (x, y) = (r(seed + 1000, i * 2), r(seed + 1000, i * 2 + 1));
                    if r(seed + 2000, i) > 0.7 {
                        Point::blanked(x, y)
                    } else {
                        white(x, y)
                    }
                })
                .collect();
            let (out, _) = mask(&zones).apply(&frame);
            assert!(lit_inside(&out, &zones).is_empty(), "seed {seed}");
            assert_eq!(lit_segments_inside(&out, &zones), 0, "seed {seed}");
        }
    }

    #[test]
    fn two_thousand_points_with_eight_zones_fit_in_a_frame() {
        let zones: Vec<Zone> = (0..MAX_ZONES)
            .map(|z| {
                let (cx, cy) = (-0.8 + 0.22 * z as f32, 0.1 * z as f32 - 0.4);
                let points = (0..MAX_VERTICES)
                    .map(|k| {
                        let a = k as f32 / MAX_VERTICES as f32 * std::f32::consts::TAU;
                        let rad = if k % 2 == 0 { 0.2 } else { 0.1 };
                        [cx + rad * a.cos(), cy + rad * a.sin()]
                    })
                    .collect();
                Zone { points, ..Default::default() }
            })
            .collect();
        let m = Mask::new(&zones, Horizon { lines: true, ..Default::default() }, [0.9; 3], 0.05);
        // A dense spiral over the whole field.
        let frame: Vec<Point> = (0..2000)
            .map(|i| {
                let t = i as f32 / 2000.0;
                let a = t * 60.0;
                white(t * a.cos(), t * a.sin())
            })
            .collect();
        let (out, _) = m.apply(&frame);
        assert!(lit_inside(&out, &zones).is_empty());
        let start = std::time::Instant::now();
        let runs = 20;
        for _ in 0..runs {
            std::hint::black_box(m.apply(std::hint::black_box(&frame)));
        }
        let per_frame = start.elapsed() / runs;
        // 16.7 ms per frame at 60 fps; the stage must be a small part of it,
        // even in an unoptimised test build.
        assert!(per_frame.as_secs_f64() < 0.008, "{per_frame:?} per frame");
        eprintln!("mask: {per_frame:?} per 2000-point frame (8 zones × 24 vertices)");
    }
}
