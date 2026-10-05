//! Audience scanning mode (T-255), locked by default.
//!
//! A show beam of a few watts is hundreds to thousands of times over the
//! maximum permissible exposure (docs/research/safety-regulation.md §2.2):
//! scanning the audience is only possible with attenuation, divergence and
//! a **hardware** scan-fail system, assessed by a qualified person. The
//! software cannot make it safe; it can only keep the beams off the
//! audience by default and make the other mode explicit, hard to reach and
//! temporary.
//!
//! - **« Au-dessus du public »** (`AboveOnly`, the default and the mode
//!   after every start): every « Public » zone (`Zone::audience`) is
//!   blanked whatever its kind, so nothing lit enters it. With a real
//!   output (the same rule as the pre-show checklist), the `audience_zone`
//!   interlock refuses arming until a Public zone or a full horizon at 0 %
//!   exists (`protected`).
//! - **« Balayage public »** (`Scanning`): Public zones keep their own kind
//!   and level. Reached only by `AudienceState::unlock`, disarmed, with
//!   every condition of `conditions` met and the typed `PHRASE`. While it
//!   lasts the static beam guard runs on its strict profile (`effective`).
//!
//! The mode lives in the session only (never in a settings file), so a
//! restart is always « Au-dessus du public ». An emergency stop, any
//! disarm the operator did not ask for, or a condition that stops being
//! met (zone removed, checklist restarted, guard loosened...) ends it
//! (`Shared::sync_audience`). Ending it is tightening: always allowed,
//! even armed.

use crate::checklist::Checklist;
use crate::dwell::DwellProfile;
use crate::power::ProjectorInfo;
use crate::safety::SafetySettings;
use crate::zones::ZoneKind;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// What the operator types to unlock audience scanning.
pub const PHRASE: &str = "JE PRENDS LA RESPONSABILITÉ";
/// Highest level of a Dim Public zone: an attenuation of at least 50 %,
/// the lowest allowed « en connaissance de cause ».
pub const MAX_AUDIENCE_LEVEL: f32 = 0.5;
/// The recommended level: an attenuation of at least 90 %.
pub const RECOMMENDED_LEVEL: f32 = 0.1;
/// Label of the `audience_zone` interlock (also the refusal message).
pub const INTERLOCK_LABEL: &str = "Aucune zone Public ni horizon complet : définissez où est le public";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudienceMode {
    /// Beams stay above the audience: Public zones are blanked.
    #[default]
    AboveOnly,
    /// The operator unlocked audience scanning at `unlocked_at`.
    Scanning { unlocked_at: SystemTime },
}

/// The settings the output stage applies in this mode: outside audience
/// scanning every Public zone is a Blank zone; while scanning, the static
/// beam guard is on its strict profile whatever is saved. Never looser
/// than `cfg`.
pub fn effective(cfg: &SafetySettings, scanning: bool) -> SafetySettings {
    let mut out = cfg.clone();
    if scanning {
        out.dwell.profile = DwellProfile::Strict;
    } else {
        for z in out.zones.iter_mut().filter(|z| z.audience) {
            z.kind = ZoneKind::Blank;
        }
    }
    out
}

/// Where the audience is has been said: a Public zone, or the horizon
/// blanking everything below it. The `audience_zone` interlock's state.
pub fn protected(cfg: &SafetySettings) -> bool {
    cfg.zones.iter().any(|z| z.audience && z.points.len() >= 3) || (cfg.horizon.lines && cfg.horizon.level == 0.0)
}

/// The projector sheet is filled in (name, power, divergence) and declares
/// a hardware scan-fail protection.
pub fn sheet_complete(p: &ProjectorInfo) -> bool {
    !p.name.trim().is_empty() && p.power_mw.iter().any(|&w| w > 0.0) && p.divergence_mrad > 0.0 && p.hw_scan_fail == Some(true)
}

/// One condition of the unlock, for the wizard (French) and the log.
#[derive(Clone, Debug, Serialize)]
pub struct Condition {
    pub id: &'static str,
    pub label_fr: &'static str,
    pub ok: bool,
    /// Where it stands now, or what is missing.
    pub detail_fr: String,
    /// The setting to open in the UI.
    pub link: &'static str,
}

/// What the conditions are checked against.
pub struct Inputs<'a> {
    pub safety: &'a SafetySettings,
    pub projector: &'a ProjectorInfo,
    pub checklist: &'a Checklist,
    pub hold_to_run: bool,
}

