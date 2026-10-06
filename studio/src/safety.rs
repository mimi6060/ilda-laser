//! Output-side safety applied to every look (T-101): a strobe limiter and a
//! provisional beam horizon. Both run on the finished frame - after the
//! layers mix, the live stage (colour, LFOs) and calibration - just before
//! the output gate. Nothing that makes light - a beat
//! gate, a brightness LFO, the audio flash, a flash cue, a fast chase -
//! can reach the laser without passing through them, and the preview
//! shows the limited frame, so it shows what the laser would draw.
//!
//! **Strobe limiter.** The limiter only looks at the light that comes
//! out: the mean drive level of the frame (the average of each point's
//! brightest colour, blanked points counting 0), which is proportional to
//! the optical power at a fixed point rate. A frame is "on" once it reaches
//! `ON_FRAC` of the recent peak and "off" once it drops to `OFF_FRAC`
//! (hysteresis, so noise can't count as flashes). Every off → on edge is a
//! flash. Flashing faster than `strobe_max_hz` (research: at most about
//! 4 Hz sustained, docs/research/festival-looks.md §4.4) starts a burst;
//! after `strobe_burst_s` (5 s) of burst the output is held continuously
//! lit - dark frames are replaced by the last lit one - for at least
//! `strobe_cooldown_s` (2 s) **and** until the input has stopped fast
//! flashing for that long. A pause shorter than the cooldown does not
//! reset the burst clock, so 4.9 s bursts with short gaps are still cut.
//!
//! **Last word on the output (T-360).** The hold puts back a frame that
//! the static-beam guard and the power caps must still see, so those run
//! after the limiter - and they can make light alternate too (the guard
//! blanks a lone beam, the next frame is a moving chase), as can the
//! hold itself (a fill that starts in a dark gap, a look really stopped
//! that comes back). So the limiter also measures the frame that really
//! goes out, after the guard and the caps (`StrobeLimiter::finish`), with
//! the same flash detector: an off → on edge that would make a run of
//! flashes closer than one allowed period last longer than the burst is
//! refused - the frame stays dark until a period has passed since the
//! last flash out. It can only darken, so the guard, caps, zones and
//! horizon still hold, and the output never flashes faster than the
//! limit for longer than the burst, whatever any stage does.
//!
//! **Beam horizon.** A beam (a lit point held in place, which is what
//! reads as a beam in haze) below `horizon.y` is always blanked, whatever
//! else is configured. The height is in output coordinates (after
//! calibration), i.e. where the beam really goes, which is also what the
//! preview draws.
//!
//! **Zones, full horizon, colour calibration (T-003, `zones.rs`).** Blank
//! and Dim polygons, the horizon extended to lines/text/sheets with a ramp,
//! per-colour gain and minimum diode level. Applied to the frame before the
//! limiter measures it (so the limiter sees the light that really goes
//! out), and again to a held frame the limiter puts back, with the current
//! settings.
//!
//! **Public zones (T-255, `audience.rs`).** A zone marked `audience` is
//! where the audience is. Outside audience scanning mode the engine applies
//! `audience::effective`, which turns every Public zone into a Blank one;
//! a Dim Public zone keeps at most `audience::MAX_AUDIENCE_LEVEL`.
//!
//! These settings belong to the machine (`safety.json`), never to a look or
//! a project. The strobe limits can never be looser than the defaults, and
//! nothing can be loosened relative to the current settings (a zone removed
//! or moved, the horizon lowered, a gain raised...) without the operator's
//! explicit confirmation (`SafetyStore::set(.., confirm_loosen)`).

use crate::dwell::DwellGuard;
use crate::outdoor::OutdoorSettings;
use crate::patterns::Point;
use crate::zones::{Horizon, Mask, Zone, ZoneGradient, ZoneKind, MAX_FEATHER, MAX_VERTICES, MAX_ZONES};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;

/// Photosensitivity practice (festival-looks.md §4.4): sustained flashing
/// at or below 4 per second, faster bursts at most 5 s.
pub const DEFAULT_MAX_HZ: f32 = 4.0;
pub const DEFAULT_BURST_S: f32 = 5.0;
pub const DEFAULT_COOLDOWN_S: f32 = 2.0;

/// Global output safety settings of this machine (never part of a look or
/// a project), saved in `safety.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "SafetyWire")]
pub struct SafetySettings {
    /// Fastest sustained flash rate allowed, in flashes per second.
    pub strobe_max_hz: f32,
    /// Longest burst of faster flashing, in seconds.
    pub strobe_burst_s: f32,
    /// How long the output stays steady after a burst was cut, in seconds.
    pub strobe_cooldown_s: f32,
    /// Projection zones (T-003), at most `zones::MAX_ZONES`.
    pub zones: Vec<Zone>,
    /// Beams below `horizon.y` are blanked; with `lines`, everything is.
    pub horizon: Horizon,
    /// Per-colour gain (red, green, blue), 0..1.
    pub color_gain: [f32; 3],
    /// Lowest drive of a lit channel (diode threshold), 0..0.5.
    pub min_diode_level: f32,
    /// Static beam guard (T-256, `dwell.rs`).
    pub dwell: DwellGuard,
    /// Outdoor mode (T-262, `outdoor.rs`): declared sectors, the vertical
    /// limit, and « Ciel coupé » (runtime, never saved).
    pub outdoor: OutdoorSettings,
}

impl Default for SafetySettings {
    fn default() -> Self {
        Self {
            strobe_max_hz: DEFAULT_MAX_HZ,
            strobe_burst_s: DEFAULT_BURST_S,
            strobe_cooldown_s: DEFAULT_COOLDOWN_S,
            zones: Vec::new(),
            horizon: Horizon::default(),
            color_gain: [1.0; 3],
            min_diode_level: 0.0,
            dwell: DwellGuard::default(),
            outdoor: OutdoorSettings::default(),
        }
    }
}

impl SafetySettings {
    /// What a damaged `safety.json` falls back to (T-402): the defaults
    /// with the static beam guard on its strict profile everywhere. Never
    /// looser than the defaults in any field.
    pub fn strictest() -> Self {
        Self { dwell: DwellGuard { profile: crate::dwell::DwellProfile::Strict, ..DwellGuard::default() }, ..Self::default() }
    }
}

/// What `safety.json` and `POST /api/safety` may contain: the settings,
/// plus T-101's `beam_floor_y` (the horizon height) from older files.
#[derive(Deserialize)]
#[serde(default)]
struct SafetyWire {
    strobe_max_hz: f32,
    strobe_burst_s: f32,
    strobe_cooldown_s: f32,
    zones: Vec<Zone>,
    horizon: Option<Horizon>,
    beam_floor_y: Option<f32>,
    color_gain: [f32; 3],
    min_diode_level: f32,
    dwell: DwellGuard,
    outdoor: OutdoorSettings,
}

impl Default for SafetyWire {
    fn default() -> Self {
        let d = SafetySettings::default();
        Self {
            strobe_max_hz: d.strobe_max_hz,
            strobe_burst_s: d.strobe_burst_s,
            strobe_cooldown_s: d.strobe_cooldown_s,
            zones: d.zones,
            horizon: None,
            beam_floor_y: None,
            color_gain: d.color_gain,
            min_diode_level: d.min_diode_level,
            dwell: d.dwell,
            outdoor: d.outdoor,
        }
    }
}

impl From<SafetyWire> for SafetySettings {
    fn from(w: SafetyWire) -> Self {
        let horizon = w.horizon.unwrap_or(Horizon { y: w.beam_floor_y.unwrap_or(Horizon::default().y), ..Horizon::default() });
        Self {
            strobe_max_hz: w.strobe_max_hz,
            strobe_burst_s: w.strobe_burst_s,
            strobe_cooldown_s: w.strobe_cooldown_s,
            zones: w.zones,
            horizon,
            color_gain: w.color_gain,
            min_diode_level: w.min_diode_level,
            dwell: w.dwell,
            outdoor: w.outdoor,
        }
    }
}

const MAX_HZ_RANGE: (f32, f32) = (0.5, DEFAULT_MAX_HZ);
const BURST_RANGE: (f32, f32) = (0.0, DEFAULT_BURST_S);
const COOLDOWN_RANGE: (f32, f32) = (DEFAULT_COOLDOWN_S, 30.0);
const FLOOR_RANGE: (f32, f32) = (-1.0, 1.0);
const RAMP_RANGE: (f32, f32) = (0.0, 0.5);
const UNIT_RANGE: (f32, f32) = (0.0, 1.0);
const MIN_DIODE_RANGE: (f32, f32) = (0.0, 0.5);
/// Soft edge width of a Dim zone (T-446), in output units.
const FEATHER_RANGE: (f32, f32) = (0.0, MAX_FEATHER);
const MAX_ZONE_NAME: usize = 40;

fn fix(v: f32, (lo, hi): (f32, f32), def: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        def
    }
}

impl SafetySettings {
    /// Refuses values that are not numbers, out of range, or looser than
    /// the safe defaults for the strobe (French messages for the UI).
    pub fn validate(&self) -> Result<()> {
        let check = |v: f32, (lo, hi): (f32, f32), what: &str| -> Result<()> {
            if !v.is_finite() || v < lo || v > hi {
                bail!("{what} : {v} hors limites ({lo} à {hi})");
            }
            Ok(())
        };
        check(self.strobe_max_hz, MAX_HZ_RANGE, "Strobe max (Hz)")?;
        check(self.strobe_burst_s, BURST_RANGE, "Rafale max (s)")?;
        check(self.strobe_cooldown_s, COOLDOWN_RANGE, "Pause après rafale (s)")?;
        check(self.horizon.y, FLOOR_RANGE, "Horizon")?;
        check(self.horizon.ramp, RAMP_RANGE, "Rampe de l'horizon")?;
        check(self.horizon.level, UNIT_RANGE, "Luminosité sous l'horizon")?;
        for (c, name) in self.color_gain.iter().zip(["rouge", "vert", "bleu"]) {
            check(*c, UNIT_RANGE, &format!("Gain {name}"))?;
        }
        check(self.min_diode_level, MIN_DIODE_RANGE, "Niveau minimum des diodes")?;
        self.dwell.validate()?;
        self.outdoor.validate()?;
        if self.zones.len() > MAX_ZONES {
            bail!("{} zones : {MAX_ZONES} au maximum", self.zones.len());
        }
        for z in &self.zones {
            let what = format!("Zone « {} »", z.name);
            if z.points.len() < 3 || z.points.len() > MAX_VERTICES {
                bail!("{what} : {} sommets (3 à {MAX_VERTICES})", z.points.len());
            }
            for p in &z.points {
                check(p[0], FLOOR_RANGE, &what)?;
                check(p[1], FLOOR_RANGE, &what)?;
            }
            check(z.level, UNIT_RANGE, &format!("{what}, luminosité"))?;
            check(z.feather, FEATHER_RANGE, &format!("{what}, bord doux"))?;
            if let Some(g) = &z.gradient {
                for v in g.from.iter().chain(&g.to) {
                    check(*v, FLOOR_RANGE, &format!("{what}, dégradé"))?;
                }
                check(g.level_to, UNIT_RANGE, &format!("{what}, luminosité à l'arrivée"))?;
                if z.audience && z.kind == ZoneKind::Dim && g.level_to > crate::audience::MAX_AUDIENCE_LEVEL {
                    bail!(
                        "{what} (Public) : luminosité à l'arrivée {:.0} %, {:.0} % au plus (atténuation d'au moins 50 %)",
                        g.level_to * 100.0,
                        crate::audience::MAX_AUDIENCE_LEVEL * 100.0
                    );
                }
            }
            // A Public zone attenuates by 50 % at least (T-255).
            if z.audience && z.kind == ZoneKind::Dim && z.level > crate::audience::MAX_AUDIENCE_LEVEL {
                bail!(
                    "{what} (Public) : luminosité {:.0} %, {:.0} % au plus (atténuation d'au moins 50 %)",
                    z.level * 100.0,
                    crate::audience::MAX_AUDIENCE_LEVEL * 100.0
                );
            }
            if z.name.chars().count() > MAX_ZONE_NAME {
                bail!("{what} : nom trop long ({MAX_ZONE_NAME} caractères au plus)");
            }
            if z.id != 0 && self.zones.iter().filter(|o| o.id == z.id).count() > 1 {
                bail!("{what} : identifiant {} en double", z.id);
            }
        }
        Ok(())
    }

