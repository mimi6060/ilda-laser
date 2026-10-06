//! Outdoor mode (T-262): declared angles, the 45° limit and « Ciel coupé ».
//!
//! In Belgium every laser beam projected into airspace needs a DGTA
//! authorisation (docs/research/safety-regulation.md §3.4). The form asks,
//! for each beam, the horizontal and vertical angle range (« de … à … »),
//! limits light beams to 45° from the vertical, and requires a beam that
//! touches an aircraft to be deviated at once. This module helps the
//! operator keep to what was declared; it is a **software aid** only: the
//! authorisation, the spotters and the hardware stop stay the operator's
//! job, and it never makes a show compliant by itself.
//!
//! **Angles.** A projector's *mounting* gives its total optical scan angle,
//! its tilt (elevation of the centre axis) and its azimuth (0 = north). A
//! normalised output point (x, y in -1..1, after calibration) points at
//! `az = azimuth + x·angle/2`, `él = tilt + y·angle/2` (small-angle
//! approximation: exact on the two axes, off by a few degrees in the
//! corners of a wide field; declare sectors with a margin).
//!
//! **What is cut.** With the mode on, a lit point outside every declared
//! sector is blanked, and so is a point above the geographic horizon
//! (él > 0) further than `max_from_vertical_deg` from the vertical, unless
//! the authorisation waives that limit (`vertical_limit_waived` plus its
//! reference). « Ciel coupé » (`sky_off`, session state, off at start)
//! blanks every point with él ≥ 0, whether the mode is on or not, and
//! leaves the ground show running.
//!
//! **How.** All of these are bounds on x and y, so the allowed region is a
//! union of rectangles in output space. Its complement is compiled into
//! Blank zones that `SafetySettings::mask` adds to the operator's zones:
//! the zones code (T-003) cuts segments at their edges, re-applies them to
//! a frame the strobe limiter holds, and the preview shows the result. It
//! runs in the safety stage, after calibration, before the guard and the
//! caps: no content, live control or geometry can undo it.
//!
//! **Fail towards cutting more.** A zone is a closed set, so the boundary
//! itself is dark. A projector with no declared mounting, or a mounting
//! that is not a valid number, gets its whole field cut while the mode or
//! « Ciel coupé » is on. A sector that is not a number or inverted is
//! refused by `validate` and dropped by `sanitized` (fewer sectors, more
//! cut). No sector at all with the mode on: everything is cut.

use crate::power::MAIN_OUTPUT;
use crate::zones::{Zone, ZoneKind};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_SECTORS: usize = 16;
pub const MAX_MOUNTINGS: usize = 8;
pub const MAX_REF: usize = 120;
pub const DEFAULT_SCAN_ANGLE: f32 = 40.0;
pub const DEFAULT_MAX_FROM_VERTICAL: f32 = 45.0;
const SCAN_RANGE: (f32, f32) = (1.0, 120.0);
const TILT_RANGE: (f32, f32) = (-90.0, 90.0);
const AZ_RANGE: (f32, f32) = (0.0, 360.0);
const HEIGHT_RANGE: (f32, f32) = (0.0, 10_000.0);
const EL_RANGE: (f32, f32) = (-90.0, 90.0);
const VERTICAL_RANGE: (f32, f32) = (0.0, 90.0);
/// The cut zones reach this far past the field (-1..1), so the field's
/// own edge is inside a cut.
const FIELD: f32 = 1.01;
/// A rectangle thinner than this (normalised units) is not allowed light.
const THIN: f32 = 1e-5;

/// Where a projector stands and points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mounting {
    /// Total optical scan angle, in degrees (the projector sheet, T-254).
    pub scan_angle_deg: f32,
    /// Elevation of the centre axis, in degrees (0 = horizontal, up > 0).
    pub tilt_deg: f32,
    /// Azimuth of the centre axis, in degrees (0 = north, 90 = east).
    pub azimuth_deg: f32,
    /// Height of the source above the ground, in metres (for the form).
    pub height_m: f32,
}

impl Default for Mounting {
    fn default() -> Self {
        Self { scan_angle_deg: DEFAULT_SCAN_ANGLE, tilt_deg: 0.0, azimuth_deg: 0.0, height_m: 0.0 }
    }
}

impl Mounting {
    /// Every value a finite number in range.
    pub fn valid(&self) -> bool {
        let ok = |v: f32, (lo, hi): (f32, f32)| v.is_finite() && v >= lo && v <= hi;
        ok(self.scan_angle_deg, SCAN_RANGE) && ok(self.tilt_deg, TILT_RANGE) && ok(self.azimuth_deg, AZ_RANGE) && ok(self.height_m, HEIGHT_RANGE)
    }