/// The conditions to unlock audience scanning, in the order the wizard
/// shows them. The typed phrase is checked by `AudienceState::unlock`.
pub fn conditions(i: &Inputs) -> Vec<Condition> {
    let dim = i.safety.zones.iter().filter(|z| z.audience && z.kind == ZoneKind::Dim && z.points.len() >= 3 && z.level <= MAX_AUDIENCE_LEVEL);
    let best = dim.map(|z| z.level).fold(None, |m: Option<f32>, l| Some(m.map_or(l, |m| m.max(l))));
    let zone_detail = match best {
        None => "Aucune zone « Atténuer » marquée Public (atténuation d'au moins 50 %)".to_string(),
        Some(l) if l > RECOMMENDED_LEVEL => format!("Atténuation {:.0} % : 90 % ou plus recommandé", (1.0 - l) * 100.0),
        Some(l) => format!("Atténuation {:.0} %", (1.0 - l) * 100.0),
    };
    let p = i.projector;
    let sheet_detail = if sheet_complete(p) {
        "Fiche remplie, anti-défaut de balayage matériel : oui".to_string()
    } else {
        let mut missing = Vec::new();
        if p.name.trim().is_empty() {
            missing.push("nom");
        }
        if !p.power_mw.iter().any(|&w| w > 0.0) {
            missing.push("puissance");
        }
        if p.divergence_mrad <= 0.0 {
            missing.push("divergence");
        }
        if p.hw_scan_fail != Some(true) {
            missing.push("anti-défaut de balayage matériel « Oui »");
        }
        format!("À compléter : {}", missing.join(", "))
    };
    let cl = i.checklist;
    let items_done = crate::checklist::AUDIENCE_ITEMS.iter().all(|id| cl.is_done(id));
    let checklist_ok = cl.config().audience_scan && items_done && cl.is_complete();
    let checklist_detail = if !cl.config().audience_scan {
        "Cochez « Balayage du public » dans la liste, puis faites-la".to_string()
    } else if !items_done {
        "Mesure d'irradiance et atténuation matérielle à cocher".to_string()
    } else if !cl.is_complete() {
        "Liste à terminer".to_string()
    } else {
        "Liste terminée".to_string()
    };
    let profile = i.safety.dwell.profile;
    vec![
        Condition { id: "dim_zone", label_fr: "Zone Public atténuée (90 % recommandé, 50 % au moins)", ok: best.is_some(), detail_fr: zone_detail, link: "sfZones" },
        Condition { id: "projector", label_fr: "Fiche projecteur complète, anti-défaut de balayage matériel déclaré", ok: sheet_complete(p), detail_fr: sheet_detail, link: "pjName" },
        Condition {
            id: "checklist",
            label_fr: "Liste « Avant le show » terminée : irradiance mesurée au point le plus proche du public, atténuation ou divergence matérielle",
            ok: checklist_ok,
            detail_fr: checklist_detail,
            link: "checklistPanel",
        },
        Condition {
            id: "hold_to_run",
            label_fr: "Mode maintien (homme mort) activé",
            ok: i.hold_to_run,
            detail_fr: if i.hold_to_run { "Activé".into() } else { "Désactivé".into() },
            link: "holdToRun",
        },
        Condition {
            id: "dwell_strict",
            label_fr: "Garde anti-point fixe en profil strict",
            ok: profile == DwellProfile::Strict,
            detail_fr: if profile == DwellProfile::Strict { "Stricte".into() } else { "À passer en « Stricte »".into() },
            link: "sfDwellProfile",
        },
    ]
}

/// Why `unlock` refused.
#[derive(Debug, PartialEq)]
pub enum UnlockError {
    /// The laser is armed: disarm first.
    Armed,
    /// These conditions are not met (French labels).
    Missing(Vec<String>),
    /// The phrase was not typed (or not exactly).
    Phrase,
}

impl UnlockError {
    pub fn message_fr(&self) -> String {
        match self {
            UnlockError::Armed => "Désarmez le laser avant de déverrouiller le balayage public".into(),
            UnlockError::Missing(_) => "Conditions non remplies pour le balayage public".into(),
            UnlockError::Phrase => format!("Saisissez « {PHRASE} » pour confirmer"),
        }
    }
}