    /// Clamps every value into its allowed range (NaN → the tighter
    /// value), for a file edited by hand. Zones are kept (dropping one
    /// would loosen), except those with fewer than 3 vertices.
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        let mut zones: Vec<Zone> = self
            .zones
            .iter()
            .filter(|z| z.points.len() >= 3)
            .map(|z| {
                let levels = if z.audience { (0.0, crate::audience::MAX_AUDIENCE_LEVEL) } else { UNIT_RANGE };
                let mut level = fix(z.level, levels, 0.0);
                let (mut feather, mut gradient) = (0.0, None);
                // Soft edges and gradients are for Dim zones only (T-446).
                if z.kind == ZoneKind::Dim {
                    // Not a number → the widest edge, the tighter value.
                    feather = fix(z.feather, FEATHER_RANGE, MAX_FEATHER);
                    if let Some(g) = &z.gradient {
                        let level_to = fix(g.level_to, levels, 0.0);
                        if g.from.iter().chain(&g.to).all(|v| v.is_finite()) {
                            let pos = |p: [f32; 2]| p.map(|v| v.clamp(FLOOR_RANGE.0, FLOOR_RANGE.1));
                            gradient = Some(ZoneGradient { from: pos(g.from), to: pos(g.to), level_to });
                        } else {
                            // No usable axis: the darker level everywhere.
                            level = level.min(level_to);
                        }
                    }
                }
                Zone {
                    points: z.points.iter().map(|p| [fix(p[0], FLOOR_RANGE, 0.0), fix(p[1], FLOOR_RANGE, 0.0)]).collect(),
                    level,
                    feather,
                    gradient,
                    ..z.clone()
                }
            })
            .collect();
        // Unique ids (0 or duplicates get fresh ones).
        let mut next = zones.iter().map(|z| z.id).max().unwrap_or(0);
        for i in 0..zones.len() {
            if zones[i].id == 0 || zones[..i].iter().any(|o| o.id == zones[i].id) {
                next += 1;
                zones[i].id = next;
            }
        }
        Self {
            strobe_max_hz: fix(self.strobe_max_hz, MAX_HZ_RANGE, d.strobe_max_hz),
            strobe_burst_s: fix(self.strobe_burst_s, BURST_RANGE, d.strobe_burst_s),
            strobe_cooldown_s: fix(self.strobe_cooldown_s, COOLDOWN_RANGE, d.strobe_cooldown_s),
            zones,
            horizon: Horizon {
                y: fix(self.horizon.y, FLOOR_RANGE, 1.0),
                lines: self.horizon.lines,
                ramp: fix(self.horizon.ramp, RAMP_RANGE, d.horizon.ramp),
                level: fix(self.horizon.level, UNIT_RANGE, 0.0),
            },
            color_gain: self.color_gain.map(|c| fix(c, UNIT_RANGE, 0.0)),
            min_diode_level: fix(self.min_diode_level, MIN_DIODE_RANGE, 0.0),
            dwell: self.dwell.sanitized(),
            outdoor: self.outdoor.sanitized(),
        }
    }

    /// The compiled zones, horizon and colour calibration, plus the
    /// outdoor cut (T-262) as extra Blank zones.
    pub fn mask(&self) -> Mask {
        let outdoor = self.outdoor.blank_zones();
        if outdoor.is_empty() {
            Mask::new(&self.zones, self.horizon, self.color_gain, self.min_diode_level)
        } else {
            let all: Vec<Zone> = self.zones.iter().cloned().chain(outdoor).collect();
            Mask::new(&all, self.horizon, self.color_gain, self.min_diode_level)
        }
    }

    /// What going from `self` to `new` would loosen, in French, for the
    /// confirmation the operator must give. Empty = only tighter or equal.
    pub fn loosenings(&self, new: &SafetySettings) -> Vec<String> {
        let mut out = Vec::new();
        let mut more = |cond: bool, msg: String| {
            if cond {
                out.push(msg);
            }
        };
        more(new.strobe_max_hz > self.strobe_max_hz, format!("Strobe max : {} → {} Hz", self.strobe_max_hz, new.strobe_max_hz));
        more(new.strobe_burst_s > self.strobe_burst_s, format!("Rafale max : {} → {} s", self.strobe_burst_s, new.strobe_burst_s));
        more(new.strobe_cooldown_s < self.strobe_cooldown_s, format!("Pause après rafale : {} → {} s", self.strobe_cooldown_s, new.strobe_cooldown_s));
        let (h, n) = (self.horizon, new.horizon);
        more(n.y < h.y, format!("Horizon abaissé : {:.2} → {:.2}", h.y, n.y));
        more(h.lines && !n.lines, "L'horizon ne coupe plus les lignes et figures".into());
        if h.lines && n.lines {
            more(n.level > h.level, format!("Luminosité sous l'horizon : {:.0} % → {:.0} %", h.level * 100.0, n.level * 100.0));
            more(n.ramp < h.ramp, format!("Rampe de l'horizon : {:.2} → {:.2}", h.ramp, n.ramp));
        }
        for (i, name) in ["rouge", "vert", "bleu"].iter().enumerate() {
            more(
                new.color_gain[i] > self.color_gain[i],
                format!("Gain {name} : {:.0} % → {:.0} %", self.color_gain[i] * 100.0, new.color_gain[i] * 100.0),
            );
        }
        more(
            new.min_diode_level > self.min_diode_level,
            format!("Niveau minimum des diodes : {:.0} % → {:.0} %", self.min_diode_level * 100.0, new.min_diode_level * 100.0),
        );
        for z in &self.zones {
            match new.zones.iter().find(|n| n.id == z.id) {
                None => more(true, format!("Zone « {} » supprimée", z.name)),
                Some(n) => {
                    more(z.audience && !n.audience, format!("Zone « {} » : n'est plus une zone Public", z.name));
                    more(z.kind == ZoneKind::Blank && n.kind == ZoneKind::Dim, format!("Zone « {} » : masquée → atténuée", z.name));
                    // A soft edge darkens around the zone (T-446); a Blank
                    // zone has none, so masking it lights that band (T-260).
                    more(
                        z.kind == ZoneKind::Dim && n.kind == ZoneKind::Blank && z.soft_feather() > 0.0,
                        format!("Zone « {} » : atténuée → masquée, le bord doux autour disparaît", z.name),
                    );
                    more(
                        z.kind == ZoneKind::Dim && n.kind == ZoneKind::Dim && n.level > z.level,
                        format!("Zone « {} » : luminosité {:.0} % → {:.0} %", z.name, z.level * 100.0, n.level * 100.0),
                    );
                    more(n.points != z.points, format!("Zone « {} » déplacée ou redessinée", z.name));
                    if z.kind == ZoneKind::Dim && n.kind == ZoneKind::Dim {
                        Self::soft_loosenings(z, n, &mut more);
                    }
                }
            }
        }
        out.extend(self.dwell.loosenings(&new.dwell));
        out.extend(self.outdoor.loosenings(&new.outdoor));
        out
    }

    /// What a Dim zone's soft edge and gradient changes loosen (T-446): a
    /// narrower soft edge; a higher arrival level, a moved axis, a removed
    /// gradient darker than the zone, or an added one brighter than it.
    fn soft_loosenings(z: &Zone, n: &Zone, more: &mut impl FnMut(bool, String)) {
        // The soft edge as a percentage of the field width (2 units).
        let pct = |f: f32| f * 50.0;
        let (zf, nf) = (z.soft_feather(), n.soft_feather());
        more(nf < zf, format!("Zone « {} » : bord doux {:.0} % → {:.0} %", z.name, pct(zf), pct(nf)));
        match (z.soft_gradient(), n.soft_gradient()) {
            (Some(g), Some(h)) => {
                more(
                    h.level_to > g.level_to,
                    format!("Zone « {} » : luminosité à l'arrivée {:.0} % → {:.0} %", z.name, g.level_to * 100.0, h.level_to * 100.0),
                );
                more(h.from != g.from || h.to != g.to, format!("Zone « {} » : dégradé déplacé", z.name));
            }
            (Some(g), None) => more(
                g.level_to < n.level,
                format!("Zone « {} » : dégradé retiré (arrivée {:.0} % → {:.0} %)", z.name, g.level_to * 100.0, n.level * 100.0),
            ),
            (None, Some(h)) => more(
                h.level_to > z.level,
                format!("Zone « {} » : luminosité à l'arrivée {:.0} % → {:.0} %", z.name, z.level * 100.0, h.level_to * 100.0),
            ),
            (None, None) => {}
        }
    }
}

/// Why `SafetyStore::set` refused new settings.
#[derive(Debug)]
pub enum SetError {
    /// Out of range, too loose for the strobe, or not saved (message).
    Invalid(String),
    /// Looser than the current settings: needs the operator's explicit
    /// confirmation. The French list of what would be loosened.
    Loosens(Vec<String>),
}

pub struct SafetyStore {
    /// None: in memory only (tests).
    path: Option<PathBuf>,
    settings: SafetySettings,
    /// The file existed but could not be read (shown in the UI).
    load_error: Option<String>,
}