    /// Degrees per normalised unit.
    fn k(&self) -> f32 {
        self.scan_angle_deg / 2.0
    }

    /// (azimuth, elevation) of an output point, in degrees (azimuth not
    /// wrapped).
    pub fn angles(&self, x: f32, y: f32) -> (f32, f32) {
        (self.azimuth_deg + x * self.k(), self.tilt_deg + y * self.k())
    }

    fn y_of(&self, el: f32) -> f32 {
        (el - self.tilt_deg) / self.k()
    }

    fn x_of(&self, az: f32) -> f32 {
        (az - self.azimuth_deg) / self.k()
    }

    fn validate(&self, what: &str) -> Result<()> {
        check(self.scan_angle_deg, SCAN_RANGE, &format!("{what} : angle de balayage"))?;
        check(self.tilt_deg, TILT_RANGE, &format!("{what} : inclinaison"))?;
        check(self.azimuth_deg, AZ_RANGE, &format!("{what} : azimut"))?;
        check(self.height_m, HEIGHT_RANGE, &format!("{what} : hauteur"))
    }

    /// Finite values clamped; None when one is not a number.
    fn sanitized(&self) -> Option<Self> {
        let all = [self.scan_angle_deg, self.tilt_deg, self.azimuth_deg, self.height_m];
        if !all.iter().all(|v| v.is_finite()) {
            return None;
        }
        Some(Self {
            scan_angle_deg: self.scan_angle_deg.clamp(SCAN_RANGE.0, SCAN_RANGE.1),
            tilt_deg: self.tilt_deg.clamp(TILT_RANGE.0, TILT_RANGE.1),
            azimuth_deg: self.azimuth_deg.clamp(AZ_RANGE.0, AZ_RANGE.1),
            height_m: self.height_m.clamp(HEIGHT_RANGE.0, HEIGHT_RANGE.1),
        })
    }
}

/// One declared beam sector, as on the DGTA form: horizontal angle from
/// `az_min` to `az_max` (clockwise, 0..360; `az_min > az_max` goes through
/// north, e.g. 350 → 10), vertical angle from `el_min` to `el_max`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sector {
    pub az_min: f32,
    pub az_max: f32,
    pub el_min: f32,
    pub el_max: f32,
}

impl Sector {
    fn finite(&self) -> bool {
        [self.az_min, self.az_max, self.el_min, self.el_max].iter().all(|v| v.is_finite())
    }

    /// The azimuth interval, unwrapped (`hi` may exceed 360).
    fn az_span(&self) -> (f32, f32) {
        if self.az_min <= self.az_max {
            (self.az_min, self.az_max)
        } else {
            (self.az_min, self.az_max + 360.0)
        }
    }

    /// (az, el) is inside, `tol` degrees of slack.
    fn contains(&self, az: f32, el: f32, tol: f32) -> bool {
        if el < self.el_min - tol || el > self.el_max + tol {
            return false;
        }
        let (lo, hi) = self.az_span();
        let a = az.rem_euclid(360.0);
        [a - 360.0, a, a + 360.0].iter().any(|&a| a >= lo - tol && a <= hi + tol)
    }
}

/// The outdoor settings (part of `SafetySettings`, so of `safety.json` and
/// of every venue profile, T-260).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OutdoorSettings {
    /// « Show en extérieur ».
    pub enabled: bool,
    /// « Référence d'autorisation DGTA »: arming is refused while the mode
    /// is on and this is empty.
    pub authorization_ref: String,
    /// WGS84 latitude, longitude of the source (for the form).
    pub wgs84: Option<(f64, f64)>,
    /// The main projector's mounting.
    pub mounting: Mounting,
    /// Other venue projectors' mountings, by projector id (T-277). A
    /// projector without one is fully cut while the mode is on.
    pub mountings: BTreeMap<String, Mounting>,
    /// Declared sectors (« faisceaux »).
    pub sectors: Vec<Sector>,
    /// « Écart max à la verticale » above the horizon, degrees.
    pub max_from_vertical_deg: f32,
    /// The authorisation waives the vertical limit (needs `waiver_ref`).
    pub vertical_limit_waived: bool,
    /// Where the authorisation says so.
    pub waiver_ref: String,
    /// « Ciel coupé » (session state, never saved; set by the engine).
    #[serde(skip)]
    pub sky_off: bool,
    /// The projector whose frame this is (set by the engine; empty = main).
    #[serde(skip)]
    pub projector: String,
}

