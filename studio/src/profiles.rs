//! Safety profiles per venue (T-260): a named snapshot of everything that
//! protects the output - zones, horizon, strobe limiter, static beam guard
//! and colour limits (`safety.rs`), the power caps of every output
//! (`power.rs`), the operator presence settings (`presence.rs`), the
//! pre-show checklist (`checklist.rs`) - plus the venue, notes, the date of
//! the last on-site check and whether the venue allows audience scanning
//! (T-255 consults it).
//!
//! Rules (safety-critical):
//! - Loading a profile is a safety change like editing those settings by
//!   hand: whatever it would loosen (a zone removed or moved, a cap raised,
//!   hold-to-run switched off, audience scanning allowed...) is refused
//!   unless the operator confirms (`confirm_loosen`); tighter applies at
//!   once. A refused load changes nothing.
//! - Loading always disarms (`DisarmReason::ProfileChange`) and restarts
//!   the checklist. It never arms.
//! - Only the operator loads a profile: the UI, the keyboard or the local
//!   API. MIDI, OSC, DMX, timecode and the system never can (`Source`).
//! - An unknown or damaged profile is never applied: the laser is disarmed
//!   and the settings in force are kept (they were validated), so a broken
//!   file can never mean « no zones ».
//! - « Aperçu sûr » is built in: never modified, never deleted. A fresh
//!   install starts on it (power 20 %, horizon 0, no audience scanning).
//!   An existing install keeps its settings unchanged, saved as the
//!   profile « Réglages actuels ».
//! - The live settings stay in their own files (`safety.json`,
//!   `outputs.json`...): a profile is a snapshot of them. Show mode
//!   refuses every profile route (423, `showmode::allowed`).
//! - A software profile never makes a show safe: the hardware stop and the
//!   on-site check remain the operator's job.

use crate::checklist::ChecklistConfig;
use crate::interlock::{ArmSource, DisarmReason};
use crate::power::{OutputLimits, MAIN_OUTPUT};
use crate::presence::PresenceSettings;
use crate::safety::SafetySettings;
use crate::Shared;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The built-in profile: cannot be modified nor deleted.
pub const SAFE_PREVIEW: &str = "Aperçu sûr";
/// Its power cap on every output it names.
pub const SAFE_PREVIEW_POWER: f32 = 0.2;
/// What an existing install's settings are saved as, unchanged.
pub const CURRENT: &str = "Réglages actuels";
/// An on-site check older than this is flagged.
pub const VERIFY_MAX_DAYS: i64 = 30;
pub const MAX_NAME: usize = 60;
pub const MAX_TEXT: usize = 2_000;
pub const MAX_PROFILES: usize = 100;

/// One profile, as saved in `<data-dir>/safety-profiles/<name>.json`.
/// Missing parts load as « keep what is in force » (caps, presence,
/// checklist) or as the strictest settings (zones and limiter), so an old
/// or minimal file loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyProfile {
    pub name: String,
    pub venue: String,
    pub notes: String,
    /// Last on-site check, `AAAA-MM-JJ`.
    pub verified_on: Option<String>,
    /// The venue allows audience scanning (T-255): false by default.
    pub audience_scan_allowed: bool,
    pub safety: SafetySettings,
    /// Power caps per output id (T-254). An output missing here keeps its caps.
    pub limits: BTreeMap<String, OutputLimits>,
    pub presence: Option<PresenceSettings>,
    pub checklist: Option<ChecklistConfig>,
}

impl Default for SafetyProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            venue: String::new(),
            notes: String::new(),
            verified_on: None,
            audience_scan_allowed: false,
            safety: SafetySettings::strictest(),
            limits: BTreeMap::new(),
            presence: None,
            checklist: None,
        }
    }
}

/// « Aperçu sûr »: power 20 %, horizon at 0, strict guard, no audience
/// scanning.
pub fn safe_preview() -> SafetyProfile {
    let mut safety = SafetySettings::strictest();
    safety.horizon.y = 0.0;
    SafetyProfile {
        name: SAFE_PREVIEW.into(),
        notes: "Profil intégré pour l'aperçu et les essais : puissance 20 %, horizon au centre, balayage public interdit.".into(),
        safety,
        limits: BTreeMap::from([(MAIN_OUTPUT.to_string(), OutputLimits { max_power: SAFE_PREVIEW_POWER, max_color: [1.0; 3] })]),
        ..Default::default()
    }
}

pub fn is_builtin(name: &str) -> bool {
    name.trim().to_lowercase() == SAFE_PREVIEW.to_lowercase()
}

impl SafetyProfile {
    /// Refuses a profile that could not be applied as is (French messages).
    pub fn validate(&self) -> Result<()> {
        check_name(&self.name)?;
        if self.venue.chars().count() > MAX_NAME * 2 || self.notes.chars().count() > MAX_TEXT {
            bail!("Lieu ou notes trop longs");
        }
        if let Some(d) = &self.verified_on {
            if crate::safety_log::parse_day(d).is_none() {
                bail!("Date de vérification invalide : « {d} »");
            }
        }
        self.safety.validate()?;
        for (id, l) in &self.limits {
            l.validate().map_err(|e| anyhow::anyhow!("Sortie « {id} » : {e}"))?;
        }
        if let Some(c) = &self.checklist {
            crate::checklist::normalise(c.clone())?;
        }
        Ok(())
    }

    /// Days since the last on-site check (None: never checked).
    pub fn days_since_check(&self, today: jiff::civil::Date) -> Option<i64> {
        let d = crate::safety_log::parse_day(self.verified_on.as_deref()?)?;
        Some((today - d).get_days() as i64)
    }

    /// Never checked, or more than `VERIFY_MAX_DAYS` ago (not for the
    /// built-in profile, which belongs to no venue).
    pub fn check_stale(&self, today: jiff::civil::Date) -> bool {
        !is_builtin(&self.name) && self.days_since_check(today).is_none_or(|d| d > VERIFY_MAX_DAYS)
    }
}

