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
//! **Soft edges and gradients** (T-446, Dim zones only; a Blank zone always
//! keeps a hard edge). A `feather` grows a band *outwards* from the
//! polygon in which the factor ramps linearly from the nearest edge
//! point's level up to 1 at the band's outer limit: it only darkens more
//! around the zone, and inside the level never exceeds the configured one.
//! A `gradient` varies the level inside linearly along `from → to`, from
//! `level` to `level_to` (clamped beyond both ends). Segments are cut where
//! they enter or leave a band, and get extra samples every `FEATHER_STEP`
//! where the factor varies, so the brightness follows the ramp.
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
/// Widest soft edge (T-446), in output units: 25 % of the field width.
pub const MAX_FEATHER: f32 = 0.5;
/// Longest step between samples inside a soft edge or a gradient (T-446),
/// so that the brightness follows the ramp instead of jumping.
pub const FEATHER_STEP: f32 = 0.02;
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
    /// A « Public » zone: where the audience is (T-255, `audience.rs`).
    /// Outside audience scanning mode it is blanked whatever its kind.
    pub audience: bool,
    /// Width of the soft edge, outwards from the polygon, in output units
    /// (0..`MAX_FEATHER`, T-446). Dim zones only: a Blank zone stays hard.
    pub feather: f32,
    /// A gradient inside the zone (T-446). Dim zones only.
    pub gradient: Option<ZoneGradient>,
}

impl Default for Zone {
    fn default() -> Self {
        Self { id: 0, name: "Zone".into(), kind: ZoneKind::Blank, level: 0.3, points: Vec::new(), audience: false, feather: 0.0, gradient: None }
    }
}

/// A linear gradient inside a Dim zone (T-446): the level is the zone's
/// `level` at `from` and before, `level_to` at `to` and after, along the
/// axis `from → to` (output coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneGradient {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub level_to: f32,
}

impl Default for ZoneGradient {
    fn default() -> Self {
        Self { from: [0.0, 1.0], to: [0.0, -1.0], level_to: 0.2 }
    }
}

impl ZoneGradient {
    /// `from` and `to` are distinct finite points (otherwise the gradient
    /// is ignored).
    pub fn usable(&self) -> bool {
        let finite = self.from.iter().chain(&self.to).all(|v| v.is_finite());
        finite && (self.to[0] - self.from[0]).hypot(self.to[1] - self.from[1]) > 1e-6
    }
}

impl Zone {
    /// The highest brightness factor inside the zone: its level, or with a
    /// gradient the larger of its two levels.
    pub fn factor(&self) -> f32 {
        match self.kind {
            ZoneKind::Blank => 0.0,
            ZoneKind::Dim => match self.soft_gradient() {
                Some(g) => unit(self.level, 0.0).max(unit(g.level_to, 0.0)),
                None => unit(self.level, 0.0),
            },
        }
    }

    /// The soft edge's width in effect: 0 for a Blank zone or a bad value.
    pub fn soft_feather(&self) -> f32 {
        if self.kind == ZoneKind::Dim && self.feather.is_finite() && self.feather > 0.0 {
            self.feather.min(MAX_FEATHER)
        } else {
            0.0
        }
    }

    /// The gradient in effect: none for a Blank zone or a degenerate axis.
    pub fn soft_gradient(&self) -> Option<ZoneGradient> {
        self.gradient.filter(|g| self.kind == ZoneKind::Dim && g.usable())
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

/// A compiled gradient: `level → level_to` along `from → to`, clamped.
#[derive(Clone, Copy)]
struct Grad {
    from: (f32, f32),
    /// (to - from) / |to - from|².
    axis: (f32, f32),
    level: f32,
    level_to: f32,
}

impl Grad {
    fn at(&self, x: f32, y: f32) -> f32 {
        let t = ((x - self.from.0) * self.axis.0 + (y - self.from.1) * self.axis.1).clamp(0.0, 1.0);
        let f = self.level + (self.level_to - self.level) * t;
        // Never outside the two levels, whatever the rounding.
        f.clamp(self.level.min(self.level_to), self.level.max(self.level_to))
    }
}

/// Where a point is relative to one zone (T-446).
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Out,
    /// In the soft edge band, outside the polygon.
    Band,
    /// Inside or on an edge.
    In,
}

struct Poly {
    factor: f32,
    v: Vec<(f32, f32)>,
    min: (f32, f32),
    max: (f32, f32),
    /// Soft edge width (T-446), 0 = hard.
    feather: f32,
    grad: Option<Grad>,
}

impl Poly {
    fn new(zone: &Zone) -> Option<Self> {
        if zone.points.len() < 3 {
            return None;
        }
        let v: Vec<(f32, f32)> = zone.points.iter().map(|p| (p[0], p[1])).collect();
        let min = v.iter().fold((f32::INFINITY, f32::INFINITY), |m, p| (m.0.min(p.0), m.1.min(p.1)));
        let max = v.iter().fold((f32::NEG_INFINITY, f32::NEG_INFINITY), |m, p| (m.0.max(p.0), m.1.max(p.1)));
        let grad = zone.soft_gradient().map(|g| {
            let d = (g.to[0] - g.from[0], g.to[1] - g.from[1]);
            let len2 = d.0 * d.0 + d.1 * d.1;
            Grad { from: (g.from[0], g.from[1]), axis: (d.0 / len2, d.1 / len2), level: unit(zone.level, 0.0), level_to: unit(g.level_to, 0.0) }
        });
        Some(Self { factor: zone.factor(), v, min, max, feather: zone.soft_feather(), grad })
    }

    /// Has a soft edge or a gradient: its factor varies continuously.
    fn is_soft(&self) -> bool {
        self.feather > 0.0 || self.grad.is_some()
    }

    /// The factor inside (or on the edge) at (x, y).
    fn core(&self, x: f32, y: f32) -> f32 {
        match &self.grad {
            Some(g) => g.at(x, y),
            None => self.factor,
        }
    }

