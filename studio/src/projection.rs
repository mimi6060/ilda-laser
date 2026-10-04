//! Projection zones and geometric correction (T-012).
//!
//! A *projection zone* is a place the content can be drawn: an area of
//! the scan field with its own geometry (size, position, rotation, shear,
//! keystone, pincushion, bow and a 4-corner warp), an optional output it
//! belongs to, and a name. Each cue lists the zones it plays on
//! (`CueSlot::zones`, « Propriétés du cue »); a cue without zones plays
//! on the default zones. A cue on two zones is drawn twice, once through
//! each zone's geometry, joined by blanked travel.
//!
//! Where it sits: content → layers → master live controls → **zone
//! geometry** → calibration (the projector's alignment) → frame size →
//! safety stage (safety zones, horizon, strobe limiter, static-beam
//! guard, power caps) → gate. Both the zone geometry and the calibration
//! clamp to -1..1, so everything after them sees the final geometry.
//!
//! Safety zones (zones.rs) and the horizon are expressed in **output
//! space**, after the correction: they keep protecting the real audience
//! area whatever a zone's geometry does, and a geometry change can never
//! move a Blank zone. A zone has no safety settings of its own.
//!
//! The settings are machine settings, like the calibration
//! (`projection.json`, never in a project); a damaged file is a safety
//! file (datafile.rs): quarantined, the defaults used, and arming waits
//! for the operator's acknowledgement.

use crate::engine::Calibration;
use crate::layers::{self, MixReport, Mixer, LAYER_COUNT};
use crate::patterns::Point;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Zone ids are 1..=MAX_ZONES.
pub const MAX_ZONES: u8 = 8;

/// The scan field's corners, in warp order: bottom left, bottom right,
/// top right, top left (y up).
pub const SQUARE: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

/// 4-corner warp (« Warp 4 coins »): the field's corners go exactly to
/// `corners` (same order as `SQUARE`), straight lines stay straight.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Warp {
    pub enabled: bool,
    pub corners: [[f32; 2]; 4],
}

impl Default for Warp {
    fn default() -> Self {
        Self { enabled: false, corners: SQUARE }
    }
}

/// A zone's geometric correction, applied in this order: shear, size,
/// rotation, position, keystone, pincushion, bow, 4-corner warp, then a
/// clamp to -1..1. All amounts are 0 (or 1 for sizes) when neutral.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Geometry {
    pub size_x: f32,
    pub size_y: f32,
    pub pos_x: f32,
    pub pos_y: f32,
    pub rotation_deg: f32,
    /// x += shear_x · y, y += shear_y · x.
    pub shear_x: f32,
    pub shear_y: f32,
    /// Trapezoid: x *= 1 + keystone_x · y (positive: wider at the top);
    /// y *= 1 + keystone_y · x (positive: taller on the right).
    pub keystone_x: f32,
    pub keystone_y: f32,
    /// x *= 1 + pincushion_x · y², y *= 1 + pincushion_y · x².
    pub pincushion_x: f32,
    pub pincushion_y: f32,
    /// Bow: x += bow_x · y² (vertical lines curve), y += bow_y · x²
    /// (horizontal lines curve).
    pub bow_x: f32,
    pub bow_y: f32,
    pub warp: Warp,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            size_x: 1.0,
            size_y: 1.0,
            pos_x: 0.0,
            pos_y: 0.0,
            rotation_deg: 0.0,
            shear_x: 0.0,
            shear_y: 0.0,
            keystone_x: 0.0,
            keystone_y: 0.0,
            pincushion_x: 0.0,
            pincushion_y: 0.0,
            bow_x: 0.0,
            bow_y: 0.0,
            warp: Warp::default(),
        }
    }
}

/// Smallest |cross product| at a warp corner: below it the quad is
/// considered flat (the warp would explode near that corner).
const MIN_CORNER_CROSS: f32 = 0.01;