impl Default for OutdoorSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            authorization_ref: String::new(),
            wgs84: None,
            mounting: Mounting::default(),
            mountings: BTreeMap::new(),
            sectors: Vec::new(),
            max_from_vertical_deg: DEFAULT_MAX_FROM_VERTICAL,
            vertical_limit_waived: false,
            waiver_ref: String::new(),
            sky_off: false,
            projector: String::new(),
        }
    }
}

/// Only the saved settings count (`sky_off` and `projector` are runtime).
impl PartialEq for OutdoorSettings {
    fn eq(&self, o: &Self) -> bool {
        self.enabled == o.enabled
            && self.authorization_ref == o.authorization_ref
            && self.wgs84 == o.wgs84
            && self.mounting == o.mounting
            && self.mountings == o.mountings
            && self.sectors == o.sectors
            && self.max_from_vertical_deg == o.max_from_vertical_deg
            && self.vertical_limit_waived == o.vertical_limit_waived
            && self.waiver_ref == o.waiver_ref
    }
}

fn check(v: f32, (lo, hi): (f32, f32), what: &str) -> Result<()> {
    if !v.is_finite() || v < lo || v > hi {
        bail!("{what} : {v} hors limites ({lo} à {hi})");
    }
    Ok(())
}

fn check_text(s: &str, what: &str) -> Result<()> {
    if s.chars().count() > MAX_REF {
        bail!("{what} : {MAX_REF} caractères au plus");
    }
    if s.chars().any(char::is_control) {
        bail!("{what} : caractères invalides");
    }
    Ok(())
}

/// Intersection of two sorted lists of closed intervals.
fn intersect(a: &[(f32, f32)], b: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    for &(a0, a1) in a {
        for &(b0, b1) in b {
            let (lo, hi) = (a0.max(b0), a1.min(b1));
            if hi - lo > THIN {
                out.push((lo, hi));
            }
        }
    }
    out
}

impl OutdoorSettings {
    /// The mode is on and the arming lock is satisfied.
    pub fn authorized(&self) -> bool {
        !self.enabled || !self.authorization_ref.trim().is_empty()
    }

    /// Anything is cut for this frame.
    pub fn active(&self) -> bool {
        self.enabled || self.sky_off
    }

    /// The vertical limit applies.
    fn vertical_limit(&self) -> bool {
        self.enabled && !(self.vertical_limit_waived && !self.waiver_ref.trim().is_empty())
    }

    /// The mounting of `projector` (empty or `main`: the main one).
    pub fn mounting_of(&self, projector: &str) -> Option<&Mounting> {
        if projector.is_empty() || projector == MAIN_OUTPUT {
            Some(&self.mounting)
        } else {
            self.mountings.get(projector)
        }
    }

    pub fn validate(&self) -> Result<()> {
        check_text(&self.authorization_ref, "Référence d'autorisation")?;
        check_text(&self.waiver_ref, "Référence de la dérogation (verticale)")?;
        if let Some((lat, lon)) = self.wgs84 {
            if !(lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
                bail!("Coordonnées WGS84 invalides : {lat}, {lon}");
            }
        }
        self.mounting.validate("Montage")?;
        if self.mountings.len() > MAX_MOUNTINGS {
            bail!("{} montages : {MAX_MOUNTINGS} au plus", self.mountings.len());
        }
        for (id, m) in &self.mountings {
            if id.trim().is_empty() || id.chars().count() > 40 {
                bail!("Montage : identifiant de projecteur invalide");
            }
            m.validate(&format!("Montage « {id} »"))?;
        }
        if self.sectors.len() > MAX_SECTORS {
            bail!("{} secteurs : {MAX_SECTORS} au plus", self.sectors.len());
        }
        for (i, s) in self.sectors.iter().enumerate() {
            let what = format!("Secteur {}", i + 1);
            check(s.az_min, AZ_RANGE, &format!("{what}, azimut de"))?;
            check(s.az_max, AZ_RANGE, &format!("{what}, azimut à"))?;
            check(s.el_min, EL_RANGE, &format!("{what}, élévation de"))?;
            check(s.el_max, EL_RANGE, &format!("{what}, élévation à"))?;
            if s.el_min > s.el_max {
                bail!("{what} : élévation de {} à {} (de ≤ à)", s.el_min, s.el_max);
            }
        }
        check(self.max_from_vertical_deg, VERTICAL_RANGE, "Écart max à la verticale")?;
        if self.vertical_limit_waived && self.waiver_ref.trim().is_empty() {
            bail!("Limite de verticale levée : la référence de l'autorisation est obligatoire");
        }
        Ok(())
    }

