//! Site profiles (T-289): what belongs to a venue rather than to a show,
//! kept together so a show prepared at home can be played elsewhere
//! without carrying the home rig's alignment along:
//!
//! - the calibration (`calibration.json`);
//! - the safety settings, as a T-260 safety profile (zones, horizon,
//!   strobe limiter, power caps, presence, checklist, audience scanning);
//! - the projection zones (T-012, `projection.json`);
//! - the output connection (selected output, rate): **noted only**, never
//!   applied by a site, since opening a device is the operator's choice in
//!   « Sorties et projecteurs ».
//!
//! One profile is active at a time (`<data-dir>/site.json`); the profiles
//! are `<data-dir>/sites/<nom>.lssite` (versioned JSON, T-288). On the
//! first start with this feature the settings in force (with the existing
//! `calibration.json`) become the profile « Par défaut », unchanged.
//!
//! Rules (safety-critical), the same as a safety profile's (profiles.rs):
//! - loading a site is a safety change like editing those settings by
//!   hand: whatever its safety part would loosen compared to what is in
//!   force (`profiles::loosenings`) needs the operator's confirmation
//!   (`confirm_loosen`); a refused load changes nothing;
//! - loading always disarms (« Changement de profil ») and restarts the
//!   checklist. Nothing here ever arms;
//! - only the operator loads a site (`profiles::operator_source`), never
//!   in show mode (423, `showmode::allowed`);
//! - a file from a newer version, a damaged or an invalid one is refused
//!   whole: never « no zones ». The calibration is clamped
//!   (`Calibration::sanitized`) and the zones validated as from the panel;
//! - a project may carry a site (key `site`, kept with the project's
//!   unknown sections): opening the project never applies it. The
//!   operator chooses *Garder le profil actuel* (the default) or *Utiliser
//!   celui du projet*, which goes through the same checks.

use crate::engine::Calibration;
use crate::interlock::{ArmSource, DisarmReason};
use crate::output_manager::OutputPrefs;
use crate::profiles::{self, LoadError, SafetyProfile};
use crate::projection::ProjectionConfig;
use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Current format of a `.lssite` file.
pub const SITE_FORMAT: u32 = 1;
pub const EXTENSION: &str = "lssite";
/// What the settings in force become on the first start.
pub const DEFAULT_SITE: &str = "Par défaut";
pub const MAX_SITES: usize = 100;
/// The key of a project's embedded site profile.
pub const PROJECT_KEY: &str = "site";
/// Larger files are refused before parsing.
const MAX_BYTES: u64 = 1 << 20;

/// One site profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SiteProfile {
    pub format_version: u32,
    pub name: String,
    pub calibration: Calibration,
    /// The safety settings; `None`: those in force are kept.
    pub safety: Option<SafetyProfile>,
    /// The projection zones; `None`: those in force are kept.
    pub zones: Option<ProjectionConfig>,
    /// The output connection, for the operator's information only.
    pub outputs: Option<OutputPrefs>,
}

impl Default for SiteProfile {
    fn default() -> Self {
        Self { format_version: SITE_FORMAT, name: String::new(), calibration: Calibration::default(), safety: None, zones: None, outputs: None }
    }
}

impl SiteProfile {
    /// Refuses a profile that could not be applied as is (French).
    pub fn validate(&self) -> Result<()> {
        profiles::check_name(&self.name).map_err(|e| anyhow::anyhow!("{}", e.to_string().replace("profil", "profil de site")))?;
        if profiles::is_builtin(&self.name) {
            bail!("« {} » est le nom d'un profil de sécurité intégré : choisissez-en un autre", profiles::SAFE_PREVIEW);
        }
        if let Some(p) = &self.safety {
            if p.name.trim() != self.name.trim() {
                bail!("profil de site « {} » : sa partie sécurité porte un autre nom", self.name);
            }
            p.validate().map_err(|e| anyhow::anyhow!("sécurité : {e}"))?;
        }
        if let Some(z) = &self.zones {
            z.validate().map_err(|e| anyhow::anyhow!("zones de projection : {e}"))?;
        }
        Ok(())
    }