pub(crate) fn check_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        bail!("Nom de profil vide");
    }
    if n.chars().count() > MAX_NAME {
        bail!("Nom de profil trop long ({MAX_NAME} caractères au plus)");
    }
    if n.chars().any(char::is_control) {
        bail!("Nom de profil invalide");
    }
    Ok(())
}

/// The file name of a profile: letters and digits, the rest as dashes.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        "profil".into()
    } else {
        s
    }
}

/// Why a profile could not be read.
#[derive(Debug, PartialEq)]
pub enum ReadError {
    Unknown,
    Damaged(String),
}

/// The list entry of a profile (`GET /api/safety/profiles`).
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub venue: String,
    pub notes: String,
    pub verified_on: Option<String>,
    pub audience_scan_allowed: bool,
    pub builtin: bool,
    /// Never checked on site, or more than 30 days ago.
    pub stale: bool,
    /// The file is unreadable or invalid: it cannot be loaded.
    pub damaged: Option<String>,
}

/// `<data-dir>/safety-profiles/` and the active profile's name
/// (`<data-dir>/safety_profile.json`).
pub struct ProfileStore {
    /// None: in memory only (tests).
    dir: Option<PathBuf>,
    active_path: Option<PathBuf>,
    mem: BTreeMap<String, SafetyProfile>,
    /// The profile in force (a copy, kept for the status bar).
    active: Option<SafetyProfile>,
    /// The remembered profile could not be read at start-up.
    load_error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ActiveFile {
    active: Option<String>,
}

impl ProfileStore {
    /// Reads the remembered active profile. An unknown or damaged one is
    /// reported and no profile is active: the settings in force (from
    /// their own files) are kept, never replaced by defaults.
    pub fn load(data_dir: &Path) -> Self {
        let mut store = Self {
            dir: Some(data_dir.join("safety-profiles")),
            active_path: Some(data_dir.join("safety_profile.json")),
            mem: BTreeMap::new(),
            active: None,
            load_error: None,
        };
        let remembered: Option<ActiveFile> = store.active_path.as_ref().and_then(|p| std::fs::read(p).ok()).map(|b| serde_json::from_slice(&b).unwrap_or_default());
        if let Some(name) = remembered.and_then(|a| a.active) {
            match store.read(&name) {
                Ok(p) => store.active = Some(p),
                Err(e) => {
                    let why = match e {
                        ReadError::Unknown => "introuvable".to_string(),
                        ReadError::Damaged(m) => format!("abîmé ({m})"),
                    };
                    store.load_error = Some(format!("Profil de sécurité « {name} » {why} : réglages de sécurité en vigueur conservés, aucun profil actif."));
                }
            }
        }
        store
    }

    /// Whether `safety_profile.json` exists (the profiles were set up).
    pub fn initialised(data_dir: &Path) -> bool {
        data_dir.join("safety_profile.json").exists()
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { dir: None, active_path: None, mem: BTreeMap::new(), active: None, load_error: None }
    }

    /// Puts a profile in memory as is (unvalidated: tests of damaged ones).
    #[cfg(test)]
    pub fn insert_for_tests(&mut self, p: SafetyProfile) {
        self.mem.insert(p.name.trim().to_string(), p);
    }

    pub fn active(&self) -> Option<&SafetyProfile> {
        self.active.as_ref()
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// The venue in force allows audience scanning (T-255). No profile
    /// active: not allowed.
    pub fn audience_allowed(&self) -> bool {
        self.active.as_ref().is_some_and(|p| p.audience_scan_allowed)
    }

    fn path_of(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{}.json", slug(name))))
    }

    /// A profile by name, validated. The built-in one never fails.
    pub fn read(&self, name: &str) -> std::result::Result<SafetyProfile, ReadError> {
        if is_builtin(name) {
            return Ok(safe_preview());
        }
        let profile = match self.path_of(name) {
            None => self.mem.get(name.trim()).cloned().ok_or(ReadError::Unknown)?,
            Some(path) => {
                let bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadError::Unknown),
                    Err(e) => return Err(ReadError::Damaged(format!("lecture impossible : {e}"))),
                };
                serde_json::from_slice::<SafetyProfile>(&bytes).map_err(|e| ReadError::Damaged(format!("fichier illisible : {e}")))?
            }
        };
        if profile.name.trim() != name.trim() {
            return Err(ReadError::Unknown);
        }
        profile.validate().map_err(|e| ReadError::Damaged(e.to_string()))?;
        Ok(profile)
    }