    /// For a file edited by hand: values clamped, and whatever is not a
    /// number fails towards cutting more (see the module docs).
    pub fn sanitized(&self) -> Self {
        let mut out = self.clone();
        out.sky_off = false;
        out.projector.clear();
        out.authorization_ref = self.authorization_ref.chars().filter(|c| !c.is_control()).take(MAX_REF).collect();
        out.waiver_ref = self.waiver_ref.chars().filter(|c| !c.is_control()).take(MAX_REF).collect();
        out.wgs84 = self.wgs84.filter(|(lat, lon)| lat.is_finite() && lon.is_finite()).map(|(lat, lon)| (lat.clamp(-90.0, 90.0), lon.clamp(-180.0, 180.0)));
        match self.mounting.sanitized() {
            Some(m) => out.mounting = m,
            None => {
                // Unknown geometry: the mode on with no sector, i.e. the
                // whole field cut (and arming waits for a reference).
                out.mounting = Mounting::default();
                out.enabled = true;
                out.sectors.clear();
            }
        }
        // A projector whose mounting is not a number loses it: fully cut.
        out.mountings = self
            .mountings
            .iter()
            .filter(|(id, _)| !id.trim().is_empty() && id.chars().count() <= 40)
            .filter_map(|(id, m)| m.sanitized().map(|m| (id.clone(), m)))
            .take(MAX_MOUNTINGS)
            .collect();
        out.sectors = out
            .sectors
            .iter()
            .filter(|s| s.finite() && s.el_min <= s.el_max)
            .map(|s| Sector {
                az_min: s.az_min.clamp(AZ_RANGE.0, AZ_RANGE.1),
                az_max: s.az_max.clamp(AZ_RANGE.0, AZ_RANGE.1),
                el_min: s.el_min.clamp(EL_RANGE.0, EL_RANGE.1),
                el_max: s.el_max.clamp(EL_RANGE.0, EL_RANGE.1),
            })
            .take(MAX_SECTORS)
            .collect();
        out.max_from_vertical_deg = if self.max_from_vertical_deg.is_finite() {
            self.max_from_vertical_deg.clamp(VERTICAL_RANGE.0, VERTICAL_RANGE.1)
        } else {
            VERTICAL_RANGE.0
        };
        if out.waiver_ref.trim().is_empty() {
            out.vertical_limit_waived = false;
        }
        out
    }

    /// What going from `self` to `new` would loosen (French), for the
    /// operator's confirmation. Empty = only tighter or equal.
    pub fn loosenings(&self, new: &OutdoorSettings) -> Vec<String> {
        let mut out = Vec::new();
        // A mounting added or changed moves the cut (or ends a projector's
        // full cut); one removed only cuts more (a projector without a
        // mounting is fully cut). « Ciel coupé » follows the mountings
        // even with the mode off.
        let others = |out: &mut Vec<String>| {
            for (id, m) in &new.mountings {
                match self.mountings.get(id) {
                    None => out.push(format!("Montage du projecteur « {id} » déclaré : il n'est plus entièrement coupé")),
                    Some(old) if old != m => out.push(format!("Montage du projecteur « {id} » modifié : ses coupures se déplacent")),
                    _ => {}
                }
            }
        };
        if !self.enabled {
            // Nothing outdoor is cut today (« Ciel coupé » aside, which
            // only follows the mountings): turning it on only tightens.
            if self.mounting != new.mounting {
                out.push("Montage du projecteur modifié (« Ciel coupé » suit le montage)".into());
            }
            others(&mut out);
            return out;
        }
        if !new.enabled {
            out.push("Mode extérieur désactivé : secteurs et limite de verticale levés".into());
            return out;
        }
        if self.mounting != new.mounting {
            out.push("Montage du projecteur modifié : les coupures se déplacent".into());
        }
        others(&mut out);
        for (i, s) in new.sectors.iter().enumerate() {
            if !self.sectors.iter().any(|o| sector_within(s, o)) {
                out.push(format!("Secteur {} ajouté ou élargi", i + 1));
            }
        }
        if self.vertical_limit() {
            if !new.vertical_limit() {
                out.push("Limite de verticale levée".into());
            } else if new.max_from_vertical_deg > self.max_from_vertical_deg {
                out.push(format!("Écart max à la verticale : {:.0}° → {:.0}°", self.max_from_vertical_deg, new.max_from_vertical_deg));
            }
        }
        out
    }

    /// Whether a point at (x, y) may be lit, straight from the angles
    /// (`tol` degrees of slack). The reference the property tests check
    /// the compiled zones against.
    pub fn allows(&self, x: f32, y: f32, tol: f32) -> bool {
        if !self.active() {
            return true;
        }
        let Some(m) = self.mounting_of(&self.projector).filter(|m| m.valid()) else { return false };
        let (az, el) = m.angles(x, y);
        if self.sky_off && el > tol {
            return false;
        }
        if self.enabled {
            if !self.sectors.iter().any(|s| s.finite() && s.contains(az, el, tol)) {
                return false;
            }
            if self.vertical_limit() && el > tol {
                let lim = self.max_from_vertical_deg.clamp(VERTICAL_RANGE.0, VERTICAL_RANGE.1);
                if (90.0 - el).abs() > lim + tol {
                    return false;
                }
            }
        }
        true
    }