    /// Brought into range: the calibration clamped, the safety part named
    /// after the site, the zones as the panel would save them.
    fn normalise(&mut self) {
        self.format_version = SITE_FORMAT;
        self.name = self.name.trim().to_string();
        self.calibration = self.calibration.sanitized();
        if let Some(p) = &mut self.safety {
            p.name = self.name.clone();
        }
        if let Some(z) = &mut self.zones {
            if z.validate().is_ok() {
                *z = z.sanitized();
            }
        }
    }

    /// The same settings (the format and the outputs aside).
    fn same_settings(&self, other: &SiteProfile) -> bool {
        self.calibration == other.calibration && self.safety == other.safety && self.zones == other.zones
    }
}

/// Parses and checks a site profile (a `.lssite` file, a pack's, a
/// project's). A newer format is refused, never decoded with fields lost.
pub fn parse_value(value: Value) -> Result<SiteProfile> {
    let Some(obj) = value.as_object() else { bail!("profil de site illisible : ce n'est pas un profil de site") };
    let from = match obj.get("format_version").and_then(Value::as_u64) {
        None => bail!("profil de site illisible : « format_version » manquant"),
        Some(0) => bail!("profil de site illisible : « format_version » invalide"),
        Some(v) => u32::try_from(v).unwrap_or(u32::MAX),
    };
    if from > SITE_FORMAT {
        bail!("profil de site créé par une version plus récente de Laser Studio (format {from}, cette version lit le format {SITE_FORMAT}) : non chargé");
    }
    let mut site: SiteProfile = serde_json::from_value(value).map_err(|e| anyhow::anyhow!("profil de site illisible : {e}"))?;
    site.normalise();
    site.validate()?;
    Ok(site)
}

pub fn parse(bytes: &[u8]) -> Result<SiteProfile> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| anyhow::anyhow!("profil de site illisible (JSON invalide : {e})"))?;
    parse_value(value)
}

/// A site profile as written: pretty JSON, a final newline.
pub fn bytes_of(site: &SiteProfile) -> Result<Vec<u8>> {
    let mut b = serde_json::to_vec_pretty(site)?;
    b.push(b'\n');
    Ok(b)
}

/// A list entry (`GET /api/sites`).
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub damaged: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ActiveFile {
    active: Option<String>,
}

/// `<data-dir>/sites/`, the active site (`<data-dir>/site.json`) and where
/// the calibration is written.
pub struct SiteStore {
    /// None: in memory only (tests).
    dir: Option<PathBuf>,
    active_path: Option<PathBuf>,
    calibration_path: Option<PathBuf>,
    mem: BTreeMap<String, SiteProfile>,
    active: Option<String>,
}

impl SiteStore {
    pub fn load(data_dir: &Path) -> Self {
        let active_path = data_dir.join("site.json");
        let active = std::fs::read(&active_path).ok().and_then(|b| serde_json::from_slice::<ActiveFile>(&b).ok()).and_then(|a| a.active);
        Self { dir: Some(data_dir.join("sites")), active_path: Some(active_path), calibration_path: Some(data_dir.join("calibration.json")), mem: BTreeMap::new(), active }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { dir: None, active_path: None, calibration_path: None, mem: BTreeMap::new(), active: None }
    }

    /// The name of the active site (a label: the settings in force live
    /// in their own files).
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    fn path_of(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{}.{EXTENSION}", profiles::slug(name))))
    }

    /// A site by name, checked. Plain files of sensible size only.
    pub fn read(&self, name: &str) -> Result<SiteProfile> {
        let site = match self.path_of(name) {
            None => self.mem.get(name.trim()).cloned().ok_or_else(|| anyhow::anyhow!("profil de site inconnu : « {name} »"))?,
            Some(path) => {
                let meta = std::fs::symlink_metadata(&path).map_err(|_| anyhow::anyhow!("profil de site inconnu : « {name} »"))?;
                if !meta.is_file() {
                    bail!("« {name} » n'est pas un profil de site");
                }
                if meta.len() > MAX_BYTES {
                    bail!("profil de site trop gros");
                }
                parse(&std::fs::read(&path).context("profil de site illisible")?)?
            }
        };
        if site.name.trim() != name.trim() {
            bail!("profil de site inconnu : « {name} »");
        }
        Ok(site)
    }

