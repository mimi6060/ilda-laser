//! Mirror targets (T-109): named points of the room the operator aims at
//! (mirror balls, mirror walls, a light cage), and the « Cibles miroir »
//! generator that sends beams to them. Our own model, from the idea of
//! bouncing beams off mirrors seen in public show footage.
//!
//! **Where a target is.** Up to `MAX_TARGETS` points `(x, y)` in the
//! look's coordinates (-1..1, the frame the generators draw in, before the
//! calibration). With the look at its neutral position and the cue on the
//! default projection zone, the main output's calibration takes a target
//! to where its beam really goes: that is where the status checks it
//! (`status`) and where the UI draws it on the 2D preview. A target is
//! aimed in the preview, then checked with the laser at low power.
//!
//! **Machine settings.** Like the calibration and the zones, the targets
//! belong to this machine (`targets.json`), never to a look, a project or
//! a profile: a cue only holds the set of targets it lights (a bit mask).
//! They are edited from the UI, the keyboard or the local API only (no
//! control, MIDI or network input reaches them). Adding a target or moving
//! one sends light somewhere new, so it is a loosening: it needs the
//! operator's confirmation (T-260), is refused while the laser is armed,
//! under the safety lock (T-261) and in show mode (T-283). Removing or
//! renaming one only tightens or changes nothing.
//!
//! **No exemption.** A target's beam is a static beam aimed at a fixed
//! point, the most dangerous content there is. The generator only makes
//! points; everything after it - the mixer, the live stage, the
//! calibration and the whole safety stage (beam horizon, Blank and Public
//! zones, outdoor sectors and « Ciel coupé », the strobe limiter, the
//! static-beam guard, the power caps, the arm gate) - applies exactly as
//! to any other look. A target below the horizon is refused when it is
//! added or moved (its beam would always be cut there, T-101); one inside
//! a Blank or Public zone, outside the declared outdoor angles or under
//! « Ciel coupé » is kept but dark, and `status` says why; the guard caps
//! the dose of a beam held on one, and blanks it in its strict profile.
//!
//! **The generator.** `GenParams::a` is the mask (bit i = target i + 1);
//! `b` ≥ 0.5 chases: one target of the mask lit per step
//! (`steps_per_beat`, one per beat by default), the others visited dark,
//! backwards with `direction` < 0. A step never lasts less than half a
//! period of the strobe limit (`safety::DEFAULT_MAX_HZ`), whatever the
//! tempo: a beam hopping between two targets can't make either of them
//! flash faster than the limiter allows. No target lit: one dark point.

use crate::engine::Calibration;
use crate::generators::{BeamStyle, GenCtx, GenParams, Geometry};
use crate::safety::SafetySettings;
use crate::zones::{Horizon, Mask, ZoneKind};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::{LazyLock, RwLock};

pub const MAX_TARGETS: usize = 16;
pub const MAX_NAME: usize = 40;
/// The generator's name (`generators::GENERATOR_NAMES`).
pub const GENERATOR: &str = "mirror_targets";

/// One named point of the room.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Target {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

impl Default for Target {
    fn default() -> Self {
        Self { name: "Cible".into(), x: 0.0, y: 0.5 }
    }
}

/// `targets.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Targets {
    pub items: Vec<Target>,
}

fn finite_in_range(v: f32) -> bool {
    v.is_finite() && (-1.0..=1.0).contains(&v)
}

impl Targets {
    /// Refuses what the store must never hold (French messages).
    pub fn validate(&self) -> Result<()> {
        if self.items.len() > MAX_TARGETS {
            bail!("{} cibles : {MAX_TARGETS} au plus", self.items.len());
        }
        for (i, t) in self.items.iter().enumerate() {
            let n = i + 1;
            let name = t.name.trim();
            if name.is_empty() {
                bail!("Cible {n} : nom obligatoire");
            }
            if name.chars().count() > MAX_NAME {
                bail!("Cible {n} : {MAX_NAME} caractères au plus pour le nom");
            }
            if name.chars().any(char::is_control) {
                bail!("Cible {n} : caractères invalides dans le nom");
            }
            if !finite_in_range(t.x) || !finite_in_range(t.y) {
                bail!("Cible {n} « {name} » : position ({}, {}) hors du champ (-1 à 1)", t.x, t.y);
            }
        }
        Ok(())
    }