    /// The allowed region for this frame, as rectangles (x0, y0, x1, y1)
    /// in output space; None: nothing outdoor is cut.
    fn allowed_rects(&self) -> Option<Vec<[f32; 4]>> {
        if !self.active() {
            return None;
        }
        let Some(m) = self.mounting_of(&self.projector).filter(|m| m.valid()) else { return Some(Vec::new()) };
        let clip = |v: f32| v.clamp(-FIELD, FIELD);
        let mut ys = vec![(-FIELD, FIELD)];
        let ground = clip(m.y_of(0.0));
        if self.sky_off {
            ys = intersect(&ys, &[(-FIELD, ground)]);
        }
        if self.vertical_limit() {
            let lim = self.max_from_vertical_deg.clamp(VERTICAL_RANGE.0, VERTICAL_RANGE.1);
            ys = intersect(&ys, &[(-FIELD, ground), (clip(m.y_of(90.0 - lim)), clip(m.y_of(90.0 + lim)))]);
        }
        let mut boxes: Vec<[f32; 4]> = Vec::new();
        if self.enabled {
            for s in self.sectors.iter().filter(|s| s.finite() && s.el_min <= s.el_max) {
                let (lo, hi) = s.az_span();
                let (y0, y1) = (clip(m.y_of(s.el_min)), clip(m.y_of(s.el_max)));
                for turn in -3..=1 {
                    let shift = turn as f32 * 360.0;
                    let (x0, x1) = (clip(m.x_of(lo + shift)), clip(m.x_of(hi + shift)));
                    if x1 - x0 > THIN && y1 - y0 > THIN {
                        boxes.push([x0, y0, x1, y1]);
                    }
                }
            }
        } else {
            boxes.push([-FIELD, -FIELD, FIELD, FIELD]);
        }
        let mut out = Vec::new();
        for b in boxes {
            for (y0, y1) in intersect(&[(b[1], b[3])], &ys) {
                out.push([b[0], y0, b[2], y1]);
            }
        }
        Some(out)
    }

    /// The cut, as Blank zones in output space (empty when nothing is
    /// cut). Cells of the grid made by the allowed rectangles' edges that
    /// no rectangle covers, merged per column.
    pub fn blank_zones(&self) -> Vec<Zone> {
        let Some(allowed) = self.allowed_rects() else { return Vec::new() };
        let mut xs = vec![-FIELD, FIELD];
        let mut ys = vec![-FIELD, FIELD];
        for r in &allowed {
            xs.extend([r[0], r[2]]);
            ys.extend([r[1], r[3]]);
        }
        for v in [&mut xs, &mut ys] {
            v.sort_by(f32::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() <= THIN);
        }
        let inside = |x: f32, y: f32| allowed.iter().any(|r| x > r[0] && x < r[2] && y > r[1] && y < r[3]);
        let mut zones = Vec::new();
        let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Zone {
            id: 0,
            name: "Extérieur".into(),
            kind: ZoneKind::Blank,
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            ..Default::default()
        };
        for cx in xs.windows(2) {
            let mid_x = (cx[0] + cx[1]) / 2.0;
            let mut run: Option<f32> = None;
            for cy in ys.windows(2) {
                let blocked = !inside(mid_x, (cy[0] + cy[1]) / 2.0);
                match (blocked, run) {
                    (true, None) => run = Some(cy[0]),
                    (false, Some(y0)) => {
                        zones.push(rect(cx[0], y0, cx[1], cy[0]));
                        run = None;
                    }
                    _ => {}
                }
            }
            if let Some(y0) = run {
                zones.push(rect(cx[0], y0, cx[1], FIELD));
            }
        }
        zones
    }

    /// The sectors as on the DGTA form (T-263): one row per beam, angles
    /// exactly as entered. `;`-separated, French headers.
    pub fn export_csv(&self) -> String {
        let mut out = String::from("Faisceau;Angle horizontal de (°);Angle horizontal à (°);Angle vertical de (°);Angle vertical à (°)\n");
        for (i, s) in self.sectors.iter().enumerate() {
            out.push_str(&format!("{};{};{};{};{}\n", i + 1, s.az_min, s.az_max, s.el_min, s.el_max));
        }
        out
    }
}