    pub fn exists(&self, name: &str) -> bool {
        match self.path_of(name) {
            None => self.mem.contains_key(name.trim()),
            Some(p) => p.exists(),
        }
    }

    /// Every site, by name.
    pub fn list(&self) -> Vec<Entry> {
        let mut out: Vec<Entry> = match &self.dir {
            None => self.mem.values().map(|s| Entry { name: s.name.clone(), damaged: None }).collect(),
            Some(dir) => std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|f| f.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(EXTENSION))
                .map(|p| {
                    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    let read = std::fs::symlink_metadata(&p)
                        .ok()
                        .filter(|m| m.is_file() && m.len() <= MAX_BYTES)
                        .ok_or_else(|| anyhow::anyhow!("fichier refusé"))
                        .and_then(|_| parse(&std::fs::read(&p)?));
                    match read {
                        Ok(s) if profiles::slug(&s.name) == stem => Entry { name: s.name, damaged: None },
                        Ok(_) => Entry { name: stem.clone(), damaged: Some(format!("fichier {stem}.{EXTENSION} mal nommé")) },
                        Err(e) => Entry { name: stem.clone(), damaged: Some(format!("{e:#}")) },
                    }
                })
                .collect(),
        };
        out.sort_by_key(|e| e.name.to_lowercase());
        out
    }

    /// Saves a site (replacing the one of that name).
    pub fn write(&mut self, site: &SiteProfile) -> Result<()> {
        let mut site = site.clone();
        site.normalise();
        site.validate()?;
        let exists = self.exists(&site.name);
        match self.path_of(&site.name) {
            None => {
                if !exists && self.mem.len() >= MAX_SITES {
                    bail!("{MAX_SITES} profils de site au plus");
                }
                self.mem.insert(site.name.clone(), site);
            }
            Some(path) => {
                if exists {
                    // Another profile may use this file name (« Club! » and « Club ? »).
                    let other = std::fs::read(&path).ok().and_then(|b| parse(&b).ok()).map(|s| s.name);
                    if other.as_deref().is_some_and(|o| o != site.name) {
                        bail!("Nom trop proche du profil de site « {} » : choisissez-en un autre", other.unwrap_or_default());
                    }
                } else if self.list().len() >= MAX_SITES {
                    bail!("{MAX_SITES} profils de site au plus");
                }
                crate::datafile::replace(&path, &bytes_of(&site)?)?;
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        match self.path_of(name) {
            None => {
                if self.mem.remove(name.trim()).is_none() {
                    bail!("profil de site inconnu : « {name} »");
                }
            }
            Some(path) => {
                if !path.is_file() {
                    bail!("profil de site inconnu : « {name} »");
                }
                std::fs::remove_file(&path).map_err(|e| anyhow::anyhow!("suppression impossible : {e}"))?;
            }
        }
        if self.active.as_deref().is_some_and(|a| a.trim() == name.trim()) {
            self.set_active(None)?;
        }
        Ok(())
    }

    pub fn set_active(&mut self, name: Option<String>) -> Result<()> {
        if let Some(path) = &self.active_path {
            crate::datafile::replace(path, &serde_json::to_vec_pretty(&ActiveFile { active: name.clone() })?)?;
        }
        self.active = name;
        Ok(())
    }

    /// Writes the calibration file (in memory: nothing).
    fn save_calibration(&self, cal: &Calibration) {
        if let Some(path) = &self.calibration_path {
            crate::datafile::save_json(path, cal);
        }
    }
}

// ---------- what a site changes ----------

/// The settings in force as a site named `name`. The safety part keeps
/// the active safety profile's venue, notes and on-site check while its
/// settings are unchanged.
pub fn capture(s: &Shared, name: &str) -> SiteProfile {
    let name = name.trim().to_string();
    let mut safety = profiles::snapshot(s, &name);
    if let Some(active) = s.profiles.active().filter(|_| !profiles::modified(s)) {
        safety.venue = active.venue.clone();
        safety.notes = active.notes.clone();
        safety.verified_on = active.verified_on.clone();
    }
    SiteProfile {
        format_version: SITE_FORMAT,
        name,
        calibration: s.calibration,
        safety: Some(safety),
        zones: Some(s.projection.get().clone()),
        outputs: Some(s.output_mgr.prefs()),
    }
}

/// What loading `site` would loosen compared to what is in force, in
/// French. Empty: applied without confirmation.
pub fn loosenings(s: &Shared, site: &SiteProfile) -> Vec<String> {
    site.safety.as_ref().map(|p| profiles::loosenings(s, p)).unwrap_or_default()
}

/// What loading `site` would change, in French (the loosenings aside).
pub fn differences(s: &Shared, site: &SiteProfile) -> Vec<String> {
    let mut out = Vec::new();
    let (a, b) = (s.calibration, site.calibration);
    let pct = |v: f32| format!("{:.0} %", v * 100.0);
    let mut cal = Vec::new();
    if a.offset_x != b.offset_x {
        cal.push(format!("décalage X {} → {}", pct(a.offset_x), pct(b.offset_x)));
    }
    if a.offset_y != b.offset_y {
        cal.push(format!("décalage Y {} → {}", pct(a.offset_y), pct(b.offset_y)));
    }
    if a.scale_x != b.scale_x {
        cal.push(format!("échelle X {} → {}", pct(a.scale_x), pct(b.scale_x)));
    }
    if a.scale_y != b.scale_y {
        cal.push(format!("échelle Y {} → {}", pct(a.scale_y), pct(b.scale_y)));
    }
    if a.rotation_deg != b.rotation_deg {
        cal.push(format!("rotation {:.0}° → {:.0}°", a.rotation_deg, b.rotation_deg));
    }
    if !cal.is_empty() {
        out.push(format!("Calibration : {}", cal.join(", ")));
    }
    if let Some(z) = &site.zones {
        let cur = s.projection.get();
        if z.zones != cur.zones || z.default_zones != cur.default_zones {
            out.push(format!("Zones de projection : {} → {} zone{}", cur.zones.len(), z.zones.len(), if z.zones.len() > 1 { "s" } else { "" }));
        }
    }
    if let Some(p) = &site.safety {
        let now = profiles::snapshot(s, &p.name);
        if now.safety != p.safety || p.limits.iter().any(|(id, l)| now.limits.get(id).is_some_and(|c| c != l)) || (p.presence.is_some() && now.presence != p.presence) {
            out.push("Réglages de sécurité (zones, horizon, limiteur, plafonds) différents".into());
        }
    }
    if let Some(o) = &site.outputs {
        let cur = s.output_mgr.prefs();
        if o.selected != cur.selected {
            let label = |v: &Option<String>| v.clone().unwrap_or_else(|| "aperçu seul".into());
            out.push(format!("Sortie : « {} » → « {} » (non changée : à choisir dans « Sorties et projecteurs »)", label(&cur.selected), label(&o.selected)));
        }
    }
    out
}

/// Loads `site`: see the module docs. Disarms, never arms. `Err` with
/// `Loosens` changes nothing.
pub fn apply(s: &mut Shared, site: &SiteProfile, confirm_loosen: bool, src: ArmSource) -> std::result::Result<Value, LoadError> {
    if !profiles::operator_source(src) {
        return Err(LoadError::Source);
    }
    let mut site = site.clone();
    site.normalise();
    if let Err(e) = site.validate() {
        s.gate.disarm(DisarmReason::ProfileChange, src);
        return Err(LoadError::Damaged(format!("{e:#}")));
    }
    let loosen = loosenings(s, &site);
    if !loosen.is_empty() && !confirm_loosen {
        return Err(LoadError::Loosens(loosen));
    }
    s.gate.disarm(DisarmReason::ProfileChange, src);
    let mut errors = Vec::new();
    if let Some(p) = &site.safety {
        // The safety part is a safety profile of the same name, so the
        // status bar and the restart show it like any other.
        match s.profiles.write(p) {
            Ok(()) => {
                if let Err(e) = profiles::apply(s, p.clone(), true, src) {
                    errors.push(e.message_fr());
                }
            }
            Err(e) => errors.push(format!("sécurité : {e:#}")),
        }
    } else {
        s.checklist.restart();
        s.sync_checklist();
    }
    s.calibration = site.calibration;
    s.sites.save_calibration(&site.calibration);
    if let Some(z) = &site.zones {
        if let Err(e) = s.projection.set(z.clone()) {
            errors.push(format!("zones de projection : {e:#}"));
        }
    }
    if let Err(e) = s.sites.set_active(Some(site.name.clone())) {
        errors.push(format!("{e:#}"));
    }
    let detail = json!({ "action": "load_site", "name": site.name, "confirmed_loosen": !loosen.is_empty(), "loosen": loosen, "errors": errors });
    s.gate.log().record("site_profile", Some(src.id()), detail.clone());
    if loosen.is_empty() {
        log::info!("site profile « {} » loaded", site.name);
    } else {
        log::warn!("site profile « {} » loaded, loosening confirmed by the operator: {}", site.name, loosen.join("; "));
    }
    if !errors.is_empty() {
        return Err(LoadError::Failed(errors.join("; ")));
    }
    Ok(detail)
}

/// Loads the site profile `name` of the store.
pub fn load(s: &mut Shared, name: &str, confirm_loosen: bool, src: ArmSource) -> std::result::Result<Value, LoadError> {
    if !profiles::operator_source(src) {
        return Err(LoadError::Source);
    }
    match s.sites.read(name) {
        Ok(site) => apply(s, &site, confirm_loosen, src),
        Err(e) => {
            // Like a safety profile: never applied, disarmed, settings kept.
            s.gate.disarm(DisarmReason::ProfileChange, src);
            s.gate.log().record("site_profile", Some(src.id()), json!({ "action": "refused", "name": name, "error": format!("{e:#}") }));
            Err(LoadError::Damaged(format!("{e:#}")))
        }
    }
}

/// « Enregistrer sous… »: the settings in force saved as `name`, which
/// becomes the active site. Changes no setting.
pub fn save(s: &mut Shared, name: &str) -> Result<SiteProfile> {
    let site = capture(s, name);
    site.validate()?;
    s.sites.write(&site)?;
    s.sites.set_active(Some(site.name.clone()))?;
    s.gate.log().record("site_profile", Some("ui"), json!({ "action": "save_site", "name": site.name }));
    Ok(site)
}

pub fn delete(s: &mut Shared, name: &str) -> Result<()> {
    s.sites.remove(name)?;
    s.gate.log().record("site_profile", Some("ui"), json!({ "action": "delete_site", "name": name }));
    Ok(())
}

/// The open project's site profile, if it carries one: `Ok(None)` when it
/// doesn't, `Err` when it is damaged or from a newer version.
pub fn of_project(s: &Shared) -> Result<Option<SiteProfile>> {
    s.project.extra().get(PROJECT_KEY).map(|v| parse_value(v.clone())).transpose()
}

/// *Utiliser celui du projet*: the project's site is kept in the store
/// (under its name, or « <nom> (projet) » when another profile has that
/// name), then loaded like any site. A loosening not confirmed changes
/// nothing, not even the store.
pub fn use_project(s: &mut Shared, confirm_loosen: bool, src: ArmSource) -> std::result::Result<Value, LoadError> {
    if !profiles::operator_source(src) {
        return Err(LoadError::Source);
    }
    let mut site = match of_project(s) {
        Ok(Some(site)) => site,
        Ok(None) => return Err(LoadError::Unknown("ce projet ne contient pas de profil de site".into())),
        Err(e) => return Err(LoadError::Damaged(format!("{e:#}"))),
    };
    let loosen = loosenings(s, &site);
    if !loosen.is_empty() && !confirm_loosen {
        return Err(LoadError::Loosens(loosen));
    }
    let base = site.name.clone();
    let mut n = 1;
    loop {
        let candidate = match n {
            1 => base.clone(),
            2 => format!("{base} (projet)"),
            _ => format!("{base} (projet {})", n - 1),
        };
        let candidate: String = candidate.chars().take(profiles::MAX_NAME).collect();
        let same = s.sites.read(&candidate).ok().map(|o| o.same_settings(&rename(&site, &candidate)));
        match same {
            Some(true) => {
                site = rename(&site, &candidate);
                break;
            }
            None if !s.sites.exists(&candidate) => {
                site = rename(&site, &candidate);
                if let Err(e) = s.sites.write(&site) {
                    return Err(LoadError::Failed(format!("{e:#}")));
                }
                break;
            }
            _ => n += 1,
        }
        if n > 50 {
            return Err(LoadError::Failed("trop de profils de site portent ce nom".into()));
        }
    }
    apply(s, &site, true, src)
}

fn rename(site: &SiteProfile, name: &str) -> SiteProfile {
    let mut out = site.clone();
    out.name = name.to_string();
    if let Some(p) = &mut out.safety {
        p.name = name.to_string();
    }
    out
}

/// Startup, after the safety profiles: the first start with this feature
/// keeps the settings in force (and `calibration.json`) as « Par défaut ».
/// Changes no setting, never arms.
pub fn startup(s: &mut Shared, data_dir: &Path) {
    if data_dir.join("sites").exists() {
        return;
    }
    let site = capture(s, DEFAULT_SITE);
    if let Err(e) = s.sites.write(&site).and_then(|_| s.sites.set_active(Some(site.name.clone()))) {
        log::warn!("site profiles not initialised: {e:#}");
    }
}

/// `GET /api/sites`.
pub fn view(s: &Shared) -> Value {
    let active = s.sites.active().map(str::to_string);
    let modified = active.as_deref().and_then(|n| s.sites.read(n).ok()).map(|site| !differences(s, &site).is_empty() || !loosenings(s, &site).is_empty());
    let project = match of_project(s) {
        Ok(Some(site)) => json!({ "name": site.name, "differences": differences(s, &site), "loosen": loosenings(s, &site) }),
        Ok(None) => Value::Null,
        Err(e) => json!({ "error": format!("{e:#}") }),
    };
    json!({
        "active": active,
        "modified": modified.unwrap_or(false),
        "sites": s.sites.list(),
        "project_site": project,
        "default": DEFAULT_SITE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use crate::zones::{Zone, ZoneKind};

    fn shared() -> Shared {
        test_support::shared()
    }

    fn site(name: &str) -> SiteProfile {
        let s = shared();
        capture(&s, name)
    }

    #[test]
    fn a_site_round_trips_and_keeps_its_parts() {
        let mut s = shared();
        s.calibration = Calibration { offset_x: 0.2, ..Calibration::default() };
        let site = capture(&s, "Club");
        let back = parse(&bytes_of(&site).unwrap()).unwrap();
        assert_eq!(back, site);
        assert_eq!(back.safety.as_ref().unwrap().name, "Club");
        assert_eq!(back.calibration.offset_x, 0.2);
    }

    #[test]
    fn newer_or_unversioned_sites_are_refused() {
        let mut v = serde_json::to_value(site("Club")).unwrap();
        v["format_version"] = json!(SITE_FORMAT + 1);
        let e = parse_value(v.clone()).unwrap_err().to_string();
        assert!(e.contains("plus récente"), "{e}");
        v.as_object_mut().unwrap().remove("format_version");
        assert!(parse_value(v).is_err());
        assert!(parse(b"[1,2]").is_err());
    }

    #[test]
    fn the_calibration_stays_in_range_after_parsing() {
        let mut v = serde_json::to_value(site("Club")).unwrap();
        v["calibration"] = json!({ "offset_x": 9.0, "offset_y": -7.0, "scale_x": 50.0, "scale_y": 0.0, "rotation_deg": 999.0 });
        let got = parse_value(v).unwrap().calibration;
        assert_eq!((got.offset_x, got.offset_y, got.scale_x, got.scale_y, got.rotation_deg), (1.0, -1.0, 2.0, 0.1, 180.0));
        // Every point it maps stays in -1..1.
        for (x, y) in [(1.0, 1.0), (-1.0, 1.0), (0.0, 0.0), (1.0, -1.0)] {
            let (a, b) = got.apply(x, y);
            assert!((-1.0..=1.0).contains(&a) && (-1.0..=1.0).contains(&b));
        }
    }

    #[test]
    fn a_builtin_or_mismatched_name_is_refused() {
        let mut v = site(profiles::SAFE_PREVIEW);
        v.safety = None;
        assert!(v.validate().is_err());
        let mut v = site("Club");
        v.safety.as_mut().unwrap().name = "Autre".into();
        assert!(v.validate().is_err());
    }

    fn looser_site(s: &Shared) -> SiteProfile {
        let mut site = capture(s, "Plein air");
        // No more blanking zone: looser than what is in force.
        site.safety.as_mut().unwrap().safety.zones.clear();
        site
    }

    fn with_blank_zone(s: &mut Shared) {
        let mut cfg = s.safety.get();
        cfg.zones = vec![Zone { id: 1, name: "Z1".into(), kind: ZoneKind::Blank, level: 0.0, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, -0.5]], ..Default::default() }];
        s.safety.set(cfg, true).unwrap();
    }

    #[test]
    fn a_looser_site_needs_confirmation_and_changes_nothing_without_it() {
        let mut s = shared();
        with_blank_zone(&mut s);
        let site = looser_site(&s);
        let mut moved = site.clone();
        moved.calibration.offset_x = 0.5;
        let before = (s.safety.get(), s.calibration);
        match apply(&mut s, &moved, false, ArmSource::Ui) {
            Err(LoadError::Loosens(l)) => assert!(!l.is_empty()),
            other => panic!("{other:?}"),
        }
        assert_eq!((s.safety.get(), s.calibration), before, "nothing changed");
        assert_eq!(s.sites.active(), None);
        apply(&mut s, &moved, true, ArmSource::Ui).unwrap();
        assert!(s.safety.get().zones.is_empty());
        assert_eq!(s.calibration.offset_x, 0.5);
        assert_eq!(s.sites.active(), Some("Plein air"));
        assert_eq!(s.profiles.active().unwrap().name, "Plein air");
    }

    #[test]
    fn loading_a_site_disarms_and_never_arms() {
        let mut s = shared();
        s.request_arm(ArmSource::Ui).expect("test state arms");
        assert!(s.gate.is_armed());
        let mut tighter = capture(&s, "Salle");
        tighter.calibration.scale_x = 0.5;
        apply(&mut s, &tighter, false, ArmSource::Ui).unwrap();
        assert!(!s.gate.is_armed(), "a site change disarms");
        // A refused (looser) one leaves it disarmed too, never arms.
        with_blank_zone(&mut s);
        let looser = looser_site(&s);
        assert!(apply(&mut s, &looser, false, ArmSource::Ui).is_err());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn only_the_operator_loads_a_site() {
        let mut s = shared();
        let site = capture(&s, "Club");
        for src in [ArmSource::Midi, ArmSource::Osc, ArmSource::Dmx] {
            assert!(matches!(apply(&mut s, &site, true, src), Err(LoadError::Source)));
        }
        assert_eq!(s.sites.active(), None);
    }

    #[test]
    fn the_projects_site_is_offered_never_applied_and_kept_under_a_free_name() {
        let mut s = shared();
        let mut local = capture(&s, "Club");
        local.calibration.offset_y = 0.3;
        s.sites.write(&local).unwrap();
        let mut theirs = capture(&s, "Club");
        theirs.calibration.offset_y = -0.3;
        let mut extra = serde_json::Map::new();
        extra.insert(PROJECT_KEY.into(), serde_json::to_value(&theirs).unwrap());
        s.project.set_extra_for_tests(extra);
        let before = s.calibration;
        let v = view(&s);
        assert_eq!(v["project_site"]["name"], "Club");
        assert!(v["project_site"]["differences"][0].as_str().unwrap().starts_with("Calibration"));
        assert_eq!(s.calibration, before, "offering changes nothing");
        use_project(&mut s, false, ArmSource::Ui).unwrap();
        assert_eq!(s.calibration.offset_y, -0.3);
        assert_eq!(s.sites.active(), Some("Club (projet)"));
        assert_eq!(s.sites.read("Club").unwrap().calibration.offset_y, 0.3, "the local profile is kept");
    }

    #[test]
    fn a_looser_project_site_needs_confirmation() {
        let mut s = shared();
        with_blank_zone(&mut s);
        let theirs = looser_site(&s);
        let mut extra = serde_json::Map::new();
        extra.insert(PROJECT_KEY.into(), serde_json::to_value(&theirs).unwrap());
        s.project.set_extra_for_tests(extra);
        assert!(!view(&s)["project_site"]["loosen"].as_array().unwrap().is_empty());
        assert!(matches!(use_project(&mut s, false, ArmSource::Ui), Err(LoadError::Loosens(_))));
        assert!(s.sites.list().is_empty(), "nothing stored without the confirmation");
        assert_eq!(s.safety.get().zones.len(), 1);
        use_project(&mut s, true, ArmSource::Ui).unwrap();
        assert!(s.safety.get().zones.is_empty());
    }

    /// Any site (random safety, caps and calibration, armed or not):
    /// without confirmation it is either refused with nothing changed or
    /// applied disarmed; the calibration in force always maps into -1..1.
    /// Seeded; `SAFETY_CASES` runs more.
    #[test]
    fn any_site_is_refused_unchanged_or_applied_disarmed() {
        use crate::power::MAIN_OUTPUT;
        use crate::safety_invariants::{random_limits, random_safety, Rng};
        let cases = std::env::var("SAFETY_CASES").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(100);
        for seed in 0..cases as u64 {
            let mut rng = Rng::new(seed ^ 0x0289_0289);
            let mut s = shared();
            s.safety.set(random_safety(&mut rng), true).unwrap();
            s.outputs.set(MAIN_OUTPUT, Some(random_limits(&mut rng)), None, false, true).unwrap();
            let before = (s.safety.get(), s.outputs.active_limits(), s.calibration);
            let mut site = capture(&s, "Fuzz");
            let p = site.safety.as_mut().unwrap();
            p.safety = random_safety(&mut rng);
            p.limits.insert(MAIN_OUTPUT.into(), random_limits(&mut rng));
            let wild = |rng: &mut Rng| rng.range(-1000.0, 1000.0);
            site.calibration = Calibration { offset_x: wild(&mut rng), offset_y: wild(&mut rng), scale_x: wild(&mut rng), scale_y: wild(&mut rng), rotation_deg: wild(&mut rng) };
            if rng.chance(0.5) {
                let _ = s.request_arm(ArmSource::Ui);
            }
            match apply(&mut s, &site, false, ArmSource::Ui) {
                Ok(_) => assert!(!s.gate.is_armed(), "seed {seed}"),
                Err(LoadError::Loosens(_)) => assert_eq!((s.safety.get(), s.outputs.active_limits(), s.calibration), before, "seed {seed}"),
                Err(e) => panic!("seed {seed}: {e:?}"),
            }
            for _ in 0..50 {
                let (x, y) = s.calibration.apply(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
                assert!((-1.0..=1.0).contains(&x) && (-1.0..=1.0).contains(&y), "seed {seed}");
            }
        }
    }

    #[test]
    fn the_store_on_disk_lists_saves_and_migrates_the_calibration() {
        let dir = std::env::temp_dir().join(format!("laser-studio-sites-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = shared();
        s.sites = SiteStore::load(&dir);
        s.calibration.rotation_deg = 12.0;
        startup(&mut s, &dir);
        assert_eq!(s.sites.active(), Some(DEFAULT_SITE));
        assert_eq!(s.sites.read(DEFAULT_SITE).unwrap().calibration.rotation_deg, 12.0);
        assert!(dir.join("sites").join(format!("par-défaut.{EXTENSION}")).is_file());
        // A second start leaves it alone.
        s.calibration.rotation_deg = 0.0;
        startup(&mut s, &dir);
        assert_eq!(s.sites.read(DEFAULT_SITE).unwrap().calibration.rotation_deg, 12.0);
        save(&mut s, "Salle B").unwrap();
        assert_eq!(s.sites.list().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Par défaut", "Salle B"]);
        std::fs::write(dir.join("sites").join("abime.lssite"), b"{oops").unwrap();
        assert!(s.sites.list().iter().any(|e| e.name == "abime" && e.damaged.is_some()));
        // Loading writes calibration.json, clamped.
        let mut far = s.sites.read("Salle B").unwrap();
        far.calibration.offset_x = 0.25;
        s.sites.write(&far).unwrap();
        load(&mut s, "Salle B", false, ArmSource::Ui).unwrap();
        let cal: Calibration = serde_json::from_slice(&std::fs::read(dir.join("calibration.json")).unwrap()).unwrap();
        assert_eq!(cal.offset_x, 0.25);
        delete(&mut s, "Salle B").unwrap();
        assert_eq!(s.sites.active(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
