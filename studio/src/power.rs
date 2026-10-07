//! Per-output power caps and projector sheets (T-254).
//!
//! Brightness is a look setting (0..1) that cues, LFOs, audio and MIDI can
//! all drive to 1.0. The cap is a hardware ceiling per output that nothing
//! upstream can exceed: it is applied to the finished frame, **after** the
//! whole safety stage (`safety::apply`: horizon, zones, colour calibration,
//! strobe limiter, static-beam guard) and before the limiter's check on
//! the output (T-360) and the output gate, so every source goes
//! through it and the preview shows the capped frame:
//!
//! `r = min(r, max_color[0]) * max_power` (and the same for g, b)
//!
//! A clip per colour, then a global multiplier. It can only reduce: a
//! channel never goes above `max_color[c] * max_power`, and a dark channel
//! stays dark. It is not `color_gain` (T-003's colour balance): it is a
//! safety limit, never reachable from MIDI, cues, the timeline, LFOs or the
//! look API, and never part of a project.
//!
//! The projector sheet (class, power per colour, wavelengths, aperture,
//! divergence...) is informative: T-257 (MPE / NOHD estimate) and T-263
//! (safety sheet) read it. It never arms anything and never allows
//! anything. Its one effect on arming can only *add* a lock: declaring
//! the interface a ShowNET (T-472) brings the ShowNET items of the
//! pre-show checklist and the « Arrêt matériel non vérifié » interlock
//! (checklist.rs).
//!
//! The output's colour rendering (white balance, curves, colour delay:
//! `colorcal.rs`, T-414) lives in the same entry, key `color`. It runs
//! before the safety stage, so the cap stays the last brightness
//! operation and no colour setting can push a channel above it.
//!
//! Stored in `<data-dir>/outputs.json`, one entry per output. Lowering a
//! cap applies on the next engine tick. Raising one needs the operator's
//! explicit confirmation, and is refused outright while the laser is armed.

use crate::colorcal::ColorCalibration;
use crate::patterns::Point;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The main projector's output; a venue's other projectors (venue.rs,
/// T-277) have entries under their own ids (`p2`, `p3`…).
pub const MAIN_OUTPUT: &str = "main";

/// The IEC 60825-1 classes the sheet accepts ("" = not filled in).
pub const LASER_CLASSES: [&str; 8] = ["", "1", "1M", "2", "2M", "3R", "3B", "4"];

/// The interface (DAC) model on the sheet (T-472). Choosing one fills its
/// resolution and point-rate ceiling; `ShowNet` also brings the ShowNET
/// safety reminders (checklist.rs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DacModel {
    /// Not said, or a model without a preset.
    #[default]
    Other,
    /// Laserworld ShowNET (reached through PONK → MadMapper today).
    #[serde(rename = "shownet")]
    ShowNet,
}

/// What choosing a model fills in.
#[derive(Debug, Serialize)]
pub struct ModelPreset {
    pub model: DacModel,
    pub label_fr: &'static str,
    pub xy_bits: u8,
    pub color_bits: u8,
    /// 0 = unknown.
    pub max_pps: u32,
}

/// The models offered in the sheet. ShowNET: 12-bit X/Y, 8 bits per
/// colour (RGB), 100 kpps: the figures of Laserworld's current public
/// pages (older manuals say more; plan for the lower one).
pub const MODELS: [ModelPreset; 2] = [
    ModelPreset { model: DacModel::Other, label_fr: "Autre / non renseignée", xy_bits: 16, color_bits: 16, max_pps: 0 },
    ModelPreset { model: DacModel::ShowNet, label_fr: "ShowNET (Laserworld)", xy_bits: 12, color_bits: 8, max_pps: 100_000 },
];

impl DacModel {
    pub fn preset(self) -> &'static ModelPreset {
        MODELS.iter().find(|m| m.model == self).expect("every model has a preset")
    }
}

/// Hard ceiling of one output.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputLimits {
    /// Global multiplier, 0..1. 50 % by default: deliberately cautious, the
    /// operator raises it knowingly.
    pub max_power: f32,
    /// Per-colour clip (red, green, blue), 0..1, before `max_power`.
    pub max_color: [f32; 3],
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self { max_power: 0.5, max_color: [1.0; 3] }
    }
}

/// What the operator knows about the projector on an output. Informative
/// (estimates and the safety sheet); never enough to allow anything on its
/// own: audience scanning (T-255) only *requires* a complete sheet that
/// declares a hardware scan-fail, among other conditions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectorInfo {
    pub name: String,
    /// IEC 60825-1 class, one of `LASER_CLASSES`.
    pub class: String,
    /// Maximum optical power per colour (red, green, blue), mW. 0 = unknown.
    pub power_mw: [f32; 3],
    /// Wavelength per colour, nm. 0 = unknown.
    pub wavelength_nm: [u16; 3],
    /// Beam diameter at the aperture, mm.
    pub aperture_mm: f32,
    /// Full-angle divergence, mrad.
    pub divergence_mrad: f32,
    /// Optical scan angle, degrees.
    pub scan_angle_deg: f32,
    /// Hardware scan-fail protection in the projector: yes / no / unknown.
    pub hw_scan_fail: Option<bool>,
    /// A divergence lens is fitted.
    pub divergence_lens: bool,
    /// Free notes (model, serial number, last measurement...).
    pub notes: String,
    /// The interface driving the projector (T-472).
    pub dac_model: DacModel,
    /// Resolution of X and Y, bits.
    pub xy_bits: u8,
    /// Resolution of each colour, bits.
    pub color_bits: u8,
    /// The interface's point-rate ceiling. 0 = unknown.
    pub max_pps: u32,
}

impl Default for ProjectorInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            class: "4".into(),
            power_mw: [0.0; 3],
            wavelength_nm: [0; 3],
            aperture_mm: 0.0,
            divergence_mrad: 0.0,
            scan_angle_deg: 40.0,
            hw_scan_fail: None,
            divergence_lens: false,
            notes: String::new(),
            dac_model: DacModel::Other,
            xy_bits: 16,
            color_bits: 16,
            max_pps: 0,
        }
    }
}

/// Scan limits of one output (T-171): the scan speed range its galvos
/// take and the points per frame it should get. The scan speed itself is
/// the connection's (`--pps` / `connection.pps`, 30 000 by default): every
/// output opens at that rate today.
///
/// Applied to the content, **before** the safety stage (`safety_chain`
/// in main.rs): a frame above `max_points` is thinned (`fit_frame`), one
/// below `min_points` completed (`output::pad_frame`), then the zones,
/// horizon, limiter, static-beam guard and caps run on the result. The
/// speed range bounds the scan presets and the live « Vitesse de
/// balayage » (`scan::ScanLimits`). Nothing here raises a power cap.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputScan {
    /// Slowest scan speed the galvos should be driven at, points/s.
    pub pps_min: u32,
    /// Fastest scan speed (galvo protection), points/s: the scan presets
    /// and the live scan speed never trace faster than this.
    pub pps_max: u32,
    /// Fewest points per frame (0 = off): a shorter frame is completed.
    pub min_points: usize,
    /// Most points per frame (0 = no limit of its own: the layer mixer's
    /// budget and the DAC's frame size still apply).
    pub max_points: usize,
    /// Frame rate aimed at, frames/s. The UI warns below 7/8 of it.
    pub min_fps: f32,
}