    /// Every profile, the built-in one first.
    pub fn list(&self, today: jiff::civil::Date) -> Vec<Entry> {
        let entry = |p: &SafetyProfile, damaged: Option<String>| Entry {
            name: p.name.clone(),
            venue: p.venue.clone(),
            notes: p.notes.clone(),
            verified_on: p.verified_on.clone(),
            audience_scan_allowed: p.audience_scan_allowed,
            builtin: is_builtin(&p.name),
            stale: p.check_stale(today),
            damaged,
        };
        let mut out = vec![entry(&safe_preview(), None)];
        let mut rest: Vec<Entry> = match &self.dir {
            None => self.mem.values().map(|p| entry(p, p.validate().err().map(|e| e.to_string()))).collect(),
            Some(dir) => {
                let mut v = Vec::new();
                for f in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                    let path = f.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<SafetyProfile>(&b).ok()) {
                        Some(p) if slug(&p.name) == stem => {
                            let damaged = p.validate().err().map(|e| e.to_string());
                            v.push(entry(&p, damaged));
                        }
                        _ => v.push(Entry {
                            name: stem.clone(),
                            venue: String::new(),
                            notes: String::new(),
                            verified_on: None,
                            audience_scan_allowed: false,
                            builtin: false,
                            stale: true,
                            damaged: Some(format!("fichier {stem}.json illisible")),
                        }),
                    }
                }
                v
            }
        };
        rest.retain(|e| !is_builtin(&e.name));
        rest.sort_by_key(|e| e.name.to_lowercase());
        out.extend(rest);
        out
    }

    fn count(&self) -> usize {
        match &self.dir {
            None => self.mem.len(),
            Some(dir) => std::fs::read_dir(dir).map(|r| r.flatten().filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("json")).count()).unwrap_or(0),
        }
    }

    /// Saves a user profile (never the built-in one).
    pub fn write(&mut self, profile: &SafetyProfile) -> Result<()> {
        if is_builtin(&profile.name) {
            bail!("« {SAFE_PREVIEW} » est intégré : il ne peut être ni modifié ni supprimé");
        }
        profile.validate()?;
        let exists = self.read(&profile.name).is_ok();
        match self.path_of(&profile.name) {
            None => {
                if !exists && self.mem.len() >= MAX_PROFILES {
                    bail!("{MAX_PROFILES} profils au plus");
                }
                self.mem.insert(profile.name.trim().to_string(), profile.clone());
            }
            Some(path) => {
                if path.exists() && !exists {
                    // Another (or a damaged) profile uses this file name.
                    let other = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<SafetyProfile>(&b).ok()).map(|p| p.name);
                    if other.as_deref().is_some_and(|o| o.trim() != profile.name.trim()) {
                        bail!("Nom trop proche du profil « {} » : choisissez-en un autre", other.unwrap_or_default());
                    }
                }
                if !path.exists() && self.count() >= MAX_PROFILES {
                    bail!("{MAX_PROFILES} profils au plus");
                }
                crate::datafile::replace(&path, &serde_json::to_vec_pretty(profile)?)?;
            }
        }
        if self.active.as_ref().is_some_and(|a| a.name == profile.name) {
            self.active = Some(profile.clone());
        }
        Ok(())
    }

    /// Deletes a user profile. Deleting the active one leaves no profile
    /// active (the settings in force stay as they are).
    pub fn remove(&mut self, name: &str) -> Result<()> {
        if is_builtin(name) {
            bail!("« {SAFE_PREVIEW} » est intégré : il ne peut être ni modifié ni supprimé");
        }
        match self.path_of(name) {
            None => {
                if self.mem.remove(name.trim()).is_none() {
                    bail!("Profil inconnu : « {name} »");
                }
            }
            Some(path) => {
                if !path.exists() {
                    bail!("Profil inconnu : « {name} »");
                }
                std::fs::remove_file(&path).map_err(|e| anyhow::anyhow!("suppression impossible : {e}"))?;
            }
        }
        if self.active.as_ref().is_some_and(|a| a.name.trim() == name.trim()) {
            self.set_active(None)?;
        }
        Ok(())
    }

    /// Remembers the profile in force (restored at start-up, disarmed).
    pub fn set_active(&mut self, profile: Option<SafetyProfile>) -> Result<()> {
        if let Some(path) = &self.active_path {
            let f = ActiveFile { active: profile.as_ref().map(|p| p.name.clone()) };
            crate::datafile::replace(path, &serde_json::to_vec_pretty(&f)?)?;
        }
        self.active = profile;
        self.load_error = None;
        Ok(())
    }
}

// ---------- what a profile changes ----------

/// The settings in force, as a profile named `name` (no metadata).
pub fn snapshot(s: &Shared, name: &str) -> SafetyProfile {
    SafetyProfile {
        name: name.trim().to_string(),
        audience_scan_allowed: s.profiles.audience_allowed(),
        safety: s.safety.get(),
        limits: s.outputs.list().iter().map(|o| (o.id.clone(), o.limits)).collect(),
        presence: Some(s.presence.settings.clone()),
        checklist: Some(s.checklist.config().clone()),
        ..Default::default()
    }
}

/// Whether the settings in force still match the active profile.
pub fn modified(s: &Shared) -> bool {
    let Some(p) = s.profiles.active() else { return false };
    p.safety != s.safety.get()
        || p.limits.iter().any(|(id, l)| s.outputs.get(id).is_some_and(|o| o.limits != *l))
        || p.presence.as_ref().is_some_and(|ps| {
            let mut ps = ps.clone();
            ps.sanitize();
            ps != s.presence.settings
        })
        || p.checklist.as_ref().is_some_and(|c| crate::checklist::normalise(c.clone()).ok().as_ref() != Some(s.checklist.config()))
}

/// What loading `p` would loosen compared to what is in force, in French.
/// Empty: only tighter or equal, applied without confirmation.
pub fn loosenings(s: &Shared, p: &SafetyProfile) -> Vec<String> {
    let mut out = s.safety.get().loosenings(&p.safety);
    for (id, l) in &p.limits {
        if let Some(o) = s.outputs.get(id) {
            let prefix = if id == MAIN_OUTPUT { String::new() } else { format!("{id} : ") };
            out.extend(o.limits.raises(l).into_iter().map(|r| format!("{prefix}{r}")));
        }
    }
    if let Some(np) = &p.presence {
        let mut np = np.clone();
        np.sanitize();
        let cur = &s.presence.settings;
        if cur.hold_to_run && !np.hold_to_run {
            out.push("Mode maintien (homme mort) désactivé".into());
        }
        if np.ui_timeout_ms > cur.ui_timeout_ms {
            out.push(format!("Coupure si l'interface ne répond plus : {} → {} ms", cur.ui_timeout_ms, np.ui_timeout_ms));
        }
        if np.hold_release_disarm_s > cur.hold_release_disarm_s {
            out.push(format!("Désarmement après relâchement : {} → {} s", cur.hold_release_disarm_s, np.hold_release_disarm_s));
        }
    }
    if let Some(nc) = &p.checklist {
        let cur = s.checklist.config();
        for item in &cur.items {
            if !nc.items.iter().any(|i| i.id == item.id) {
                out.push(format!("Liste de contrôle : « {} » retiré", item.text_fr));
            }
        }
        if cur.outdoor && !nc.outdoor {
            out.push("Liste de contrôle : points « extérieur » retirés".into());
        }
        if cur.audience_scan && !nc.audience_scan {
            out.push("Liste de contrôle : points « balayage du public » retirés".into());
        }
    }
    if p.audience_scan_allowed && !s.profiles.audience_allowed() {
        out.push("Balayage public autorisé dans ce lieu".into());
    }
    out
}