    /// For a file edited by hand: a target whose position is not a number
    /// is dropped (no beam is better than a guessed one), positions
    /// clamped to the field, names cleaned, at most `MAX_TARGETS`.
    pub fn sanitized(&self) -> Self {
        let items = self
            .items
            .iter()
            .filter(|t| t.x.is_finite() && t.y.is_finite())
            .take(MAX_TARGETS)
            .enumerate()
            .map(|(i, t)| {
                let name: String = t.name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect::<String>().trim().to_string();
                Target { name: if name.is_empty() { format!("Cible {}", i + 1) } else { name }, x: t.x.clamp(-1.0, 1.0), y: t.y.clamp(-1.0, 1.0) }
            })
            .collect();
        Self { items }
    }

    /// The targets of `new` that light somewhere new: added at an index
    /// that had none, or moved. A cue lights targets by index, so a
    /// target that takes the place of another one (one removed before it)
    /// counts as moved.
    fn changed<'a>(&self, new: &'a Targets) -> Vec<(usize, &'a Target)> {
        new.items
            .iter()
            .enumerate()
            .filter(|(i, t)| self.items.get(*i).is_none_or(|old| old.x != t.x || old.y != t.y))
            .collect()
    }

    /// What going from `self` to `new` would loosen (French), for the
    /// operator's confirmation. Empty = only tighter or equal (removing
    /// the last targets, renaming).
    pub fn loosenings(&self, new: &Targets) -> Vec<String> {
        self.changed(new)
            .into_iter()
            .map(|(i, t)| match self.items.get(i) {
                None => format!("Cible {} « {} » ajoutée : un faisceau fixe pourra y être envoyé", i + 1, t.name.trim()),
                Some(old) => format!(
                    "Cible {} « {} » déplacée de ({:.2}, {:.2}) à ({:.2}, {:.2}) : ses faisceaux vont ailleurs",
                    i + 1,
                    t.name.trim(),
                    old.x,
                    old.y,
                    t.x,
                    t.y
                ),
            })
            .collect()
    }
}

/// Where a target's beam goes and what the safety stage does to it there.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TargetStatus {
    /// Output coordinates (after the main output's calibration).
    pub out: [f32; 2],
    /// The zones and the horizon leave this much of its light (0 = cut).
    pub level: f32,
    /// Why it is cut or dimmed (French); empty = nothing in the way.
    pub reasons: Vec<String>,
}

/// The output point of a target through the calibration.
pub fn output_of(t: &Target, cal: &Calibration) -> (f32, f32) {
    cal.sanitized().apply(t.x, t.y)
}

/// What the safety settings in force (`Shared::effective_safety`: Public
/// zones blanked outside audience scanning, « Ciel coupé ») do to a beam
/// on `t`. The static-beam guard depends on time: the page shows its
/// cells from the frame (`DwellStatus`).
pub fn status(t: &Target, cfg: &SafetySettings, cal: &Calibration) -> TargetStatus {
    let (x, y) = output_of(t, cal);
    let mut reasons = Vec::new();
    let mut level = cfg.mask().factor(x, y).clamp(0.0, 1.0);
    if y <= cfg.horizon.y {
        reasons.push("Sous l'horizon : un faisceau y est toujours coupé".to_string());
        level = 0.0;
    }
    let no_lines = Horizon { lines: false, ..cfg.horizon };
    for z in &cfg.zones {
        let f = Mask::new(std::slice::from_ref(z), no_lines, [1.0; 3], 0.0).factor(x, y);
        if f >= 1.0 {
            continue;
        }
        let name = z.name.trim();
        reasons.push(if z.audience {
            if z.kind == ZoneKind::Blank || f <= 0.0 {
                format!("Dans la zone Public « {name} » : coupée")
            } else {
                format!("Dans la zone Public « {name} » : atténuée à {:.0} %", f * 100.0)
            }
        } else if z.kind == ZoneKind::Blank || f <= 0.0 {
            format!("Dans la zone sans laser « {name} » : coupée")
        } else {
            format!("Dans la zone « {name} » : atténuée à {:.0} %", f * 100.0)
        });
    }
    if cfg.horizon.lines && y < cfg.horizon.y + cfg.horizon.ramp && y > cfg.horizon.y {
        reasons.push("Dans le fondu de l'horizon : atténuée".to_string());
    }
    if cfg.outdoor.active() {
        let outdoor_only = SafetySettings { zones: Vec::new(), horizon: no_lines, color_gain: [1.0; 3], min_diode_level: 0.0, ..cfg.clone() };
        if outdoor_only.mask().factor(x, y) <= 0.0 {
            reasons.push(if cfg.outdoor.sky_off && y >= 0.0 {
                "Ciel coupé : éteinte".to_string()
            } else {
                "Hors des angles déclarés (mode extérieur) : coupée".to_string()
            });
        }
    }
    TargetStatus { out: [x, y], level, reasons }
}

