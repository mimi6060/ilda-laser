//! Pre-show checklist (T-258): an aide-mémoire of the checks done before a
//! show on a real laser (key switch and hardware e-stop, zones, power caps,
//! permits...), never a statement of legal compliance.
//!
//! The list itself (items, outdoor / audience-scanning flags) is kept in
//! `<data-dir>/checklist.json` and survives restarts. What was ticked is
//! the *run*: in memory only, so every start of the studio begins with an
//! empty list.
//!
//! When a real output is configured (`--device`, `--ponk`, or the hidden
//! `--test-checklist` that simulates one), the `checklist` interlock blocks
//! arming until the run is completed. In pure preview there is no
//! interlock. Completing the list never arms the laser: it only lets the
//! operator arm it.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Label of the `checklist` interlock (also the arm button's hint).
pub const LABEL_FR: &str = "Liste de contrôle avant show à faire";
pub const MAX_ITEMS: usize = 40;
pub const MAX_TEXT: usize = 400;
pub const MAX_VALUE: usize = 200;
pub const MAX_OPERATOR: usize = 80;
const MAX_ID: usize = 40;

/// When an item applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemWhen {
    #[default]
    Always,
    /// Only for an outdoor show.
    Outdoor,
    /// Only when the show scans the audience.
    AudienceScan,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChecklistItem {
    pub id: String,
    pub text_fr: String,
    pub when: ItemWhen,
    /// Ticking it needs a typed value (e.g. a permit reference).
    pub needs_value: bool,
    /// The label of that value's field.
    pub value_label_fr: String,
}

impl ChecklistItem {
    fn new(id: &str, text_fr: &str, when: ItemWhen) -> Self {
        Self { id: id.into(), text_fr: text_fr.into(), when, needs_value: false, value_label_fr: String::new() }
    }
}

/// `checklist.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChecklistConfig {
    pub items: Vec<ChecklistItem>,
    /// Outdoor show: shows the `Outdoor` items.
    pub outdoor: bool,
    /// The show scans the audience: shows the `AudienceScan` items.
    pub audience_scan: bool,
}

impl Default for ChecklistConfig {
    fn default() -> Self {
        Self { items: default_items(), outdoor: false, audience_scan: false }
    }
}

/// Our own wording of the usual good practice (IEC 60825-3, ILDA, DGTA
/// form), see docs/research/safety-regulation.md.
pub fn default_items() -> Vec<ChecklistItem> {
    use ItemWhen::*;
    let mut permit = ChecklistItem::new(
        "outdoor_permit",
        "Autorisation DGTA obtenue ; personne joignable sur GSM pendant le show ; angles déclarés configurés (zones, horizon).",
        Outdoor,
    );
    permit.needs_value = true;
    permit.value_label_fr = "Référence de l'autorisation".into();
    vec![
        ChecklistItem::new("projector", "Projecteur classé et étiqueté (IEC/EN 60825-1) ; interrupteur à clé et arrêt d'urgence matériel testés.", Always),
        ChecklistItem::new("software_estop", "Arrêt d'urgence logiciel (Échap) testé : la sortie est coupée.", Always),
        ChecklistItem::new("zones", "Zones de sécurité et horizon vérifiés sur place, avec une mire, à faible puissance.", Always),
        ChecklistItem::new("power_caps", "Plafonds de puissance de la sortie vérifiés.", Always),
        ChecklistItem::new("reflections", "Aucun faisceau vers des miroirs, vitres ou surfaces réfléchissantes non prévues.", Always),
        ChecklistItem::new("height", "Faisceaux à au moins 3 m au-dessus du sol du public (exigé en France) et hors des zones de passage.", Always),
        ChecklistItem::new("operator", "Opérateur formé présent pendant tout le show, avec accès à l'arrêt d'urgence.", Always),
        permit,
        ChecklistItem::new(
            "audience_scan",
            "Irradiance mesurée au point le plus proche du public ; lentille de divergence ou atténuation matérielle ; protection matérielle contre l'arrêt du balayage.",
            AudienceScan,
        ),
        ChecklistItem::new("signage", "Signalisation ou annonce au public, si le lieu l'exige.", Always),
    ]
}