/// Why a profile was not loaded.
#[derive(Debug)]
pub enum LoadError {
    /// Not from the operator (MIDI, OSC, DMX, timecode, system): refused,
    /// nothing changed.
    Source,
    /// Unknown: disarmed, the settings in force kept.
    Unknown(String),
    /// Damaged or invalid: disarmed, the settings in force kept.
    Damaged(String),
    /// Looser than what is in force, not confirmed: nothing changed.
    Loosens(Vec<String>),
    /// Applying failed part-way (a file could not be written): disarmed.
    Failed(String),
}

impl LoadError {
    pub fn message_fr(&self) -> String {
        match self {
            LoadError::Source => "Un profil de sécurité se charge depuis l'interface seulement".into(),
            LoadError::Unknown(n) => format!("Profil inconnu : « {n} ». Laser désarmé, réglages en vigueur conservés."),
            LoadError::Damaged(m) => format!("Profil abîmé, non chargé : {m}. Laser désarmé, réglages en vigueur conservés."),
            LoadError::Loosens(_) => "Ce profil assouplit la sécurité : confirmation requise".into(),
            LoadError::Failed(m) => format!("Profil appliqué en partie : {m}. Laser désarmé."),
        }
    }
}

/// Whether `src` may change the safety profile: the operator only.
pub fn operator_source(src: ArmSource) -> bool {
    matches!(src, ArmSource::Ui | ArmSource::Keyboard | ArmSource::Api)
}

/// Loads a profile (`POST /api/safety/profiles/load`): see the module docs.
/// Returns what was applied (for the reply and the safety log).
pub fn load(s: &mut Shared, name: &str, confirm_loosen: bool, src: ArmSource) -> std::result::Result<Value, LoadError> {
    if !operator_source(src) {
        return Err(LoadError::Source);
    }
    let profile = match s.profiles.read(name) {
        Ok(p) => p,
        Err(e) => {
            s.gate.disarm(DisarmReason::ProfileChange, src);
            let err = match e {
                ReadError::Unknown => LoadError::Unknown(name.to_string()),
                ReadError::Damaged(m) => LoadError::Damaged(m),
            };
            s.gate.log().record("safety_profile", Some(src.id()), json!({ "action": "refused", "name": name, "error": err.message_fr() }));
            s.sync_audience();
            return Err(err);
        }
    };
    apply(s, profile, confirm_loosen, src)
}

/// Applies a profile already read and validated (`load`, or the safety
/// part of a site profile, T-289): the same loosening check, disarm,
/// checklist restart and safety log as `load`. Never arms.
pub fn apply(s: &mut Shared, profile: SafetyProfile, confirm_loosen: bool, src: ArmSource) -> std::result::Result<Value, LoadError> {
    if !operator_source(src) {
        return Err(LoadError::Source);
    }
    profile.validate().map_err(|e| LoadError::Damaged(e.to_string()))?;
    let loosen = loosenings(s, &profile);
    if !loosen.is_empty() && !confirm_loosen {
        return Err(LoadError::Loosens(loosen));
    }
    s.gate.disarm(DisarmReason::ProfileChange, src);
    let mut errors = Vec::new();
    if let Err(e) = s.safety.set(profile.safety.clone(), true) {
        errors.push(match e {
            crate::safety::SetError::Invalid(m) => m,
            crate::safety::SetError::Loosens(l) => l.join("; "),
        });
    }
    let mut skipped = Vec::new();
    for (id, l) in &profile.limits {
        if s.outputs.get(id).is_none() {
            skipped.push(id.clone());
            continue;
        }
        if let Err(e) = s.outputs.set(id, Some(*l), None, false, true) {
            errors.push(format!("{id} : {e:?}"));
        }
    }
    if let Some(p) = &profile.presence {
        s.presence.set_settings(p.clone());
    }
    let armed = s.gate.is_armed();
    match &profile.checklist {
        Some(c) => {
            if let Err(e) = s.checklist.set_config(c.clone(), armed) {
                errors.push(e.to_string());
                s.checklist.restart();
            }
        }
        None => {
            s.checklist.restart();
        }
    }
    let mut applied = profile.clone();
    if !errors.is_empty() {
        // Not all of it is in force: no profile shown as active.
        let _ = s.profiles.set_active(None);
    } else if let Err(e) = s.profiles.set_active(Some(applied.clone())) {
        errors.push(e.to_string());
    }
    applied.notes.clear();
    let detail = json!({
        "action": "load",
        "name": profile.name,
        "venue": profile.venue,
        "confirmed_loosen": !loosen.is_empty(),
        "loosen": loosen,
        "skipped": skipped,
        "errors": errors,
    });
    s.gate.log().record("safety_profile", Some(src.id()), detail.clone());
    if loosen.is_empty() {
        log::info!("safety profile « {} » loaded", profile.name);
    } else {
        log::warn!("safety profile « {} » loaded, loosening confirmed by the operator: {}", profile.name, loosen.join("; "));
    }
    // Audience scanning ends (a disarm the operator did not ask for, and
    // perhaps a venue that forbids it); the interlocks follow.
    s.sync_checklist();
    if !errors.is_empty() {
        return Err(LoadError::Failed(errors.join("; ")));
    }
    Ok(detail)
}