pub enum SetError {
    /// 400: not a valid list, or a new / moved target below the horizon.
    Invalid(String),
    /// 409: needs the operator's confirmation.
    Loosens(Vec<String>),
    /// 409: loosening while armed (or under the safety lock), confirmed or not.
    Armed(Vec<String>),
}

/// The machine's targets (`targets.json`).
pub struct TargetStore {
    path: Option<PathBuf>,
    targets: Targets,
}

impl TargetStore {
    pub fn load(path: PathBuf) -> Self {
        let targets: Targets = crate::load_json(&path);
        Self::with(Some(path), targets.sanitized())
    }

    pub fn in_memory() -> Self {
        Self::with(None, Targets::default())
    }

    fn with(path: Option<PathBuf>, targets: Targets) -> Self {
        publish(&targets);
        Self { path, targets }
    }

    pub fn get(&self) -> &Targets {
        &self.targets
    }

    /// Replaces the list. `cfg`, `cal`: the safety settings in force and
    /// the main output's calibration (a new or moved target below the
    /// horizon is refused). `locked`: armed or under the safety lock (no
    /// loosening at all); `confirm`: the operator confirmed the loosening.
    pub fn set(&mut self, new: Targets, cfg: &SafetySettings, cal: &Calibration, locked: bool, confirm: bool) -> std::result::Result<Targets, SetError> {
        new.validate().map_err(|e| SetError::Invalid(e.to_string()))?;
        let new = Targets { items: new.items.into_iter().map(|t| Target { name: t.name.trim().to_string(), ..t }).collect() };
        for (i, t) in self.targets.changed(&new) {
            let (_, y) = output_of(t, cal);
            if y <= cfg.horizon.y {
                return Err(SetError::Invalid(format!(
                    "Cible {} « {} » refusée : sous l'horizon (y = {:.2} en sortie, horizon à {:.2}), un faisceau y est toujours coupé. Visez au-dessus de l'horizon.",
                    i + 1,
                    t.name,
                    y,
                    cfg.horizon.y
                )));
            }
        }
        let loosen = self.targets.loosenings(&new);
        if !loosen.is_empty() {
            if locked {
                return Err(SetError::Armed(loosen));
            }
            if !confirm {
                return Err(SetError::Loosens(loosen));
            }
        }
        self.targets = new;
        publish(&self.targets);
        if let Some(path) = &self.path {
            crate::save_json(path, &self.targets);
        }
        Ok(self.targets.clone())
    }

    /// Tests: a list stored as a hand-edited file could hold it (sanitised,
    /// no check against the safety settings).
    #[cfg(test)]
    pub fn insert_for_tests(&mut self, targets: Targets) {
        self.targets = targets.sanitized();
        publish(&self.targets);
    }
}

// ---------- what the generator reads ----------

type Points = Arc<Vec<(f32, f32)>>;

/// The positions the generator draws, shared with every thread that
/// renders looks (the engine, the exports), like the ILDA clips.
#[cfg(not(test))]
static CURRENT: LazyLock<RwLock<Points>> = LazyLock::new(Default::default);

#[cfg(test)]
thread_local! {
    /// Tests: per thread, so parallel tests (and the fuzz's workers) each
    /// see their own studio's targets.
    static CURRENT: std::cell::RefCell<Points> = std::cell::RefCell::new(Points::default());
}

