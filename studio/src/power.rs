//! Per-output power caps and projector sheets (T-254).
//!
//! Brightness is a look setting (0..1) that cues, LFOs, audio and MIDI can
//! all drive to 1.0. The cap is a hardware ceiling per output that nothing
//! upstream can exceed: it is applied to the finished frame, **after** the
//! whole safety stage (`safety::apply`: horizon, zones, colour calibration,
//! strobe limiter) and before the output gate, so every source goes
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
//! (safety sheet) read it. It never arms anything and never blocks arming.
//!
//! Stored in `<data-dir>/outputs.json`, one entry per output. Lowering a
//! cap applies on the next engine tick. Raising one needs the operator's
//! explicit confirmation, and is refused outright while the laser is armed.

use crate::patterns::Point;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The one output the studio drives today (T-277 adds more).
pub const MAIN_OUTPUT: &str = "main";

/// The IEC 60825-1 classes the sheet accepts ("" = not filled in).
pub const LASER_CLASSES: [&str; 8] = ["", "1", "1M", "2", "2M", "3R", "3B", "4"];

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
/// only (estimates and the safety sheet); never used to allow anything.
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
        }
    }
}

/// One output: its cap and its projector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub id: String,
    pub limits: OutputLimits,
    pub projector: ProjectorInfo,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self { id: MAIN_OUTPUT.into(), limits: OutputLimits::default(), projector: ProjectorInfo::default() }
    }
}

/// `outputs.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputsFile {
    pub outputs: Vec<OutputConfig>,
}

const UNIT: (f32, f32) = (0.0, 1.0);
const POWER_MW: (f32, f32) = (0.0, 1_000_000.0);
const WAVELENGTH_NM: (u16, u16) = (0, 2_000);
const APERTURE_MM: (f32, f32) = (0.0, 100.0);
const DIVERGENCE_MRAD: (f32, f32) = (0.0, 100.0);
const SCAN_ANGLE_DEG: (f32, f32) = (0.0, 180.0);
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
        Ok(())
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
        }
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
    /// A missing file (older installs) gives the defaults. An unreadable
    /// one also does, and is kept aside as `outputs.json.bad` with the
    /// error reported, so the operator notices their caps were not loaded.
    /// The defaults are the cautious ones (50 %), never looser.
    pub fn load_or_create(path: PathBuf) -> Self {
        let (file, load_error) = match std::fs::read_to_string(&path) {
            Err(_) => (OutputsFile::default(), None),
            Ok(text) => match serde_json::from_str::<OutputsFile>(&text) {
                Ok(f) => (f, None),
                Err(e) => {
                    let bad = path.with_extension("json.bad");
                    std::fs::copy(&path, &bad).ok();
                    log::warn!("unreadable {} ({e}), kept as {}; using the default power caps", path.display(), bad.display());
                    (OutputsFile::default(), Some(format!("outputs.json illisible ({e}) : copie dans {}, plafonds par défaut", bad.display())))
                }
            },
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
            seen.push(OutputConfig { id: o.id.clone(), limits: o.limits.sanitized(), projector: o.projector.sanitized() });
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

    /// The highest brightness that can reach the active output (T-208).
    pub fn max_brightness_effective(&self) -> f32 {
        self.active_limits().max_power
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
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
            std::fs::write(path, json).map_err(|e| SetError::Invalid(format!("enregistrement impossible de {} : {e}", path.display())))?;
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
        std::fs::write(&path, "{ not json").unwrap();
        let st = OutputStore::load_or_create(path.clone());
        assert_eq!(st.active_limits(), OutputLimits::default(), "cautious defaults");
        assert!(st.load_error().unwrap().contains("outputs.json illisible"));
        assert!(dir.join("outputs.json.bad").exists());
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
}