/// What was ticked this session. Never saved.
#[derive(Clone, Debug, Default)]
pub struct ChecklistRun {
    pub operator: String,
    /// (item id, when it was ticked, typed value).
    pub done: Vec<(String, SystemTime, Option<String>)>,
    pub completed_at: Option<SystemTime>,
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub struct Checklist {
    path: Option<PathBuf>,
    config: ChecklistConfig,
    run: ChecklistRun,
    enforced: bool,
    load_error: Option<String>,
}

impl Checklist {
    /// A missing file gives the default list; a damaged one too, once
    /// moved to the quarantine folder (T-402). Not a safety file: the
    /// defaults are the full list.
    pub fn load(path: PathBuf, enforced: bool) -> Self {
        let (config, load_error) = match crate::datafile::load::<ChecklistConfig>(&path, &crate::datafile::beside(&path), false) {
            crate::datafile::Loaded::Missing => (ChecklistConfig::default(), None),
            crate::datafile::Loaded::Ok(c) => match normalise(c) {
                Ok(c) => (c, None),
                Err(e) => (ChecklistConfig::default(), Some(format!("{e} Liste par défaut."))),
            },
            crate::datafile::Loaded::Quarantined(w) => (ChecklistConfig::default(), Some(format!("{} Liste par défaut.", w.message_fr()))),
        };
        Self { path: Some(path), config, run: ChecklistRun::default(), enforced, load_error }
    }

    pub fn in_memory(enforced: bool) -> Self {
        Self { path: None, config: ChecklistConfig::default(), run: ChecklistRun::default(), enforced, load_error: None }
    }

    /// A real output is configured: the list gates arming.
    pub fn enforced(&self) -> bool {
        self.enforced
    }

    pub fn config(&self) -> &ChecklistConfig {
        &self.config
    }

    pub fn is_complete(&self) -> bool {
        self.run.completed_at.is_some()
    }

    /// The `checklist` interlock's state.
    pub fn satisfied(&self) -> bool {
        !self.enforced || self.is_complete()
    }

    fn visible(&self, item: &ChecklistItem) -> bool {
        match item.when {
            ItemWhen::Always => true,
            ItemWhen::Outdoor => self.config.outdoor,
            ItemWhen::AudienceScan => self.config.audience_scan,
        }
    }

    pub fn visible_items(&self) -> impl Iterator<Item = &ChecklistItem> {
        self.config.items.iter().filter(|i| self.visible(i))
    }

    fn done(&self, id: &str) -> Option<&(String, SystemTime, Option<String>)> {
        self.run.done.iter().find(|(d, _, _)| d == id)
    }

    fn not_completed(&self) -> Result<()> {
        if self.is_complete() {
            bail!("Liste déjà terminée : recommencez-la pour la modifier");
        }
        Ok(())
    }

    pub fn set_operator(&mut self, name: &str) -> Result<()> {
        self.not_completed()?;
        let name = name.trim();
        if name.chars().count() > MAX_OPERATOR {
            bail!("Nom de l'opérateur trop long ({MAX_OPERATOR} caractères au plus)");
        }
        self.run.operator = name.to_string();
        Ok(())
    }

    /// Ticks (or unticks) a shown item. An item that needs a value is
    /// ticked only with a non-empty one.
    pub fn check(&mut self, id: &str, on: bool, value: Option<&str>) -> Result<()> {
        self.not_completed()?;
        let Some(item) = self.config.items.iter().find(|i| i.id == id).filter(|i| self.visible(i)).cloned() else {
            bail!("Élément inconnu : « {id} »");
        };
        self.run.done.retain(|(d, _, _)| d != id);
        if !on {
            return Ok(());
        }
        let value = value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        if value.as_ref().is_some_and(|v| v.chars().count() > MAX_VALUE) {
            bail!("Valeur trop longue ({MAX_VALUE} caractères au plus)");
        }
        if item.needs_value && value.is_none() {
            let label = if item.value_label_fr.is_empty() { "la valeur demandée" } else { item.value_label_fr.as_str() };
            bail!("Saisissez d'abord : {label}");
        }
        self.run.done.push((item.id, SystemTime::now(), value));
        Ok(())
    }

    /// What is still missing to complete the list (French), empty if none.
    pub fn missing(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.run.operator.is_empty() {
            missing.push("Nom de l'opérateur".to_string());
        }
        missing.extend(self.visible_items().filter(|i| self.done(&i.id).is_none()).map(|i| i.text_fr.clone()));
        missing
    }

    /// Marks the run complete and returns the safety log's detail
    /// (operator, every shown item with when it was ticked and its value).
    /// Never arms anything.
    pub fn complete(&mut self) -> std::result::Result<Value, Vec<String>> {
        if self.is_complete() {
            return Err(vec!["Liste déjà terminée".into()]);
        }
        let missing = self.missing();
        if !missing.is_empty() {
            return Err(missing);
        }
        let items: Vec<Value> = self
            .visible_items()
            .filter_map(|i| self.done(&i.id).map(|(_, at, value)| json!({ "id": i.id, "text": i.text_fr, "at": ms(*at), "value": value })))
            .collect();
        self.run.completed_at = Some(SystemTime::now());
        Ok(json!({
            "operator": self.run.operator,
            "items": items,
            "outdoor": self.config.outdoor,
            "audience_scan": self.config.audience_scan,
        }))
    }