impl Geometry {
    /// Clamps every amount to its range (the ones the UI offers). The warp
    /// corners stay inside the scan field; a quad that isn't convex turns
    /// the warp off.
    pub fn sanitized(&self) -> Self {
        let c = |v: f32, lo: f32, hi: f32, neutral: f32| if v.is_finite() { v.clamp(lo, hi) } else { neutral };
        let mut warp = self.warp;
        for p in &mut warp.corners {
            p[0] = c(p[0], -1.0, 1.0, 0.0);
            p[1] = c(p[1], -1.0, 1.0, 0.0);
        }
        if warp.enabled && check_quad(&warp.corners).is_err() {
            warp.enabled = false;
        }
        Self {
            size_x: c(self.size_x, 0.05, 2.0, 1.0),
            size_y: c(self.size_y, 0.05, 2.0, 1.0),
            pos_x: c(self.pos_x, -1.0, 1.0, 0.0),
            pos_y: c(self.pos_y, -1.0, 1.0, 0.0),
            rotation_deg: c(self.rotation_deg, -180.0, 180.0, 0.0),
            shear_x: c(self.shear_x, -1.0, 1.0, 0.0),
            shear_y: c(self.shear_y, -1.0, 1.0, 0.0),
            keystone_x: c(self.keystone_x, -0.9, 0.9, 0.0),
            keystone_y: c(self.keystone_y, -0.9, 0.9, 0.0),
            pincushion_x: c(self.pincushion_x, -0.5, 0.5, 0.0),
            pincushion_y: c(self.pincushion_y, -0.5, 0.5, 0.0),
            bow_x: c(self.bow_x, -0.5, 0.5, 0.0),
            bow_y: c(self.bow_y, -0.5, 0.5, 0.0),
            warp,
        }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self { warp: Warp { enabled: false, ..self.warp }, ..Self::default() }
    }

    /// The point transform, prepared once per frame.
    pub fn placer(&self) -> Placer {
        let g = self.sanitized();
        let (sin, cos) = g.rotation_deg.to_radians().sin_cos();
        let warp = g.warp.enabled.then(|| Homography::square_to(&g.warp.corners));
        Placer { g, sin, cos, warp, identity: self.is_identity() }
    }

    /// One point through the geometry (see `Placer::apply`).
    pub fn apply(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        self.placer().apply(x, y)
    }
}

/// Why a warp quad is refused (French, shown to the operator).
pub fn check_quad(c: &[[f32; 2]; 4]) -> Result<()> {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let (a, b, d) = (c[i], c[(i + 1) % 4], c[(i + 2) % 4]);
        let cross = (b[0] - a[0]) * (d[1] - b[1]) - (b[1] - a[1]) * (d[0] - b[0]);
        if !cross.is_finite() || cross.abs() < MIN_CORNER_CROSS {
            bail!("les 4 coins du warp sont alignés ou confondus");
        }
        if sign != 0.0 && cross.signum() != sign {
            bail!("les 4 coins du warp doivent former un quadrilatère convexe (sans croisement)");
        }
        sign = cross.signum();
    }
    Ok(())
}

/// Projective map from the scan field square to a convex quad
/// (Heckbert's square-to-quad), in f64.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Homography {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    g: f64,
    h: f64,
}