/// `inner` lies inside `outer` (both declared sectors).
fn sector_within(inner: &Sector, outer: &Sector) -> bool {
    if inner.el_min < outer.el_min || inner.el_max > outer.el_max {
        return false;
    }
    let (lo, hi) = inner.az_span();
    let (olo, ohi) = outer.az_span();
    [-360.0, 0.0, 360.0].iter().any(|s| lo + s >= olo && hi + s <= ohi)
}

/// Lit samples (and segments with a lit end) that the angles forbid, with
/// `tol` degrees of slack (the property tests' check, invariant 4).
#[cfg(test)]
pub fn violations(frame: &[crate::patterns::Point], o: &OutdoorSettings, tol: f32) -> usize {
    if !o.active() {
        return 0;
    }
    let samples = frame.iter().filter(|p| p.is_lit() && !o.allows(p.x, p.y, tol)).count();
    let segments = frame
        .windows(2)
        .filter(|w| w[0].is_lit() || w[1].is_lit())
        .filter(|w| {
            (1..20).any(|s| {
                let t = s as f32 / 20.0;
                !o.allows(w[0].x + (w[1].x - w[0].x) * t, w[0].y + (w[1].y - w[0].y) * t, tol)
            })
        })
        .count();
    samples + segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patterns::Point;
    use crate::safety::{self, SafetySettings, StrobeLimiter};

    fn outdoor(sectors: Vec<Sector>) -> OutdoorSettings {
        OutdoorSettings { enabled: true, authorization_ref: "DGTA-1".into(), sectors, ..Default::default() }
    }

    fn sector(az_min: f32, az_max: f32, el_min: f32, el_max: f32) -> Sector {
        Sector { az_min, az_max, el_min, el_max }
    }

    /// The frame through the real safety stage.
    fn run(o: &OutdoorSettings, frame: Vec<Point>) -> Vec<Point> {
        let cfg = SafetySettings { outdoor: o.clone(), horizon: crate::zones::Horizon { y: -1.0, ..Default::default() }, ..Default::default() };
        safety::apply(frame, 0.0, &cfg, &mut StrobeLimiter::default())
    }

    fn lit_at(out: &[Point], x: f32, y: f32) -> bool {
        out.iter().any(|p| p.is_lit() && (p.x - x).abs() < 1e-4 && (p.y - y).abs() < 1e-4)
    }

    /// A beam (held point) at (x, y).
    fn beam(x: f32, y: f32) -> Vec<Point> {
        vec![Point::lit(x, y, 1.0, 1.0, 1.0); 12]
    }

    #[test]
    fn angles_follow_the_mounting() {
        let m = Mounting { scan_angle_deg: 60.0, tilt_deg: 20.0, azimuth_deg: 90.0, height_m: 3.0 };
        assert_eq!(m.angles(0.0, 0.0), (90.0, 20.0));
        assert_eq!(m.angles(1.0, -1.0), (120.0, -10.0));
        assert_eq!(m.angles(-0.5, 0.5), (75.0, 35.0));
    }

    #[test]
    fn the_45_degree_limit_cuts_30_and_passes_60() {
        // Tilt 45°, field ±30°: y = (él − 45) / 30.
        let o = OutdoorSettings { mounting: Mounting { scan_angle_deg: 60.0, tilt_deg: 45.0, ..Default::default() }, ..outdoor(vec![sector(0.0, 360.0, -90.0, 90.0)]) };
        let y30 = (30.0 - 45.0) / 30.0;
        let y60 = (60.0 - 45.0) / 30.0;
        assert!(!o.allows(0.0, y30, 0.0));
        assert!(o.allows(0.0, y60, 0.0));
        let out = run(&o, [beam(0.0, y30), beam(0.0, y60)].concat());
        assert!(!lit_at(&out, 0.0, y30), "30° elevation (60° from vertical) is cut");
        assert!(lit_at(&out, 0.0, y60), "60° elevation passes");
        // Waived with its reference: 30° passes too.
        let waived = OutdoorSettings { vertical_limit_waived: true, waiver_ref: "GDF-12 art. X".into(), ..o.clone() };
        assert!(lit_at(&run(&waived, beam(0.0, y30)), 0.0, y30));
        // Waived without a reference: refused, and the limit stays.
        let bare = OutdoorSettings { vertical_limit_waived: true, ..o };
        assert!(bare.validate().is_err());
        assert!(!bare.allows(0.0, y30, 0.0));
        assert!(!bare.sanitized().vertical_limit_waived);
    }

    #[test]
    fn a_beam_outside_the_sectors_is_never_lit_and_lines_are_cut_at_the_edge() {
        // Field ±20°, azimuth 0: sector 350..10 through north, ground only.
        let o = outdoor(vec![sector(350.0, 10.0, -20.0, 0.0)]);
        let out = run(&o, [beam(0.25, -0.5), beam(0.75, -0.5), beam(-0.75, -0.5)].concat());
        assert!(lit_at(&out, 0.25, -0.5), "5° east, 10° down: inside");
        assert!(!lit_at(&out, 0.75, -0.5), "15° east: outside");
        assert!(!lit_at(&out, -0.75, -0.5), "15° west: outside");
        // A line across the sector's east edge (x = 0.5): cut there.
        let line = vec![Point::lit(0.0, -0.5, 1.0, 0.0, 0.0), Point::lit(1.0, -0.5, 1.0, 0.0, 0.0)];
        let out = run(&o, line);
        assert_eq!(violations(&out, &o, 1e-3), 0);
        assert!(out.iter().any(|p| p.is_lit() && p.x > 0.49 && p.x < 0.5), "lit up to the edge");
        assert!(!out.iter().any(|p| p.is_lit() && p.x >= 0.5));
    }

    #[test]
    fn sky_off_cuts_the_sky_and_keeps_the_ground_show() {
        let o = OutdoorSettings { sky_off: true, ..Default::default() };
        let out = run(&o, [beam(0.0, 0.5), beam(0.3, -0.5), beam(-0.2, 0.0)].concat());
        assert!(!lit_at(&out, 0.0, 0.5), "above the horizon: cut");
        assert!(!lit_at(&out, -0.2, 0.0), "on the horizon: cut");
        assert!(lit_at(&out, 0.3, -0.5), "the ground show goes on");
        // Mode off and sky on: nothing cut.
        assert!(OutdoorSettings::default().blank_zones().is_empty());
        assert!(lit_at(&run(&OutdoorSettings::default(), beam(0.0, 0.5)), 0.0, 0.5));
    }

    #[test]
    fn missing_or_broken_geometry_cuts_everything() {
        let field = [beam(0.0, -0.9), beam(0.5, 0.5), beam(-0.9, 0.9)].concat();
        // No sector declared.
        assert!(!run(&outdoor(vec![]), field.clone()).iter().any(|p| p.is_lit()));
        // A projector with no mounting, mode on or sky off.
        for o in [OutdoorSettings { projector: "p2".into(), ..outdoor(vec![sector(0.0, 360.0, -90.0, 90.0)]) }, OutdoorSettings { projector: "p2".into(), sky_off: true, ..Default::default() }] {
            assert!(!run(&o, field.clone()).iter().any(|p| p.is_lit()));
        }
        // A NaN mounting: refused, and a file with one cuts everything.
        let nan = OutdoorSettings { mounting: Mounting { tilt_deg: f32::NAN, ..Default::default() }, ..Default::default() };
        assert!(nan.validate().is_err());
        let fixed = nan.sanitized();
        assert!(fixed.enabled && fixed.sectors.is_empty() && !fixed.authorized());
        assert!(!run(&fixed, field.clone()).iter().any(|p| p.is_lit()));
        // An inverted or NaN sector is refused and dropped.
        let bad = outdoor(vec![sector(0.0, 10.0, 20.0, -20.0), sector(f32::NAN, 10.0, 0.0, 1.0)]);
        assert!(bad.validate().is_err());
        assert!(bad.sanitized().sectors.is_empty());
        // A broken mounting itself still cuts (whatever reached the engine).
        let nan_on = OutdoorSettings { enabled: true, ..nan };
        assert!(!run(&nan_on, field).iter().any(|p| p.is_lit()));
    }

    #[test]
    fn loosening_needs_confirmation() {
        let o = outdoor(vec![sector(0.0, 90.0, -10.0, 60.0)]);
        assert!(OutdoorSettings::default().loosenings(&o).is_empty(), "turning it on tightens");
        assert!(o.loosenings(&o).is_empty());
        let narrower = outdoor(vec![sector(10.0, 80.0, 0.0, 50.0)]);
        assert!(o.loosenings(&narrower).is_empty());
        assert!(o.loosenings(&OutdoorSettings { max_from_vertical_deg: 30.0, ..o.clone() }).is_empty());
        for (new, what) in [
            (OutdoorSettings { enabled: false, ..o.clone() }, "désactivé"),
            (outdoor(vec![sector(0.0, 100.0, -10.0, 60.0)]), "élargi"),
            (OutdoorSettings { max_from_vertical_deg: 60.0, ..o.clone() }, "verticale"),
            (OutdoorSettings { vertical_limit_waived: true, waiver_ref: "X".into(), ..o.clone() }, "levée"),
            (OutdoorSettings { mounting: Mounting { tilt_deg: -5.0, ..Default::default() }, ..o.clone() }, "Montage"),
        ] {
            let l = o.loosenings(&new);
            assert!(l.iter().any(|m| m.contains(what)), "{what}: {l:?}");
        }
    }

    /// T-477: another projector's mounting, mode on or off (« Ciel coupé »
    /// follows it): declaring or changing one lets light out, removing
    /// one only cuts more.
    #[test]
    fn another_projectors_mounting_needs_confirmation_unless_removed() {
        let p2 = Mounting { scan_angle_deg: 30.0, tilt_deg: 10.0, azimuth_deg: 180.0, height_m: 4.0 };
        for enabled in [true, false] {
            let base = OutdoorSettings { enabled, ..outdoor(vec![sector(0.0, 90.0, -10.0, 60.0)]) };
            let with = OutdoorSettings { mountings: [("p2".to_string(), p2)].into_iter().collect(), ..base.clone() };
            let l = base.loosenings(&with);
            assert!(l.iter().any(|m| m.contains("« p2 » déclaré")), "{enabled}: {l:?}");
            let moved = OutdoorSettings { mountings: [("p2".to_string(), Mounting { tilt_deg: 20.0, ..p2 })].into_iter().collect(), ..base.clone() };
            let l = with.loosenings(&moved);
            assert!(l.iter().any(|m| m.contains("« p2 » modifié")), "{enabled}: {l:?}");
            assert!(with.loosenings(&base).is_empty(), "{enabled}: removing a mounting only cuts more");
            assert!(with.loosenings(&with).is_empty());
        }
    }

    #[test]
    fn export_matches_what_was_entered() {
        let o = outdoor(vec![sector(350.0, 10.0, 5.5, 80.0), sector(90.0, 120.25, -10.0, 0.0)]);
        let csv = o.export_csv();
        let rows: Vec<&str> = csv.lines().collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], "1;350;10;5.5;80");
        assert_eq!(rows[2], "2;90;120.25;-10;0");
        let back: Vec<Sector> = rows[1..]
            .iter()
            .map(|r| {
                let v: Vec<f32> = r.split(';').skip(1).map(|c| c.parse().unwrap()).collect();
                sector(v[0], v[1], v[2], v[3])
            })
            .collect();
        assert_eq!(back, o.sectors);
    }

    #[test]
    fn old_files_load_with_the_mode_off() {
        let s: SafetySettings = serde_json::from_str(r#"{"strobe_max_hz":3}"#).unwrap();
        assert_eq!(s.outdoor, OutdoorSettings::default());
        let o: OutdoorSettings = serde_json::from_str(r#"{"enabled":true,"sky_off":true}"#).unwrap();
        assert!(o.enabled && !o.sky_off, "« Ciel coupé » is never loaded from a file");
        assert!(!serde_json::to_string(&o).unwrap().contains("sky_off"));
    }

    /// Random mountings, sectors, limits, calibrations and frames: nothing
    /// lit where the angles forbid (invariant 4 of safety_invariants, here
    /// on the safety stage alone with many more geometries). The frame is
    /// calibrated first, as in the engine: the cut is in output space.
    #[test]
    fn no_lit_point_where_the_angles_forbid() {
        use crate::safety_invariants::{random_calibration, random_outdoor, Rng};
        let mut rng = Rng::new(0x0d00_0262);
        let cases = std::env::var("SAFETY_CASES").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(300);
        let (mut lit_cases, mut cut_cases) = (0, 0);
        for case in 0..cases {
            let on = rng.chance(0.8);
            let o = random_outdoor(&mut rng, on);
            let cal = random_calibration(&mut rng);
            let frame: Vec<Point> = (0..rng.below(60) + 2)
                .map(|_| {
                    let (x, y) = cal.apply(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
                    if rng.chance(0.8) {
                        Point::lit(x, y, rng.f(), rng.f(), rng.f())
                    } else {
                        Point::blanked(x, y)
                    }
                })
                .collect();
            let lit_in = frame.iter().filter(|p| p.is_lit()).count();
            let out = run(&o, frame);
            let lit_out = out.iter().filter(|p| p.is_lit()).count();
            lit_cases += (lit_out > 0) as usize;
            cut_cases += (lit_out < lit_in) as usize;
            let v = violations(&out, &o, 1e-3);
            assert_eq!(v, 0, "case {case}: {v} violations with {o:?}");
        }
        // The cases must exercise both sides of the cut.
        assert!(lit_cases * 5 > cases && cut_cases * 5 > cases, "{lit_cases} lit, {cut_cases} cut of {cases}");
    }
}