    /// Within reach of the zone or its soft edge (bounding box grown by
    /// the feather and `ON_EDGE`).
    fn near(&self, x: f32, y: f32) -> bool {
        let f = self.feather + ON_EDGE;
        x >= self.min.0 - f && x <= self.max.0 + f && y >= self.min.1 - f && y <= self.max.1 + f
    }

    /// Distance from (x, y) to the outline, and the nearest outline point.
    fn nearest(&self, x: f32, y: f32) -> (f32, (f32, f32)) {
        let mut best = (f32::INFINITY, (x, y));
        let mut j = self.v.len() - 1;
        for i in 0..self.v.len() {
            let (p, q) = (self.v[j], self.v[i]);
            let e = (q.0 - p.0, q.1 - p.1);
            let w = (x - p.0, y - p.1);
            let len2 = e.0 * e.0 + e.1 * e.1;
            let s = if len2 > 0.0 { ((w.0 * e.0 + w.1 * e.1) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let n = (p.0 + s * e.0, p.1 + s * e.1);
            let d2 = (x - n.0).powi(2) + (y - n.1).powi(2);
            if d2 < best.0 {
                best = (d2, n);
            }
            j = i;
        }
        (best.0.sqrt(), best.1)
    }

    /// This zone at (x, y) (T-446): its factor and where the point is. The
    /// core inside or on an edge; in the soft edge band outside, a linear
    /// ramp from the nearest outline point's core up to 1 at `feather`; 1
    /// beyond. Never above 1, and inside never above the core.
    fn probe(&self, x: f32, y: f32) -> (f32, Place) {
        if !self.near(x, y) {
            return (1.0, Place::Out);
        }
        if self.covers(x, y) {
            return (self.core(x, y), Place::In);
        }
        if self.feather > 0.0 {
            let (d, n) = self.nearest(x, y);
            if d < self.feather {
                let base = self.core(n.0, n.1);
                return ((base + (1.0 - base) * d / self.feather).clamp(base, 1.0), Place::Band);
            }
        }
        (1.0, Place::Out)
    }

    fn factor_at(&self, x: f32, y: f32) -> f32 {
        self.probe(x, y).0
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

/// The audience (« Public ») zones as polygons (T-255): where the static
/// beam guard applies its strict rules (dwell.rs).
#[derive(Default)]
pub struct AudienceArea {
    polys: Vec<Poly>,
}

impl AudienceArea {
    pub fn new(zones: &[Zone]) -> Self {
        Self { polys: zones.iter().filter(|z| z.audience).filter_map(Poly::new).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.polys.is_empty()
    }

    /// (x, y) is inside a Public zone or on its edge.
    pub fn covers(&self, x: f32, y: f32) -> bool {
        self.polys.iter().any(|p| p.covers(x, y))
    }

    /// The rectangle (x0, y0)–(x1, y1) touches a Public zone: a corner of
    /// one inside the other, or two edges crossing.
    pub fn overlaps_rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        self.polys.iter().any(|p| {
            if p.max.0 < x0 || p.min.0 > x1 || p.max.1 < y0 || p.min.1 > y1 {
                return false;
            }
            if corners.iter().any(|&(x, y)| p.covers(x, y)) || p.v.iter().any(|&(x, y)| (x0..=x1).contains(&x) && (y0..=y1).contains(&y)) {
                return true;
            }
            let mut ts = Vec::new();
            (0..4).any(|i| {
                ts.clear();
                p.crossings(corners[i], corners[(i + 1) % 4], &mut ts);
                !ts.is_empty()
            })
        })
    }
}

/// The compiled zones, horizon and colour calibration for one frame.
pub struct Mask {
    polys: Vec<Poly>,
    /// Some zone has a soft edge or a gradient (T-446).
    soft: bool,
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
        let polys: Vec<Poly> = zones.iter().filter_map(Poly::new).collect();
        Self {
            soft: polys.iter().any(Poly::is_soft),
            polys,
            horizon,
            gain: gain.map(|g| unit(g, 1.0)),
            min_level: unit(min_level, 0.0).min(0.5),
        }
    }

    /// True when the mask changes nothing.
    pub fn is_identity(&self) -> bool {
        self.polys.is_empty() && !self.horizon.lines && self.gain == [1.0; 3] && self.min_level == 0.0
    }

    /// Brightness factor at (x, y): horizon × every zone containing it,
    /// or whose soft edge reaches it (T-446). A point on a zone's edge
    /// counts as inside (T-351).
    pub fn factor(&self, x: f32, y: f32) -> f32 {
        let mut f = self.horizon.factor(y);
        for p in &self.polys {
            if f == 0.0 {
                break;
            }
            f *= p.factor_at(x, y);
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
        let mut soft = Vec::new();
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
                    self.soft_steps(a, b, &mut soft);
                    if !ts.is_empty() || !soft.is_empty() {
                        self.split(a, b, &ts, &soft, &mut out);
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

    /// The parameters t in (0, 1) of the extra samples on a→b where a soft
    /// edge or a gradient makes the factor vary (T-446): steps of at most
    /// `FEATHER_STEP` there, plus the points where the segment enters or
    /// leaves a soft edge band. Empty without soft zones.
    fn soft_steps(&self, a: &Point, b: &Point, out: &mut Vec<f32>) {
        out.clear();
        if !self.soft {
            return;
        }
        let len = (b.x - a.x).hypot(b.y - a.y);
        if len.is_nan() || len <= FEATHER_STEP {
            return;
        }
        let (lo, hi) = ((a.x.min(b.x), a.y.min(b.y)), (a.x.max(b.x), a.y.max(b.y)));
        let polys: Vec<&Poly> = self
            .polys
            .iter()
            .filter(|p| {
                let f = p.feather;
                p.is_soft() && !(hi.0 < p.min.0 - f || lo.0 > p.max.0 + f || hi.1 < p.min.1 - f || lo.1 > p.max.1 + f)
            })
            .collect();
        if polys.is_empty() {
            return;
        }
        let at = |t: f32| (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        // Which bands (bit per zone) hold the point at t, and whether the
        // factor varies there (a band, or inside a gradient).
        let probe = |t: f32| {
            let (x, y) = at(t);
            polys.iter().enumerate().fold((0u32, false), |(m, soft), (i, p)| match p.probe(x, y).1 {
                Place::Band => (m | 1 << (i % 32), true),
                Place::In => (m, soft || p.grad.is_some()),
                Place::Out => (m, soft),
            })
        };
        let bands = |t: f32| probe(t).0;
        let n = (len / FEATHER_STEP).ceil() as usize;
        let tj = |j: usize| j as f32 / n as f32;
        let probes: Vec<(u32, bool)> = (0..=n).map(|j| probe(tj(j))).collect();
        let soft: Vec<bool> = probes.iter().map(|p| p.1).collect();
        let mut prev = probes[0].0;
        for j in 1..=n {
            let here = probes[j].0;
            // A band's outer limit (or a band handing over to another)
            // between two steps: bisect for it.
            if here != prev {
                let (mut t0, mut t1) = (tj(j - 1), tj(j));
                for _ in 0..16 {
                    let m = (t0 + t1) / 2.0;
                    if bands(m) == prev {
                        t0 = m;
                    } else {
                        t1 = m;
                    }
                }
                out.push((t0 + t1) / 2.0);
            }
            if j < n && (soft[j - 1] || soft[j] || soft[j + 1]) {
                out.push(tj(j));
            }
            prev = here;
        }
        out.retain(|&t| t > 0.0 && t < 1.0);
        out.sort_by(|x, y| x.total_cmp(y));
        out.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
    }

    /// Inserts the samples around each crossing of a→b (the segment takes
    /// b's colour; a is already emitted, b is emitted by the caller), and
    /// the soft edge / gradient samples `soft` between them, each at the
    /// factor of its own position (T-446).
    fn split(&self, a: &Point, b: &Point, ts: &[f32], soft: &[f32], out: &mut Vec<Point>) {
        let at = |t: f32| (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let gap = if len > 0.0 { EDGE_GAP / len } else { 0.0 };
        // Interval k runs from bounds[k] to bounds[k+1]; its level is taken
        // at its middle (no edge inside an interval, so it is constant). A
        // crossing may be at t = 0 or 1, making an empty first/last interval
        // whose level is the end's own. With soft zones the level varies
        // inside an interval: it is then taken right next to the crossing.
        let bound = |k: usize| if k == 0 { 0.0 } else if k > ts.len() { 1.0 } else { ts[k - 1] };
        let level = |k: usize| {
            let (x, y) = at((bound(k) + bound(k + 1)) / 2.0);
            self.factor(x, y)
        };
        // The soft samples strictly between parameters `from` and `to`.
        let mut next_soft = 0;
        let mut soft_until = |from: f32, to: f32, out: &mut Vec<Point>| {
            while next_soft < soft.len() && soft[next_soft] < to - 1e-6 {
                let t = soft[next_soft];
                if t > from + 1e-6 {
                    let (x, y) = at(t);
                    out.push(self.sample(x, y, b, self.factor(x, y)));
                }
                next_soft += 1;
            }
        };
        let mut last = 0.0;
        let mut before = level(0);
        for (k, &t) in ts.iter().enumerate() {
            let lo_t = t - gap.min((t - bound(k)) / 2.0);
            let hi_t = t + gap.min((bound(k + 2) - t) / 2.0);
            let (lo, hi) = (at(lo_t), at(hi_t));
            let mut after = level(k + 1);
            if self.soft {
                before = self.factor(lo.0, lo.1);
                after = self.factor(hi.0, hi.1);
            }
            // The edge itself can be darker than both sides: a segment that
            // only touches a zone (a corner, or a vertex of a concave one).
            let (x, y) = at(t);
            let low = before.min(after).min(self.factor(x, y));
            if before > low || after > low {
                soft_until(last, lo_t, out);
                let mut emit = |(x, y): (f32, f32), f: f32| out.push(self.sample(x, y, b, f.min(self.factor(x, y))));
                emit(lo, before);
                if before > low {
                    emit(lo, low);
                }
                if after > low {
                    emit(hi, low);
                }
                emit(hi, after);
                last = hi_t;
            }
            before = after;
        }
        soft_until(last, 1.0, out);
    }
}

/// Zone outline tracing (T-447): how far outside a zone its outline is
/// drawn, in output units.
pub const TRACE_MARGIN: f32 = 0.02;
/// The trace's only colour level (green), fixed and low: no look, master,
/// LFO or MIDI changes it. The output's power caps can only lower it.
pub const TRACE_LEVEL: f32 = 0.10;
/// The verification mode ends by itself after this long.
pub const TRACE_SECONDS: u64 = 60;
/// Longest step between two samples of a traced outline. Half of it is
/// well under the margin, so a lit step between two samples that are each
/// at least the margin away from a Blank zone never reaches it.
const TRACE_STEP: f32 = 0.01;
/// Dash length on a Dim zone's outline.
const TRACE_DASH: f32 = 0.05;
/// Lit samples are repeated this many times where a stroke starts or ends.
const TRACE_DWELL: usize = 3;

/// The outlines of the zones and the horizon to check in the room (T-447),
/// in output coordinates: each Blank zone's outline offset `TRACE_MARGIN`
/// outwards, each Dim zone's the same in dashes, and with `only == None`
/// a line `TRACE_MARGIN` above the horizon. Green at `TRACE_LEVEL`,
/// joined by blanked travel, densified, within -1..1.
///
/// Every sample is checked: a sample inside a Blank or a Public zone, or
/// closer than the margin to one (the offset of a concave corner, two
/// zones side by side, a point pulled back into the field), is dark, and
/// a lit step joins only two lit samples at most a step apart. The output
/// safety stage still applies to the result like to any frame.
pub fn trace_frame(zones: &[Zone], horizon: Horizon, only: Option<u32>) -> Vec<Point> {
    let forbidden: Vec<Poly> = zones.iter().filter(|z| z.kind == ZoneKind::Blank || z.audience).filter_map(Poly::new).collect();
    // Small float tolerance on the margin: the samples sit at exactly the margin.
    let clear = |x: f32, y: f32| {
        x.is_finite() && y.is_finite() && forbidden.iter().all(|p| !p.covers(x, y) && p.nearest(x, y).0 >= TRACE_MARGIN * 0.99)
    };
    let mut pen = TracePen::default();
    for zone in zones.iter().filter(|z| only.is_none_or(|id| z.id == id)) {
        let dashed = zone.kind == ZoneKind::Dim && !zone.audience;
        for (x, y, on) in outline_samples(zone, dashed) {
            let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
            pen.sample(x, y, on && clear(x, y));
        }
        pen.lift();
    }
    if only.is_none() && horizon.y.is_finite() {
        let y = horizon.y + TRACE_MARGIN;
        if y <= 1.0 {
            let y = y.max(-1.0);
            let n = (2.0 / TRACE_STEP).ceil() as usize;
            for k in 0..=n {
                let x = -1.0 + 2.0 * k as f32 / n as f32;
                pen.sample(x, y, clear(x, y));
            }
            pen.lift();
        }
    }
    crate::engine::densify(&pen.out)
}

/// The samples of a zone's outline offset `TRACE_MARGIN` outwards, closed,
/// with round joins at the convex corners: (x, y, wanted lit).
fn outline_samples(zone: &Zone, dashed: bool) -> Vec<(f32, f32, bool)> {
    let v: Vec<(f32, f32)> = zone.points.iter().map(|p| (p[0], p[1])).collect();
    if v.len() < 3 || v.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return Vec::new();
    }
    // Twice the signed area: > 0 counter-clockwise.
    let area: f32 = (0..v.len()).map(|i| {
        let (p, q) = (v[i], v[(i + 1) % v.len()]);
        p.0 * q.1 - q.0 * p.1
    }).sum();
    if area.abs() < 1e-9 {
        return Vec::new();
    }
    let side = area.signum();
    // Outward unit normal of each edge (none for a zero-length edge).
    let normals: Vec<Option<(f32, f32)>> = (0..v.len())
        .map(|i| {
            let (p, q) = (v[i], v[(i + 1) % v.len()]);
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            let len = dx.hypot(dy);
            (len > 1e-6).then(|| (side * dy / len, -side * dx / len))
        })
        .collect();
    let m = TRACE_MARGIN;
    let mut out: Vec<(f32, f32)> = Vec::new();
    for i in 0..v.len() {
        let Some(n) = normals[i] else { continue };
        let (p, q) = (v[i], v[(i + 1) % v.len()]);
        let len = (q.0 - p.0).hypot(q.1 - p.1);
        let steps = (len / TRACE_STEP).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            out.push((p.0 + (q.0 - p.0) * t + n.0 * m, p.1 + (q.1 - p.1) * t + n.1 * m));
        }
        // Round join around q towards the next edge's normal, at a convex
        // corner only (a concave one's offsets cross; its samples are
        // checked and darkened by the caller).
        if let Some(nn) = (1..v.len()).find_map(|j| normals[(i + j) % v.len()]) {
            let turn = (n.0 * nn.1 - n.1 * nn.0).atan2(n.0 * nn.0 + n.1 * nn.1);
            if turn * side > 0.0 {
                let a0 = n.1.atan2(n.0);
                let k_max = ((m * turn.abs()) / TRACE_STEP).ceil() as usize;
                for k in 1..k_max {
                    let a = a0 + turn * k as f32 / k_max as f32;
                    out.push((q.0 + m * a.cos(), q.1 + m * a.sin()));
                }
            }
        }
    }
    if let Some(&first) = out.first() {
        out.push(first);
    }
    let mut s = 0.0;
    let mut prev: Option<(f32, f32)> = None;
    out.into_iter()
        .map(|(x, y)| {
            if let Some(p) = prev {
                s += (x - p.0).hypot(y - p.1);
            }
            prev = Some((x, y));
            let on = !dashed || ((s / TRACE_DASH) as u32).is_multiple_of(2);
            (x, y, on)
        })
        .collect()
}

/// Draws the traced samples: a lit sample is joined to the previous one
/// only when both are lit and close; everything else is blanked travel.
#[derive(Default)]
struct TracePen {
    out: Vec<Point>,
    /// The last lit sample of the stroke being drawn.
    last: Option<(f32, f32)>,
}

impl TracePen {
    fn sample(&mut self, x: f32, y: f32, lit: bool) {
        let near = self.last.is_some_and(|(px, py)| (x - px).hypot(y - py) <= TRACE_STEP * 1.5);
        if lit && near {
            self.out.push(Point::lit(x, y, 0.0, TRACE_LEVEL, 0.0));
            self.last = Some((x, y));
            return;
        }
        self.lift();
        if lit {
            // Dark travel to here, then the stroke starts with a dwell.
            self.out.extend(std::iter::repeat_n(Point::blanked(x, y), TRACE_DWELL));
            self.out.extend(std::iter::repeat_n(Point::lit(x, y, 0.0, TRACE_LEVEL, 0.0), TRACE_DWELL));
            self.last = Some((x, y));
        }
    }

    /// Ends the stroke being drawn: a dwell, then dark on the spot.
    fn lift(&mut self) {
        if let Some((x, y)) = self.last.take() {
            self.out.extend(std::iter::repeat_n(Point::lit(x, y, 0.0, TRACE_LEVEL, 0.0), TRACE_DWELL - 1));
            self.out.push(Point::blanked(x, y));
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

    /// T-255: the Public zones as an area, and rectangles touching it.
    #[test]
    fn the_audience_area_is_the_public_zones_only() {
        // A triangle: cases where no corner of either is inside the other.
        let tri = Zone { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]], audience: true, ..Default::default() };
        let other = rect(0.6, 0.6, 0.9, 0.9, ZoneKind::Blank);
        let area = AudienceArea::new(&[tri.clone(), other]);
        assert!(area.covers(0.0, 0.0) && area.covers(0.5, -0.5), "inside and on a vertex");
        assert!(!area.covers(0.7, 0.7), "a zone not marked Public is not in the area");
        assert!(area.overlaps_rect(-0.1, -0.1, 0.1, 0.1), "inside");
        assert!(area.overlaps_rect(-1.0, -1.0, 1.0, 1.0), "around it");
        assert!(area.overlaps_rect(-0.6, -0.1, 0.6, 0.0), "a band crossing two edges, no corner inside");
        assert!(!area.overlaps_rect(0.3, 0.3, 0.5, 0.5), "beside the slanted edge");
        assert!(!area.overlaps_rect(0.6, 0.6, 0.9, 0.9));
        assert!(AudienceArea::new(&[Zone { audience: false, ..tri }]).is_empty());
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

    fn soft(level: f32, feather: f32) -> Zone {
        Zone { level, feather, ..rect(-0.2, -0.2, 0.2, 0.2, ZoneKind::Dim) }
    }

    /// T-446: the core keeps the zone's level, the band ramps up to 1
    /// monotonically outwards, nothing changes beyond it.
    #[test]
    fn a_soft_edge_ramps_outwards_only() {
        let m = mask(&[soft(0.2, 0.2)]);
        assert!((m.factor(0.0, 0.0) - 0.2).abs() < 1e-6);
        assert!((m.factor(0.2, 0.0) - 0.2).abs() < 1e-6, "on the edge: the core");
        assert!((m.factor(0.3, 0.0) - 0.6).abs() < 1e-6, "half-way through the band");
        assert_eq!(m.factor(0.41, 0.0), 1.0);
        assert_eq!(m.factor(0.9, 0.9), 1.0);
        // Corner band: distance to the corner.
        let d = 0.1f32;
        assert!((m.factor(0.2 + d / 2f32.sqrt(), 0.2 + d / 2f32.sqrt()) - 0.6).abs() < 1e-4);
        // Inside never above the level; outwards never decreasing.
        let mut prev = 0.0;
        for k in 0..=200 {
            let x = -0.2 + 0.7 * k as f32 / 200.0;
            let f = m.factor(x, 0.05);
            if x <= 0.2 {
                assert!(f <= 0.2 + 1e-6, "{x}: {f}");
            }
            assert!(f >= prev - 1e-6, "monotone at {x}: {prev} → {f}");
            prev = f;
        }
        // A wider band only darkens.
        let wide = mask(&[soft(0.2, 0.4)]);
        for k in 0..100 {
            let x = k as f32 / 100.0;
            assert!(wide.factor(x, 0.1) <= m.factor(x, 0.1) + 1e-6, "{x}");
        }
    }

    #[test]
    fn a_gradient_interpolates_inside() {
        let g = ZoneGradient { from: [0.0, 0.5], to: [0.0, -0.5], level_to: 0.2 };
        let z = Zone { level: 1.0, gradient: Some(g), ..rect(-1.0, -0.5, 1.0, 0.5, ZoneKind::Dim) };
        let m = mask(std::slice::from_ref(&z));
        assert!((m.factor(0.3, 0.5) - 1.0).abs() < 1e-6, "at from");
        assert!((m.factor(-0.3, -0.5) - 0.2).abs() < 1e-6, "at to");
        assert!((m.factor(0.7, 0.0) - 0.6).abs() < 1e-6, "half-way");
        assert_eq!(m.factor(0.0, 0.8), 1.0, "outside, no soft edge");
        assert_eq!(z.factor(), 1.0, "the highest level inside");
        // Clamped beyond both ends, never outside the two levels.
        let g2 = ZoneGradient { from: [0.0, 0.2], to: [0.0, -0.2], level_to: 0.5 };
        let m = mask(&[Zone { level: 0.1, gradient: Some(g2), ..rect(-1.0, -0.5, 1.0, 0.5, ZoneKind::Dim) }]);
        assert!((m.factor(0.0, 0.4) - 0.1).abs() < 1e-6);
        assert!((m.factor(0.0, -0.4) - 0.5).abs() < 1e-6);
        // A degenerate axis is ignored: the plain level.
        let flat = ZoneGradient { from: [0.1, 0.1], to: [0.1, 0.1], level_to: 0.9 };
        let z = Zone { level: 0.3, gradient: Some(flat), ..rect(-1.0, -0.5, 1.0, 0.5, ZoneKind::Dim) };
        assert!((mask(std::slice::from_ref(&z)).factor(0.0, 0.0) - 0.3).abs() < 1e-6);
        assert_eq!(z.factor(), 0.3);
    }

    /// With a soft edge and a gradient, the band starts from the nearest
    /// edge point's level.
    #[test]
    fn a_soft_edge_starts_from_the_gradient_level() {
        let g = ZoneGradient { from: [0.0, 0.2], to: [0.0, -0.2], level_to: 0.2 };
        let m = mask(&[Zone { level: 1.0, feather: 0.2, gradient: Some(g), ..rect(-0.2, -0.2, 0.2, 0.2, ZoneKind::Dim) }]);
        assert!((m.factor(0.0, -0.3) - 0.6).abs() < 1e-5, "below: from 0.2");
        assert!((m.factor(0.3, 0.0) - 0.8).abs() < 1e-5, "right, half-way down: from 0.6");
        assert!((m.factor(0.0, 0.3) - 1.0).abs() < 1e-5, "above: from 1");
    }

    #[test]
    fn overlapping_soft_zones_multiply() {
        let a = soft(0.5, 0.2);
        let b = Zone { level: 0.5, feather: 0.1, ..rect(0.25, -0.2, 0.6, 0.2, ZoneKind::Dim) };
        let (ma, mb, both) = (mask(std::slice::from_ref(&a)), mask(std::slice::from_ref(&b)), mask(&[a, b]));
        for k in 0..50 {
            let x = -0.3 + k as f32 / 50.0;
            assert!((both.factor(x, 0.0) - ma.factor(x, 0.0) * mb.factor(x, 0.0)).abs() < 1e-6, "{x}");
        }
    }

    /// Blank zones ignore soft edges and gradients: the same frame out.
    #[test]
    fn a_blank_zone_stays_hard() {
        let hard = rect(-0.2, -0.2, 0.2, 0.2, ZoneKind::Blank);
        let g = ZoneGradient { from: [0.0, 0.2], to: [0.0, -0.2], level_to: 1.0 };
        let fuzzy = Zone { feather: 0.4, gradient: Some(g), ..hard.clone() };
        assert_eq!(fuzzy.soft_feather(), 0.0);
        assert!(fuzzy.soft_gradient().is_none());
        let frame = vec![white(-1.0, 0.0), white(1.0, 0.05), white(0.0, -1.0), white(0.1, 1.0)];
        assert_eq!(mask(std::slice::from_ref(&fuzzy)).apply(&frame), mask(&[hard]).apply(&frame));
        assert!(!mask(&[fuzzy]).soft);
    }

    /// A segment crossing a 0.2-wide band gets samples every 0.02 at
    /// most: the factor never jumps more than one step's worth.
    #[test]
    fn a_segment_through_a_band_follows_the_ramp() {
        let zones = [soft(0.2, 0.2)];
        let (out, _) = mask(&zones).apply(&[white(-1.0, 0.0), white(1.0, 0.0)]);
        assert!(out.len() > 20, "{}", out.len());
        // Along x, consecutive samples' brightness differs by at most one
        // step of the ramp (0.8 over 0.2, a step of 0.02: 0.08).
        let max_jump = 0.8 * FEATHER_STEP / 0.2 + 1e-3;
        for w in out.windows(2) {
            assert!(w[0].x <= w[1].x + 1e-6, "in order: {w:?}");
            assert!((w[0].r - w[1].r).abs() <= max_jump, "jump {w:?}");
        }
        // Cut at the outer limit of the band, both sides.
        for edge in [-0.4f32, 0.4] {
            assert!(out.iter().any(|p| (p.x - edge).abs() < 1e-4 && (p.r - 1.0).abs() < 1e-3), "{edge}: {out:?}");
        }
        // Inside never above the level.
        assert!(out.iter().filter(|p| p.x.abs() < 0.2).all(|p| p.r <= 0.2 + 1e-6));
    }

    /// A short segment (below one step) is left alone; a hard-only mask
    /// adds no sample for soft zones it doesn't have.
    #[test]
    fn soft_steps_only_where_the_factor_varies() {
        let m = mask(&[soft(0.2, 0.1)]);
        let (out, _) = m.apply(&[white(0.5, 0.5), white(0.9, 0.9)]);
        assert_eq!(out.len(), 2, "away from the band: {out:?}");
        let (out, _) = m.apply(&[white(0.25, 0.0), white(0.26, 0.0)]);
        assert_eq!(out.len(), 2, "shorter than a step: {out:?}");
    }

    /// Property (T-446): with soft edges (and gradients no brighter than
    /// the zone) the output is never brighter than without them, and Blank
    /// zones nearby keep nothing lit.
    #[test]
    fn soft_edges_never_brighten() {
        let r = |seed: u64, i: u64| crate::beat::seeded_rand(seed, i) * 2.0 - 1.0;
        for seed in 0..150u64 {
            let hard: Vec<Zone> = (0..4u64)
                .map(|z| {
                    let (cx, cy, s) = (r(seed, z * 100), r(seed, z * 100 + 1), 0.15 + 0.3 * r(seed, z * 100 + 2).abs());
                    let n = 3 + (seed + z) as usize % 5;
                    let points = (0..n)
                        .map(|k| {
                            let a = k as f32 / n as f32 * std::f32::consts::TAU;
                            let rad = s * (0.4 + 0.6 * r(seed, z * 100 + 10 + k as u64).abs());
                            [(cx + rad * a.cos()).clamp(-1.0, 1.0), (cy + rad * a.sin()).clamp(-1.0, 1.0)]
                        })
                        .collect();
                    let kind = if z % 2 == 0 { ZoneKind::Dim } else { ZoneKind::Blank };
                    Zone { kind, level: r(seed, z * 100 + 3).abs(), points, ..Default::default() }
                })
                .collect();
            let softened: Vec<Zone> = hard
                .iter()
                .enumerate()
                .map(|(i, z)| {
                    let i = i as u64;
                    let gradient = (r(seed, 500 + i) > 0.0).then(|| ZoneGradient {
                        from: [r(seed, 510 + i), r(seed, 520 + i)],
                        to: [r(seed, 530 + i), r(seed, 540 + i)],
                        level_to: z.level * r(seed, 550 + i).abs(),
                    });
                    Zone { feather: MAX_FEATHER * r(seed, 560 + i).abs(), gradient, ..z.clone() }
                })
                .collect();
            let frame: Vec<Point> = (0..40u64)
                .map(|i| {
                    let (x, y) = (r(seed + 1000, i * 2), r(seed + 1000, i * 2 + 1));
                    let v = r(seed + 3000, i).abs();
                    if r(seed + 2000, i) > 0.7 {
                        Point::blanked(x, y)
                    } else {
                        Point::lit(x, y, v, v, 1.0)
                    }
                })
                .collect();
            let (h, s) = (mask(&hard), mask(&softened));
            let (out, _) = s.apply(&frame);
            for p in &out {
                let peak = p.r.max(p.g).max(p.b);
                assert!(peak <= h.factor(p.x, p.y) + 1e-5, "seed {seed}: {p:?} brighter than without soft edges");
                assert!(peak <= s.factor(p.x, p.y) + 1e-5, "seed {seed}: {p:?} brighter than its own factor");
                assert!((-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y), "seed {seed}: {p:?}");
            }
            assert_eq!(blank_violations(&out, &softened), 0, "seed {seed}");
            assert!(lit_inside(&out, &softened).is_empty(), "seed {seed}");
            assert_eq!(lit_segments_inside(&out, &softened), 0, "seed {seed}");
        }
    }

    #[test]
    fn three_thousand_points_with_eight_soft_zones_fit_in_a_frame() {
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
                let gradient = Some(ZoneGradient { from: [cx, cy + 0.2], to: [cx, cy - 0.2], level_to: 0.1 });
                Zone { kind: ZoneKind::Dim, level: 0.5, feather: 0.1, gradient, points, ..Default::default() }
            })
            .collect();
        let m = Mask::new(&zones, Horizon { lines: true, ..Default::default() }, [0.9; 3], 0.05);
        let frame: Vec<Point> = (0..3000)
            .map(|i| {
                let t = i as f32 / 3000.0;
                let a = t * 60.0;
                white(t * a.cos(), t * a.sin())
            })
            .collect();
        // The best of 10 runs: the stage's own cost, not the machine's load
        // (other test binaries and studios share the CPU).
        let per_frame = (0..10)
            .map(|_| {
                let start = std::time::Instant::now();
                std::hint::black_box(m.apply(std::hint::black_box(&frame)));
                start.elapsed()
            })
            .min()
            .unwrap();
        // Under 1 ms in a release build; an unoptimised test build is
        // several times slower.
        assert!(per_frame.as_secs_f64() < 0.03, "{per_frame:?} per frame");
        eprintln!("soft mask: {per_frame:?} per 3000-point frame (8 soft zones × 24 vertices)");
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
        // The best of 20 runs: the stage's own cost, not the machine's load
        // (other test binaries and studios share the CPU).
        let per_frame = (0..20)
            .map(|_| {
                let start = std::time::Instant::now();
                std::hint::black_box(m.apply(std::hint::black_box(&frame)));
                start.elapsed()
            })
            .min()
            .unwrap();
        // 16.7 ms per frame at 60 fps; the stage must be a small part of it,
        // even in an unoptimised test build.
        assert!(per_frame.as_secs_f64() < 0.008, "{per_frame:?} per frame");
        eprintln!("mask: {per_frame:?} per 2000-point frame (8 zones × 24 vertices)");
    }

    // ---- T-447: tracing the zones for verification ----

    /// The lit samples of a trace.
    fn lit_of(frame: &[Point]) -> Vec<Point> {
        frame.iter().copied().filter(Point::is_lit).collect()
    }

    /// Every lit sample is green at `TRACE_LEVEL`, every sample in the field.
    fn assert_trace_shape(frame: &[Point]) {
        for p in frame {
            assert!((-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y), "{p:?}");
            if p.is_lit() {
                assert_eq!((p.r, p.g, p.b), (0.0, TRACE_LEVEL, 0.0), "{p:?}");
            }
        }
    }

    /// Lit samples or lit segments closer than `d` to a Blank or Public zone.
    fn too_close(frame: &[Point], zones: &[Zone], d: f32) -> usize {
        let polys: Vec<Poly> = zones.iter().filter(|z| z.kind == ZoneKind::Blank || z.audience).filter_map(Poly::new).collect();
        let near = |x: f32, y: f32| polys.iter().any(|z| z.covers(x, y) || z.nearest(x, y).0 < d);
        frame
            .windows(2)
            .filter(|w| w[0].is_lit() || w[1].is_lit())
            .filter(|w| (0..=10).any(|s| {
                let t = s as f32 / 10.0;
                near(w[0].x + (w[1].x - w[0].x) * t, w[0].y + (w[1].y - w[0].y) * t)
            }))
            .count()
    }

    #[test]
    fn trace_of_a_square_runs_just_outside_it() {
        let zones = [rect(-0.3, -0.3, 0.3, 0.3, ZoneKind::Blank)];
        let f = trace_frame(&zones, Horizon::default(), Some(0));
        assert_trace_shape(&f);
        assert_eq!(blank_violations(&f, &zones), 0);
        let lit = lit_of(&f);
        assert!(lit.len() > 200, "the whole outline: {}", lit.len());
        let poly = Poly::new(&zones[0]).unwrap();
        for p in &lit {
            let d = poly.nearest(p.x, p.y).0;
            assert!(!poly.covers(p.x, p.y) && (d - TRACE_MARGIN).abs() < 1e-3, "{p:?} at {d}");
        }
        // Round corners: the outline passes right of, above, left of and
        // below the square, and around its corners.
        for (x, y) in [(0.32, 0.0), (0.0, 0.32), (-0.32, 0.0), (0.0, -0.32), (0.3 + TRACE_MARGIN * 0.707, 0.3 + TRACE_MARGIN * 0.707)] {
            assert!(lit.iter().any(|p| (p.x - x).hypot(p.y - y) < 0.006), "near ({x}, {y})");
        }
        // Clockwise the same way.
        let cw = Zone { points: zones[0].points.iter().rev().copied().collect(), ..zones[0].clone() };
        let g = trace_frame(std::slice::from_ref(&cw), Horizon::default(), Some(0));
        assert_eq!(blank_violations(&g, &[cw]), 0);
        assert!(lit_of(&g).len() > 200);
    }

    #[test]
    fn trace_of_a_concave_zone_never_enters_it() {
        // An L, and a deep U whose arms are closer than twice the margin.
        let l = Zone { points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.0], [0.0, 0.0], [0.0, 0.5], [-0.5, 0.5]], ..Default::default() };
        let u = Zone { id: 1, points: vec![[-0.4, -0.4], [0.4, -0.4], [0.4, 0.4], [0.015, 0.4], [0.015, -0.3], [-0.015, -0.3], [-0.015, 0.4], [-0.4, 0.4]], ..Default::default() };
        for z in [l, u] {
            let zones = [z];
            let f = trace_frame(&zones, Horizon::default(), None);
            assert_trace_shape(&f);
            assert_eq!(blank_violations(&f, &zones), 0);
            assert_eq!(too_close(&f, &zones, TRACE_MARGIN * 0.5), 0);
            assert!(lit_of(&f).len() > 100);
        }
    }

    #[test]
    fn trace_of_two_zones_side_by_side_lights_neither() {
        // A gap narrower than twice the margin: nothing can be traced in it.
        let zones = [rect(-0.5, -0.3, -0.01, 0.3, ZoneKind::Blank), Zone { id: 1, ..rect(0.01, -0.3, 0.5, 0.3, ZoneKind::Blank) }];
        let f = trace_frame(&zones, Horizon::default(), None);
        assert_trace_shape(&f);
        assert_eq!(blank_violations(&f, &zones), 0);
        assert_eq!(too_close(&f, &zones, TRACE_MARGIN * 0.5), 0);
        assert!(!lit_of(&f).iter().any(|p| p.x.abs() < 0.01 && p.y.abs() < 0.3), "nothing lit in the gap");
        // One zone at a time: the other one still forbids its own area.
        let one = trace_frame(&zones, Horizon::default(), Some(0));
        assert_eq!(blank_violations(&one, &zones), 0);
        assert!(lit_of(&one).iter().all(|p| p.x < 0.0), "only the left zone");
    }

    #[test]
    fn trace_at_the_edge_of_the_field_stays_inside_it() {
        let zones = [rect(0.6, 0.6, 1.0, 1.0, ZoneKind::Blank), Zone { id: 1, ..rect(-1.0, -1.0, -0.5, 1.0, ZoneKind::Blank) }];
        let f = trace_frame(&zones, Horizon::default(), None);
        assert_trace_shape(&f);
        assert_eq!(blank_violations(&f, &zones), 0);
        assert!(lit_of(&f).iter().any(|p| (p.x - 0.58).abs() < 0.005 && p.y > 0.7), "its inner side is drawn");
    }

    #[test]
    fn trace_of_a_dim_zone_is_dashed_and_a_public_zone_is_forbidden() {
        let dim = Zone { kind: ZoneKind::Dim, ..rect(-0.3, -0.3, 0.3, 0.3, ZoneKind::Dim) };
        // Strokes: runs of lit samples.
        let strokes = |f: &[Point]| f.windows(2).filter(|w| !w[0].is_lit() && w[1].is_lit()).count();
        let dashed = strokes(&trace_frame(&[dim], Horizon::default(), Some(0)));
        let solid = strokes(&trace_frame(&[rect(-0.3, -0.3, 0.3, 0.3, ZoneKind::Blank)], Horizon::default(), Some(0)));
        assert_eq!(solid, 1);
        assert!(dashed > 15, "dashes: {dashed}");
        // A Dim zone marked Public is as forbidden as a Blank one.
        let public = Zone { kind: ZoneKind::Dim, audience: true, ..rect(-0.3, -0.3, 0.3, 0.3, ZoneKind::Dim) };
        let near = Zone { id: 1, kind: ZoneKind::Dim, ..rect(-0.29, 0.25, 0.29, 0.6, ZoneKind::Dim) };
        let zones = [public.clone(), near];
        let f = trace_frame(&zones, Horizon::default(), None);
        let as_blank = [Zone { kind: ZoneKind::Blank, ..public }];
        assert_eq!(blank_violations(&f, &as_blank), 0);
    }

    #[test]
    fn trace_of_the_horizon_is_a_line_just_above_it() {
        let f = trace_frame(&[], Horizon { y: -0.4, ..Default::default() }, None);
        assert_trace_shape(&f);
        let lit = lit_of(&f);
        assert!(lit.len() > 150);
        assert!(lit.iter().all(|p| (p.y - (-0.4 + TRACE_MARGIN)).abs() < 1e-6));
        assert!(lit.iter().any(|p| p.x <= -0.99) && lit.iter().any(|p| p.x >= 0.99));
        // A horizon at the top of the field: nothing to draw above it.
        assert!(lit_of(&trace_frame(&[], Horizon { y: 1.0, ..Default::default() }, None)).is_empty());
        // One zone selected: no horizon.
        assert!(lit_of(&trace_frame(&[], Horizon::default(), Some(3))).is_empty());
        // The horizon line breaks around a Blank zone it would cross.
        let zones = [rect(-0.2, -0.5, 0.2, 0.0, ZoneKind::Blank)];
        let f = trace_frame(&zones, Horizon { y: -0.4, ..Default::default() }, None);
        assert_eq!(blank_violations(&f, &zones), 0);
    }

    #[test]
    fn random_zone_traces_never_light_a_blank_zone() {
        let r = |seed: u64, i: u64| crate::beat::seeded_rand(seed, i) * 2.0 - 1.0;
        for seed in 0..200u64 {
            let zones: Vec<Zone> = (0..4)
                .map(|z| {
                    let n = 3 + (seed + z) as usize % 8;
                    let (cx, cy, s) = (r(seed, z * 100), r(seed, z * 100 + 1), 0.1 + 0.4 * r(seed, z * 100 + 2).abs());
                    let points = (0..n)
                        .map(|k| {
                            let a = k as f32 / n as f32 * std::f32::consts::TAU;
                            let rad = s * (0.3 + 0.7 * r(seed, z * 100 + 10 + k as u64).abs());
                            [(cx + rad * a.cos()).clamp(-1.0, 1.0), (cy + rad * a.sin()).clamp(-1.0, 1.0)]
                        })
                        .collect();
                    let kind = if r(seed, z * 100 + 3) > 0.3 { ZoneKind::Dim } else { ZoneKind::Blank };
                    Zone { id: z as u32, points, kind, audience: r(seed, z * 100 + 4) > 0.6, ..Default::default() }
                })
                .collect();
            let only = (seed % 3 == 0).then_some((seed % 4) as u32);
            let f = trace_frame(&zones, Horizon { y: r(seed, 999), ..Default::default() }, only);
            assert_trace_shape(&f);
            let forbidden: Vec<Zone> = zones.iter().filter(|z| z.kind == ZoneKind::Blank || z.audience).map(|z| Zone { kind: ZoneKind::Blank, ..z.clone() }).collect();
            assert_eq!(blank_violations(&f, &forbidden), 0, "seed {seed}");
            assert_eq!(too_close(&f, &zones, TRACE_MARGIN * 0.5), 0, "seed {seed}");
            // And the mask, applied after, finds nothing to darken.
            let (_, darkened) = mask(&forbidden).apply(&f);
            assert_eq!(darkened, 0, "seed {seed}");
        }
    }
}