/// The phrase as typed: case and surrounding or repeated spaces aside.
fn phrase_matches(typed: &str) -> bool {
    typed.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase() == PHRASE
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The session's audience mode. In memory only: every start is `AboveOnly`.
#[derive(Debug, Default)]
pub struct AudienceState {
    mode: AudienceMode,
}

impl AudienceState {
    pub fn mode(&self) -> AudienceMode {
        self.mode
    }

    pub fn is_scanning(&self) -> bool {
        matches!(self.mode, AudienceMode::Scanning { .. })
    }

    pub fn unlocked_at(&self) -> Option<SystemTime> {
        match self.mode {
            AudienceMode::Scanning { unlocked_at } => Some(unlocked_at),
            AudienceMode::AboveOnly => None,
        }
    }

    /// Unlocks audience scanning: refused while `armed`, while any of
    /// `conds` is not met, and without the exact phrase, in that order.
    /// Returns the safety log's detail. Never arms anything.
    pub fn unlock(&mut self, conds: &[Condition], phrase: &str, armed: bool) -> Result<Value, UnlockError> {
        if armed {
            return Err(UnlockError::Armed);
        }
        let missing: Vec<String> = conds.iter().filter(|c| !c.ok).map(|c| c.label_fr.to_string()).collect();
        if !missing.is_empty() {
            return Err(UnlockError::Missing(missing));
        }
        if !phrase_matches(phrase) {
            return Err(UnlockError::Phrase);
        }
        self.mode = AudienceMode::Scanning { unlocked_at: SystemTime::now() };
        Ok(json!({
            "mode": "scanning",
            "conditions": conds.iter().map(|c| json!({ "id": c.id, "label": c.label_fr, "ok": c.ok, "detail": c.detail_fr })).collect::<Vec<_>>(),
        }))
    }

    /// Back to « Au-dessus du public ». Returns whether it was scanning.
    pub fn lock(&mut self) -> bool {
        let was = self.is_scanning();
        self.mode = AudienceMode::AboveOnly;
        was
    }

    /// For `/api/state` and `/api/audience`.
    pub fn view(&self) -> Value {
        json!({
            "mode": if self.is_scanning() { "scanning" } else { "above" },
            "scanning": self.is_scanning(),
            "since": self.unlocked_at().map(ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::Zone;

    fn public(kind: ZoneKind, level: f32) -> Zone {
        Zone { id: 1, name: "Public".into(), kind, level, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, -0.4], [-1.0, -0.4]], audience: true, ..Default::default() }
    }

    fn sheet() -> ProjectorInfo {
        ProjectorInfo { name: "Projecteur".into(), power_mw: [500.0, 1000.0, 800.0], divergence_mrad: 1.2, hw_scan_fail: Some(true), ..Default::default() }
    }

    /// A checklist with the audience items shown, ticked and the run complete.
    fn done_checklist() -> Checklist {
        let mut cl = Checklist::in_memory(false);
        let mut config = cl.config().clone();
        config.audience_scan = true;
        cl.set_config(config, false).unwrap();
        cl.set_operator("Op").unwrap();
        let ids: Vec<(String, bool)> = cl.visible_items().map(|i| (i.id.clone(), i.needs_value)).collect();
        for (id, value) in ids {
            cl.check(&id, true, value.then_some("1.2")).unwrap();
        }
        cl.complete().unwrap();
        cl
    }

    fn ready() -> (SafetySettings, ProjectorInfo, Checklist) {
        let mut safety = SafetySettings { zones: vec![public(ZoneKind::Dim, 0.1)], ..Default::default() };
        safety.dwell.profile = DwellProfile::Strict;
        (safety, sheet(), done_checklist())
    }

    fn conds(safety: &SafetySettings, projector: &ProjectorInfo, checklist: &Checklist, hold_to_run: bool) -> Vec<Condition> {
        conditions(&Inputs { safety, projector, checklist, hold_to_run })
    }

    #[test]
    fn starts_above_and_blanks_every_public_zone() {
        let st = AudienceState::default();
        assert_eq!(st.mode(), AudienceMode::AboveOnly);
        let cfg = SafetySettings { zones: vec![public(ZoneKind::Dim, 0.1), Zone { audience: false, ..public(ZoneKind::Dim, 0.3) }], ..Default::default() };
        let above = effective(&cfg, false);
        assert_eq!(above.zones[0].kind, ZoneKind::Blank, "a Dim Public zone is blanked");
        assert_eq!(above.zones[1].kind, ZoneKind::Dim, "other zones untouched");
        let scanning = effective(&cfg, true);
        assert_eq!(scanning.zones[0].kind, ZoneKind::Dim);
        assert_eq!(scanning.dwell.profile, DwellProfile::Strict, "scanning forces the strict guard");
    }

    #[test]
    fn protection_needs_a_public_zone_or_a_full_horizon() {
        let mut cfg = SafetySettings::default();
        assert!(!protected(&cfg), "a fresh install says nothing about the audience");
        cfg.horizon.lines = true;
        assert!(protected(&cfg));
        cfg.horizon.level = 0.2;
        assert!(!protected(&cfg), "a horizon that only dims is not enough");
        cfg.zones.push(public(ZoneKind::Dim, 0.3));
        assert!(protected(&cfg), "a Public zone is blanked outside scanning, Dim or not");
        cfg.zones[0].audience = false;
        assert!(!protected(&cfg));
    }

    #[test]
    fn every_missing_condition_blocks_with_its_label() {
        let (safety, projector, checklist) = ready();
        let all = conds(&safety, &projector, &checklist, true);
        assert!(all.iter().all(|c| c.ok), "{all:?}");
        let fails = |c: Vec<Condition>, id: &str| {
            let bad: Vec<&str> = c.iter().filter(|c| !c.ok).map(|c| c.id).collect();
            assert_eq!(bad, vec![id]);
            let mut st = AudienceState::default();
            match st.unlock(&c, PHRASE, false) {
                Err(UnlockError::Missing(m)) => assert_eq!(m.len(), 1),
                other => panic!("{id}: {other:?}"),
            }
            assert!(!st.is_scanning());
        };
        // 1. The Public zone: missing, Blank, too bright, or not Public.
        fails(conds(&SafetySettings { zones: vec![], ..safety.clone() }, &projector, &checklist, true), "dim_zone");
        fails(conds(&SafetySettings { zones: vec![public(ZoneKind::Blank, 0.1)], ..safety.clone() }, &projector, &checklist, true), "dim_zone");
        fails(conds(&SafetySettings { zones: vec![public(ZoneKind::Dim, 0.6)], ..safety.clone() }, &projector, &checklist, true), "dim_zone");
        fails(conds(&SafetySettings { zones: vec![Zone { audience: false, ..public(ZoneKind::Dim, 0.1) }], ..safety.clone() }, &projector, &checklist, true), "dim_zone");
        // 2. The sheet: no scan-fail declared, or unknown, or incomplete.
        fails(conds(&safety, &ProjectorInfo { hw_scan_fail: Some(false), ..sheet() }, &checklist, true), "projector");
        fails(conds(&safety, &ProjectorInfo { hw_scan_fail: None, ..sheet() }, &checklist, true), "projector");
        fails(conds(&safety, &ProjectorInfo { divergence_mrad: 0.0, ..sheet() }, &checklist, true), "projector");
        // 3. The checklist: not done.
        fails(conds(&safety, &projector, &Checklist::in_memory(false), true), "checklist");
        // 4. Hold-to-run off.
        fails(conds(&safety, &projector, &checklist, false), "hold_to_run");
        // 5. The guard not strict.
        let mut beams = safety.clone();
        beams.dwell.profile = DwellProfile::Beams;
        fails(conds(&beams, &projector, &checklist, true), "dwell_strict");
    }

    #[test]
    fn a_completed_checklist_without_the_audience_items_is_not_enough() {
        let (safety, projector, _) = ready();
        let mut cl = Checklist::in_memory(false);
        cl.set_operator("Op").unwrap();
        let ids: Vec<(String, bool)> = cl.visible_items().map(|i| (i.id.clone(), i.needs_value)).collect();
        for (id, value) in ids {
            cl.check(&id, true, value.then_some("x")).unwrap();
        }
        cl.complete().unwrap();
        let c = conds(&safety, &projector, &cl, true);
        assert!(!c.iter().find(|c| c.id == "checklist").unwrap().ok);
    }

    #[test]
    fn unlock_needs_disarmed_then_the_exact_phrase() {
        let (safety, projector, checklist) = ready();
        let c = conds(&safety, &projector, &checklist, true);
        let mut st = AudienceState::default();
        assert_eq!(st.unlock(&c, PHRASE, true), Err(UnlockError::Armed));
        assert_eq!(st.unlock(&c, "", false), Err(UnlockError::Phrase));
        assert_eq!(st.unlock(&c, "je prends la responsabilite", false), Err(UnlockError::Phrase), "the accent is part of it");
        assert!(!st.is_scanning());
        let detail = st.unlock(&c, "  je prends   la responsabilité ", false).unwrap();
        assert!(st.is_scanning());
        assert_eq!(detail["conditions"].as_array().unwrap().len(), 5, "the log lists every condition");
        assert!(st.unlocked_at().is_some());
        assert_eq!(st.view()["mode"], "scanning");
        assert!(st.lock());
        assert!(!st.lock(), "already above");
        assert_eq!(st.view()["mode"], "above");
    }
}