impl Default for OutputScan {
    fn default() -> Self {
        Self { pps_min: 10_000, pps_max: 40_000, min_points: 200, max_points: 0, min_fps: 40.0 }
    }
}

/// One output: its cap, its projector and its colour rendering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub id: String,
    pub limits: OutputLimits,
    pub projector: ProjectorInfo,
    /// White balance, curves and colour delay (T-414, `colorcal.rs`).
    /// Absent in older files: identity.
    pub color: ColorCalibration,
    /// Scan speed range and points per frame (T-171). Absent in older
    /// files: the defaults.
    pub scan: OutputScan,
    /// What the exposure estimator (T-257) is given: audience distance and
    /// attenuation. Informative only. Absent in older files: the defaults.
    pub exposure: crate::mpe::ExposureSettings,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            id: MAIN_OUTPUT.into(),
            limits: OutputLimits::default(),
            projector: ProjectorInfo::default(),
            color: ColorCalibration::default(),
            scan: OutputScan::default(),
            exposure: crate::mpe::ExposureSettings::default(),
        }
    }
}

/// `outputs.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputsFile {
    pub outputs: Vec<OutputConfig>,
    /// Which output to connect and how (T-401, `output_manager.rs`).
    pub connection: crate::output_manager::OutputPrefs,
}

const UNIT: (f32, f32) = (0.0, 1.0);
const POWER_MW: (f32, f32) = (0.0, 1_000_000.0);
const WAVELENGTH_NM: (u16, u16) = (0, 2_000);
const APERTURE_MM: (f32, f32) = (0.0, 100.0);
const DIVERGENCE_MRAD: (f32, f32) = (0.0, 100.0);
const SCAN_ANGLE_DEG: (f32, f32) = (0.0, 180.0);
const BITS: (u8, u8) = (1, 16);
const MAX_PPS: u32 = 1_000_000;
/// Scan speed bounds an output may declare (what DACs take, as
/// `OutputStore::connection` clamps the rate).
pub const SCAN_PPS: (u32, u32) = (1_000, 100_000);
/// Points per frame bounds of `OutputScan` (the layer mixer's range).
pub const SCAN_POINTS: (usize, usize) = (crate::layers::MIN_POINT_BUDGET, crate::layers::MAX_POINT_BUDGET);
pub const SCAN_FPS: (f32, f32) = (1.0, 120.0);
const MAX_NAME: usize = 60;
const MAX_NOTES: usize = 1000;
const COLOURS: [&str; 3] = ["rouge", "vert", "bleu"];

fn check(v: f32, (lo, hi): (f32, f32), what: &str) -> Result<()> {
    if !v.is_finite() || v < lo || v > hi {
        bail!("{what} : {v} hors limites ({lo} à {hi})");
    }
    Ok(())
}

/// Clamps a hand-edited value; not a number → `def`.
fn fix(v: f32, (lo, hi): (f32, f32), def: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        def
    }
}

impl OutputLimits {
    pub fn validate(&self) -> Result<()> {
        check(self.max_power, UNIT, "Puissance max")?;
        for (c, name) in self.max_color.iter().zip(COLOURS) {
            check(*c, UNIT, &format!("{} max", capitalised(name)))?;
        }
        Ok(())
    }

    /// Clamped into 0..1; a value that is not a number becomes 0 (the
    /// tighter end: a broken file never lights anything up).
    pub fn sanitized(&self) -> Self {
        Self { max_power: fix(self.max_power, UNIT, 0.0), max_color: self.max_color.map(|c| fix(c, UNIT, 0.0)) }
    }

    /// The highest drive any channel can leave the studio with.
    pub fn ceiling(&self, channel: usize) -> f32 {
        self.max_color[channel] * self.max_power
    }

    /// What going from `self` to `new` would raise, in French. Empty =
    /// only lower or equal.
    pub fn raises(&self, new: &OutputLimits) -> Vec<String> {
        let mut out = Vec::new();
        if new.max_power > self.max_power {
            out.push(format!("Puissance max : {:.0} % → {:.0} %", self.max_power * 100.0, new.max_power * 100.0));
        }
        for (i, name) in COLOURS.iter().enumerate() {
            if new.max_color[i] > self.max_color[i] {
                out.push(format!("{} max : {:.0} % → {:.0} %", capitalised(name), self.max_color[i] * 100.0, new.max_color[i] * 100.0));
            }
        }
        out
    }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

impl ProjectorInfo {
    pub fn validate(&self) -> Result<()> {
        if self.name.chars().count() > MAX_NAME {
            bail!("Nom du projecteur trop long ({MAX_NAME} caractères au plus)");
        }
        if self.notes.chars().count() > MAX_NOTES {
            bail!("Notes trop longues ({MAX_NOTES} caractères au plus)");
        }
        if !LASER_CLASSES.contains(&self.class.as_str()) {
            bail!("Classe laser inconnue : « {} » (1, 1M, 2, 2M, 3R, 3B ou 4)", self.class);
        }
        for (i, name) in COLOURS.iter().enumerate() {
            check(self.power_mw[i], POWER_MW, &format!("Puissance {name} (mW)"))?;
            let w = self.wavelength_nm[i];
            if w > WAVELENGTH_NM.1 {
                bail!("Longueur d'onde {name} : {w} nm hors limites (0 à {})", WAVELENGTH_NM.1);
            }
        }
        check(self.aperture_mm, APERTURE_MM, "Diamètre de sortie (mm)")?;
        check(self.divergence_mrad, DIVERGENCE_MRAD, "Divergence (mrad)")?;
        check(self.scan_angle_deg, SCAN_ANGLE_DEG, "Angle de balayage (°)")?;
        for (bits, what) in [(self.xy_bits, "Résolution X/Y"), (self.color_bits, "Résolution des couleurs")] {
            if !(BITS.0..=BITS.1).contains(&bits) {
                bail!("{what} : {bits} bits hors limites ({} à {})", BITS.0, BITS.1);
            }
        }
        if self.max_pps > MAX_PPS {
            bail!("Points par seconde max : {} hors limites (0 à {MAX_PPS})", self.max_pps);
        }
        Ok(())
    }

    /// The sheet with `model` chosen and its preset filled in (resolution
    /// and point rate); everything else is kept.
    pub fn with_model(&self, model: DacModel) -> Self {
        let p = model.preset();
        Self { dac_model: model, xy_bits: p.xy_bits, color_bits: p.color_bits, max_pps: p.max_pps, ..self.clone() }
    }

    /// Clamped into range, for a file edited by hand.
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        Self {
            name: self.name.chars().take(MAX_NAME).collect(),
            class: if LASER_CLASSES.contains(&self.class.as_str()) { self.class.clone() } else { d.class },
            power_mw: self.power_mw.map(|p| fix(p, POWER_MW, 0.0)),
            wavelength_nm: self.wavelength_nm.map(|w| w.min(WAVELENGTH_NM.1)),
            aperture_mm: fix(self.aperture_mm, APERTURE_MM, 0.0),
            divergence_mrad: fix(self.divergence_mrad, DIVERGENCE_MRAD, 0.0),
            scan_angle_deg: fix(self.scan_angle_deg, SCAN_ANGLE_DEG, d.scan_angle_deg),
            hw_scan_fail: self.hw_scan_fail,
            divergence_lens: self.divergence_lens,
            notes: self.notes.chars().take(MAX_NOTES).collect(),
            dac_model: self.dac_model,
            xy_bits: self.xy_bits.clamp(BITS.0, BITS.1),
            color_bits: self.color_bits.clamp(BITS.0, BITS.1),
            max_pps: self.max_pps.min(MAX_PPS),
        }
    }
}