/// « Enregistrer sous… »: the settings in force saved as `name`, which
/// becomes the active profile. Allowing audience scanning in the venue in
/// force is a loosening (409 without `confirm_loosen`).
pub struct SaveRequest {
    pub name: String,
    pub venue: Option<String>,
    pub notes: Option<String>,
    pub audience_scan_allowed: Option<bool>,
}

pub fn save(s: &mut Shared, req: SaveRequest, confirm_loosen: bool, src: ArmSource) -> std::result::Result<SafetyProfile, LoadError> {
    if !operator_source(src) {
        return Err(LoadError::Source);
    }
    if is_builtin(&req.name) {
        return Err(LoadError::Damaged(format!("« {SAFE_PREVIEW} » est intégré : il ne peut être ni modifié ni supprimé")));
    }
    check_name(&req.name).map_err(|e| LoadError::Damaged(e.to_string()))?;
    let old = s.profiles.read(&req.name).ok();
    let mut p = snapshot(s, &req.name);
    p.venue = req.venue.or_else(|| old.as_ref().map(|o| o.venue.clone())).unwrap_or_default().trim().to_string();
    p.notes = req.notes.or_else(|| old.as_ref().map(|o| o.notes.clone())).unwrap_or_default();
    p.audience_scan_allowed = req.audience_scan_allowed.or_else(|| old.as_ref().map(|o| o.audience_scan_allowed)).unwrap_or(false);
    // The on-site check stays valid only for the very same settings.
    p.verified_on = old.as_ref().filter(|o| same_settings(o, &p)).and_then(|o| o.verified_on.clone());
    if p.audience_scan_allowed && !s.profiles.audience_allowed() && !confirm_loosen {
        return Err(LoadError::Loosens(vec!["Balayage public autorisé dans ce lieu".into()]));
    }
    s.profiles.write(&p).map_err(|e| LoadError::Damaged(e.to_string()))?;
    s.profiles.set_active(Some(p.clone())).map_err(|e| LoadError::Failed(e.to_string()))?;
    s.gate.log().record(
        "safety_profile",
        Some(src.id()),
        json!({ "action": "save", "name": p.name, "venue": p.venue, "audience_scan_allowed": p.audience_scan_allowed }),
    );
    // Forbidding audience scanning ends it (T-255).
    s.sync_audience();
    Ok(p)
}

fn same_settings(a: &SafetyProfile, b: &SafetyProfile) -> bool {
    a.safety == b.safety && a.limits == b.limits && a.presence == b.presence && a.checklist == b.checklist && a.audience_scan_allowed == b.audience_scan_allowed
}

/// « Marquer comme vérifié aujourd'hui ».
pub fn verify(s: &mut Shared, name: &str, today: jiff::civil::Date) -> Result<SafetyProfile> {
    if is_builtin(name) {
        bail!("« {SAFE_PREVIEW} » est intégré : il ne peut être ni modifié ni supprimé");
    }
    let mut p = s.profiles.read(name).map_err(|e| anyhow::anyhow!("Profil « {name} » : {e:?}"))?;
    p.verified_on = Some(today.to_string());
    s.profiles.write(&p)?;
    s.gate.log().record("safety_profile", Some("ui"), json!({ "action": "verify", "name": p.name, "verified_on": p.verified_on }));
    Ok(p)
}

pub fn delete(s: &mut Shared, name: &str) -> Result<()> {
    s.profiles.remove(name)?;
    s.gate.log().record("safety_profile", Some("ui"), json!({ "action": "delete", "name": name }));
    s.sync_audience();
    Ok(())
}

/// Start-up (after every store is loaded; the gate is disarmed). The
/// remembered profile is only a label: the settings in force come from
/// their own files. First start of this feature: an existing install's
/// settings become « Réglages actuels », unchanged; a fresh install gets
/// « Aperçu sûr » (only tighter than the defaults).
pub fn startup(s: &mut Shared, data_dir: &Path, fresh_install: bool) {
    if ProfileStore::initialised(data_dir) {
        return;
    }
    let result = if fresh_install {
        let p = safe_preview();
        // Tighter than the defaults everywhere: no confirmation needed.
        let _ = s.safety.set(p.safety.clone(), false);
        for (id, l) in &p.limits {
            let _ = s.outputs.set(id, Some(*l), None, false, false);
        }
        s.profiles.set_active(Some(p))
    } else {
        let p = snapshot(s, CURRENT);
        s.profiles.write(&p).and_then(|_| s.profiles.set_active(Some(p)))
    };
    if let Err(e) = result {
        log::warn!("safety profiles not initialised: {e:#}");
    }
}

/// `GET /api/safety/profiles`.
pub fn view(s: &Shared) -> Value {
    let today = crate::safety_log::today();
    let active = s.profiles.active();
    json!({
        "active": active.map(|p| &p.name),
        "active_profile": active.map(|p| json!({
            "name": p.name, "venue": p.venue, "notes": p.notes, "verified_on": p.verified_on,
            "audience_scan_allowed": p.audience_scan_allowed, "builtin": is_builtin(&p.name),
            "stale": p.check_stale(today), "days_since_check": p.days_since_check(today),
        })),
        "modified": modified(s),
        "load_error": s.profiles.load_error(),
        "profiles": s.profiles.list(today),
        "builtin": SAFE_PREVIEW,
        "verify_max_days": VERIFY_MAX_DAYS,
    })
}