fn publish(t: &Targets) {
    let points: Points = Arc::new(t.items.iter().map(|t| (t.x, t.y)).collect());
    #[cfg(not(test))]
    {
        *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = points;
    }
    #[cfg(test)]
    CURRENT.with(|c| *c.borrow_mut() = points);
}

fn current() -> Points {
    #[cfg(not(test))]
    {
        Arc::clone(&CURRENT.read().unwrap_or_else(|e| e.into_inner()))
    }
    #[cfg(test)]
    CURRENT.with(|c| Arc::clone(&c.borrow()))
}

/// The target indices bit-set in `a` (non-finite or negative: none).
pub fn mask_indices(a: f32, n: usize) -> Vec<usize> {
    if !a.is_finite() || a < 0.5 {
        return Vec::new();
    }
    let bits = (a.round().min(u16::MAX as f32)) as u32;
    (0..n.min(MAX_TARGETS)).filter(|i| bits & (1 << i) != 0).collect()
}

/// Chase steps per beat, halved until a step lasts at least half a period
/// of the strobe limit at `bpm`.
fn safe_steps_per_beat(steps: f32, bpm: f32) -> f32 {
    let max_steps_per_s = 2.0 * crate::safety::DEFAULT_MAX_HZ;
    let bps = if bpm.is_finite() && bpm > 0.0 { bpm / 60.0 } else { 4.0 * crate::tempo::MAX_BPM as f32 / 60.0 };
    let mut s = if steps.is_finite() && steps > 0.0 { steps.min(16.0) } else { 1.0 };
    while s * bps > max_steps_per_s && s > 1.0 / 64.0 {
        s /= 2.0;
    }
    s
}

/// The step of a chase (see `safe_steps_per_beat`).
fn chase_step(p: &GenParams, ctx: &GenCtx) -> u64 {
    let spb = safe_steps_per_beat(p.steps_per_beat, ctx.bpm);
    crate::beat::step_index(ctx.beat_pos, spb)
}

/// « Cibles miroir »; `None` for any other generator name.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    if name != GENERATOR {
        return None;
    }
    Some(geometry(&current(), p, ctx))
}