impl OutputScan {
    pub fn validate(&self) -> Result<()> {
        let (lo, hi) = SCAN_PPS;
        for (v, what) in [(self.pps_min, "Vitesse min"), (self.pps_max, "Vitesse max")] {
            if !(lo..=hi).contains(&v) {
                bail!("{what} : {v} points/s hors limites ({lo} à {hi})");
            }
        }
        if self.pps_min > self.pps_max {
            bail!("Vitesse min ({}) au-dessus de la vitesse max ({})", self.pps_min, self.pps_max);
        }
        if self.min_points > SCAN_POINTS.1 {
            bail!("Points minimum : {} hors limites (0 à {})", self.min_points, SCAN_POINTS.1);
        }
        if self.max_points != 0 && !(SCAN_POINTS.0..=SCAN_POINTS.1).contains(&self.max_points) {
            bail!("Points maximum : {} hors limites (0 = sans limite, ou {} à {})", self.max_points, SCAN_POINTS.0, SCAN_POINTS.1);
        }
        if self.max_points != 0 && self.min_points > self.max_points {
            bail!("Points minimum ({}) au-dessus du maximum ({})", self.min_points, self.max_points);
        }
        check(self.min_fps, SCAN_FPS, "Images/s visées")
    }

    /// Clamped into range, for a file edited by hand. A minimum above the
    /// maximum is brought down to it.
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        let pps_max = self.pps_max.clamp(SCAN_PPS.0, SCAN_PPS.1);
        let max_points = if self.max_points == 0 { 0 } else { self.max_points.clamp(SCAN_POINTS.0, SCAN_POINTS.1) };
        let min_points = self.min_points.min(if max_points == 0 { SCAN_POINTS.1 } else { max_points });
        Self { pps_min: self.pps_min.clamp(SCAN_PPS.0, pps_max), pps_max, min_points, max_points, min_fps: fix(self.min_fps, SCAN_FPS, d.min_fps) }
    }

    /// The scan speed range as a % of the output's rate `pps`, for the
    /// scan presets and the live scan speed (`scan::ScanLimits`): tracing
    /// at `pct` % moves the galvos like `pps × pct / 100` would. Within
    /// the presets' 50..150 % whatever the rate.
    pub fn scan_limits(&self, pps: u32) -> crate::scan::ScanLimits {
        use crate::scan::{MAX_SCAN_PCT, MIN_SCAN_PCT};
        let o = self.sanitized();
        let pct = |v: u32| if pps == 0 { 100.0 } else { (100.0 * v as f64 / pps as f64) as f32 };
        let max_scan_pct = pct(o.pps_max).clamp(MIN_SCAN_PCT, MAX_SCAN_PCT);
        let min_scan_pct = pct(o.pps_min).clamp(MIN_SCAN_PCT, max_scan_pct);
        crate::scan::ScanLimits {
            min_scan_pct,
            max_scan_pct,
            min_points: o.min_points.max(SCAN_POINTS.0),
            max_points: if o.max_points == 0 { SCAN_POINTS.1 } else { o.max_points },
        }
    }

    /// Points per frame the output takes: what `output::fit_frame_limits`
    /// applies before the safety stage.
    pub fn frame_limits(&self) -> crate::output::FrameLimits {
        let o = self.sanitized();
        crate::output::FrameLimits { min: o.min_points, max: (o.max_points > 0).then_some(o.max_points) }
    }

    /// Below this estimated frame rate the UI warns: 7/8 of the target
    /// (35 frames/s for the default 40).
    pub fn alert_fps(&self) -> f32 {
        self.sanitized().min_fps * 7.0 / 8.0
    }

    /// What does not fit together, in French, for the UI: the output's
    /// rate outside its speed range, or above the projector sheet's
    /// interface ceiling (`max_pps`, when known). Never clamped silently:
    /// the operator sees it and decides.
    pub fn warnings(&self, pps: u32, sheet: &ProjectorInfo) -> Vec<String> {
        let o = self.sanitized();
        let mut out = Vec::new();
        if pps > o.pps_max {
            out.push(format!("La sortie tourne à {pps} points/s, au-dessus de la vitesse max ({}) : le tracé est ralenti autant que possible.", o.pps_max));
        } else if pps < o.pps_min {
            out.push(format!("La sortie tourne à {pps} points/s, sous la vitesse min ({}).", o.pps_min));
        }
        if sheet.max_pps > 0 && pps > sheet.max_pps {
            out.push(format!("{pps} points/s dépasse le maximum de l'interface sur la fiche du projecteur ({} points/s).", sheet.max_pps));
        }
        if sheet.max_pps > 0 && o.pps_max > sheet.max_pps {
            out.push(format!("Vitesse max ({}) au-dessus du maximum de l'interface sur la fiche du projecteur ({} points/s).", o.pps_max, sheet.max_pps));
        }
        out
    }

    /// What going from `self` to `new` loosens, in French: a faster
    /// fastest scan. Empty = only tighter or equal.
    pub fn loosenings(&self, new: &OutputScan) -> Vec<String> {
        let mut out = Vec::new();
        if new.pps_max > self.pps_max {
            out.push(format!("Vitesse max : {} → {} points/s", self.pps_max, new.pps_max));
        }
        out
    }
}

/// Estimated frames per second of a frame of `points` points at `pps`.
pub fn fps(pps: u32, points: usize) -> f32 {
    if points == 0 {
        0.0
    } else {
        pps as f32 / points as f32
    }
}

/// Caps one frame in place (see the module docs). A channel that is not a
/// number goes dark.
pub fn cap(frame: &mut [Point], limits: &OutputLimits) {
    let l = limits.sanitized();
    let ch = |v: f32, c: f32| if v.is_finite() { v.max(0.0).min(c) * l.max_power } else { 0.0 };
    for p in frame {
        p.r = ch(p.r, l.max_color[0]);
        p.g = ch(p.g, l.max_color[1]);
        p.b = ch(p.b, l.max_color[2]);
    }
}

/// Why `OutputStore::set` refused a change.
#[derive(Debug)]
pub enum SetError {
    /// Out of range, unknown output, or not saved (message).
    Invalid(String),
    /// Raises a cap: needs the operator's explicit confirmation.
    Raises(Vec<String>),
    /// Raises a cap while the laser is armed: never allowed, disarm first.
    Armed(Vec<String>),
}

pub struct OutputStore {
    /// None: in memory only (tests).
    path: Option<PathBuf>,
    file: OutputsFile,
    /// The file existed but could not be read (shown in the UI).
    load_error: Option<String>,
}