impl Homography {
    fn square_to(q: &[[f32; 2]; 4]) -> Self {
        let p = |i: usize| (q[i][0] as f64, q[i][1] as f64);
        let ((x0, y0), (x1, y1), (x2, y2), (x3, y3)) = (p(0), p(1), p(2), p(3));
        let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
        let (g, h) = if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            (0.0, 0.0)
        } else {
            let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
            let det = dx1 * dy2 - dx2 * dy1;
            ((sx * dy2 - dx2 * sy) / det, (dx1 * sy - sx * dy1) / det)
        };
        Self { a: x1 - x0 + g * x1, b: x3 - x0 + h * x3, c: x0, d: y1 - y0 + g * y1, e: y3 - y0 + h * y3, f: y0, g, h }
    }

    /// (x, y) in -1..1 → the quad. None when the map degenerates there
    /// (never for a convex quad).
    fn apply(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (u, v) = ((x as f64 + 1.0) / 2.0, (y as f64 + 1.0) / 2.0);
        let w = self.g * u + self.h * v + 1.0;
        if w.is_nan() || w <= 1e-9 {
            return None;
        }
        let (x, y) = ((self.a * u + self.b * v + self.c) / w, (self.d * u + self.e * v + self.f) / w);
        (x.is_finite() && y.is_finite()).then_some((x as f32, y as f32))
    }
}

/// A geometry ready to place points.
#[derive(Clone, Copy, Debug)]
pub struct Placer {
    g: Geometry,
    sin: f32,
    cos: f32,
    warp: Option<Homography>,
    identity: bool,
}

impl Placer {
    /// The corrected point, clamped to -1..1; None if it can't be placed
    /// (a non-finite input), in which case the caller blanks it.
    pub fn apply(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        if self.identity {
            return Some((x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)));
        }
        let g = &self.g;
        let (x, y) = (x + g.shear_x * y, y + g.shear_y * x);
        let (x, y) = (x * g.size_x, y * g.size_y);
        let (x, y) = (x * self.cos - y * self.sin, x * self.sin + y * self.cos);
        let (x, y) = (x + g.pos_x, y + g.pos_y);
        let (x, y) = (x * (1.0 + g.keystone_x * y), y * (1.0 + g.keystone_y * x));
        let (x, y) = (x * (1.0 + g.pincushion_x * y * y), y * (1.0 + g.pincushion_y * x * x));
        let (x, y) = (x + g.bow_x * y * y, y + g.bow_y * x * x);
        let (x, y) = match &self.warp {
            // The field square onto the quad: what fell outside the field
            // is pinned to its edge first, so nothing leaves the quad.
            Some(h) => h.apply(x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0))?,
            None => (x, y),
        };
        (x.is_finite() && y.is_finite()).then(|| (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)))
    }

    /// A frame through the zone geometry, then the calibration. A point
    /// that can't be placed is blanked at the centre.
    pub fn place(&self, frame: &[Point], calibration: &Calibration) -> Vec<Point> {
        frame
            .iter()
            .map(|p| match self.apply(p.x, p.y) {
                Some((x, y)) => {
                    let (x, y) = calibration.apply(x, y);
                    Point { x, y, ..*p }
                }
                None => Point::blanked(0.0, 0.0),
            })
            .collect()
    }
}

/// One projection zone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectionZone {
    /// 1..=MAX_ZONES.
    pub id: u8,
    pub name: String,
    /// Off: nothing is drawn in it.
    pub enabled: bool,
    /// The output (output_manager id) it belongs to; None: whatever output
    /// is connected. While nothing is connected (preview) every zone shows.
    pub output: Option<String>,
    pub geometry: Geometry,
}

impl Default for ProjectionZone {
    fn default() -> Self {
        Self { id: 1, name: "Principale".into(), enabled: true, output: None, geometry: Geometry::default() }
    }
}

/// `projection.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectionConfig {
    pub zones: Vec<ProjectionZone>,
    /// Where cues without zones of their own play (and timeline looks).
    pub default_zones: Vec<u8>,
}

impl Default for ProjectionConfig {
    fn default() -> Self {
        Self { zones: vec![ProjectionZone::default()], default_zones: vec![1] }
    }
}