/// The status bar's summary (in `/api/state`).
pub fn summary(s: &Shared) -> Value {
    let today = crate::safety_log::today();
    match s.profiles.active() {
        Some(p) => json!({ "name": p.name, "modified": modified(s), "stale": p.check_stale(today) }),
        None => json!({ "name": null, "modified": false, "stale": false }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::ArmSource;
    use crate::patterns::Point;
    use crate::safety_invariants::{random_limits, random_safety, Rng};
    use crate::test_support;
    use crate::zones::{Zone, ZoneKind};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-profiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn shared() -> Shared {
        test_support::shared()
    }

    fn arm(s: &mut Shared) {
        s.request_arm(ArmSource::Ui).expect("test state arms");
        assert!(s.gate.is_armed());
    }

    fn zone(id: u32) -> Zone {
        Zone { id, name: format!("Z{id}"), kind: ZoneKind::Blank, level: 0.0, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, -0.5]], ..Default::default() }
    }

    fn save_as(s: &mut Shared, name: &str) -> SafetyProfile {
        save(s, SaveRequest { name: name.into(), venue: Some("Club".into()), notes: None, audience_scan_allowed: None }, false, ArmSource::Ui).expect("saved")
    }

    #[test]
    fn a_profile_round_trips_and_a_minimal_one_loads_with_defaults() {
        let s = shared();
        let p = SafetyProfile { venue: "Salle".into(), verified_on: Some("2026-10-01".into()), ..snapshot(&s, "Salle A") };
        let back: SafetyProfile = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        // Older or minimal files: the parts of future tasks are absent.
        let min: SafetyProfile = serde_json::from_str(r#"{"name":"Vieux"}"#).unwrap();
        assert_eq!(min.safety, SafetySettings::strictest());
        assert!(min.limits.is_empty() && min.presence.is_none() && min.checklist.is_none() && !min.audience_scan_allowed);
        min.validate().unwrap();
        // Unknown fields (a newer version) are ignored.
        let newer: SafetyProfile = serde_json::from_str(r#"{"name":"N","outdoor":{"x":1}}"#).unwrap();
        assert_eq!(newer.name, "N");
    }

    #[test]
    fn loading_while_armed_disarms_with_the_profile_change_reason() {
        let mut s = shared();
        save_as(&mut s, "Club");
        arm(&mut s);
        let detail = load(&mut s, "Club", false, ArmSource::Ui).expect("loads");
        assert_eq!(detail["name"], "Club");
        assert!(!s.gate.is_armed());
        let (reason, _) = s.gate.last_disarm().unwrap();
        assert_eq!(*reason, DisarmReason::ProfileChange);
        assert_eq!(reason.label_fr(), "Changement de profil");
        assert_eq!(s.profiles.active().unwrap().name, "Club");
    }

    #[test]
    fn loading_restarts_the_checklist() {
        let mut s = shared();
        save_as(&mut s, "Club");
        s.checklist.set_operator("Michel").unwrap();
        let ids: Vec<String> = s.checklist.visible_items().map(|i| i.id.clone()).collect();
        for id in ids {
            s.checklist.check(&id, true, Some("1")).unwrap();
        }
        s.checklist.complete().unwrap();
        assert!(s.checklist.is_complete());
        load(&mut s, "Club", false, ArmSource::Ui).unwrap();
        assert!(!s.checklist.is_complete());
    }

    #[test]
    fn a_looser_profile_needs_a_confirmation_and_a_tighter_one_does_not() {
        let mut s = shared();
        save_as(&mut s, "Sans zone");
        // Tighter: a zone added, the cap lowered.
        let mut cfg = s.safety.get();
        cfg.zones.push(zone(0));
        s.safety.set(cfg, false).unwrap();
        s.outputs.set(MAIN_OUTPUT, Some(OutputLimits { max_power: 0.3, max_color: [1.0; 3] }), None, false, false).unwrap();
        save_as(&mut s, "Avec zone");
        // Back to « Sans zone »: zone removed and cap raised, refused.
        let before = (s.safety.get(), s.outputs.active_limits());
        arm(&mut s);
        match load(&mut s, "Sans zone", false, ArmSource::Ui) {
            Err(LoadError::Loosens(l)) => {
                assert!(l.iter().any(|m| m.contains("supprimée")), "{l:?}");
                assert!(l.iter().any(|m| m.contains("Puissance max")), "{l:?}");
            }
            other => panic!("expected a loosening refusal, got {other:?}"),
        }
        assert_eq!((s.safety.get(), s.outputs.active_limits()), before, "a refused load changes nothing");
        assert_eq!(s.profiles.active().unwrap().name, "Avec zone");
        // Confirmed: applied, disarmed.
        load(&mut s, "Sans zone", true, ArmSource::Ui).unwrap();
        assert!(s.safety.get().zones.is_empty());
        assert_eq!(s.outputs.active_limits().max_power, 0.5);
        assert!(!s.gate.is_armed());
        // And back to the tighter one without confirmation.
        load(&mut s, "Avec zone", false, ArmSource::Ui).unwrap();
        assert_eq!(s.safety.get().zones.len(), 1);
    }

    #[test]
    fn the_builtin_profile_cannot_be_modified_nor_deleted() {
        let mut s = shared();
        assert!(save(&mut s, SaveRequest { name: SAFE_PREVIEW.into(), venue: None, notes: None, audience_scan_allowed: None }, true, ArmSource::Ui).is_err());
        assert!(save(&mut s, SaveRequest { name: "aperçu SÛR ".into(), venue: None, notes: None, audience_scan_allowed: None }, true, ArmSource::Ui).is_err());
        assert!(delete(&mut s, SAFE_PREVIEW).is_err());
        assert!(verify(&mut s, SAFE_PREVIEW, crate::safety_log::today()).is_err());
        assert!(s.profiles.write(&safe_preview()).is_err());
        let p = s.profiles.read(SAFE_PREVIEW).unwrap();
        assert_eq!(p, safe_preview());
        assert_eq!(p.limits[MAIN_OUTPUT].max_power, SAFE_PREVIEW_POWER);
        assert_eq!(p.safety.horizon.y, 0.0);
        assert!(!p.audience_scan_allowed);
        // From the defaults it only tightens: loads without confirmation.
        load(&mut s, SAFE_PREVIEW, false, ArmSource::Ui).unwrap();
        assert_eq!(s.outputs.active_limits().max_power, SAFE_PREVIEW_POWER);
    }

    #[test]
    fn only_the_operator_switches_profile() {
        let mut s = shared();
        save_as(&mut s, "Club");
        arm(&mut s);
        let before = s.safety.get();
        for src in [ArmSource::Midi, ArmSource::Network, ArmSource::Osc, ArmSource::Dmx, ArmSource::System] {
            assert!(matches!(load(&mut s, SAFE_PREVIEW, true, src), Err(LoadError::Source)));
            assert!(matches!(
                save(&mut s, SaveRequest { name: "X".into(), venue: None, notes: None, audience_scan_allowed: Some(true) }, true, src),
                Err(LoadError::Source)
            ));
        }
        assert!(s.gate.is_armed(), "a refused source changes nothing, not even the arming");
        assert_eq!(s.safety.get(), before);
        assert_eq!(s.profiles.active().unwrap().name, "Club");
    }

    #[test]
    fn an_unknown_or_damaged_profile_disarms_and_keeps_the_settings_in_force() {
        let mut s = shared();
        let mut cfg = s.safety.get();
        cfg.zones.push(zone(0));
        s.safety.set(cfg, false).unwrap();
        let before = s.safety.get();
        arm(&mut s);
        assert!(matches!(load(&mut s, "Inconnu", true, ArmSource::Ui), Err(LoadError::Unknown(_))));
        assert!(!s.gate.is_armed());
        assert_eq!(s.safety.get(), before, "never « no zones »");
        // A damaged profile (out of range values, written by hand).
        let mut bad = snapshot(&s, "Abîmé");
        bad.safety.horizon.y = 7.0;
        s.profiles.mem.insert("Abîmé".into(), bad);
        arm(&mut s);
        assert!(matches!(load(&mut s, "Abîmé", true, ArmSource::Ui), Err(LoadError::Damaged(_))));
        assert!(!s.gate.is_armed());
        assert_eq!(s.safety.get(), before);
        assert!(s.profiles.list(crate::safety_log::today()).iter().any(|e| e.name == "Abîmé" && e.damaged.is_some()));
    }

    #[test]
    fn files_persist_and_the_active_profile_comes_back_disarmed() {
        let dir = temp_dir("profiles");
        let mut s = shared();
        s.profiles = ProfileStore::load(&dir);
        save_as(&mut s, "Salle des fêtes");
        let today = crate::safety_log::today();
        verify(&mut s, "Salle des fêtes", today).unwrap();
        assert!(dir.join("safety-profiles").join("salle-des-fêtes.json").exists());
        // Restart: the active profile is remembered (the gate of a new
        // state always starts disarmed).
        let back = ProfileStore::load(&dir);
        let p = back.active().expect("remembered");
        assert_eq!(p.name, "Salle des fêtes");
        assert_eq!(p.verified_on.as_deref(), Some(today.to_string().as_str()));
        assert!(!p.check_stale(today));
        assert!(p.check_stale(today + jiff::Span::new().days(31)));
        // A damaged active profile: reported, nothing active.
        std::fs::write(dir.join("safety-profiles").join("salle-des-fêtes.json"), b"{oops").unwrap();
        let broken = ProfileStore::load(&dir);
        assert!(broken.active().is_none());
        assert!(broken.load_error().unwrap().contains("abîmé"));
        // Deleting the active profile leaves none active.
        let mut s2 = shared();
        s2.profiles = ProfileStore::load(&dir);
        save_as(&mut s2, "Autre");
        delete(&mut s2, "Autre").unwrap();
        assert!(s2.profiles.active().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_start_keeps_existing_settings_and_gives_a_fresh_install_the_safe_preview() {
        // Existing install: unchanged, saved as « Réglages actuels ».
        let dir = temp_dir("profiles-existing");
        let mut s = shared();
        s.profiles = ProfileStore::load(&dir);
        let mut cfg = s.safety.get();
        cfg.zones.push(zone(0));
        s.safety.set(cfg, false).unwrap();
        let (safety, caps) = (s.safety.get(), s.outputs.active_limits());
        startup(&mut s, &dir, false);
        assert_eq!((s.safety.get(), s.outputs.active_limits()), (safety, caps));
        assert_eq!(s.profiles.active().unwrap().name, CURRENT);
        assert!(!modified(&s));
        // Fresh install: « Aperçu sûr ».
        let fresh = temp_dir("profiles-fresh");
        let mut s = shared();
        s.profiles = ProfileStore::load(&fresh);
        startup(&mut s, &fresh, true);
        assert_eq!(s.profiles.active().unwrap().name, SAFE_PREVIEW);
        assert_eq!(s.outputs.active_limits().max_power, SAFE_PREVIEW_POWER);
        assert!(!s.gate.is_armed());
        // Second start: nothing changes any more.
        s.outputs.set(MAIN_OUTPUT, Some(OutputLimits { max_power: 0.1, max_color: [1.0; 3] }), None, false, false).unwrap();
        startup(&mut s, &fresh, true);
        assert_eq!(s.outputs.active_limits().max_power, 0.1);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh);
    }

    #[test]
    fn allowing_audience_scanning_is_a_confirmed_loosening_and_unlocking_consults_it() {
        let mut s = shared();
        save_as(&mut s, "Club");
        let req = || SaveRequest { name: "Club".into(), venue: None, notes: None, audience_scan_allowed: Some(true) };
        assert!(matches!(save(&mut s, req(), false, ArmSource::Ui), Err(LoadError::Loosens(_))));
        assert!(!s.profiles.audience_allowed());
        save(&mut s, req(), true, ArmSource::Ui).unwrap();
        assert!(s.profiles.audience_allowed());
        let conds = s.audience_conditions();
        assert!(conds.iter().any(|c| c.id == "venue_profile" && c.ok));
        // Back to a venue that forbids it: the condition fails.
        load(&mut s, SAFE_PREVIEW, true, ArmSource::Ui).unwrap();
        assert!(s.audience_conditions().iter().any(|c| c.id == "venue_profile" && !c.ok));
    }

    #[test]
    fn presence_and_checklist_loosenings_are_listed() {
        let mut s = shared();
        s.presence.settings.hold_to_run = true;
        let mut p = snapshot(&s, "P");
        p.presence.as_mut().unwrap().hold_to_run = false;
        p.presence.as_mut().unwrap().ui_timeout_ms = 9_000;
        p.checklist.as_mut().unwrap().items.remove(0);
        let l = loosenings(&s, &p);
        assert!(l.iter().any(|m| m.contains("maintien")), "{l:?}");
        assert!(l.iter().any(|m| m.contains("ne répond plus")), "{l:?}");
        assert!(l.iter().any(|m| m.contains("retiré")), "{l:?}");
        assert!(loosenings(&s, &snapshot(&s, "Q")).is_empty());
    }

    // ---------- property: never brighter until confirmed ----------

    /// The brightest drive of each colour a lone lit point at (x, y) can
    /// leave with: zones, horizon, colour limits, then the caps.
    fn drive(cfg: &SafetySettings, caps: &OutputLimits, x: f32, y: f32, c: [f32; 3]) -> [f32; 3] {
        let (mut out, _) = cfg.mask().apply(&[Point::lit(x, y, c[0], c[1], c[2])]);
        crate::power::cap(&mut out, caps);
        let mut m = [0.0f32; 3];
        for p in &out {
            m[0] = m[0].max(p.r);
            m[1] = m[1].max(p.g);
            m[2] = m[2].max(p.b);
        }
        m
    }

    /// A profile tighter than `cur` in random ways (zones added, cap and
    /// gains lowered, horizon raised...), so both branches get exercised.
    fn tightened(cur: &SafetySettings, caps: &OutputLimits, rng: &mut Rng) -> (SafetySettings, OutputLimits) {
        let mut n = cur.clone();
        let mut c = *caps;
        if rng.chance(0.5) {
            let extra = random_safety(rng);
            let next = n.zones.iter().map(|z| z.id).max().unwrap_or(0);
            for (i, mut z) in extra.zones.into_iter().enumerate() {
                if n.zones.len() < crate::zones::MAX_ZONES {
                    z.id = next + 1 + i as u32;
                    n.zones.push(z);
                }
            }
        }
        if rng.chance(0.5) {
            n.horizon.y = rng.range(n.horizon.y, 1.0);
        }
        if rng.chance(0.4) {
            for g in &mut n.color_gain {
                *g *= rng.f();
            }
        }
        if rng.chance(0.5) {
            c.max_power *= rng.f();
        }
        if rng.chance(0.3) {
            for z in &mut n.zones {
                if z.kind == ZoneKind::Dim && rng.chance(0.5) {
                    z.level *= rng.f();
                } else if z.kind == ZoneKind::Dim && rng.chance(0.5) {
                    // Dimmed → masked: darker inside, but its soft edge goes.
                    z.kind = ZoneKind::Blank;
                }
            }
        }
        (n, c)
    }

    /// Applying any profile never makes the output brighter than what is
    /// in force until the operator confirms: a refused load changes
    /// nothing, and one applied without confirmation is never brighter,
    /// anywhere, than either the settings it replaced or its own (so,
    /// never brighter than the darker of the two). Seeded; `SAFETY_CASES`
    /// runs more.
    #[test]
    fn applying_any_profile_never_brightens_until_confirmed() {
        let cases = std::env::var("SAFETY_CASES").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(150);
        let mut applied = 0;
        let mut refused = 0;
        for seed in 0..cases as u64 {
            let mut rng = Rng::new(seed ^ 0x0260_0260);
            let mut s = shared();
            let cur = random_safety(&mut rng);
            let cur_caps = random_limits(&mut rng);
            s.safety.set(cur, true).unwrap();
            s.outputs.set(MAIN_OUTPUT, Some(cur_caps), None, false, true).unwrap();
            let cur = s.safety.get();
            let (new, new_caps) = if rng.chance(0.5) { tightened(&cur, &cur_caps, &mut rng) } else { (random_safety(&mut rng), random_limits(&mut rng)) };
            let mut p = snapshot(&s, "Fuzz");
            p.safety = new.clone();
            p.limits.insert(MAIN_OUTPUT.into(), new_caps);
            if rng.chance(0.3) {
                p.audience_scan_allowed = true;
            }
            s.profiles.mem.insert("Fuzz".into(), p);
            if rng.chance(0.5) {
                let _ = s.request_arm(ArmSource::Ui);
            }
            let r = load(&mut s, "Fuzz", false, ArmSource::Ui);
            let (after, after_caps) = (s.safety.get(), s.outputs.active_limits());
            match &r {
                Ok(_) => {
                    applied += 1;
                    assert!(!s.gate.is_armed(), "seed {seed}: a load leaves the laser disarmed");
                }
                Err(LoadError::Loosens(_)) => {
                    refused += 1;
                    assert_eq!((&after, &after_caps), (&cur, &cur_caps), "seed {seed}: a refused load changed the settings");
                }
                Err(e) => panic!("seed {seed}: unexpected {e:?}"),
            }
            for _ in 0..200 {
                let (x, y) = (rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
                let c = [rng.f(), rng.f(), rng.f()];
                let before = drive(&cur, &cur_caps, x, y, c);
                let now = drive(&after, &after_caps, x, y, c);
                let theirs = drive(&new, &new_caps, x, y, c);
                for ch in 0..3 {
                    assert!(now[ch] <= before[ch] + 1e-4, "seed {seed}: ({x}, {y}) channel {ch} brighter than before: {} > {} ({r:?})", now[ch], before[ch]);
                    if r.is_ok() {
                        assert!(now[ch] <= theirs[ch] + 1e-4, "seed {seed}: applied but not the profile's light");
                    }
                }
            }
        }
        assert!(applied > cases / 10 && refused > cases / 10, "both branches exercised: {applied} applied, {refused} refused");
    }
}