impl SafetyStore {
    /// A missing file gives the safe defaults. A damaged one is moved to
    /// the quarantine folder (T-402), the *strictest* settings are used
    /// (`SafetySettings::strictest`) and the error is reported, so the
    /// operator sees that their zones were not loaded; arming waits for
    /// their acknowledgement (`data_ack`).
    pub fn load_or_create(path: PathBuf) -> Self {
        let (settings, load_error) = match crate::datafile::load::<SafetySettings>(&path, &crate::datafile::beside(&path), true) {
            crate::datafile::Loaded::Missing => (SafetySettings::default(), None),
            crate::datafile::Loaded::Ok(s) => (s.sanitized(), None),
            crate::datafile::Loaded::Quarantined(w) => (SafetySettings::strictest(), Some(format!("{} Réglages de sécurité les plus stricts appliqués.", w.message_fr()))),
        };
        Self { path: Some(path), settings, load_error }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { path: None, settings: SafetySettings::default(), load_error: None }
    }

    pub fn get(&self) -> SafetySettings {
        self.settings.clone()
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Validates, gives new zones an id, and saves. Anything looser than
    /// the current settings is refused unless `confirm_loosen` (the
    /// operator's explicit action). Returns the stored settings.
    pub fn set(&mut self, settings: SafetySettings, confirm_loosen: bool) -> std::result::Result<SafetySettings, SetError> {
        settings.validate().map_err(|e| SetError::Invalid(e.to_string()))?;
        let mut settings = settings;
        // « Ciel coupé » and the projector are the engine's, never stored.
        settings.outdoor.sky_off = false;
        settings.outdoor.projector.clear();
        let mut next = self.settings.zones.iter().chain(&settings.zones).map(|z| z.id).max().unwrap_or(0);
        for z in &mut settings.zones {
            if z.id == 0 {
                next += 1;
                z.id = next;
            }
        }
        let loosens = self.settings.loosenings(&settings);
        if !loosens.is_empty() && !confirm_loosen {
            return Err(SetError::Loosens(loosens));
        }
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&settings).map_err(|e| SetError::Invalid(e.to_string()))?;
            crate::datafile::replace(path, json.as_bytes()).map_err(|e| SetError::Invalid(format!("enregistrement impossible de {} : {e:#}", path.display())))?;
        }
        if !loosens.is_empty() {
            log::warn!("safety settings loosened by the operator: {}", loosens.join("; "));
        }
        self.settings = settings.clone();
        self.load_error = None;
        Ok(settings)
    }
}

// ---------- beam horizon ----------

/// This many coincident lit points in a row make a beam. Beams are drawn
/// with 12 (generators) or 6 (the "dots" shape) held points; outline
/// corners and line ends get 4 (`engine::CORNER_DWELL` + 1).
const BEAM_RUN: usize = 6;
/// Points closer than this (normalised units) count as the same spot, so
/// a figure shrunk to nearly nothing counts as a beam too.
const SAME_SPOT: f32 = 2e-3;

/// Blanks every beam whose position is below `floor_y`. Returns how many
/// beams were blanked. Idempotent.
pub fn blank_low_beams(points: &mut [Point], floor_y: f32) -> usize {
    let mut blanked = 0;
    let mut i = 0;
    while i < points.len() {
        let anchor = points[i];
        let mut j = i + 1;
        while j < points.len() && (points[j].x - anchor.x).abs() <= SAME_SPOT && (points[j].y - anchor.y).abs() <= SAME_SPOT {
            j += 1;
        }
        let lit = points[i..j].iter().filter(|p| p.is_lit()).count();
        if lit >= BEAM_RUN && points[i..j].iter().any(|p| p.is_lit() && p.y < floor_y) {
            for p in &mut points[i..j] {
                *p = Point::blanked(p.x, p.y);
            }
            blanked += 1;
        }
        i = j;
    }
    blanked
}

// ---------- strobe limiter ----------

/// A frame counts as "on" at this fraction of the recent peak level...
const ON_FRAC: f32 = 0.6;
/// ...and as "off" again at this fraction.
const OFF_FRAC: f32 = 0.3;
/// Below this mean level a frame is dark whatever the peak.
const MIN_LEVEL: f32 = 1e-3;
/// Time constant of the peak follower, in seconds.
const PEAK_DECAY_S: f32 = 1.0;
/// Flash onsets kept to measure the rate.
const RATE_ONSETS: usize = 8;
/// Timing slack for frame quantisation (two frames at 60 fps), so a
/// strobe at exactly the limit, sampled with jitter, is never cut.
const SLACK_S: f64 = 0.034;

/// What the limiter is doing, for the UI and `/api/frame`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct StrobeStatus {
    /// The output is being held steady (the « Limiteur actif » light).
    pub active: bool,
    /// The input is flashing faster than the limit right now.
    pub fast: bool,
    /// Measured flash rate of the input (0 when not flashing).
    pub rate_hz: f32,
    /// Seconds of the current fast burst (0 when none).
    pub burst_s: f32,
    /// Beams blanked by the horizon in the last frame.
    pub beams_blanked: usize,
    /// Lit samples darkened by a zone or the horizon in the last frame.
    pub points_masked: usize,
    /// The last frame was kept dark by the check on the output (T-360):
    /// it would have flashed too soon after the previous flash out.
    pub output_cut: bool,
}

/// The strobe part of the settings, clamped (a copy per frame).
#[derive(Clone, Copy)]
struct StrobeCfg {
    strobe_max_hz: f32,
    strobe_burst_s: f32,
    strobe_cooldown_s: f32,
}

impl StrobeCfg {
    fn of(cfg: &SafetySettings) -> Self {
        let d = SafetySettings::default();
        Self {
            strobe_max_hz: fix(cfg.strobe_max_hz, MAX_HZ_RANGE, d.strobe_max_hz),
            strobe_burst_s: fix(cfg.strobe_burst_s, BURST_RANGE, d.strobe_burst_s),
            strobe_cooldown_s: fix(cfg.strobe_cooldown_s, COOLDOWN_RANGE, d.strobe_cooldown_s),
        }
    }
}

#[derive(Default)]
pub struct StrobeLimiter {
    last_t: Option<f64>,
    peak: f32,
    on: bool,
    onsets: VecDeque<f64>,
    burst_start: Option<f64>,
    last_fast: Option<f64>,
    hold_since: Option<f64>,
    dark_since: Option<f64>,
    held: Vec<Point>,
    status: StrobeStatus,
    /// The check on what really goes out (`finish`).
    out: OutputCheck,
}

/// The limiter's check on the final output (T-360): the same flash
/// detector as the input side, on the frame that leaves for the laser.
#[derive(Default)]
struct OutputCheck {
    last_t: Option<f64>,
    peak: f32,
    on: bool,
    /// The last flash (off → on edge) that went out.
    last_onset: Option<f64>,
    /// First flash of the current run of too-close flashes.
    burst_start: Option<f64>,
    /// Last time a flash came too soon after the previous one.
    last_fast: Option<f64>,
}

/// Mean drive level of a frame: proportional to its light output.
pub fn level(frame: &[Point]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    frame.iter().map(|p| p.r.max(p.g).max(p.b).clamp(0.0, 1.0)).sum::<f32>() / frame.len() as f32
}

impl StrobeLimiter {
    pub fn status(&self) -> StrobeStatus {
        self.status
    }

    /// Takes the frame rendered at time `t` (seconds, monotonic) and
    /// returns the frame to output (the limiter alone, for tests; the
    /// engine goes through `apply`).
    #[cfg(test)]
    pub fn process(&mut self, frame: Vec<Point>, t: f64, cfg: &SafetySettings) -> Vec<Point> {
        match self.step(&frame, frame.clone(), t, cfg) {
            Some(held) => held,
            None => frame,
        }
    }

    /// Measures a finished frame without changing it, and tells whether the
    /// limiter is now cutting (holding). The ILDA export (T-355) checks
    /// its looped file with a fresh limiter this way.
    pub fn would_cut(&mut self, frame: &[Point], t: f64, cfg: &SafetySettings) -> bool {
        self.step(frame, Vec::new(), t, cfg);
        self.status.active
    }

    /// Measures `measured` (the frame about to go out). Keeps `keep` as the
    /// frame to put back during a hold. Returns `Some(kept frame)` when the
    /// output must be replaced by it, `None` to send `measured` as it is.
    fn step(&mut self, measured: &[Point], keep: Vec<Point>, t: f64, cfg: &SafetySettings) -> Option<Vec<Point>> {
        let cfg = StrobeCfg::of(cfg);
        let dt = self.last_t.map_or(0.0, |last| (t - last).max(0.0)) as f32;
        self.last_t = Some(t);

        let e = level(measured);
        self.peak = e.max(self.peak * (-dt / PEAK_DECAY_S).exp());
        let bright = e > MIN_LEVEL && e >= ON_FRAC * self.peak;
        if !self.on && bright {
            self.on = true;
            self.onsets.push_back(t);
            if self.onsets.len() > RATE_ONSETS {
                self.onsets.pop_front();
            }
        } else if self.on && (e <= MIN_LEVEL || e <= OFF_FRAC * self.peak) {
            self.on = false;
        }

        let period = 1.0 / cfg.strobe_max_hz as f64;
        let fast_since = self.fast_since(t, period);
        if let Some(start) = fast_since {
            self.last_fast = Some(t);
            self.burst_start.get_or_insert(start);
        } else if self.hold_since.is_none() && self.last_fast.is_some_and(|lf| t - lf >= cfg.strobe_cooldown_s as f64) {
            self.burst_start = None;
        }
        if self.hold_since.is_none() && self.burst_start.is_some_and(|b| t - b >= cfg.strobe_burst_s as f64 - 1e-9) {
            self.hold_since = Some(t);
        }
        if let Some(h) = self.hold_since {
            let quiet = self.last_fast.is_none_or(|lf| t - lf >= cfg.strobe_cooldown_s as f64);
            if t - h >= cfg.strobe_cooldown_s as f64 && quiet {
                self.hold_since = None;
                self.burst_start = None;
            }
        }

        let out = if bright {
            self.held = keep;
            self.dark_since = None;
            None
        } else {
            let dark_for = t - *self.dark_since.get_or_insert(t);
            // While held, a dip shorter than one allowed flash period is
            // filled with the last lit frame; a longer one (the look was
            // really stopped) goes dark, which is at most one slow flash.
            if self.hold_since.is_some() && dark_for < period && !self.held.is_empty() {
                Some(self.held.clone())
            } else {
                None
            }
        };

        self.status = StrobeStatus {
            active: self.hold_since.is_some(),
            fast: fast_since.is_some(),
            rate_hz: self.rate(t, period),
            burst_s: self.burst_start.map_or(0.0, |b| (t - b) as f32),
            beams_blanked: 0,
            points_masked: 0,
            output_cut: false,
        };
        out
    }