    /// Back to an empty run (same operator name kept for convenience).
    /// Returns whether the run had been completed.
    pub fn restart(&mut self) -> bool {
        let was_complete = self.is_complete();
        let operator = std::mem::take(&mut self.run.operator);
        self.run = ChecklistRun { operator, ..Default::default() };
        was_complete
    }

    /// Replaces the list (saved). Refused while `armed` when the list
    /// gates arming, since it restarts the run.
    pub fn set_config(&mut self, config: ChecklistConfig, armed: bool) -> Result<()> {
        if armed && self.enforced {
            bail!("Désarmez le laser pour modifier la liste de contrôle");
        }
        let config = normalise(config)?;
        if let Some(path) = &self.path {
            let json = serde_json::to_vec_pretty(&config)?;
            crate::datafile::replace(path, &json)?;
        }
        self.config = config;
        self.load_error = None;
        self.restart();
        Ok(())
    }

    /// `GET /api/checklist`.
    pub fn view(&self) -> Value {
        let items: Vec<Value> = self
            .config
            .items
            .iter()
            .map(|i| {
                let done = self.done(&i.id);
                json!({
                    "id": i.id, "text_fr": i.text_fr, "when": i.when, "needs_value": i.needs_value,
                    "value_label_fr": i.value_label_fr, "visible": self.visible(i),
                    "done": done.is_some(), "at": done.map(|d| ms(d.1)), "value": done.and_then(|d| d.2.clone()),
                })
            })
            .collect();
        json!({
            "enforced": self.enforced,
            "complete": self.is_complete(),
            "completed_at": self.run.completed_at.map(ms),
            "operator": self.run.operator,
            "outdoor": self.config.outdoor,
            "audience_scan": self.config.audience_scan,
            "items": items,
            "missing": self.missing(),
            "load_error": self.load_error,
        })
    }
}

/// Checks a list and gives an id to the items without one.
fn normalise(mut config: ChecklistConfig) -> Result<ChecklistConfig> {
    if config.items.is_empty() {
        bail!("La liste de contrôle doit avoir au moins un élément");
    }
    if config.items.len() > MAX_ITEMS {
        bail!("Liste de contrôle trop longue ({MAX_ITEMS} éléments au plus)");
    }
    let mut next = 1;
    for index in 0..config.items.len() {
        let item = &mut config.items[index];
        item.text_fr = item.text_fr.trim().to_string();
        item.value_label_fr = item.value_label_fr.trim().to_string();
        if item.text_fr.is_empty() {
            bail!("Élément {} de la liste vide", index + 1);
        }
        if item.text_fr.chars().count() > MAX_TEXT || item.value_label_fr.chars().count() > MAX_TEXT {
            bail!("Élément {} trop long ({MAX_TEXT} caractères au plus)", index + 1);
        }
        let id = item.id.trim().to_string();
        if !id.is_empty() && (id.len() > MAX_ID || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
            bail!("Identifiant d'élément invalide : « {id} »");
        }
        config.items[index].id = id;
        if config.items[..index].iter().any(|o| !o.id.is_empty() && o.id == config.items[index].id) {
            bail!("Identifiant d'élément en double : « {} »", config.items[index].id);
        }
    }
    for index in 0..config.items.len() {
        if config.items[index].id.is_empty() {
            let id = loop {
                let id = format!("item-{next}");
                next += 1;
                if !config.items.iter().any(|o| o.id == id) {
                    break id;
                }
            };
            config.items[index].id = id;
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick_all(c: &mut Checklist) {
        let items: Vec<ChecklistItem> = c.visible_items().cloned().collect();
        for i in items {
            c.check(&i.id, true, i.needs_value.then_some("REF-1")).unwrap();
        }
    }

    #[test]
    fn preview_is_never_blocked() {
        let c = Checklist::in_memory(false);
        assert!(c.satisfied());
        assert!(!c.is_complete());
    }

    #[test]
    fn enforced_needs_operator_and_every_shown_item() {
        let mut c = Checklist::in_memory(true);
        assert!(!c.satisfied());
        assert_eq!(c.visible_items().count(), 8, "outdoor and audience-scan items hidden by default");
        tick_all(&mut c);
        assert_eq!(c.complete().unwrap_err(), vec!["Nom de l'opérateur".to_string()]);
        c.set_operator("  Michel ").unwrap();
        c.check("zones", false, None).unwrap();
        let missing = c.complete().unwrap_err();
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("Zones de sécurité"));
        c.check("zones", true, None).unwrap();
        let detail = c.complete().unwrap();
        assert!(c.satisfied());
        assert_eq!(detail["operator"], "Michel");
        assert_eq!(detail["items"].as_array().unwrap().len(), 8);
        assert!(detail["items"][0]["at"].as_u64().unwrap() > 0);
        // Locked once complete.
        assert!(c.check("zones", false, None).is_err());
        assert!(c.set_operator("x").is_err());
    }

    #[test]
    fn outdoor_items_show_only_when_outdoor_and_need_their_value() {
        let mut c = Checklist::in_memory(true);
        assert!(c.check("outdoor_permit", true, Some("X")).is_err(), "hidden item");
        let mut cfg = c.config().clone();
        cfg.outdoor = true;
        c.set_config(cfg, false).unwrap();
        assert_eq!(c.visible_items().count(), 9);
        assert!(c.check("outdoor_permit", true, None).is_err());
        assert!(c.check("outdoor_permit", true, Some("   ")).is_err());
        c.check("outdoor_permit", true, Some(" DGTA-42 ")).unwrap();
        c.set_operator("Op").unwrap();
        tick_all(&mut c);
        let detail = c.complete().unwrap();
        let permit = detail["items"].as_array().unwrap().iter().find(|i| i["id"] == "outdoor_permit").unwrap().clone();
        assert_eq!(permit["value"], "REF-1");
        assert_eq!(detail["outdoor"], true);
    }

    #[test]
    fn restart_and_config_changes_clear_the_run() {
        let mut c = Checklist::in_memory(true);
        c.set_operator("Op").unwrap();
        tick_all(&mut c);
        c.complete().unwrap();
        assert!(c.restart());
        assert!(!c.satisfied());
        assert!(c.view()["items"].as_array().unwrap().iter().all(|i| i["done"] == false));
        assert_eq!(c.view()["operator"], "Op");
        tick_all(&mut c);
        c.complete().unwrap();
        assert!(c.set_config(c.config().clone(), true).is_err(), "refused while armed");
        assert!(c.is_complete());
        c.set_config(c.config().clone(), false).unwrap();
        assert!(!c.is_complete());
    }

    #[test]
    fn invalid_lists_are_refused_and_new_items_get_an_id() {
        let mut c = Checklist::in_memory(false);
        assert!(c.set_config(ChecklistConfig { items: vec![], ..Default::default() }, false).is_err());
        let blank = ChecklistItem::new("a", "  ", ItemWhen::Always);
        assert!(c.set_config(ChecklistConfig { items: vec![blank], ..Default::default() }, false).is_err());
        let dup = vec![ChecklistItem::new("a", "x", ItemWhen::Always), ChecklistItem::new("a", "y", ItemWhen::Always)];
        assert!(c.set_config(ChecklistConfig { items: dup, ..Default::default() }, false).is_err());
        let bad_id = ChecklistItem::new("a b", "x", ItemWhen::Always);
        assert!(c.set_config(ChecklistConfig { items: vec![bad_id], ..Default::default() }, false).is_err());
        let fresh = vec![ChecklistItem::new("item-1", "x", ItemWhen::Always), ChecklistItem::new("", "Nouveau", ItemWhen::Always)];
        c.set_config(ChecklistConfig { items: fresh, ..Default::default() }, false).unwrap();
        assert_eq!(c.config().items[1].id, "item-2");
    }

    #[test]
    fn the_list_survives_a_restart_but_not_the_ticks() {
        let dir = crate::datafile::tests::temp_dir("checklist");
        let path = dir.join("checklist.json");
        let mut c = Checklist::load(path.clone(), true);
        let mut cfg = c.config().clone();
        cfg.items.push(ChecklistItem::new("", "Extincteur à portée", ItemWhen::Always));
        c.set_config(cfg, false).unwrap();
        c.set_operator("Op").unwrap();
        tick_all(&mut c);
        c.complete().unwrap();
        let again = Checklist::load(path, true);
        assert_eq!(again.config(), c.config());
        assert!(again.config().items.iter().any(|i| i.text_fr == "Extincteur à portée"));
        assert!(!again.is_complete() && !again.satisfied());
        assert_eq!(again.view()["operator"], "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_file_gives_the_default_list() {
        let (dir, path) = crate::datafile::tests::with_damaged("checklist-damaged", "checklist.json");
        let c = Checklist::load(path, true);
        assert_eq!(c.config(), &ChecklistConfig::default());
        assert!(c.view()["load_error"].is_string());
        crate::datafile::tests::check_kept(&dir, "checklist.json", false);
    }
}