impl ProjectionConfig {
    /// What `POST /api/projection` refuses (French).
    pub fn validate(&self) -> Result<()> {
        if self.zones.is_empty() {
            bail!("il faut au moins une zone");
        }
        if self.zones.len() > MAX_ZONES as usize {
            bail!("au plus {MAX_ZONES} zones");
        }
        for (i, z) in self.zones.iter().enumerate() {
            if !(1..=MAX_ZONES).contains(&z.id) {
                bail!("numéro de zone {} hors de 1..{MAX_ZONES}", z.id);
            }
            if self.zones[..i].iter().any(|o| o.id == z.id) {
                bail!("deux zones portent le numéro {}", z.id);
            }
            if z.geometry.warp.enabled {
                check_quad(&z.geometry.warp.corners).map_err(|e| anyhow::anyhow!("zone {} : {e}", z.id))?;
            }
        }
        Ok(())
    }

    /// Repairs what a hand-edited file could hold: ids in range and
    /// unique, at most MAX_ZONES zones, at least one, names trimmed,
    /// geometries clamped, default zones that exist.
    pub fn sanitized(&self) -> Self {
        let mut zones: Vec<ProjectionZone> = Vec::new();
        for z in &self.zones {
            if !(1..=MAX_ZONES).contains(&z.id) || zones.iter().any(|o| o.id == z.id) {
                continue;
            }
            let mut z = z.clone();
            z.name = z.name.trim().chars().take(40).collect();
            if z.name.is_empty() {
                z.name = format!("Zone {}", z.id);
            }
            z.output = z.output.map(|o| o.trim().to_string()).filter(|o| !o.is_empty());
            z.geometry = z.geometry.sanitized();
            zones.push(z);
        }
        if zones.is_empty() {
            zones.push(ProjectionZone::default());
        }
        zones.sort_by_key(|z| z.id);
        let mut default_zones: Vec<u8> = self.default_zones.iter().copied().filter(|id| zones.iter().any(|z| z.id == *id)).collect();
        default_zones.sort_unstable();
        default_zones.dedup();
        if default_zones.is_empty() {
            default_zones.push(zones[0].id);
        }
        Self { zones, default_zones }
    }

    pub fn zone(&self, id: u8) -> Option<&ProjectionZone> {
        self.zones.iter().find(|z| z.id == id)
    }

    /// The zones drawn this frame, with `output` the connected output's id.
    pub fn active(&self, output: Option<&str>) -> Vec<&ProjectionZone> {
        self.zones
            .iter()
            .filter(|z| z.enabled && match (&z.output, output) {
                (Some(want), Some(have)) => want == have,
                _ => true,
            })
            .collect()
    }

    /// A cue's zones: its own, else the default ones.
    pub fn resolve<'a>(&'a self, zones: &'a [u8]) -> &'a [u8] {
        if zones.is_empty() {
            &self.default_zones
        } else {
            zones
        }
    }
}

/// The projection settings and their file.
pub struct ProjectionStore {
    cfg: ProjectionConfig,
    path: Option<PathBuf>,
    /// Alignment (« Aligner ») : the zone a test pattern goes through.
    /// Memory only; None: the test pattern only goes through the calibration.
    pub align: Option<u8>,
}

impl ProjectionStore {
    /// `projection.json`, or one neutral zone. A damaged file is a safety
    /// file: quarantined, defaults used, arming waits for the operator.
    pub fn load_or_create(path: PathBuf) -> Self {
        let cfg: ProjectionConfig = crate::datafile::load_or_default(&path, &crate::datafile::beside(&path), true);
        Self { cfg: cfg.sanitized(), path: Some(path), align: None }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { cfg: ProjectionConfig::default(), path: None, align: None }
    }

    pub fn get(&self) -> &ProjectionConfig {
        &self.cfg
    }

    /// Validates, clamps and saves. The alignment zone is dropped if it
    /// no longer exists.
    pub fn set(&mut self, cfg: ProjectionConfig) -> Result<()> {
        cfg.validate()?;
        self.cfg = cfg.sanitized();
        if self.align.is_some_and(|id| self.cfg.zone(id).is_none()) {
            self.align = None;
        }
        if let Some(path) = &self.path {
            crate::datafile::save_json(Path::new(path), &self.cfg);
        }
        Ok(())
    }