    /// The last stage before the gate (T-360): measures `frame`, the frame
    /// that really goes out (after the static-beam guard and the caps),
    /// and darkens it when it would start a flash that makes the output
    /// flash faster than the limit for longer than the burst. Only ever
    /// darkens: a refused flash stays dark until one allowed period has
    /// passed since the last flash out.
    pub fn finish(&mut self, frame: &mut [Point], t: f64, cfg: &SafetySettings) {
        let cfg = StrobeCfg::of(cfg);
        let o = &mut self.out;
        let dt = o.last_t.map_or(0.0, |last| (t - last).max(0.0)) as f32;
        o.last_t = Some(t);
        let period = 1.0 / cfg.strobe_max_hz as f64;
        let cooldown = cfg.strobe_cooldown_s as f64;
        // Quiet for the cooldown: a new burst may start.
        if o.last_fast.is_none_or(|lf| t - lf >= cooldown) {
            o.burst_start = None;
        }

        let e = level(frame);
        let decayed = o.peak * (-dt / PEAK_DECAY_S).exp();
        let peak = e.max(decayed);
        let mut cut = false;
        if !o.on && e > MIN_LEVEL && e >= ON_FRAC * peak {
            // A flash would start. Too soon after the last one, it extends
            // a fast run; past the burst (plus the frame timing slack the
            // input side has too), it is refused.
            if let Some(last) = o.last_onset.filter(|&last| t - last < period - SLACK_S) {
                o.last_fast = Some(t);
                let start = *o.burst_start.get_or_insert(last);
                cut = t - start > cfg.strobe_burst_s as f64 + SLACK_S;
            }
            if !cut {
                o.on = true;
                o.last_onset = Some(t);
            }
        } else if o.on && (e <= MIN_LEVEL || e <= OFF_FRAC * peak) {
            o.on = false;
        }
        if cut {
            for p in frame.iter_mut() {
                *p = Point::blanked(p.x, p.y);
            }
            o.peak = decayed;
        } else {
            o.peak = peak;
        }
        self.status.output_cut = cut;
        self.status.active |= cut;
    }

    /// When the input is flashing faster than one flash per `period`, the
    /// time of the first onset of the longest recent run that is too fast.
    /// A run of k onsets is too fast if it spans less than (k-1) periods,
    /// minus a little slack for frame timing. Two onsets already make a
    /// run (T-354): the burst clock starts at the first of two flashes too
    /// close together, so with a burst shorter than about two periods the
    /// hold fills the dark gap before a third fast flash can go out.
    fn fast_since(&self, t: f64, period: f64) -> Option<f64> {
        let last = *self.onsets.back()?;
        if t - last > period + SLACK_S {
            return None; // stopped (or slowed) since the last flash
        }
        let n = self.onsets.len();
        (2..=n)
            .rev()
            .map(|k| self.onsets[n - k])
            .zip((2..=n).rev())
            .find(|&(first, k)| last - first < (k - 1) as f64 * period - SLACK_S)
            .map(|(first, _)| first)
    }

    /// Flash rate over the onsets that are still recent (0 when idle).
    fn rate(&self, t: f64, period: f64) -> f32 {
        let recent: Vec<f64> = self.onsets.iter().copied().filter(|&o| t - o <= 2.0).collect();
        match (recent.first(), recent.last()) {
            (Some(&a), Some(&b)) if recent.len() >= 2 && b > a && t - b <= 2.0 * period + SLACK_S => {
                ((recent.len() - 1) as f64 / (b - a)) as f32
            }
            _ => 0.0,
        }
    }
}