fn geometry(points: &[(f32, f32)], p: &GenParams, ctx: &GenCtx) -> Geometry {
    let chosen = mask_indices(p.a, points.len());
    if chosen.is_empty() {
        return Geometry::dots(vec![(0.0, 0.0)]).with_styles(vec![BeamStyle { intensity: 0.0, ..Default::default() }]);
    }
    let dots: Vec<(f32, f32)> = chosen.iter().map(|&i| points[i]).collect();
    let chase = p.b.is_finite() && p.b >= 0.5;
    if !chase {
        return Geometry::dots(dots);
    }
    let n = chosen.len() as u64;
    let k = chase_step(p, ctx) % n;
    let lit = if p.direction < 0 { (n - k) % n } else { k } as usize;
    let styles = (0..chosen.len()).map(|i| BeamStyle { intensity: if i == lit { 1.0 } else { 0.0 }, ..Default::default() }).collect();
    Geometry::dots(dots).with_styles(styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::Zone;

    fn t(name: &str, x: f32, y: f32) -> Target {
        Target { name: name.into(), x, y }
    }

    fn list(items: &[Target]) -> Targets {
        Targets { items: items.to_vec() }
    }

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn validation_refuses_bad_names_positions_and_too_many() {
        assert!(list(&[t("Boule", 0.2, 0.5)]).validate().is_ok());
        assert!(list(&[t("  ", 0.2, 0.5)]).validate().is_err());
        assert!(list(&[t(&"x".repeat(MAX_NAME + 1), 0.2, 0.5)]).validate().is_err());
        assert!(list(&[t("a\nb", 0.2, 0.5)]).validate().is_err());
        assert!(list(&[t("A", f32::NAN, 0.5)]).validate().is_err());
        assert!(list(&[t("A", 0.0, 1.5)]).validate().is_err());
        assert!(list(&[t("A", f32::INFINITY, 0.0)]).validate().is_err());
        let many: Vec<Target> = (0..=MAX_TARGETS).map(|i| t(&format!("C{i}"), 0.0, 0.5)).collect();
        assert!(list(&many).validate().is_err());
        assert!(list(&many[..MAX_TARGETS]).validate().is_ok());
    }

    #[test]
    fn a_hand_edited_file_drops_what_is_not_a_number() {
        let s = list(&[t("A", f32::NAN, 0.5), t("", 3.0, -4.0), t("B\u{7}", 0.1, 0.2)]).sanitized();
        assert_eq!(s.items, vec![t("Cible 1", 1.0, -1.0), t("B", 0.1, 0.2)]);
        let many: Vec<Target> = (0..40).map(|i| t(&format!("C{i}"), 0.0, 0.5)).collect();
        assert_eq!(list(&many).sanitized().items.len(), MAX_TARGETS);
    }

    #[test]
    fn adding_or_moving_loosens_removing_and_renaming_do_not() {
        let a = list(&[t("A", 0.1, 0.5), t("B", -0.4, 0.6)]);
        assert!(a.loosenings(&a).is_empty());
        assert!(a.loosenings(&list(&[t("A", 0.1, 0.5)])).is_empty(), "the last one removed");
        assert!(a.loosenings(&list(&[t("Boule", 0.1, 0.5), t("Mur", -0.4, 0.6)])).is_empty(), "renamed");
        assert!(a.loosenings(&Targets::default()).is_empty());
        let added = a.loosenings(&list(&[t("A", 0.1, 0.5), t("B", -0.4, 0.6), t("C", 0.0, 0.8)]));
        assert_eq!(added.len(), 1);
        assert!(added[0].contains("Cible 3 « C » ajoutée"), "{added:?}");
        let moved = a.loosenings(&list(&[t("A", 0.1, 0.5), t("B", -0.4, 0.7)]));
        assert!(moved.len() == 1 && moved[0].contains("déplacée"), "{moved:?}");
        // The first removed: B takes index 1, where cues light A.
        let shifted = a.loosenings(&list(&[t("B", -0.4, 0.6)]));
        assert!(shifted.len() == 1 && shifted[0].contains("Cible 1 « B » déplacée"), "{shifted:?}");
    }

    #[test]
    fn the_store_needs_a_confirmation_refuses_armed_and_below_the_horizon() {
        let mut store = TargetStore::in_memory();
        let cfg = SafetySettings::default();
        let cal = Calibration::default();
        let one = list(&[t("Boule", 0.2, 0.5)]);
        assert!(matches!(store.set(one.clone(), &cfg, &cal, false, false), Err(SetError::Loosens(l)) if l.len() == 1));
        assert!(matches!(store.set(one.clone(), &cfg, &cal, true, true), Err(SetError::Armed(_))), "armed or locked: never");
        assert!(store.get().items.is_empty());
        assert_eq!(store.set(one.clone(), &cfg, &cal, false, true).ok(), Some(one.clone()));
        // Renaming while armed: nothing loosened.
        let renamed = list(&[t("Miroir", 0.2, 0.5)]);
        assert!(store.set(renamed.clone(), &cfg, &cal, true, false).is_ok());
        // Below the horizon (y = 0 by default): refused with a message, even confirmed.
        let low = list(&[t("Miroir", 0.2, 0.5), t("Sol", 0.0, -0.3)]);
        match store.set(low, &cfg, &cal, false, true) {
            Err(SetError::Invalid(msg)) => assert!(msg.contains("sous l'horizon") && msg.contains("Sol"), "{msg}"),
            _ => panic!("a target below the horizon must be refused"),
        }
        // On the horizon itself: refused too (the boundary is dark).
        assert!(matches!(store.set(list(&[t("Miroir", 0.2, 0.0)]), &cfg, &cal, false, true), Err(SetError::Invalid(_))));
        // Through the calibration: aimed above, sent below.
        let down = Calibration { offset_y: -0.8, ..Default::default() };
        assert!(matches!(store.set(list(&[t("Miroir", 0.2, 0.5), t("B", 0.0, 0.5)]), &cfg, &down, false, true), Err(SetError::Invalid(_))));
        // A target already there under a raised horizon can still be renamed.
        let high = SafetySettings { horizon: Horizon { y: 0.8, ..Default::default() }, ..Default::default() };
        assert!(store.set(list(&[t("Boule haute", 0.2, 0.5)]), &high, &cal, false, false).is_ok());
        assert_eq!(store.get().items[0].name, "Boule haute");
    }

    #[test]
    fn targets_persist_across_a_restart() {
        let dir = std::env::temp_dir().join(format!("laser-studio-targets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("targets.json");
        let one = list(&[t("Boule", 0.2, 0.5), t("Mur", -0.6, 0.3)]);
        {
            let mut store = TargetStore::load(path.clone());
            store.set(one.clone(), &SafetySettings::default(), &Calibration::default(), false, true).ok().unwrap();
        }
        crate::datafile::flush();
        let again = TargetStore::load(path);
        assert_eq!(again.get(), &one);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_status_says_why_a_target_is_cut() {
        let cal = Calibration::default();
        let blank = Zone { id: 1, name: "Régie".into(), kind: ZoneKind::Blank, points: square(0.0, 0.0, 0.5, 1.0), ..Default::default() };
        let public = Zone { id: 2, name: "Fosse".into(), kind: ZoneKind::Dim, level: 0.3, points: square(-1.0, 0.0, -0.5, 1.0), audience: true, ..Default::default() };
        let dim = Zone { id: 3, name: "Bar".into(), kind: ZoneKind::Dim, level: 0.5, points: square(0.6, 0.6, 1.0, 1.0), ..Default::default() };
        let cfg = SafetySettings { zones: vec![blank, public, dim], ..Default::default() };
        // Outside audience scanning the engine blanks Public zones.
        let cfg = crate::audience::effective(&cfg, false);
        let free = status(&t("Libre", -0.2, 0.5), &cfg, &cal);
        assert_eq!((free.level, free.reasons.len()), (1.0, 0));
        let s = status(&t("A", 0.25, 0.5), &cfg, &cal);
        assert_eq!(s.level, 0.0);
        assert!(s.reasons[0].contains("sans laser « Régie »"), "{:?}", s.reasons);
        let s = status(&t("B", -0.75, 0.5), &cfg, &cal);
        assert_eq!(s.level, 0.0);
        assert!(s.reasons[0].contains("Public « Fosse » : coupée"), "{:?}", s.reasons);
        let s = status(&t("C", 0.8, 0.8), &cfg, &cal);
        assert!((s.level - 0.5).abs() < 1e-6 && s.reasons[0].contains("atténuée à 50 %"), "{s:?}");
        let s = status(&t("D", 0.0, -0.5), &cfg, &cal);
        assert_eq!(s.level, 0.0);
        assert!(s.reasons[0].contains("Sous l'horizon"), "{:?}", s.reasons);
        // Through the calibration: the output point is what is checked.
        let shifted = Calibration { offset_x: 0.45, ..Default::default() };
        let s = status(&t("E", -0.2, 0.5), &cfg, &shifted);
        assert!((s.out[0] - 0.25).abs() < 1e-6 && s.level == 0.0, "{s:?}");
        // « Ciel coupé »: everything above the horizon line goes dark.
        let mut sky = SafetySettings::default();
        sky.outdoor.sky_off = true;
        let s = status(&t("F", -0.2, 0.5), &sky, &cal);
        assert_eq!(s.level, 0.0);
        assert!(s.reasons.iter().any(|r| r.contains("Ciel coupé")), "{:?}", s.reasons);
        // Outdoor mode with no sector: everything is cut.
        let mut out = SafetySettings::default();
        out.outdoor.enabled = true;
        let s = status(&t("G", -0.2, 0.5), &out, &cal);
        assert_eq!(s.level, 0.0);
        assert!(s.reasons.iter().any(|r| r.contains("angles déclarés")), "{:?}", s.reasons);
    }

    #[test]
    fn the_mask_picks_targets_by_bit() {
        assert_eq!(mask_indices(0.0, 4), Vec::<usize>::new());
        assert_eq!(mask_indices(1.0, 4), vec![0]);
        assert_eq!(mask_indices(5.0, 4), vec![0, 2]);
        assert_eq!(mask_indices(255.0, 3), vec![0, 1, 2], "bits past the list are ignored");
        assert_eq!(mask_indices(65535.0, 20).len(), MAX_TARGETS);
        assert_eq!(mask_indices(1e9, 2), vec![0, 1]);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -3.0] {
            assert!(mask_indices(bad, 4).is_empty(), "{bad}");
        }
    }

    fn ctx(beat_pos: f64, bpm: f32) -> GenCtx {
        GenCtx { beat_pos, bpm, ..GenCtx::at_time(0.0, 0.0, 0.0, 0.7) }
    }

    fn lit(geo: &Geometry) -> Vec<(f32, f32)> {
        geo.strokes.iter().enumerate().filter(|(i, _)| geo.styles.get(*i).is_none_or(|s| s.intensity > 0.0)).map(|(_, s)| s[0]).collect()
    }

    #[test]
    fn the_generator_lights_the_masked_targets_at_their_positions() {
        let pts = [(0.1, 0.5), (-0.4, 0.6), (0.7, 0.9)];
        let all = GenParams { a: 5.0, b: 0.0, ..Default::default() };
        let geo = geometry(&pts, &all, &ctx(0.0, 120.0));
        assert!(geo.dots);
        assert_eq!(lit(&geo), vec![(0.1, 0.5), (0.7, 0.9)], "not scaled by the look's size");
        // Nothing chosen (or no target): one dark point.
        for (p, a) in [(&pts[..], 0.0), (&[][..], 7.0)] {
            let geo = geometry(p, &GenParams { a, ..Default::default() }, &ctx(0.0, 120.0));
            assert_eq!(geo.strokes.len(), 1);
            assert!(lit(&geo).is_empty());
        }
        // Through `generators::generate`: the published list.
        TargetStore::in_memory().insert_for_tests(Targets { items: pts.iter().map(|&(x, y)| t("T", x, y)).collect() });
        let geo = crate::generators::generate(GENERATOR, &GenParams { a: 2.0, b: 0.0, ..Default::default() }, &ctx(0.0, 120.0)).unwrap();
        assert_eq!(lit(&geo), vec![(-0.4, 0.6)]);
        TargetStore::in_memory();
        let geo = crate::generators::generate(GENERATOR, &GenParams { a: 2.0, b: 0.0, ..Default::default() }, &ctx(0.0, 120.0)).unwrap();
        assert!(lit(&geo).is_empty(), "a new store: no targets");
    }

    #[test]
    fn the_chase_lights_one_target_per_beat() {
        let pts = [(0.1, 0.5), (-0.4, 0.6), (0.7, 0.9), (0.0, 0.3)];
        let p = GenParams { a: 7.0, b: 1.0, steps_per_beat: 1.0, ..Default::default() };
        let seq: Vec<Vec<(f32, f32)>> = (0..6).map(|beat| lit(&geometry(&pts, &p, &ctx(beat as f64 + 0.5, 120.0)))).collect();
        assert_eq!(seq, vec![vec![pts[0]], vec![pts[1]], vec![pts[2]], vec![pts[0]], vec![pts[1]], vec![pts[2]]]);
        // Every chosen target is visited, dark, so the timing stays.
        assert_eq!(geometry(&pts, &p, &ctx(0.5, 120.0)).strokes.len(), 3);
        // Backwards.
        let back = GenParams { direction: -1, ..p.clone() };
        let seq: Vec<(f32, f32)> = (0..3).map(|beat| lit(&geometry(&pts, &back, &ctx(beat as f64 + 0.5, 120.0)))[0]).collect();
        assert_eq!(seq, vec![pts[0], pts[2], pts[1]]);
    }

    #[test]
    fn a_chase_step_never_lasts_less_than_half_a_strobe_period() {
        let max = 2.0 * crate::safety::DEFAULT_MAX_HZ;
        for bpm in [60.0f32, 128.0, 174.0, 300.0, 0.0, f32::NAN] {
            for steps in [0.25f32, 1.0, 4.0, 16.0, 1000.0, 0.0, f32::NAN] {
                let s = safe_steps_per_beat(steps, bpm);
                let bps = if bpm.is_finite() && bpm > 0.0 { bpm / 60.0 } else { 4.0 * crate::tempo::MAX_BPM as f32 / 60.0 };
                assert!(s > 0.0 && s * bps <= max + 1e-4, "{steps} steps per beat at {bpm} BPM: {s}");
            }
        }
        assert_eq!(safe_steps_per_beat(1.0, 120.0), 1.0, "one per beat stays one per beat");
    }
}