    /// The geometry a test pattern goes through (T-404 + alignment).
    pub fn align_geometry(&self) -> Option<Geometry> {
        self.align.and_then(|id| self.cfg.zone(id)).map(|z| z.geometry)
    }
}

/// One rendered look and where it goes: (layer, points, zones; empty =
/// the default zones).
pub type Routed = (u8, Vec<Point>, Vec<u8>);

/// A look as `layers::mix` takes it: (layer, points).
type Layered = (u8, Vec<Point>);

/// Mixes the looks of each active zone (its share of the point budget),
/// runs `finish` (the master live controls, and the zone's DMX remote of
/// T-407: `finish(zone id, mix)`) on each zone's mix, places it
/// through the zone geometry and the calibration, and joins the zones
/// with blanked travel. With one neutral zone this is exactly
/// mix → finish → calibration.
pub fn render(
    rendered: Vec<Routed>,
    cfg: &ProjectionConfig,
    output: Option<&str>,
    mixer: &Mixer,
    finish: impl Fn(u8, &[Point]) -> Vec<Point>,
    calibration: &Calibration,
) -> (Vec<Point>, MixReport) {
    let zones: Vec<(&ProjectionZone, Vec<Layered>)> = cfg
        .active(output)
        .into_iter()
        .map(|z| {
            let looks = rendered.iter().filter(|(_, _, on)| cfg.resolve(on).contains(&z.id)).map(|(l, p, _)| (*l, p.clone())).collect::<Vec<_>>();
            (z, looks)
        })
        .filter(|(_, looks)| !looks.is_empty())
        .collect();
    let mut report = MixReport { budget: mixer.point_budget.max(1), ..Default::default() };
    if zones.is_empty() {
        // Nothing routed to a drawn zone: still count what the layers made.
        for (l, p, _) in &rendered {
            report.per_layer[((*l).clamp(1, LAYER_COUNT as u8) - 1) as usize] += p.len();
        }
        return (Vec::new(), report);
    }
    let share = Mixer { point_budget: (mixer.point_budget / zones.len()).max(1), ..mixer.clone() };
    let mut placed: Vec<Vec<Point>> = Vec::new();
    for (zone, looks) in zones {
        let (mix, r) = layers::mix(looks, &share);
        report.demand += r.demand;
        report.points += r.points;
        report.decimated |= r.decimated;
        for d in r.dropped {
            if !report.dropped.contains(&d) {
                report.dropped.push(d);
            }
        }
        for (sum, n) in report.per_layer.iter_mut().zip(r.per_layer) {
            *sum = (*sum).max(n);
        }
        placed.push(zone.geometry.placer().place(&finish(zone.id, &mix), calibration));
    }
    if placed.len() == 1 {
        return (placed.pop().unwrap_or_default(), report);
    }
    let frame = crate::engine::join_looks(placed, Some(&mixer.optimizer));
    report.points = frame.len();
    (frame, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: [f32; 2]) -> bool {
        (a.0 - b[0]).abs() < 1e-5 && (a.1 - b[1]).abs() < 1e-5
    }

    fn warp(corners: [[f32; 2]; 4]) -> Geometry {
        Geometry { warp: Warp { enabled: true, corners }, ..Default::default() }
    }

    const QUADS: [[[f32; 2]; 4]; 4] = [
        [[-0.8, -0.9], [0.7, -0.6], [0.9, 0.8], [-0.6, 0.5]],
        [[-1.0, -1.0], [1.0, -1.0], [0.4, 0.6], [-0.4, 0.6]],
        [[-0.2, -0.9], [0.95, -0.1], [0.1, 0.9], [-0.9, 0.0]],
        // Mirrored (a projector hung upside down): clockwise.
        [[-0.5, 0.5], [0.5, 0.5], [0.6, -0.7], [-0.6, -0.7]],
    ];

    #[test]
    fn identity_by_default_and_clamps() {
        let g = Geometry::default();
        assert!(g.is_identity());
        assert_eq!(g.apply(0.3, -0.4), Some((0.3, -0.4)));
        assert_eq!(g.apply(1.5, -2.0), Some((1.0, -1.0)));
        assert_eq!(g.apply(f32::NAN, 0.0), None);
    }

    #[test]
    fn four_corner_warp_is_exact_at_the_corners() {
        for q in QUADS {
            check_quad(&q).unwrap();
            let p = warp(q).placer();
            for (corner, target) in SQUARE.iter().zip(q) {
                let got = p.apply(corner[0], corner[1]).unwrap();
                assert!(close(got, target), "{corner:?} → {got:?}, want {target:?}");
            }
        }
    }

    #[test]
    fn the_warp_keeps_straight_lines_straight_and_stays_in_the_quad() {
        for q in QUADS {
            let p = warp(q).placer();
            // Points along a diagonal of the field stay collinear.
            let pts: Vec<(f32, f32)> = (0..=10).map(|i| p.apply(-1.0 + i as f32 * 0.2, -1.0 + i as f32 * 0.14).unwrap()).collect();
            let (a, b) = (pts[0], pts[10]);
            for m in &pts {
                let cross = (b.0 - a.0) * (m.1 - a.1) - (b.1 - a.1) * (m.0 - a.0);
                assert!(cross.abs() < 1e-4, "{m:?} off the line {a:?}–{b:?}");
            }
            // Far outside the field: pinned to the quad's edge, inside -1..1.
            for (x, y) in [(5.0, 5.0), (-9.0, 0.3), (0.0, -3.0)] {
                let (x, y) = p.apply(x, y).unwrap();
                assert!((-1.0..=1.0).contains(&x) && (-1.0..=1.0).contains(&y));
            }
        }
    }

    #[test]
    fn a_square_quad_is_the_identity() {
        let p = warp(SQUARE).placer();
        for (x, y) in [(0.0, 0.0), (0.5, -0.25), (-0.9, 0.9)] {
            assert!(close(p.apply(x, y).unwrap(), [x, y]));
        }
    }

    #[test]
    fn flat_or_crossed_quads_are_refused_and_turned_off_on_load() {
        let crossed = [[-1.0, -1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, 1.0]];
        let flat = [[-1.0, 0.0], [0.0, 0.0], [1.0, 0.0], [0.5, 0.5]];
        let concave = [[-1.0, -1.0], [1.0, -1.0], [0.0, -0.8], [-1.0, 1.0]];
        for q in [crossed, flat, concave] {
            assert!(check_quad(&q).is_err(), "{q:?}");
            let cfg = ProjectionConfig { zones: vec![ProjectionZone { geometry: warp(q), ..Default::default() }], ..Default::default() };
            assert!(cfg.validate().is_err());
            assert!(!cfg.sanitized().zones[0].geometry.warp.enabled);
        }
    }

    #[test]
    fn keystone_pincushion_and_bow_act_as_documented() {
        let k = Geometry { keystone_x: 0.2, ..Default::default() };
        let top = k.apply(0.5, 1.0).unwrap().0;
        let bottom = k.apply(0.5, -1.0).unwrap().0;
        assert!(top > 0.5 && bottom < 0.5, "wider at the top: {top} vs {bottom}");
        assert_eq!(k.apply(0.5, 0.0), Some((0.5, 0.0)), "the centre line keeps its width");

        let pc = Geometry { pincushion_x: 0.2, ..Default::default() };
        assert!((pc.apply(0.5, 0.8).unwrap().0 - 0.5 * (1.0 + 0.2 * 0.64)).abs() < 1e-6);

        let bow = Geometry { bow_y: -0.1, ..Default::default() };
        let (_, y_edge) = bow.apply(0.9, 0.5).unwrap();
        let (_, y_mid) = bow.apply(0.0, 0.5).unwrap();
        assert!(y_edge < y_mid, "a horizontal line bows down at the edges");

        let moved = Geometry { size_x: 0.5, size_y: 0.5, pos_x: -0.5, ..Default::default() };
        assert!(close(moved.apply(1.0, 1.0).unwrap(), [0.0, 0.5]));
        let rot = Geometry { rotation_deg: 90.0, ..Default::default() };
        assert!(close(rot.apply(0.5, 0.0).unwrap(), [0.0, 0.5]));
        let shear = Geometry { shear_x: 0.5, ..Default::default() };
        assert!(close(shear.apply(0.0, 0.4).unwrap(), [0.2, 0.4]));
    }

    #[test]
    fn extreme_geometries_stay_within_the_field() {
        let wild = Geometry {
            size_x: 2.0,
            size_y: 2.0,
            pos_x: 1.0,
            rotation_deg: 33.0,
            shear_x: 1.0,
            keystone_x: 0.9,
            keystone_y: -0.9,
            pincushion_x: 0.5,
            pincushion_y: -0.5,
            bow_x: 0.5,
            bow_y: 0.5,
            warp: Warp { enabled: true, corners: QUADS[2] },
            ..Default::default()
        };
        let p = wild.placer();
        for i in 0..=40 {
            for j in 0..=40 {
                let (x, y) = p.apply(-1.0 + i as f32 * 0.05, -1.0 + j as f32 * 0.05).expect("placed");
                assert!(x.is_finite() && y.is_finite() && (-1.0..=1.0).contains(&x) && (-1.0..=1.0).contains(&y));
            }
        }
        // Out-of-range values are clamped, not trusted.
        let s = Geometry { size_x: 50.0, keystone_x: f32::INFINITY, pincushion_y: -7.0, ..Default::default() }.sanitized();
        assert_eq!((s.size_x, s.keystone_x, s.pincushion_y), (2.0, 0.0, -0.5));
    }

    #[test]
    fn placing_blanks_points_that_cannot_be_placed_and_applies_the_calibration() {
        let frame = vec![Point { x: 0.5, y: 0.0, r: 1.0, g: 1.0, b: 1.0 }, Point { x: f32::NAN, y: 0.0, r: 1.0, g: 0.0, b: 0.0 }];
        let cal = Calibration { offset_x: 0.25, ..Default::default() };
        let out = Geometry::default().placer().place(&frame, &cal);
        assert!(close((out[0].x, out[0].y), [0.75, 0.0]) && out[0].is_lit());
        assert!(!out[1].is_lit() && out[1].x == 0.0);
    }

    #[test]
    fn config_sanitizes_ids_names_and_default_zones() {
        let cfg = ProjectionConfig {
            zones: vec![
                ProjectionZone { id: 3, name: "  ".into(), ..Default::default() },
                ProjectionZone { id: 3, name: "dup".into(), ..Default::default() },
                ProjectionZone { id: 0, ..Default::default() },
                ProjectionZone { id: 2, name: "Gauche".into(), output: Some(" ".into()), ..Default::default() },
            ],
            default_zones: vec![7, 3, 3],
        };
        assert!(cfg.validate().is_err());
        let s = cfg.sanitized();
        assert_eq!(s.zones.iter().map(|z| z.id).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(s.zones[1].name, "Zone 3");
        assert_eq!(s.zones[0].output, None);
        assert_eq!(s.default_zones, [3]);
        let empty = ProjectionConfig { zones: vec![], default_zones: vec![] }.sanitized();
        assert_eq!(empty, ProjectionConfig::default());
        // An old or empty file: one neutral zone, the default.
        let old: ProjectionConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old.sanitized(), ProjectionConfig::default());
    }

    #[test]
    fn zones_follow_their_output() {
        let cfg = ProjectionConfig {
            zones: vec![
                ProjectionZone { id: 1, ..Default::default() },
                ProjectionZone { id: 2, output: Some("test".into()), ..Default::default() },
                ProjectionZone { id: 3, enabled: false, ..Default::default() },
            ],
            default_zones: vec![1],
        };
        let ids = |o: Option<&str>| cfg.active(o).iter().map(|z| z.id).collect::<Vec<_>>();
        assert_eq!(ids(None), [1, 2], "preview: every enabled zone");
        assert_eq!(ids(Some("test")), [1, 2]);
        assert_eq!(ids(Some("helios:1")), [1]);
    }

    fn line(x: f32) -> Vec<Point> {
        (0..=10).map(|i| Point { x, y: -0.5 + i as f32 * 0.1, r: 1.0, g: 1.0, b: 1.0 }).collect()
    }

    fn two_zones() -> ProjectionConfig {
        ProjectionConfig {
            zones: vec![
                ProjectionZone { id: 1, geometry: Geometry { size_x: 0.5, pos_x: -0.5, ..Default::default() }, ..Default::default() },
                ProjectionZone { id: 2, geometry: Geometry { size_x: 0.5, pos_x: 0.5, ..Default::default() }, ..Default::default() },
            ],
            default_zones: vec![1],
        }
    }

    #[test]
    fn a_cue_can_play_on_several_zones() {
        let cfg = two_zones();
        let (frame, report) = render(vec![(1, line(0.0), vec![1, 2])], &cfg, None, &Mixer::default(), |_, p| p.to_vec(), &Calibration::default());
        let lit: Vec<&Point> = frame.iter().filter(|p| p.is_lit()).collect();
        assert!(lit.iter().any(|p| (p.x + 0.5).abs() < 1e-6), "drawn in zone 1");
        assert!(lit.iter().any(|p| (p.x - 0.5).abs() < 1e-6), "drawn in zone 2");
        assert!(lit.iter().all(|p| (p.x.abs() - 0.5).abs() < 1e-6), "nothing lit between the zones");
        assert_eq!(report.points, frame.len());
        assert!(frame.len() <= Mixer::default().point_budget + 200);
    }

    #[test]
    fn cues_without_zones_play_on_the_default_ones_and_disabled_zones_stay_dark() {
        let mut cfg = two_zones();
        let draw = |cfg: &ProjectionConfig, zones: Vec<u8>| render(vec![(1, line(0.0), zones)], cfg, None, &Mixer::default(), |_, p| p.to_vec(), &Calibration::default()).0;
        assert!(draw(&cfg, vec![]).iter().filter(|p| p.is_lit()).all(|p| (p.x + 0.5).abs() < 1e-6));
        cfg.default_zones = vec![2];
        assert!(draw(&cfg, vec![]).iter().filter(|p| p.is_lit()).all(|p| (p.x - 0.5).abs() < 1e-6));
        cfg.zones[1].enabled = false;
        assert!(draw(&cfg, vec![2]).is_empty(), "a cue on a disabled zone is not drawn");
    }

    #[test]
    fn one_neutral_zone_is_exactly_mix_then_calibration() {
        let looks = vec![(1, line(0.2), vec![]), (2, line(-0.3), vec![])];
        let cal = Calibration { offset_x: 0.1, scale_y: 0.8, rotation_deg: 12.0, ..Default::default() };
        let mixer = Mixer::default();
        let (frame, report) = render(looks.clone(), &ProjectionConfig::default(), None, &mixer, |_, p| p.to_vec(), &cal);
        let (mix, want_report) = layers::mix(looks.into_iter().map(|(l, p, _)| (l, p)).collect(), &mixer);
        let want: Vec<Point> = mix
            .into_iter()
            .map(|p| {
                let (x, y) = cal.apply(p.x, p.y);
                Point { x, y, ..p }
            })
            .collect();
        assert_eq!(frame, want);
        assert_eq!(report, want_report);
    }
}