/// The whole output safety stage: beam horizon, zones / full horizon /
/// colour calibration, then the strobe limiter (its input side: the
/// engine ends the chain with `StrobeLimiter::finish`). A frame the limiter puts
/// back goes through the horizon and zones again with the current
/// settings (they may have been tightened since). Returns the output frame.
pub fn apply(frame: Vec<Point>, t: f64, cfg: &SafetySettings, limiter: &mut StrobeLimiter) -> Vec<Point> {
    let mask = cfg.mask();
    let mut raw = frame;
    let blanked = blank_low_beams(&mut raw, cfg.horizon.y);
    let (shaped, masked) = mask.apply(&raw);
    let out = match limiter.step(&shaped, raw, t, cfg) {
        None => shaped,
        Some(mut held) => {
            blank_low_beams(&mut held, cfg.horizon.y);
            mask.apply(&held).0
        }
    };
    limiter.status.beams_blanked = blanked;
    limiter.status.points_masked = masked;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, BeatClock, Content, Settings};
    use crate::generators::GenParams;

    const FPS: f64 = 60.0;

    fn lit_frame() -> Vec<Point> {
        (0..100).map(|i| Point::lit(i as f32 / 100.0, 0.5, 1.0, 1.0, 1.0)).collect()
    }

    fn dark_frame() -> Vec<Point> {
        (0..100).map(|i| Point::blanked(i as f32 / 100.0, 0.5)).collect()
    }

    /// Square-wave strobe at `hz` (duty 0..1), lit at t = 0.
    fn strobe(hz: f64, duty: f64) -> impl Fn(f64) -> bool {
        move |t: f64| (t * hz).fract() < duty
    }

    /// Runs `input(t)` (lit or dark) through a limiter at 60 fps for
    /// `secs`, returning (t, output lit?) per frame.
    fn run(input: impl Fn(f64) -> bool, secs: f64, cfg: &SafetySettings) -> (Vec<(f64, bool)>, StrobeLimiter) {
        let mut lim = StrobeLimiter::default();
        let mut out = Vec::new();
        for i in 0..(secs * FPS) as usize {
            let t = i as f64 / FPS;
            let frame = if input(t) { lit_frame() } else { dark_frame() };
            let f = lim.process(frame, t, cfg);
            out.push((t, level(&f) > 0.5));
        }
        (out, lim)
    }

    /// Time of the first frame from which the output stays lit to the end
    /// of `until` (None if it still flickers).
    fn steady_from(out: &[(f64, bool)], until: f64) -> Option<f64> {
        let within: Vec<_> = out.iter().filter(|(t, _)| *t < until).collect();
        let last_dark = within.iter().rposition(|(_, lit)| !lit);
        match last_dark {
            None => within.first().map(|(t, _)| *t),
            Some(i) if i + 1 < within.len() => Some(within[i + 1].0),
            Some(_) => None,
        }
    }

    fn edges(out: &[(f64, bool)], from: f64, to: f64) -> usize {
        let w: Vec<_> = out.iter().filter(|(t, _)| *t >= from && *t < to).collect();
        w.windows(2).filter(|p| !p[0].1 && p[1].1).count()
    }

    #[test]
    fn eight_hz_is_cut_to_steady_after_five_seconds() {
        let cfg = SafetySettings::default();
        let (out, lim) = run(strobe(8.0, 0.5), 7.0, &cfg);
        assert!(edges(&out, 0.0, 4.9) >= 38, "flashes freely during the burst");
        let steady = steady_from(&out, 7.0).expect("held steady");
        assert!((steady - 5.0).abs() <= 0.1, "cut after 5 s ± 0.1 s, got {steady}");
        assert!(lim.status().active);
        assert!(lim.status().rate_hz > 7.5 && lim.status().rate_hz < 8.5, "{:?}", lim.status());
    }

    #[test]
    fn seventeen_hz_is_cut_after_five_seconds_even_with_short_pulses() {
        let cfg = SafetySettings::default();
        // 1/8 beat at 128 BPM, 20 % duty (a stab).
        let (out, _) = run(strobe(17.0, 0.2), 7.0, &cfg);
        let steady = steady_from(&out, 7.0).expect("held steady");
        assert!((steady - 5.0).abs() <= 0.1, "got {steady}");
    }

    #[test]
    fn four_hz_and_slower_are_never_limited() {
        let cfg = SafetySettings::default();
        for hz in [2.0, 3.0, 4.0] {
            for duty in [0.1, 0.5] {
                let (out, lim) = run(strobe(hz, duty), 30.0, &cfg);
                let flashes = edges(&out, 0.0, 30.0);
                assert!(flashes as f64 >= hz * 30.0 - 2.0, "{hz} Hz: {flashes} flashes");
                assert!(out.iter().all(|&(t, lit)| lit == strobe(hz, duty)(t)), "{hz} Hz output = input");
                assert!(!lim.status().active && !lim.status().fast);
            }
        }
    }

    #[test]
    fn four_hz_with_frame_jitter_is_never_limited() {
        // Frames 10..26 ms apart (a busy machine), strobe exactly 4 Hz.
        let cfg = SafetySettings::default();
        let mut lim = StrobeLimiter::default();
        let (mut t, mut i) = (0.0, 0u64);
        let input = strobe(4.0, 0.5);
        while t < 30.0 {
            let frame = if input(t) { lit_frame() } else { dark_frame() };
            let f = lim.process(frame, t, &cfg);
            assert_eq!(level(&f) > 0.5, input(t), "t={t}");
            assert!(!lim.status().active);
            i += 1;
            t += 0.010 + (crate::beat::seeded_rand(7, i) as f64) * 0.016;
        }
    }

    #[test]
    fn a_gate_at_250_bpm_is_over_the_limit() {
        // T-100's beat gate flashes once per beat: 250 BPM = 4.17 Hz.
        let (out, _) = run(strobe(250.0 / 60.0, 0.25), 8.0, &SafetySettings::default());
        let steady = steady_from(&out, 8.0).expect("held steady");
        assert!(steady < 6.6, "cut once the burst reached 5 s (rate needs a few flashes to measure), got {steady}");
    }

    #[test]
    fn the_hold_lasts_while_the_strobe_goes_on_then_releases_after_the_cooldown() {
        let cfg = SafetySettings::default();
        // 8 Hz for 12 s, then steady light for 5 s, then 8 Hz again.
        let input = |t: f64| if !(12.0..17.0).contains(&t) { strobe(8.0, 0.5)(t) } else { true };
        let (out, _) = run(input, 20.0, &cfg);
        assert_eq!(edges(&out, 5.05, 17.0), 0, "held for as long as the fast input lasts");
        // Fast input stopped at 12 s; released at ~14 s, then fast strobing
        // is allowed again for a new burst.
        assert!(edges(&out, 17.0, 20.0) >= 20, "a new burst is allowed after the cooldown");
    }

    #[test]
    fn short_pauses_do_not_reset_the_burst_clock() {
        // 4 s of 10 Hz, 1 s dark, 4 s of 10 Hz: still one burst.
        let input = |t: f64| if (4.0..5.0).contains(&t) { false } else { strobe(10.0, 0.5)(t) };
        let (out, _) = run(input, 9.0, &SafetySettings::default());
        assert_eq!(edges(&out, 6.1, 9.0), 0, "cut 5 s into the burst (at ~6 s), not at 10 s");
    }

    #[test]
    fn stopping_the_look_while_held_goes_dark() {
        let input = |t: f64| t < 6.0 && strobe(8.0, 0.5)(t);
        let (out, _) = run(input, 8.0, &SafetySettings::default());
        assert!(out.iter().filter(|(t, _)| *t > 6.3).all(|(_, lit)| !lit), "dark 0.25 s after the look stopped");
    }

    #[test]
    fn a_tighter_setting_cuts_sooner() {
        let cfg = SafetySettings { strobe_max_hz: 2.0, strobe_burst_s: 1.0, ..Default::default() };
        let (out, _) = run(strobe(3.0, 0.5), 5.0, &cfg);
        let steady = steady_from(&out, 5.0).expect("held");
        assert!(steady < 2.0, "3 Hz is over a 2 Hz limit, cut after ~1 s: {steady}");
    }

    #[test]
    fn a_burst_shorter_than_two_periods_is_cut_before_the_third_flash() {
        // T-354 (SAFETY_SEED=2002967): limit 1.13 Hz, burst 0.52 s, a gate
        // flashing every 0.3 s. Two flashes (0 s, 0.3 s) are already a fast
        // run; the third, at 0.6 s, would make it 0.6 s long, so the dark
        // gap before it must be filled once the burst reaches 0.52 s (the
        // held frame lighting up then is the run's last onset, at the limit).
        let cfg = SafetySettings { strobe_max_hz: 1.13, strobe_burst_s: 0.52, ..Default::default() };
        let (out, lim) = run(strobe(1.0 / 0.3, 0.25), 9.0, &cfg);
        let onsets: Vec<f64> = out.windows(2).filter(|p| !p[0].1 && p[1].1).map(|p| p[1].0).collect();
        assert!(onsets.iter().all(|&t| t <= 0.52 + 1.0 / FPS), "no flash after the burst limit: {onsets:?}");
        let steady = steady_from(&out, 9.0).expect("held steady");
        assert!(steady <= 0.52 + 1.0 / FPS, "held from the burst limit on, got {steady}");
        assert!(lim.status().active);
    }

    #[test]
    fn a_single_double_flash_never_engages_the_default_hold() {
        // Two quick flashes are a (short) fast run, but far from 5 s.
        let input = |t: f64| t < 0.2 && strobe(10.0, 0.5)(t);
        let (out, lim) = run(input, 4.0, &SafetySettings::default());
        assert_eq!(edges(&out, 0.0, 4.0), 1);
        assert!(out.iter().filter(|(t, _)| *t > 0.2).all(|(_, lit)| !lit));
        assert!(!lim.status().active && !lim.status().fast);
    }

    #[test]
    fn a_brightness_dip_that_is_not_a_flash_is_ignored() {
        // 17 Hz between 100 % and 50 %: a shimmer, not an on/off flash.
        let mut lim = StrobeLimiter::default();
        for i in 0..600 {
            let t = i as f64 / FPS;
            let k = if strobe(17.0, 0.5)(t) { 1.0 } else { 0.5 };
            let frame: Vec<Point> = (0..100).map(|j| Point::lit(j as f32 / 100.0, 0.5, k, k, k)).collect();
            lim.process(frame, t, &SafetySettings::default());
            assert!(!lim.status().active);
        }
    }

    fn floor(y: f32) -> SafetySettings {
        SafetySettings { horizon: Horizon { y, ..Horizon::default() }, ..Default::default() }
    }

    fn zone(id: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> Zone {
        Zone { id, name: format!("Z{id}"), points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], ..Default::default() }
    }

    #[test]
    fn looser_settings_are_refused_and_file_values_are_clamped() {
        let d = SafetySettings::default();
        assert!(d.validate().is_ok());
        for bad in [
            SafetySettings { strobe_max_hz: 8.0, ..d.clone() },
            SafetySettings { strobe_burst_s: 10.0, ..d.clone() },
            SafetySettings { strobe_cooldown_s: 0.5, ..d.clone() },
            floor(f32::NAN),
            floor(-2.0),
            SafetySettings { color_gain: [1.2, 1.0, 1.0], ..d.clone() },
            SafetySettings { min_diode_level: 0.8, ..d.clone() },
            SafetySettings { zones: vec![Zone { points: vec![[0.0, 0.0], [1.0, 0.0]], ..Default::default() }], ..d.clone() },
            SafetySettings { zones: vec![zone(1, 0.0, 0.0, 2.0, 1.0)], ..d.clone() },
            SafetySettings { zones: vec![zone(1, 0.0, 0.0, 1.0, 1.0), zone(1, -1.0, -1.0, 0.0, 0.0)], ..d.clone() },
            SafetySettings { zones: (0..MAX_ZONES as u32 + 1).map(|i| zone(i + 1, 0.0, 0.0, 0.5, 0.5)).collect(), ..d.clone() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let ok = SafetySettings { strobe_max_hz: 3.0, strobe_burst_s: 2.0, strobe_cooldown_s: 5.0, ..floor(-0.5) };
        assert!(ok.validate().is_ok());
        let s = SafetySettings { strobe_max_hz: 20.0, strobe_burst_s: f32::NAN, strobe_cooldown_s: 0.0, color_gain: [2.0, 0.5, -1.0], ..floor(3.0) }.sanitized();
        assert_eq!(
            s,
            SafetySettings { strobe_max_hz: 4.0, strobe_burst_s: 5.0, strobe_cooldown_s: 2.0, color_gain: [1.0, 0.5, 0.0], ..floor(1.0) }
        );
        let old: SafetySettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, d);
    }

    #[test]
    fn a_t101_file_keeps_its_horizon() {
        let old: SafetySettings = serde_json::from_str(r#"{"strobe_max_hz":3,"beam_floor_y":0.25}"#).unwrap();
        assert_eq!(old.strobe_max_hz, 3.0);
        assert_eq!(old.horizon, Horizon { y: 0.25, ..Horizon::default() });
        assert!(old.zones.is_empty());
        // Saved in the new shape, and read back the same.
        let json = serde_json::to_string(&old).unwrap();
        assert!(!json.contains("beam_floor_y"));
        assert_eq!(serde_json::from_str::<SafetySettings>(&json).unwrap(), old);
    }

    #[test]
    fn loosening_needs_an_explicit_confirmation() {
        let mut store = SafetyStore::in_memory();
        // Tightening goes straight through: a zone, a higher horizon, lines.
        let tight = SafetySettings {
            zones: vec![Zone { id: 0, ..zone(0, -1.0, -1.0, 1.0, -0.5) }],
            horizon: Horizon { y: 0.1, lines: true, ramp: 0.2, level: 0.0 },
            color_gain: [0.8, 1.0, 1.0],
            ..Default::default()
        };
        let stored = store.set(tight, false).unwrap();
        assert_eq!(stored.zones[0].id, 1, "a new zone gets an id");
        let base = store.get();
        // Each of these is looser than `base` and is refused on its own.
        let mut moved = base.clone();
        moved.zones[0].points[0] = [-0.9, -1.0];
        let mut dimmed = base.clone();
        dimmed.zones[0].kind = ZoneKind::Dim;
        let cases = [
            (SafetySettings { zones: vec![], ..base.clone() }, "supprimée"),
            (moved, "déplacée"),
            (dimmed, "atténuée"),
            (SafetySettings { horizon: Horizon { y: 0.0, ..base.horizon }, ..base.clone() }, "Horizon abaissé"),
            (SafetySettings { horizon: Horizon { lines: false, ..base.horizon }, ..base.clone() }, "ne coupe plus"),
            (SafetySettings { horizon: Horizon { level: 0.5, ..base.horizon }, ..base.clone() }, "sous l'horizon"),
            (SafetySettings { horizon: Horizon { ramp: 0.0, ..base.horizon }, ..base.clone() }, "Rampe"),
            (SafetySettings { color_gain: [1.0; 3], ..base.clone() }, "Gain rouge"),
            (SafetySettings { min_diode_level: 0.1, ..base.clone() }, "diodes"),
        ];
        for (looser, what) in cases {
            match store.set(looser, false) {
                Err(SetError::Loosens(list)) => assert!(list.iter().any(|l| l.contains(what)), "{what}: {list:?}"),
                other => panic!("{what}: {other:?}"),
            }
            assert_eq!(store.get(), base, "unchanged after a refusal");
        }
        // A strobe limit tightened below the default can't be raised back
        // without confirmation either.
        store.set(SafetySettings { strobe_max_hz: 2.0, ..base.clone() }, false).unwrap();
        assert!(matches!(store.set(base.clone(), false), Err(SetError::Loosens(l)) if l[0].contains("Strobe")));
        store.set(base.clone(), true).unwrap();
        // Renaming is not loosening; confirmed loosening goes through.
        let mut renamed = base.clone();
        renamed.zones[0].name = "Public".into();
        store.set(renamed, false).unwrap();
        store.set(SafetySettings::default(), true).unwrap();
        assert_eq!(store.get(), SafetySettings::default());
    }

    fn dim(id: u32, level: f32, feather: f32, gradient: Option<ZoneGradient>) -> Zone {
        Zone { kind: ZoneKind::Dim, level, feather, gradient, name: "Public".into(), ..zone(id, -1.0, -1.0, 1.0, -0.2) }
    }

    /// T-446: soft edges and gradients, validated, sanitized, and their
    /// changes sorted into tightening (applied) and loosening (confirmed).
    #[test]
    fn soft_zones_validate_sanitize_and_loosen() {
        let g = |level_to: f32| Some(ZoneGradient { from: [0.0, -0.2], to: [0.0, -1.0], level_to });
        let ok = SafetySettings { zones: vec![dim(1, 0.5, 0.2, g(0.2))], ..Default::default() };
        assert!(ok.validate().is_ok());
        for bad in [dim(1, 0.5, 0.6, None), dim(1, 0.5, f32::NAN, None), dim(1, 0.5, -0.1, None), dim(1, 0.5, 0.0, g(1.5)), dim(1, 0.5, 0.0, g(f32::INFINITY))] {
            let s = SafetySettings { zones: vec![bad], ..Default::default() };
            assert!(s.validate().is_err(), "{s:?}");
        }
        let off_field = Zone { gradient: Some(ZoneGradient { from: [0.0, 2.0], ..ZoneGradient::default() }), ..dim(1, 0.5, 0.0, None) };
        assert!(SafetySettings { zones: vec![off_field], ..Default::default() }.validate().is_err());
        // A Public Dim zone: the arrival level is capped like its level.
        let public = Zone { audience: true, ..dim(1, 0.3, 0.0, g(0.8)) };
        let err = SafetySettings { zones: vec![public], ..Default::default() }.validate().unwrap_err().to_string();
        assert!(err.contains("arrivée"), "{err}");

        // sanitized: Blank zones lose both; bad values are clamped tighter.
        let blank = Zone { feather: 0.3, gradient: g(0.9), ..zone(2, 0.0, 0.0, 0.5, 0.5) };
        let nan = dim(3, 0.6, f32::NAN, Some(ZoneGradient { from: [f32::NAN, 0.0], to: [0.0, 1.0], level_to: 0.1 }));
        let s = SafetySettings { zones: vec![blank, nan, dim(4, 0.5, 3.0, g(2.0))], ..Default::default() }.sanitized();
        assert_eq!((s.zones[0].feather, s.zones[0].gradient), (0.0, None));
        assert_eq!((s.zones[1].feather, s.zones[1].gradient, s.zones[1].level), (MAX_FEATHER, None, 0.1));
        assert_eq!((s.zones[2].feather, s.zones[2].gradient.unwrap().level_to), (MAX_FEATHER, 1.0));
        assert!(s.validate().is_ok());

        // A file from before T-446 loads with no soft edge and no gradient.
        let old: SafetySettings =
            serde_json::from_str(r#"{"zones":[{"id":1,"name":"P","kind":"dim","level":0.4,"points":[[0,0],[1,0],[1,1]]}]}"#).unwrap();
        assert_eq!((old.zones[0].feather, old.zones[0].gradient), (0.0, None));
        let partial: Zone = serde_json::from_str(r#"{"kind":"dim","gradient":{"level_to":0.1}}"#).unwrap();
        assert_eq!(partial.gradient, Some(ZoneGradient { level_to: 0.1, ..ZoneGradient::default() }));

        // Loosenings.
        let base = SafetySettings { zones: vec![dim(1, 0.5, 0.2, g(0.2))], ..Default::default() };
        let with = |z: Zone| SafetySettings { zones: vec![z], ..Default::default() };
        let loosens = |z: Zone| base.loosenings(&with(z));
        assert!(loosens(dim(1, 0.5, 0.3, g(0.2))).is_empty(), "wider soft edge tightens");
        assert!(loosens(dim(1, 0.5, 0.2, g(0.1))).is_empty(), "lower arrival tightens");
        // Masked: darker inside, but the soft edge around it goes (T-260).
        assert!(loosens(Zone { kind: ZoneKind::Blank, ..dim(1, 0.5, 0.0, None) })[0].contains("bord doux"), "the soft edge goes");
        let hard = SafetySettings { zones: vec![dim(1, 0.5, 0.0, None)], ..Default::default() };
        assert!(hard.loosenings(&with(Zone { kind: ZoneKind::Blank, ..dim(1, 0.5, 0.0, None) })).is_empty(), "Blank tightens a hard-edged zone");
        assert_eq!(loosens(dim(1, 0.5, 0.08, g(0.2))), vec!["Zone « Public » : bord doux 10 % → 4 %"]);
        assert_eq!(loosens(dim(1, 0.5, 0.2, g(0.5))), vec!["Zone « Public » : luminosité à l'arrivée 20 % → 50 %"]);
        let moved = Zone { gradient: Some(ZoneGradient { from: [0.0, -0.3], to: [0.0, -1.0], level_to: 0.2 }), ..dim(1, 0.5, 0.2, None) };
        assert!(loosens(moved)[0].contains("dégradé déplacé"));
        assert!(loosens(dim(1, 0.5, 0.2, None))[0].contains("dégradé retiré"), "removing a darker gradient");
        // Adding a gradient: at or below the level tightens, above loosens.
        let plain = SafetySettings { zones: vec![dim(1, 0.5, 0.0, None)], ..Default::default() };
        assert!(plain.loosenings(&with(dim(1, 0.5, 0.0, g(0.5)))).is_empty());
        assert!(plain.loosenings(&with(dim(1, 0.5, 0.0, g(0.7))))[0].contains("arrivée"));
        // A degenerate gradient is no gradient: adding one changes nothing.
        let flat = Some(ZoneGradient { from: [0.0, 0.0], to: [0.0, 0.0], level_to: 1.0 });
        assert!(plain.loosenings(&with(dim(1, 0.5, 0.0, flat))).is_empty());

        // Through the store: narrowing needs confirmation, widening not.
        let mut store = SafetyStore::in_memory();
        store.set(base.clone(), true).unwrap();
        assert!(matches!(store.set(with(dim(1, 0.5, 0.05, g(0.2))), false), Err(SetError::Loosens(_))));
        store.set(with(dim(1, 0.5, 0.4, g(0.2))), false).unwrap();
        assert_eq!(store.get().zones[0].feather, 0.4);
    }

    #[test]
    fn the_store_saves_and_reloads() {
        let dir = std::env::temp_dir().join(format!("laser-studio-safety-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("safety.json");
        std::fs::remove_file(&path).ok();
        let mut store = SafetyStore::load_or_create(path.clone());
        assert_eq!(store.get(), SafetySettings::default());
        assert!(matches!(store.set(SafetySettings { strobe_max_hz: 9.0, ..Default::default() }, true), Err(SetError::Invalid(_))));
        let cfg = SafetySettings { zones: vec![zone(0, -1.0, -1.0, 1.0, -0.4)], color_gain: [0.9, 1.0, 1.0], ..floor(0.2) };
        store.set(cfg, false).unwrap();
        let back = SafetyStore::load_or_create(path.clone());
        assert_eq!(back.get().horizon.y, 0.2);
        assert_eq!(back.get().zones.len(), 1);
        assert_eq!(back.get(), store.get());
        assert!(back.load_error().is_none());
        std::fs::remove_dir_all(dir.join(crate::datafile::QUARANTINE)).ok();
        // A broken file: the strictest settings, moved to the quarantine
        // folder (T-402), reported, and never written over.
        std::fs::write(&path, "{ not json").unwrap();
        let mut broken = SafetyStore::load_or_create(path.clone());
        assert_eq!(broken.get(), SafetySettings::strictest());
        assert!(broken.get().loosenings(&SafetySettings::default()).len() == 1, "strictest is stricter than the defaults");
        assert!(SafetySettings::default().loosenings(&broken.get()).is_empty(), "and never looser");
        assert!(broken.load_error().unwrap().contains("Fichier illisible : safety.json"));
        let moved = crate::datafile::tests::quarantined(&dir);
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{ not json");
        broken.set(SafetySettings { strobe_max_hz: 3.0, ..SafetySettings::strictest() }, false).unwrap();
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{ not json");
        let w = crate::datafile::take_pending(&dir.join(crate::datafile::QUARANTINE));
        assert!(w.len() == 1 && w[0].safety && w[0].file == "safety.json");
        std::fs::remove_dir_all(dir).ok();
    }

    // ---------- horizon ----------

    fn render(content: Content, secs: f32) -> Vec<Point> {
        let settings = Settings { content, brightness: 1.0, scale: 0.8, ..Default::default() };
        let mut a = Animator::default();
        let mut frame = Vec::new();
        for _ in 0..(secs * 60.0) as usize {
            frame = a.render(&settings, Default::default(), 1.0 / 60.0, &BeatClock::default());
        }
        frame
    }

    fn lit_beams_below(frame: &[Point], floor: f32) -> usize {
        let mut f = frame.to_vec();
        blank_low_beams(&mut f, floor)
    }

    #[test]
    fn beams_below_the_floor_are_blanked() {
        // A fan of beams pushed down to y = -0.5.
        let gen = |b: f32| Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 6, b, ..Default::default() } };
        let mut frame = render(gen(-0.6), 0.5);
        assert!(frame.iter().any(|p| p.is_lit()));
        assert!(frame.iter().filter(|p| p.is_lit()).all(|p| p.y < 0.0));
        let n = blank_low_beams(&mut frame, 0.0);
        assert_eq!(n, 6);
        assert!(frame.iter().all(|p| !p.is_lit()), "no lit point left");
        // Above the floor: untouched.
        let high = render(gen(0.6), 0.5);
        let mut kept = high.clone();
        assert_eq!(blank_low_beams(&mut kept, 0.0), 0);
        assert_eq!(kept, high);
        // Positions are kept (the mirrors still travel the same way).
        assert_eq!(frame.len(), render(gen(-0.6), 0.5).len());
    }

    #[test]
    fn a_beam_circle_keeps_only_its_upper_beams() {
        let content = Content::Generator { generator: "beam_circle".into(), params: GenParams { count: 8, ..Default::default() } };
        let mut frame = render(content, 0.3);
        blank_low_beams(&mut frame, 0.0);
        assert!(frame.iter().any(|p| p.is_lit()));
        assert!(frame.iter().filter(|p| p.is_lit()).all(|p| p.y >= 0.0));
        assert_eq!(lit_beams_below(&frame, 0.0), 0, "idempotent");
    }

    #[test]
    fn outlines_and_text_are_not_beams() {
        for shape in ["circle", "square", "triangle", "cross", "line", "star", "spiral"] {
            let frame = render(Content::Shape { shape: shape.into() }, 0.2);
            assert_eq!(lit_beams_below(&frame, 1.0), 0, "{shape} has no beam");
        }
        let text = render(Content::Text { text: "LASER 42".into(), style: Default::default() }, 0.2);
        assert_eq!(lit_beams_below(&text, 1.0), 0);
        let wave = render(Content::Wave, 0.2);
        assert_eq!(lit_beams_below(&wave, 1.0), 0);
        // The "dots" shape is a grid of beams: its lower half goes.
        let mut dots = render(Content::Shape { shape: "dots".into() }, 0.2);
        assert_eq!(blank_low_beams(&mut dots, 0.0), 8);
    }

    #[test]
    fn a_figure_shrunk_to_a_point_is_a_beam() {
        let settings = Settings { content: Content::Shape { shape: "circle".into() }, scale: 0.0, brightness: 1.0, ..Default::default() };
        let mut frame = Animator::default().render(&settings, Default::default(), 1.0 / 60.0, &BeatClock::default());
        for p in &mut frame {
            p.y -= 0.3;
        }
        assert!(blank_low_beams(&mut frame, 0.0) >= 1);
        assert!(frame.iter().all(|p| !p.is_lit()));
    }

    #[test]
    fn the_stage_blanks_low_beams_and_reports_them() {
        let content = Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 4, b: -0.5, ..Default::default() } };
        let frame = render(content, 0.2);
        let mut lim = StrobeLimiter::default();
        let out = apply(frame, 0.0, &SafetySettings::default(), &mut lim);
        assert!(out.iter().all(|p| !p.is_lit()));
        assert_eq!(lim.status().beams_blanked, 4);
        // Lowering the floor (explicit setting) lets them through.
        let frame = render(Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 4, b: -0.5, ..Default::default() } }, 0.2);
        let out = apply(frame, 0.1, &floor(-1.0), &mut lim);
        assert!(out.iter().any(|p| p.is_lit()));
    }

    #[test]
    fn a_beat_gated_look_through_the_real_renderer_is_limited() {
        // A T-100 beat gate (lit for 1/4 beat after each beat), rendered by
        // the real Animator with the clock running at 8 beats per second.
        let params = GenParams { beat_sync: true, gate_beats: 0.25, count: 5, ..Default::default() };
        let settings = Settings { content: Content::Generator { generator: "beam_fan".into(), params }, brightness: 1.0, ..Default::default() };
        let mut a = Animator::default();
        let mut lim = StrobeLimiter::default();
        let cfg = floor(-1.0);
        let mut lit_after = Vec::new();
        for i in 0..(8.0 * FPS) as usize {
            let t = i as f64 / FPS;
            // 480 BPM worth of beats = 8 gates per second.
            let clock = BeatClock { beat: t * 8.0, bpm: 480.0, beats_per_bar: 4 };
            let frame = a.render(&settings, Default::default(), 1.0 / FPS as f32, &clock);
            let out = apply(frame, t, &cfg, &mut lim);
            if t > 5.2 {
                lit_after.push(out.iter().any(|p| p.is_lit()));
            }
        }
        assert!(lit_after.iter().all(|&l| l), "steady after the burst");
        assert!(lim.status().active);
    }

    #[test]
    fn a_1_8_beat_look_strobe_at_128_bpm_is_cut_after_5_s() {
        // T-103: the look's « Rythme » strobe at 1/8 beat (17 Hz) on a
        // chaser, rendered by the real Animator: it flashes for the burst,
        // then the limiter holds the output steady.
        let params = GenParams { count: 8, a: 0.0, b: 2.0, ..Default::default() };
        let settings = Settings {
            content: Content::Generator { generator: "chase_fan".into(), params },
            brightness: 1.0,
            strobe_div: 8.0,
            ..Default::default()
        };
        let mut a = Animator::default();
        let mut lim = StrobeLimiter::default();
        let cfg = floor(-1.0);
        let (mut dark_before, mut dark_after) = (0, 0);
        for i in 0..(8.0 * FPS) as usize {
            let t = i as f64 / FPS;
            let clock = BeatClock { beat: t * 128.0 / 60.0, bpm: 128.0, beats_per_bar: 4 };
            let frame = a.render(&settings, Default::default(), 1.0 / FPS as f32, &clock);
            let out = apply(frame, t, &cfg, &mut lim);
            let lit = out.iter().any(|p| p.is_lit());
            if t < 4.8 && !lit {
                dark_before += 1;
            }
            if t > 5.2 && !lit {
                dark_after += 1;
            }
        }
        assert!(dark_before > 100, "the strobe goes through during the burst ({dark_before} dark frames)");
        assert_eq!(dark_after, 0, "held steady after 5 s");
        assert!(lim.status().active && lim.status().rate_hz > 10.0);
    }

    // ---------- zones and full horizon (T-003) ----------

    /// Random zones for `seed`: one or two Blank polygons and a Dim one.
    fn random_zones(seed: u64) -> Vec<Zone> {
        let r = |i: u64| crate::beat::seeded_rand(seed, i) * 2.0 - 1.0;
        let n = 1 + (seed % 2) as usize;
        let mut zones: Vec<Zone> = (0..n as u64)
            .map(|z| {
                let (cx, cy) = (r(z * 50) * 0.8, r(z * 50 + 1) * 0.8);
                let k = 3 + ((seed + z) % 5) as usize;
                let points = (0..k)
                    .map(|j| {
                        let a = j as f32 / k as f32 * std::f32::consts::TAU + r(z * 50 + 2);
                        let rad = 0.15 + 0.35 * r(z * 50 + 3 + j as u64).abs();
                        [(cx + rad * a.cos()).clamp(-1.0, 1.0), (cy + rad * a.sin()).clamp(-1.0, 1.0)]
                    })
                    .collect();
                Zone { id: z as u32 + 1, points, ..Default::default() }
            })
            .collect();
        zones.push(Zone { id: 9, kind: ZoneKind::Dim, level: 0.3, ..zone(9, -1.0, 0.6, 1.0, 1.0) });
        zones
    }

    #[test]
    fn no_catalogue_cue_lights_a_blank_zone() {
        use crate::engine::AudioFeatures;
        let mut checked = 0;
        for (n, p) in crate::presets::catalog().iter().enumerate() {
            let zones = random_zones(n as u64);
            // Every other cue also with the full horizon.
            let horizon = Horizon { y: -0.3, lines: n % 2 == 0, ..Horizon::default() };
            let cfg = SafetySettings { zones: zones.clone(), horizon, ..Default::default() };
            let mut a = Animator::default();
            let mut lim = StrobeLimiter::default();
            for i in 0..12u64 {
                let audio = AudioFeatures { level: 0.5, bass: 0.5, beat: i / 4, ..Default::default() };
                let clock = BeatClock { beat: i as f64 * 0.37, ..BeatClock::default() };
                let frame = a.render(&p.settings, audio, 1.0 / 20.0, &clock);
                let out = apply(frame, i as f64 / 60.0, &cfg, &mut lim);
                assert_eq!(crate::zones::blank_violations(&out, &zones), 0, "cue {} ({}) frame {i}", p.name, p.id);
                if horizon.lines {
                    assert!(out.iter().all(|q| !q.is_lit() || q.y >= horizon.y), "cue {} lit below the horizon", p.id);
                }
                // Inside the Dim zone: at most 30 % of full drive.
                assert!(out.iter().filter(|q| q.y > 0.6 + 2e-3).all(|q| q.r.max(q.g).max(q.b) <= 0.3 + 1e-4), "cue {} dim zone", p.id);
                checked += 1;
            }
        }
        assert!(checked > 1000, "{checked} frames");
    }

    /// T-351: zones whose edges sit on the values the content is drawn on
    /// (0, ±0.25, ±0.5, the diagonal...), so that many lit samples - and
    /// whole lines - fall exactly on an edge. Nothing lit may end up inside
    /// a Blank zone or on its edge, corners included.
    #[test]
    fn no_catalogue_cue_lights_a_grid_aligned_blank_zone() {
        use crate::engine::AudioFeatures;
        let tri = |id: u32, pts: [[f32; 2]; 3]| Zone { id, points: pts.to_vec(), ..Default::default() };
        let sets = [
            vec![zone(1, 0.0, 0.0, 0.5, 0.5), zone(2, -0.3, -0.9, 0.3, -0.3), zone(3, -1.0, 0.0, 1.0, 0.03)],
            vec![zone(1, -0.5, -0.25, 0.0, 0.25), zone(2, 0.25, -0.75, 0.75, 0.0), tri(3, [[0.0, 0.0], [0.6, 0.6], [0.6, 0.0]])],
            vec![zone(1, -0.25, -0.25, 0.25, 0.25), zone(2, -0.75, 0.5, -0.25, 1.0), tri(3, [[0.0, 0.0], [-0.6, -0.6], [0.0, -0.6]])],
        ];
        // How many lit input samples sit exactly on an edge: the test must
        // really exercise the case.
        let on_edge = |q: &Point, z: &Zone| {
            (0..z.points.len()).any(|i| {
                let (p0, p1) = (z.points[i], z.points[(i + 1) % z.points.len()]);
                let (ex, ey) = (p1[0] - p0[0], p1[1] - p0[1]);
                let (wx, wy) = (q.x - p0[0], q.y - p0[1]);
                let t = (wx * ex + wy * ey) / (ex * ex + ey * ey);
                wx * ey == wy * ex && (0.0..=1.0).contains(&t)
            })
        };
        let (mut checked, mut on_edges) = (0, 0);
        for (n, p) in crate::presets::catalog().iter().enumerate() {
            let zones = &sets[n % sets.len()];
            let cfg = SafetySettings { zones: zones.clone(), ..Default::default() };
            let mut a = Animator::default();
            let mut lim = StrobeLimiter::default();
            for i in 0..8u64 {
                let audio = AudioFeatures { level: 0.5, bass: 0.5, beat: i / 4, ..Default::default() };
                let clock = BeatClock { beat: i as f64 * 0.37, ..BeatClock::default() };
                let frame = a.render(&p.settings, audio, 1.0 / 20.0, &clock);
                on_edges += frame.iter().filter(|q| q.is_lit() && zones.iter().any(|z| on_edge(q, z))).count();
                let out = apply(frame, i as f64 / 60.0, &cfg, &mut lim);
                assert_eq!(crate::zones::blank_violations(&out, zones), 0, "cue {} ({}) frame {i}", p.name, p.id);
                checked += 1;
            }
        }
        assert!(checked > 500, "{checked} frames");
        assert!(on_edges > 1000, "only {on_edges} lit samples on an edge");
    }

    #[test]
    fn a_sheet_rotated_below_the_horizon_is_cut_when_lines_are_covered() {
        // A horizontal line through the whole field at y = -0.5 (as a sheet
        // brought down by rotation/calibration would be).
        let line: Vec<Point> = (0..=40).map(|i| Point::lit(-1.0 + i as f32 / 20.0, -0.5, 0.0, 1.0, 0.0)).collect();
        let mut lim = StrobeLimiter::default();
        // Beams only (the default): a line is not a beam, it stays.
        let out = apply(line.clone(), 0.0, &SafetySettings::default(), &mut lim);
        assert!(out.iter().any(|p| p.is_lit()));
        let cfg = SafetySettings { horizon: Horizon { lines: true, ..Horizon::default() }, ..Default::default() };
        let out = apply(line, 0.1, &cfg, &mut lim);
        assert!(out.iter().all(|p| !p.is_lit()));
        assert_eq!(lim.status().points_masked, 41);
    }

    #[test]
    fn beams_below_the_horizon_stay_blanked_whatever_the_level() {
        // Lines dimmed to 50 % under the horizon, but beams still go.
        let content = Content::Generator { generator: "beam_fan".into(), params: GenParams { count: 4, b: -0.5, ..Default::default() } };
        let frame = render(content, 0.2);
        let cfg = SafetySettings { horizon: Horizon { lines: true, level: 0.5, ..Horizon::default() }, ..Default::default() };
        let out = apply(frame, 0.0, &cfg, &mut StrobeLimiter::default());
        assert!(out.iter().all(|p| !p.is_lit()));
    }

    #[test]
    fn a_held_frame_gets_the_zones_added_during_the_hold() {
        // 8 Hz strobe until held, then a zone over everything is added:
        // the frame put back by the limiter is blanked too.
        let mut lim = StrobeLimiter::default();
        let mut cfg = SafetySettings::default();
        let mut i = 0;
        while !lim.status().active {
            let t = i as f64 / FPS;
            let frame = if strobe(8.0, 0.5)(t) { lit_frame() } else { dark_frame() };
            apply(frame, t, &cfg, &mut lim);
            i += 1;
            assert!(i < 600, "never held");
        }
        cfg.zones = vec![zone(1, -1.0, -1.0, 1.0, 1.0)];
        for _ in 0..10 {
            let t = i as f64 / FPS;
            let out = apply(dark_frame(), t, &cfg, &mut lim);
            assert!(out.iter().all(|p| !p.is_lit()), "held frame still lit in a Blank zone");
            i += 1;
        }
    }

    #[test]
    fn a_zone_changes_the_output_only_where_it_is() {
        // Default settings: the stage is the identity for a normal look.
        let frame = render(Content::Shape { shape: "circle".into() }, 0.2);
        let mut lim = StrobeLimiter::default();
        assert_eq!(apply(frame.clone(), 0.0, &SafetySettings::default(), &mut lim), frame);
        // A zone over the left half: the right half is untouched.
        let cfg = SafetySettings { zones: vec![zone(1, -1.0, -1.0, 0.0, 1.0)], ..Default::default() };
        let out = apply(frame.clone(), 0.1, &cfg, &mut lim);
        assert!(out.iter().any(|p| p.is_lit() && p.x > 0.0));
        assert!(out.iter().all(|p| !p.is_lit() || p.x > 0.0));
        let right_in: Vec<&Point> = frame.iter().filter(|p| p.x > 0.01).collect();
        let right_out: Vec<&Point> = out.iter().filter(|p| p.x > 0.01).collect();
        assert_eq!(right_in, right_out);
    }

    // ---------- static beam guard (T-256) ----------

    #[test]
    fn the_dwell_guard_loosens_only_with_confirmation_and_old_files_load() {
        use crate::dwell::DwellProfile;
        let mut store = SafetyStore::in_memory();
        let strict = SafetySettings { dwell: DwellGuard { profile: DwellProfile::Strict, min_extent: 0.1, ..Default::default() }, ..Default::default() };
        assert!(store.set(strict.clone(), false).is_ok(), "tightening applies at once");
        for looser in [DwellProfile::Beams, DwellProfile::Off] {
            let next = SafetySettings { dwell: DwellGuard { profile: looser, ..strict.dwell }, ..strict.clone() };
            match store.set(next, false) {
                Err(SetError::Loosens(l)) => assert!(l.iter().any(|m| m.contains("Garde anti-point fixe")), "{l:?}"),
                other => panic!("{looser:?}: {other:?}"),
            }
        }
        let smaller = SafetySettings { dwell: DwellGuard { min_extent: 0.05, ..strict.dwell }, ..strict.clone() };
        assert!(matches!(store.set(smaller.clone(), false), Err(SetError::Loosens(_))));
        assert_eq!(store.get(), strict, "unchanged");
        assert!(store.set(smaller.clone(), true).is_ok());
        assert_eq!(store.get(), smaller);
        // Out of range: 400, even confirmed.
        let bad = SafetySettings { dwell: DwellGuard { max_cell_dose: 0.9, ..Default::default() }, ..Default::default() };
        assert!(matches!(store.set(bad, true), Err(SetError::Invalid(_))));
        // A safety.json from before T-256 gets the default guard.
        let old: SafetySettings = serde_json::from_str(r#"{"strobe_max_hz":3,"horizon":{"y":0.2}}"#).unwrap();
        assert_eq!(old.dwell, DwellGuard::default());
    }

    #[test]
    fn a_look_shrunk_to_a_point_goes_dark_through_the_whole_stage() {
        // The real Animator at size 0 (what the live master size does),
        // through horizon, zones, limiter and then the guard, as the engine.
        use crate::dwell::{DwellProfile, DwellState};
        let point = Settings { content: Content::Shape { shape: "star".into() }, scale: 0.0, brightness: 1.0, ..Default::default() };
        let full = Settings { scale: 0.5, ..point.clone() };
        for profile in [DwellProfile::Strict, DwellProfile::Beams] {
            let cfg = SafetySettings { dwell: DwellGuard { profile, ..Default::default() }, ..Default::default() };
            let (mut a, mut lim, mut guard) = (Animator::default(), StrobeLimiter::default(), DwellState::default());
            let mut last = Vec::new();
            for i in 0..90 {
                let t = i as f64 / FPS;
                let look = if i < 60 { &point } else { &full };
                let frame = apply(a.render(look, Default::default(), 1.0 / 60.0, &BeatClock::default()), t, &cfg, &mut lim);
                let out = guard.apply(frame, t, &cfg.dwell, &crate::dwell::StrictArea::new(cfg.horizon.y, &cfg.zones), true);
                if i == 59 {
                    let peak = level_max(&out);
                    match profile {
                        DwellProfile::Strict => assert_eq!(peak, 0.0, "strict: dark"),
                        _ => assert!(peak > 0.0 && peak <= 0.6, "beams, above the horizon: capped, got {peak}"),
                    }
                }
                last = out;
            }
            assert!(level_max(&last) > 0.99, "{profile:?}: back at full size once restored");
        }
    }

    /// Output flash onsets of a run, measured like the limiter (and the
    /// safety invariants) do; returns the longest run of flashes closer
    /// than one allowed period, in seconds.
    fn longest_fast_run(levels: &[(f64, f32)], period: f64) -> f64 {
        let (mut peak, mut on, mut onsets) = (0.0f32, false, Vec::new());
        let mut last_t = None;
        for &(t, e) in levels {
            let dt = last_t.map_or(0.0, |l: f64| t - l) as f32;
            last_t = Some(t);
            peak = e.max(peak * (-dt / PEAK_DECAY_S).exp());
            if !on && e > MIN_LEVEL && e >= ON_FRAC * peak {
                on = true;
                onsets.push(t);
            } else if on && (e <= MIN_LEVEL || e <= OFF_FRAC * peak) {
                on = false;
            }
        }
        let (mut best, mut start) = (0.0f64, None::<f64>);
        for w in onsets.windows(2) {
            if w[1] - w[0] < period - SLACK_S {
                best = best.max(w[1] - *start.get_or_insert(w[0]));
            } else {
                start = None;
            }
        }
        best
    }

    /// T-360: content that alternates between a lone beam and a wide
    /// figure, about 6 times a second. The input is steadily lit, so the
    /// limiter's input side never engages; the strict guard after it
    /// blanks every lone beam and the figure comes back as the guard
    /// lifts, so the light that goes out flashes at 6 Hz. The output
    /// check cuts it to the limit once the burst is over.
    #[test]
    fn a_guard_blanking_a_lone_beam_cannot_strobe_the_output() {
        use crate::dwell::{DwellProfile, DwellState, StrictArea};
        let beam = || vec![Point::lit(0.2, 0.5, 1.0, 1.0, 1.0); 12];
        let wide = || (0..40).map(|i| Point::lit(-0.8 + i as f32 * 0.04, 0.5, 1.0, 1.0, 1.0)).collect::<Vec<_>>();
        let cfg = SafetySettings { strobe_burst_s: 1.0, dwell: DwellGuard { profile: DwellProfile::Strict, ..Default::default() }, ..Default::default() };
        let area = StrictArea::new(cfg.horizon.y, &cfg.zones);
        let period = 1.0 / cfg.strobe_max_hz as f64;
        for check in [false, true] {
            let (mut lim, mut guard) = (StrobeLimiter::default(), DwellState::default());
            let mut levels = Vec::new();
            for i in 0..(8.0 * FPS) as usize {
                let t = i as f64 / FPS;
                let frame = if i % 10 == 0 { beam() } else { wide() };
                let frame = apply(frame, t, &cfg, &mut lim);
                assert!(!lim.status().active, "the input is steady: no hold");
                let mut frame = guard.apply(frame, t, &cfg.dwell, &area, true);
                if check {
                    lim.finish(&mut frame, t, &cfg);
                }
                levels.push((t, level(&frame)));
            }
            let run = longest_fast_run(&levels, period);
            if check {
                assert!(run <= cfg.strobe_burst_s as f64 + SLACK_S, "{run} s of fast flashing out");
                assert!(levels.iter().any(|&(t, e)| t > 7.0 && e > 0.5), "still drawn, at the limit");
            } else {
                assert!(run > 5.0, "without the output check the guard strobes the output ({run} s)");
            }
        }
    }

    #[test]
    fn the_output_check_leaves_a_held_strobe_steady_and_slow_flashes_alone() {
        // The usual case: the input side holds the output lit after the
        // burst, so the output check sees no fast flash past it.
        let cfg = SafetySettings::default();
        let mut lim = StrobeLimiter::default();
        let mut cut = 0;
        let mut out = Vec::new();
        for i in 0..(9.0 * FPS) as usize {
            let t = i as f64 / FPS;
            let input = if strobe(8.0, 0.5)(t) { lit_frame() } else { dark_frame() };
            let mut f = apply(input, t, &cfg, &mut lim);
            lim.finish(&mut f, t, &cfg);
            cut += lim.status().output_cut as usize;
            out.push((t, level(&f) > 0.5));
        }
        let steady = steady_from(&out, 9.0).expect("held steady");
        assert!((steady - 5.0).abs() <= 0.1, "cut to steady after 5 s ± 0.1 s, got {steady}");
        assert_eq!(cut, 0, "nothing for the output check to do");
        // 4 Hz, the limit itself, is never touched.
        let mut lim = StrobeLimiter::default();
        for i in 0..(10.0 * FPS) as usize {
            let t = i as f64 / FPS;
            let input = if strobe(4.0, 0.5)(t) { lit_frame() } else { dark_frame() };
            let mut f = apply(input.clone(), t, &cfg, &mut lim);
            lim.finish(&mut f, t, &cfg);
            assert_eq!(level(&f), level(&input), "t = {t}");
        }
    }

    /// Found by the profile property test (T-260): masking a Dim zone that
    /// has a soft edge lights the band around it, so it is a loosening.
    #[test]
    fn masking_a_soft_dim_zone_is_a_loosening() {
        let dim = Zone { id: 1, name: "Bord".into(), kind: ZoneKind::Dim, level: 0.5, feather: 0.1, points: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]], ..Default::default() };
        let cur = SafetySettings { zones: vec![dim.clone()], ..Default::default() };
        let masked = SafetySettings { zones: vec![Zone { kind: ZoneKind::Blank, ..dim.clone() }], ..Default::default() };
        // Just outside the zone, in its soft edge: brighter once masked.
        assert!(masked.mask().factor(0.55, 0.0) > cur.mask().factor(0.55, 0.0));
        assert!(cur.loosenings(&masked).iter().any(|m| m.contains("bord doux")));
        // Without a soft edge, masking only tightens.
        let hard = SafetySettings { zones: vec![Zone { feather: 0.0, ..dim.clone() }], ..Default::default() };
        let hard_masked = SafetySettings { zones: vec![Zone { kind: ZoneKind::Blank, feather: 0.0, ..dim }], ..Default::default() };
        assert!(hard.loosenings(&hard_masked).is_empty());
    }

    fn level_max(frame: &[Point]) -> f32 {
        frame.iter().map(|p| p.r.max(p.g).max(p.b)).fold(0.0, f32::max)
    }
}