impl OutputStore {
    /// A missing file (older installs) gives the defaults. A damaged one
    /// also does, once moved to the quarantine folder (T-402), with the
    /// error reported so the operator notices their caps were not loaded;
    /// arming waits for their acknowledgement (`data_ack`). The defaults
    /// are the cautious ones (50 %), never looser.
    pub fn load_or_create(path: PathBuf) -> Self {
        let (file, load_error) = match crate::datafile::load::<OutputsFile>(&path, &crate::datafile::beside(&path), true) {
            crate::datafile::Loaded::Missing => (OutputsFile::default(), None),
            crate::datafile::Loaded::Ok(f) => (f, None),
            crate::datafile::Loaded::Quarantined(w) => (OutputsFile::default(), Some(format!("{} Plafonds par défaut.", w.message_fr()))),
        };
        let mut store = Self { path: Some(path), file, load_error };
        store.normalise();
        store
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        let mut store = Self { path: None, file: OutputsFile::default(), load_error: None };
        store.normalise();
        store
    }

    /// Sanitises every entry, drops duplicates and makes sure the main
    /// output exists.
    fn normalise(&mut self) {
        let mut seen: Vec<OutputConfig> = Vec::new();
        for o in &self.file.outputs {
            if o.id.is_empty() || seen.iter().any(|s| s.id == o.id) {
                continue;
            }
            seen.push(OutputConfig {
                id: o.id.clone(),
                limits: o.limits.sanitized(),
                projector: o.projector.sanitized(),
                color: o.color.sanitized(),
                scan: o.scan.sanitized(),
                exposure: o.exposure.sanitized(),
            });
        }
        if !seen.iter().any(|o| o.id == MAIN_OUTPUT) {
            seen.insert(0, OutputConfig::default());
        }
        self.file.outputs = seen;
    }

    pub fn list(&self) -> &[OutputConfig] {
        &self.file.outputs
    }

    pub fn get(&self, id: &str) -> Option<&OutputConfig> {
        self.file.outputs.iter().find(|o| o.id == id)
    }

    /// The output the engine drives.
    pub fn active(&self) -> &OutputConfig {
        self.get(MAIN_OUTPUT).expect("normalise keeps the main output")
    }

    /// The caps the engine applies this frame.
    pub fn active_limits(&self) -> OutputLimits {
        self.active().limits
    }

    /// The colour rendering the engine applies this frame (T-414).
    pub fn active_color(&self) -> ColorCalibration {
        self.active().color
    }

    /// The highest brightness that can reach the active output (T-208).
    pub fn max_brightness_effective(&self) -> f32 {
        self.active_limits().max_power
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// The remembered connection (T-401). A file without it (older
    /// installs) or a damaged one gives preview only. The points per
    /// second are kept within what DACs take.
    pub fn connection(&self) -> crate::output_manager::OutputPrefs {
        let mut prefs = self.file.connection.clone();
        prefs.pps = prefs.pps.clamp(1_000, 100_000);
        prefs
    }

    /// A venue projector's entry (T-277), keyed by its projector id, or
    /// the defaults (the cautious caps) while it has none.
    pub fn config_for(&self, id: &str) -> OutputConfig {
        self.get(id).cloned().unwrap_or_else(|| OutputConfig { id: id.into(), ..OutputConfig::default() })
    }

    /// Adds a venue projector's entry with the cautious defaults (T-277),
    /// so its caps and colour can be set like the main output's. An
    /// existing entry is kept as it is.
    pub fn ensure(&mut self, id: &str) -> Result<()> {
        if id.is_empty() || self.get(id).is_some() {
            return Ok(());
        }
        let mut file = self.file.clone();
        file.outputs.push(OutputConfig { id: id.into(), ..OutputConfig::default() });
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&file)?;
            crate::datafile::replace(path, json.as_bytes()).map_err(|e| anyhow::anyhow!("enregistrement impossible de {} : {e:#}", path.display()))?;
        }
        self.file = file;
        Ok(())
    }

    /// Saves the connection part of `outputs.json`, keeping the caps.
    pub fn set_connection(&mut self, prefs: crate::output_manager::OutputPrefs) -> Result<()> {
        if self.file.connection == prefs {
            return Ok(());
        }
        let mut file = self.file.clone();
        file.connection = prefs;
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&file)?;
            crate::datafile::replace(path, json.as_bytes()).map_err(|e| anyhow::anyhow!("enregistrement impossible de {} : {e:#}", path.display()))?;
        }
        self.file = file;
        Ok(())
    }

    /// Changes an output's caps and/or projector sheet (None = keep).
    /// Lowering a cap is applied at once. Raising one is refused while
    /// `armed` (whatever `confirm_raise` says), and otherwise needs
    /// `confirm_raise` (the operator's explicit action). The projector
    /// sheet is informative and never needs a confirmation.
    pub fn set(
        &mut self,
        id: &str,
        limits: Option<OutputLimits>,
        projector: Option<ProjectorInfo>,
        armed: bool,
        confirm_raise: bool,
    ) -> std::result::Result<OutputConfig, SetError> {
        let Some(current) = self.get(id).cloned() else {
            return Err(SetError::Invalid(format!("Sortie inconnue : « {id} »")));
        };
        let next = OutputConfig {
            id: id.into(),
            limits: limits.unwrap_or(current.limits),
            projector: projector.unwrap_or_else(|| current.projector.clone()),
            color: current.color,
            scan: current.scan,
            exposure: current.exposure,
        };
        next.limits.validate().map_err(|e| SetError::Invalid(e.to_string()))?;
        next.projector.validate().map_err(|e| SetError::Invalid(e.to_string()))?;
        let raises = current.limits.raises(&next.limits);
        if !raises.is_empty() && armed {
            return Err(SetError::Armed(raises));
        }
        if !raises.is_empty() && !confirm_raise {
            return Err(SetError::Raises(raises));
        }
        let mut file = self.file.clone();
        if let Some(slot) = file.outputs.iter_mut().find(|o| o.id == id) {
            *slot = next.clone();
        }
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&file).map_err(|e| SetError::Invalid(e.to_string()))?;
            crate::datafile::replace(path, json.as_bytes()).map_err(|e| SetError::Invalid(format!("enregistrement impossible de {} : {e:#}", path.display())))?;
        }
        // Every change of a safety limit is logged (T-259 will move this
        // to the safety journal): old and new values.
        if current.limits != next.limits {
            let level = if raises.is_empty() { log::Level::Info } else { log::Level::Warn };
            log::log!(level, "output {id}: power caps {:?} -> {:?}{}", current.limits, next.limits, if raises.is_empty() { "" } else { " (raised, confirmed by the operator)" });
        }
        if current.projector != next.projector {
            log::info!("output {id}: projector sheet changed ({:?} -> {:?})", current.projector.name, next.projector.name);
        }
        self.file = file;
        self.load_error = None;
        Ok(next)
    }

    /// Changes what the exposure estimator of an output is given (T-257).
    /// Informative: never touches a cap or the output, so it applies armed
    /// or not. Out of range values are refused.
    pub fn set_exposure(&mut self, id: &str, exposure: crate::mpe::ExposureSettings) -> Result<OutputConfig> {
        let Some(current) = self.get(id).cloned() else {
            bail!("Sortie inconnue : « {id} »");
        };
        exposure.validate()?;
        let next = OutputConfig { exposure, ..current.clone() };
        let mut file = self.file.clone();
        if let Some(slot) = file.outputs.iter_mut().find(|o| o.id == id) {
            *slot = next.clone();
        }
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&file)?;
            crate::datafile::replace(path, json.as_bytes()).map_err(|e| anyhow::anyhow!("enregistrement impossible de {} : {e:#}", path.display()))?;
        }
        if current.exposure != next.exposure {
            log::info!("output {id}: exposure estimator inputs {:?} -> {:?}", current.exposure, next.exposure);
        }
        self.file = file;
        Ok(next)
    }

    /// Changes an output's scan limits (T-171). Tighter or equal applies at
    /// once; a faster fastest scan (`pps_max` raised) is refused while
    /// `armed`, like raising a cap. Out of range values are refused.
    pub fn set_scan(&mut self, id: &str, scan: OutputScan, armed: bool) -> std::result::Result<OutputConfig, SetError> {
        let Some(current) = self.get(id).cloned() else {
            return Err(SetError::Invalid(format!("Sortie inconnue : « {id} »")));
        };
        scan.validate().map_err(|e| SetError::Invalid(e.to_string()))?;
        let loosens = current.scan.loosenings(&scan);
        if !loosens.is_empty() && armed {
            return Err(SetError::Armed(loosens));
        }
        let next = OutputConfig { scan, ..current.clone() };
        let mut file = self.file.clone();
        if let Some(slot) = file.outputs.iter_mut().find(|o| o.id == id) {
            *slot = next.clone();
        }
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&file).map_err(|e| SetError::Invalid(e.to_string()))?;
            crate::datafile::replace(path, json.as_bytes()).map_err(|e| SetError::Invalid(format!("enregistrement impossible de {} : {e:#}", path.display())))?;
        }
        if current.scan != next.scan {
            log::info!("output {id}: scan limits {:?} -> {:?}", current.scan, next.scan);
        }
        self.file = file;
        Ok(next)
    }

    /// Changes an output's colour rendering (T-414). Darker or equal
    /// applies at once, armed or not. Brighter (a gain raised, a curve
    /// giving more light at the same input) is treated like raising a cap:
    /// refused while `armed` (whatever `confirm_loosen` says), and
    /// otherwise needs `confirm_loosen`. Never above the caps: those apply
    /// after, whatever this is.
    pub fn set_color(&mut self, id: &str, color: ColorCalibration, armed: bool, confirm_loosen: bool) -> std::result::Result<OutputConfig, SetError> {
        let Some(current) = self.get(id).cloned() else {
            return Err(SetError::Invalid(format!("Sortie inconnue : « {id} »")));
        };
        color.validate().map_err(|e| SetError::Invalid(e.to_string()))?;
        let color = color.sanitized();
        let loosens = current.color.loosenings(&color);
        if !loosens.is_empty() && armed {
            return Err(SetError::Armed(loosens));
        }
        if !loosens.is_empty() && !confirm_loosen {
            return Err(SetError::Raises(loosens));
        }
        let next = OutputConfig { color, ..current.clone() };
        let mut file = self.file.clone();
        if let Some(slot) = file.outputs.iter_mut().find(|o| o.id == id) {
            *slot = next.clone();
        }
        if let Some(path) = &self.path {
            let json = serde_json::to_string_pretty(&file).map_err(|e| SetError::Invalid(e.to_string()))?;
            crate::datafile::replace(path, json.as_bytes()).map_err(|e| SetError::Invalid(format!("enregistrement impossible de {} : {e:#}", path.display())))?;
        }
        if current.color != next.color {
            let level = if loosens.is_empty() { log::Level::Info } else { log::Level::Warn };
            log::log!(level, "output {id}: colour rendering {:?} -> {:?}{}", current.color, next.color, if loosens.is_empty() { "" } else { " (brighter, confirmed by the operator)" });
        }
        self.file = file;
        self.load_error = None;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beat::seeded_rand;
    use crate::controls::{ControlInput, ControlKind};
    use crate::engine::{Animator, AudioFeatures, BeatClock};
    use crate::lfo::{Modulator, Wave};
    use crate::live::{LiveState, Rate};
    use crate::safety::{SafetySettings, StrobeLimiter};

    fn white(n: usize) -> Vec<Point> {
        (0..n).map(|i| Point::lit(i as f32 / n as f32, 0.0, 1.0, 1.0, 1.0)).collect()
    }

    fn assert_within(frame: &[Point], l: &OutputLimits, what: &str) {
        for p in frame {
            for (c, v) in [p.r, p.g, p.b].into_iter().enumerate() {
                assert!(v.is_finite() && v >= 0.0, "{what}: channel {c} = {v}");
                assert!(v <= l.ceiling(c) + 1e-6, "{what}: channel {c} = {v} > {} ({l:?})", l.ceiling(c));
            }
        }
    }

    #[test]
    fn a_venue_projector_starts_with_the_cautious_caps() {
        let mut store = OutputStore::in_memory();
        assert_eq!(store.config_for("p2").limits, OutputLimits::default(), "no entry yet: the defaults");
        store.ensure("p2").unwrap();
        store.set("p2", Some(OutputLimits { max_power: 0.1, ..Default::default() }), None, false, false).unwrap();
        store.ensure("p2").unwrap();
        assert_eq!(store.config_for("p2").limits.max_power, 0.1, "ensure keeps an existing entry");
        assert_eq!(store.active().id, MAIN_OUTPUT, "the main output stays the active one");
        assert!(OutputLimits::default().max_power <= 0.5);
    }

    #[test]
    fn full_brightness_is_scaled_by_max_power() {
        let l = OutputLimits { max_power: 0.3, ..Default::default() };
        let mut f = white(50);
        cap(&mut f, &l);
        assert!(f.iter().all(|p| (p.r - 0.3).abs() < 1e-6 && (p.g - 0.3).abs() < 1e-6 && (p.b - 0.3).abs() < 1e-6));
    }

    #[test]
    fn a_colour_is_clipped_before_the_global_scale() {
        let l = OutputLimits { max_power: 1.0, max_color: [1.0, 0.2, 1.0] };
        let mut f = vec![Point::lit(0.0, 0.0, 0.9, 0.9, 0.1), Point::lit(0.1, 0.0, 0.0, 0.1, 0.0)];
        cap(&mut f, &l);
        assert_eq!((f[0].r, f[0].g, f[0].b), (0.9, 0.2, 0.1));
        assert_eq!((f[1].r, f[1].g, f[1].b), (0.0, 0.1, 0.0), "under the clip: unchanged");
        let l = OutputLimits { max_power: 0.5, max_color: [1.0, 0.2, 1.0] };
        let mut f = vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0)];
        cap(&mut f, &l);
        assert_eq!((f[0].r, f[0].g, f[0].b), (0.5, 0.1, 0.5));
    }

    #[test]
    fn the_cap_only_reduces_and_keeps_dark_points_and_positions() {
        let l = OutputLimits { max_power: 0.7, max_color: [0.4, 0.8, 0.0] };
        let mut f = vec![Point::blanked(0.3, -0.2), Point::lit(0.1, 0.2, 0.2, 0.5, 0.9), Point::lit(0.0, 0.0, f32::NAN, 2.0, -1.0)];
        let before = f.clone();
        cap(&mut f, &l);
        assert_eq!(f[0], before[0]);
        for (a, b) in f.iter().zip(&before) {
            assert_eq!((a.x, a.y), (b.x, b.y));
        }
        assert_within(&f, &l, "odd values");
        assert_eq!((f[2].r, f[2].b), (0.0, 0.0), "NaN and negative go dark");
        // Identity at 100 % / 100 %.
        let mut g = before[..2].to_vec();
        cap(&mut g, &OutputLimits { max_power: 1.0, max_color: [1.0; 3] });
        assert_eq!(g, before[..2].to_vec());
        // A broken limit (NaN) is the tightest: dark.
        let mut h = white(3);
        cap(&mut h, &OutputLimits { max_power: f32::NAN, max_color: [1.0; 3] });
        assert!(h.iter().all(|p| !p.is_lit()));
    }

    #[test]
    fn validation_and_file_clamping() {
        assert!(OutputLimits::default().validate().is_ok());
        for bad in [
            OutputLimits { max_power: 1.2, ..Default::default() },
            OutputLimits { max_power: -0.1, ..Default::default() },
            OutputLimits { max_power: f32::NAN, ..Default::default() },
            OutputLimits { max_color: [1.0, 1.5, 1.0], ..Default::default() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let s = OutputLimits { max_power: 3.0, max_color: [f32::NAN, -1.0, 0.4] }.sanitized();
        assert_eq!(s, OutputLimits { max_power: 1.0, max_color: [0.0, 0.0, 0.4] });

        assert!(ProjectorInfo::default().validate().is_ok());
        let d = ProjectorInfo::default();
        for bad in [
            ProjectorInfo { class: "5".into(), ..d.clone() },
            ProjectorInfo { name: "x".repeat(61), ..d.clone() },
            ProjectorInfo { power_mw: [0.0, -1.0, 0.0], ..d.clone() },
            ProjectorInfo { wavelength_nm: [638, 5000, 445], ..d.clone() },
            ProjectorInfo { divergence_mrad: f32::INFINITY, ..d.clone() },
            ProjectorInfo { scan_angle_deg: 200.0, ..d.clone() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let p = ProjectorInfo { class: "9".into(), power_mw: [f32::NAN, 500.0, -3.0], scan_angle_deg: 999.0, ..d.clone() }.sanitized();
        assert_eq!(p.class, "4");
        assert_eq!(p.power_mw, [0.0, 500.0, 0.0]);
        assert_eq!(p.scan_angle_deg, 180.0);
        for bad in [
            ProjectorInfo { xy_bits: 0, ..d.clone() },
            ProjectorInfo { color_bits: 17, ..d.clone() },
            ProjectorInfo { max_pps: 2_000_000, ..d.clone() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let p = ProjectorInfo { xy_bits: 0, color_bits: 40, max_pps: u32::MAX, ..d.clone() }.sanitized();
        assert_eq!((p.xy_bits, p.color_bits, p.max_pps), (1, 16, 1_000_000));
    }

    #[test]
    fn the_shownet_model_fills_its_resolution_and_point_rate() {
        let d = ProjectorInfo { name: "Projecteur 1".into(), ..Default::default() };
        assert_eq!((d.dac_model, d.xy_bits, d.color_bits, d.max_pps), (DacModel::Other, 16, 16, 0));
        let s = d.with_model(DacModel::ShowNet);
        assert_eq!((s.dac_model, s.xy_bits, s.color_bits, s.max_pps), (DacModel::ShowNet, 12, 8, 100_000));
        assert_eq!(s.name, "Projecteur 1", "the rest of the sheet is kept");
        assert!(s.validate().is_ok());
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["dac_model"], "shownet");
        // A sheet saved before T-472 still loads, as « other ».
        let old: ProjectorInfo = serde_json::from_str(r#"{"name":"P","class":"3B"}"#).unwrap();
        assert_eq!((old.dac_model, old.xy_bits, old.color_bits, old.max_pps), (DacModel::Other, 16, 16, 0));
        assert_eq!(s.with_model(DacModel::Other).max_pps, 0);
    }

    #[test]
    fn lowering_applies_raising_needs_confirmation_and_never_while_armed() {
        let mut st = OutputStore::in_memory();
        assert_eq!(st.active_limits(), OutputLimits::default());
        assert_eq!(st.max_brightness_effective(), 0.5);
        let low = OutputLimits { max_power: 0.3, max_color: [1.0, 0.2, 1.0] };
        // Lowering: at once, even armed.
        st.set(MAIN_OUTPUT, Some(low), None, true, false).unwrap();
        assert_eq!(st.active_limits(), low);
        // Raising while armed: refused, even confirmed.
        let high = OutputLimits { max_power: 0.8, ..low };
        match st.set(MAIN_OUTPUT, Some(high), None, true, true) {
            Err(SetError::Armed(list)) => assert!(list[0].contains("30 % → 80 %"), "{list:?}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(st.active_limits(), low, "unchanged");
        // Disarmed, not confirmed: the list of what goes up.
        match st.set(MAIN_OUTPUT, Some(OutputLimits { max_color: [1.0; 3], ..low }), None, false, false) {
            Err(SetError::Raises(list)) => assert_eq!(list, vec!["Vert max : 20 % → 100 %".to_string()]),
            other => panic!("{other:?}"),
        }
        assert_eq!(st.active_limits(), low);
        // Confirmed: applied.
        st.set(MAIN_OUTPUT, Some(high), None, false, true).unwrap();
        assert_eq!(st.active_limits(), high);
        // The projector sheet alone never asks, even armed; limits kept.
        let sheet = ProjectorInfo { name: "Projecteur 1".into(), class: "3B".into(), power_mw: [300.0, 200.0, 500.0], ..Default::default() };
        st.set(MAIN_OUTPUT, None, Some(sheet.clone()), true, false).unwrap();
        assert_eq!(st.active().projector, sheet);
        assert_eq!(st.active_limits(), high);
        // Invalid values and unknown outputs.
        assert!(matches!(st.set(MAIN_OUTPUT, Some(OutputLimits { max_power: 2.0, ..high }), None, false, true), Err(SetError::Invalid(_))));
        assert!(matches!(st.set("other", None, None, false, false), Err(SetError::Invalid(_))));
    }

    /// T-401: the connection and the caps share `outputs.json`; saving one
    /// keeps the other, and a file without `connection` is preview only.
    #[test]
    fn the_connection_and_the_caps_share_the_file() {
        use crate::output_manager::OutputPrefs;
        let dir = std::env::temp_dir().join(format!("laser-studio-power-conn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("outputs.json");
        std::fs::write(&path, r#"{"outputs":[{"id":"main","limits":{"max_power":0.3}}]}"#).unwrap();
        let mut st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.connection(), OutputPrefs::default());
        assert_eq!(st.connection().selected, None, "older file: preview only");
        let prefs = OutputPrefs { selected: Some("idn:heliospro.local".into()), auto_reconnect: false, pps: 40_000 };
        st.set_connection(prefs.clone()).unwrap();
        let low = OutputLimits { max_power: 0.2, ..st.active_limits() };
        st.set(MAIN_OUTPUT, Some(low), None, false, false).unwrap();
        let st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.connection(), prefs);
        assert_eq!(st.active_limits().max_power, 0.2);
        std::fs::write(&path, r#"{"connection":{"pps":5}}"#).unwrap();
        assert_eq!(OutputStore::load_or_create(path.clone()).connection().pps, 1_000, "clamped");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_installs_without_a_file_get_the_defaults_and_a_broken_file_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("laser-studio-power-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("outputs.json");
        std::fs::remove_file(&path).ok();
        let mut st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.active_limits(), OutputLimits::default());
        assert!(st.load_error().is_none());
        assert!(!path.exists(), "nothing written until a change");
        let l = OutputLimits { max_power: 0.25, max_color: [0.9, 0.5, 1.0] };
        st.set(MAIN_OUTPUT, Some(l), Some(ProjectorInfo { name: "P".into(), ..Default::default() }), false, false).unwrap();
        let st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.active_limits(), l, "persists");
        assert_eq!(st.active().projector.name, "P");
        // Partial and hand-edited files.
        std::fs::write(&path, r#"{"outputs":[{"id":"main","limits":{"max_power":7}}]}"#).unwrap();
        let st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.active_limits(), OutputLimits { max_power: 1.0, max_color: [1.0; 3] });
        assert_eq!(st.active().projector, ProjectorInfo::default());
        std::fs::remove_dir_all(dir.join(crate::datafile::QUARANTINE)).ok();
        std::fs::write(&path, "{ not json").unwrap();
        let mut st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.active_limits(), OutputLimits::default(), "cautious defaults");
        assert!(st.load_error().unwrap().contains("Fichier illisible : outputs.json"));
        // Moved to the quarantine folder (T-402); a save can't destroy it.
        let moved = crate::datafile::tests::quarantined(&dir);
        st.set(MAIN_OUTPUT, Some(OutputLimits { max_power: 0.2, max_color: [1.0; 3] }), None, false, false).unwrap();
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{ not json");
        let w = crate::datafile::take_pending(&dir.join(crate::datafile::QUARANTINE));
        assert!(w.len() == 1 && w[0].safety && w[0].file == "outputs.json");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// T-414: the colour rendering shares `outputs.json` (key `color`);
    /// older files load as identity, a damaged colour entry quarantines
    /// the file like any other damage (arming waits for the ack).
    #[test]
    fn output_colour_persists_beside_the_caps_and_old_files_load_as_identity() {
        use crate::colorcal::ChannelCurve;
        let dir = crate::datafile::tests::temp_dir("power-color");
        let path = dir.join("outputs.json");
        std::fs::write(&path, r#"{"outputs":[{"id":"main","limits":{"max_power":0.4}}],"connection":{}}"#).unwrap();
        let mut st = OutputStore::load_or_create(path.clone());
        assert!(st.load_error().is_none());
        assert!(st.active_color().is_identity(), "no `color` key: identity");
        let darker = ColorCalibration { white_balance: [0.7, 1.0, 1.0], curve: [ChannelCurve::Gamma { gamma: 2.2 }; 3], color_delay_points: -2 };
        st.set_color(MAIN_OUTPUT, darker, false, false).unwrap();
        let brighter = ColorCalibration { white_balance: [1.0; 3], ..darker };
        assert!(matches!(st.set_color(MAIN_OUTPUT, brighter, false, false), Err(SetError::Raises(l)) if l.len() == 1));
        assert!(matches!(st.set_color("ailleurs", darker, false, true), Err(SetError::Invalid(_))));
        // Armed: brighter is refused even confirmed; darker still applies.
        assert!(matches!(st.set_color(MAIN_OUTPUT, brighter, true, true), Err(SetError::Armed(l)) if l.len() == 1));
        assert_eq!(st.active_color(), darker, "unchanged");
        let dimmer = ColorCalibration { white_balance: [0.5, 1.0, 1.0], ..darker };
        st.set_color(MAIN_OUTPUT, dimmer, true, false).unwrap();
        st.set_color(MAIN_OUTPUT, darker, false, true).unwrap();
        let st = OutputStore::load_or_create(path.clone());
        assert_eq!((st.active_color(), st.active_limits().max_power), (darker, 0.4), "persists, caps kept");
        // Hand-edited: a decreasing curve is straightened, a delay clamped.
        std::fs::write(&path, r#"{"outputs":[{"id":"main","color":{"curve":[{"type":"points","out":[0.2,0.6,0.4,0.8,1]},{"type":"gamma","gamma":1},{"type":"gamma","gamma":1}],"color_delay_points":40}}]}"#).unwrap();
        let st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.active_color().curve[0], ChannelCurve::Points { out: [0.0, 0.6, 0.6, 0.8, 1.0] });
        assert_eq!(st.active_color().color_delay_points, crate::colorcal::MAX_DELAY);
        // Damaged: quarantined, identity, the error reported.
        std::fs::write(&path, r#"{"outputs":[{"id":"main","color":{"curve":"oops"}}]}"#).unwrap();
        let st = OutputStore::load_or_create(path.clone());
        assert!(st.active_color().is_identity());
        assert!(st.load_error().is_some());
        let w = crate::datafile::take_pending(&dir.join(crate::datafile::QUARANTINE));
        assert!(w.len() == 1 && w[0].safety, "{w:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The full pipeline as the engine runs it - catalogue cue, random
    /// live modifiers set through the control registry (as MIDI would),
    /// random LFOs on every modulatable control, random audio, the live
    /// stage, the safety stage - then the cap: no channel ever goes over
    /// its ceiling.
    #[test]
    fn no_catalogue_cue_exceeds_the_cap_under_random_modulation() {
        let mut s = crate::test_support::shared();
        let reg_ids: Vec<(String, bool)> = s
            .controls
            .list()
            .iter()
            .filter(|d| d.external && (d.id.starts_with("master.") || d.id.starts_with("look.")))
            .filter_map(|d| match d.kind {
                ControlKind::Continuous { .. } => Some((d.id.clone(), false)),
                ControlKind::Choice { .. } => Some((d.id.clone(), true)),
                _ => None,
            })
            .collect();
        let modulatable: Vec<String> =
            s.controls.list().iter().filter(|d| crate::lfo::modulatable(&s.controls, &d.id).is_some()).map(|d| d.id.clone()).collect();
        assert!(modulatable.iter().any(|id| id == "master.brightness"), "{modulatable:?}");
        let waves = [Wave::Sine, Wave::Triangle, Wave::Square, Wave::SawUp, Wave::SawDown, Wave::Random];
        let (mut frames, mut near_cap) = (0, 0);
        let catalog = crate::presets::catalog();
        for (n, p) in catalog.iter().enumerate() {
            let seed = n as u64 * 7919 + 13;
            let r = |i: u64| seeded_rand(seed, i);
            let limits = OutputLimits { max_power: 0.05 + 0.9 * r(1), max_color: [r(2), 0.1 + 0.9 * r(3), r(4)] };
            // The look at full brightness, then random live controls.
            s.settings = p.settings.clone();
            s.settings.brightness = 1.0;
            s.live = Default::default();
            for (k, (id, _choice)) in reg_ids.iter().enumerate() {
                if r(100 + k as u64) < 0.35 {
                    crate::controls::apply(&mut s, id, ControlInput::Norm(r(200 + k as u64)), true).ok();
                }
            }
            s.live.brightness = s.live.brightness.max(0.8);
            let mods: Vec<Modulator> = (0..4u64)
                .map(|k| Modulator {
                    target: modulatable[(r(300 + k) * modulatable.len() as f32) as usize % modulatable.len()].clone(),
                    wave: waves[(r(310 + k) * 6.0) as usize % 6],
                    rate: if r(320 + k) < 0.5 { Rate::Hz(0.2 + 8.0 * r(330 + k)) } else { Rate::Beats(0.25 + 4.0 * r(340 + k)) },
                    depth: r(350 + k),
                    phase: r(360 + k),
                    offset: 2.0 * r(370 + k) - 1.0,
                    enabled: true,
                    speed_group: 0,
                    part: None,
                })
                .collect();
            let mut animator = Animator::default();
            let mut live_state = LiveState::default();
            let mut limiter = StrobeLimiter::default();
            let safety = SafetySettings::default();
            for i in 0..10u64 {
                let t = i as f64 / 30.0 + n as f64;
                let clock = BeatClock { beat: t * 2.1, bpm: 126.0, beats_per_bar: 4 };
                let audio = AudioFeatures {
                    level: r(400 + i),
                    bass: r(420 + i),
                    beat: i / 3,
                    onset: i / 2,
                    kick: i / 4,
                    kick_strength: r(440 + i),
                    buildup: r(460 + i),
                    ..Default::default()
                };
                let (mut settings, mut live) = (s.settings.clone(), s.live.clone());
                crate::lfo::modulate(&mods, &s.controls, &mut settings, &mut live, t, clock.beat);
                live_state.set_clock(t, clock.beat);
                live_state.advance(&live, 1.0 / 30.0, clock.bpm, clock.beats_per_bar);
                let raw = animator.render(&settings, audio, 1.0 / 30.0, &clock);
                let lit = crate::live::apply(&raw, &live, &live_state, &[]);
                let mut out = crate::safety::apply(lit, t, &safety, &mut limiter);
                cap(&mut out, &limits);
                assert_within(&out, &limits, &format!("cue {} frame {i}", p.id));
                frames += 1;
                // The cap is really reached (the test is not vacuous).
                near_cap += out.iter().any(|q| [q.r, q.g, q.b].iter().enumerate().any(|(c, v)| *v > 0.5 * limits.ceiling(c))) as usize;
            }
        }
        assert!(frames >= 10 * catalog.len() && catalog.len() > 50, "{frames} frames");
        assert!(near_cap * 2 > frames, "only {near_cap} of {frames} frames came near the cap");
        // No control (what MIDI, OSC and the look panel go through) ever
        // touched the caps.
        for d in s.controls.list().to_vec() {
            if matches!(d.kind, ControlKind::Continuous { .. } | ControlKind::Choice { .. } | ControlKind::Toggle { .. }) {
                crate::controls::apply(&mut s, &d.id, ControlInput::Norm(1.0), true).ok();
            }
        }
        assert_eq!(s.outputs.active_limits(), OutputLimits::default());
    }

    #[test]
    fn scan_limits_default_load_from_older_files_and_are_checked() {
        let d = OutputScan::default();
        assert_eq!((d.pps_min, d.pps_max, d.min_points, d.max_points, d.min_fps), (10_000, 40_000, 200, 0, 40.0));
        // A file from before T-171: the defaults, and the rest kept.
        let dir = std::env::temp_dir().join(format!("ls-outscan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("outputs.json");
        std::fs::write(&path, r#"{"outputs":[{"id":"main","limits":{"max_power":0.2}}]}"#).unwrap();
        let mut store = OutputStore::load_or_create(path.clone());
        assert_eq!(store.active().scan, d);
        assert_eq!(store.active_limits().max_power, 0.2);
        // Saved, and back after a restart.
        let mine = OutputScan { pps_min: 12_000, pps_max: 30_000, min_points: 150, max_points: 900, min_fps: 30.0 };
        store.set_scan(MAIN_OUTPUT, mine, false).unwrap();
        assert_eq!(OutputStore::load_or_create(path.clone()).active().scan, mine);
        // Out of range or inconsistent: refused, nothing changed.
        for bad in [
            OutputScan { pps_min: 50_000, ..d },
            OutputScan { pps_max: 500_000, ..d },
            OutputScan { pps_min: 500, ..d },
            OutputScan { max_points: 50, ..d },
            OutputScan { min_points: 900, max_points: 500, ..d },
            OutputScan { min_fps: f32::NAN, ..d },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
            assert!(matches!(store.set_scan(MAIN_OUTPUT, bad, false), Err(SetError::Invalid(_))));
        }
        assert!(matches!(store.set_scan("p9", d, false), Err(SetError::Invalid(_))));
        assert_eq!(store.active().scan, mine);
        // A hand-edited file is brought into range.
        std::fs::write(&path, r#"{"outputs":[{"id":"main","scan":{"pps_min":90000,"pps_max":5,"min_points":99999,"max_points":3,"min_fps":-1}}]}"#).unwrap();
        let s = OutputStore::load_or_create(path).active().scan;
        assert!(s.validate().is_ok(), "{s:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_faster_scan_is_refused_while_armed_and_slower_applies() {
        let mut store = OutputStore::in_memory();
        let d = OutputScan::default();
        let faster = OutputScan { pps_max: 60_000, ..d };
        assert!(matches!(store.set_scan(MAIN_OUTPUT, faster, true), Err(SetError::Armed(_))));
        assert_eq!(store.active().scan, d);
        // Slower applies armed; faster applies disarmed.
        store.set_scan(MAIN_OUTPUT, OutputScan { pps_max: 20_000, ..d }, true).unwrap();
        store.set_scan(MAIN_OUTPUT, faster, false).unwrap();
        assert_eq!(store.active().scan.pps_max, 60_000);
    }

    #[test]
    fn the_scan_range_bounds_the_presets_and_warns_above_the_sheet() {
        let d = OutputScan::default();
        // 30 kpps, 10..40 kpps: 50 (not 33) .. 133 %.
        let l = d.scan_limits(30_000);
        assert_eq!(l.min_scan_pct, 50.0);
        assert!((l.max_scan_pct - 133.333).abs() < 0.01, "{l:?}");
        assert_eq!((l.min_points, l.max_points), (200, crate::layers::MAX_POINT_BUDGET));
        // Running above the range: slower than as drawn.
        assert!((d.scan_limits(80_000).max_scan_pct - 50.0).abs() < 1e-3);
        // Running far below: within 50..150 % still.
        assert_eq!(d.scan_limits(5_000).min_scan_pct, 150.0);
        assert_eq!(d.scan_limits(0).max_scan_pct, 100.0);
        // Points per frame for the engine.
        assert_eq!(d.frame_limits(), crate::output::FrameLimits { min: 200, max: None });
        assert_eq!(OutputScan { max_points: 600, ..d }.frame_limits().max, Some(600));
        // Frame rate: estimate and alert (35 for 40).
        assert_eq!(fps(30_000, 750), 40.0);
        assert_eq!(fps(30_000, 0), 0.0);
        assert_eq!(d.alert_fps(), 35.0);
        // Warnings, never a silent clamp.
        let sheet = ProjectorInfo::default();
        assert!(d.warnings(30_000, &sheet).is_empty());
        assert_eq!(d.warnings(50_000, &sheet).len(), 1);
        assert_eq!(d.warnings(5_000, &sheet).len(), 1);
        let slow_dac = ProjectorInfo { max_pps: 20_000, ..sheet };
        let w = d.warnings(30_000, &slow_dac);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w.iter().any(|m| m.contains("20000")));
        assert_eq!(d.scan_limits(30_000), d.scan_limits(30_000), "the sheet never changes the limits");
    }
}
