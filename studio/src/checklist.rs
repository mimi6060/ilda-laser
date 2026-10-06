//! Pre-show checklist (T-258): an aide-mémoire of the checks done before a
//! show on a real laser (key switch and hardware e-stop, zones, power caps,
//! permits...), never a statement of legal compliance.
//!
//! Audience scanning (T-255): « Balayage public » unlocks only with the
//! « Balayage du public » flag on and its items ticked in a completed run
//! (`AUDIENCE_ITEMS`: irradiance measured, hardware attenuation); a
//! restarted or edited list ends that mode (`audience.rs`).
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
//!
//! ShowNET outputs (T-472): what the ShowNET does when the stream stops
//! (network lost, studio frozen) is not documented, so it may hold its
//! last frame, a static beam. When the output is a ShowNET path (PONK to
//! MadMapper, or any output whose projector sheet says ShowNET; never a
//! `laser-dac` DAC), three more items show, which the operator cannot
//! delete, and the `hw_stop` interlock refuses arming until « Arrêt
//! matériel testé » is ticked, checklist enforced or not.

use crate::power::DacModel;
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

/// The kind of output an item is only for (T-472).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputKind {
    /// A ShowNET path: PONK to MadMapper (which drives the ShowNET), the
    /// SD card over Art-Net (T-301), the SDK, or an output whose projector
    /// sheet says ShowNET.
    #[serde(rename = "shownet")]
    ShowNet,
}

impl OutputKind {
    /// The kind of the connected output (`Output::kind`, None = preview)
    /// with the projector sheet's interface model. A `laser-dac` DAC
    /// (Helios, Ether Dream, IDN) never reaches a ShowNET, whatever the
    /// sheet says; PONK always counts as one (it is how the studio reaches
    /// the ShowNET today).
    pub fn of(output: Option<&str>, model: DacModel) -> Option<Self> {
        match output {
            None | Some("dac") => None,
            Some("ponk" | "shownet" | crate::artnet::KIND) => Some(Self::ShowNet),
            Some(_) => (model == DacModel::ShowNet).then_some(Self::ShowNet),
        }
    }
}

/// Id of the ShowNET item that gates arming (`hw_stop` interlock).
pub const HW_STOP_ITEM: &str = "shownet_hw_stop";

/// The ShowNET items (T-472). Always in the list: put back if deleted.
pub fn shownet_items() -> Vec<ChecklistItem> {
    [
        (HW_STOP_ITEM, "Arrêt matériel testé (interlock ou arrêt d'urgence du projecteur)"),
        ("shownet_wired", "Liaison par câble, pas de Wi-Fi"),
        ("shownet_exclusive", "Aucun autre logiciel connecté au boîtier"),
    ]
    .into_iter()
    .map(|(id, text)| ChecklistItem { only_for: Some(OutputKind::ShowNet), ..ChecklistItem::new(id, text, ItemWhen::Always) })
    .collect()
}

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
    /// Shown only for this kind of output (None: for every output).
    pub only_for: Option<OutputKind>,
}

impl ChecklistItem {
    fn new(id: &str, text_fr: &str, when: ItemWhen) -> Self {
        Self { id: id.into(), text_fr: text_fr.into(), when, needs_value: false, value_label_fr: String::new(), only_for: None }
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
    let mut items = vec![
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
            "Protection matérielle contre l'arrêt du balayage (scan-fail) testée ; évaluation des risques faite par une personne qualifiée.",
            AudienceScan,
        ),
        ChecklistItem::new("signage", "Signalisation ou annonce au public, si le lieu l'exige.", Always),
    ];
    items.extend(audience_items());
    items.extend(shownet_items());
    items
}

/// Ids of the audience scanning items the unlock of « Balayage public »
/// needs ticked (T-255, `audience.rs`).
pub const AUDIENCE_ITEMS: [&str; 2] = ["audience_irradiance", "audience_attenuation"];

/// The audience scanning items (T-255). Always in the list: put back if
/// deleted, shown when the show scans the audience.
pub fn audience_items() -> Vec<ChecklistItem> {
    let mut irradiance = ChecklistItem::new(
        AUDIENCE_ITEMS[0],
        "Mesure d'irradiance faite au point le plus proche du public, toutes couleurs à pleine puissance de show, sous l'exposition maximale permise.",
        ItemWhen::AudienceScan,
    );
    irradiance.needs_value = true;
    irradiance.value_label_fr = "Valeur mesurée (mW/cm²)".into();
    let attenuation = ChecklistItem::new(AUDIENCE_ITEMS[1], "Lentille de divergence ou atténuation matérielle en place sur le projecteur.", ItemWhen::AudienceScan);
    vec![irradiance, attenuation]
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
    /// The kind of the connected output, for `only_for` (T-472).
    output: Option<OutputKind>,
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
        Self { path: Some(path), config, run: ChecklistRun::default(), enforced, load_error, output: None }
    }

    pub fn in_memory(enforced: bool) -> Self {
        Self { path: None, config: ChecklistConfig::default(), run: ChecklistRun::default(), enforced, load_error: None, output: None }
    }

    /// A real output is configured: the list gates arming.
    pub fn enforced(&self) -> bool {
        self.enforced
    }

    /// A real output was connected from RÉGLAGES › Sorties (T-401): the
    /// list gates arming from now on, for the rest of this run.
    pub fn enforce(&mut self) {
        self.enforced = true;
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

    pub fn output(&self) -> Option<OutputKind> {
        self.output
    }

    /// The connected output's kind changed (T-472). A completed run that
    /// now misses a shown item (the ShowNET ones) starts again; returns
    /// whether it did. Never arms.
    pub fn set_output(&mut self, output: Option<OutputKind>) -> bool {
        if self.output == output {
            return false;
        }
        self.output = output;
        if self.is_complete() && self.visible_items().any(|i| self.done(&i.id).is_none()) {
            self.restart();
            return true;
        }
        false
    }

    /// The `hw_stop` interlock's state: on a ShowNET output, « Arrêt
    /// matériel testé » must be ticked, whether the list is enforced or not.
    pub fn hw_stop_ok(&self) -> bool {
        self.output != Some(OutputKind::ShowNet) || self.done(HW_STOP_ITEM).is_some()
    }

    fn visible(&self, item: &ChecklistItem) -> bool {
        if item.only_for.is_some_and(|k| self.output != Some(k)) {
            return false;
        }
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

    /// The item `id` is ticked in this run.
    pub fn is_done(&self, id: &str) -> bool {
        self.done(id).is_some()
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
            "output": self.output,
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
                    "value_label_fr": i.value_label_fr, "only_for": i.only_for, "visible": self.visible(i),
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
            "output": self.output,
            "hw_stop_ok": self.hw_stop_ok(),
        })
    }
}

/// Checks a list, gives an id to the items without one and puts back the
/// ShowNET items (T-472) and the audience items (T-255) if they are
/// missing (a list saved before them, or edited): the ShowNET ones stay for
/// ShowNET outputs only, always shown there; the audience ones show when
/// the show scans the audience.
pub(crate) fn normalise(mut config: ChecklistConfig) -> Result<ChecklistConfig> {
    if config.items.is_empty() {
        bail!("La liste de contrôle doit avoir au moins un élément");
    }
    // The ShowNET and audience items do not count: they are put back below.
    let required: Vec<ChecklistItem> = audience_items().into_iter().chain(shownet_items()).collect();
    if config.items.iter().filter(|i| !required.iter().any(|r| r.id == i.id.trim())).count() > MAX_ITEMS {
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
    for required in required {
        match config.items.iter_mut().find(|i| i.id == required.id) {
            Some(item) => {
                item.only_for = required.only_for;
                item.when = required.when;
                if required.needs_value {
                    item.needs_value = true;
                    if item.value_label_fr.is_empty() {
                        item.value_label_fr = required.value_label_fr;
                    }
                }
            }
            None => config.items.push(required),
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

    fn ids(c: &Checklist) -> Vec<String> {
        c.visible_items().map(|i| i.id.clone()).collect()
    }

    #[test]
    fn shownet_items_show_only_for_a_shownet_output() {
        use crate::power::DacModel::{Other, ShowNet};
        // Helios, Ether Dream and IDN are all `laser-dac` outputs ("dac").
        assert_eq!(OutputKind::of(Some("dac"), ShowNet), None, "a DAC never reaches a ShowNET");
        assert_eq!(OutputKind::of(Some("dac"), Other), None);
        assert_eq!(OutputKind::of(None, ShowNet), None, "preview");
        assert_eq!(OutputKind::of(Some("ponk"), Other), Some(OutputKind::ShowNet));
        assert_eq!(OutputKind::of(Some("artnet"), Other), Some(OutputKind::ShowNet), "the SD card over Art-Net (T-301)");
        assert_eq!(OutputKind::of(Some("test"), Other), None);
        assert_eq!(OutputKind::of(Some("test"), ShowNet), Some(OutputKind::ShowNet), "declared on the sheet");

        let mut c = Checklist::in_memory(true);
        assert!(c.hw_stop_ok(), "not a ShowNET");
        assert!(!ids(&c).iter().any(|i| i.starts_with("shownet_")));
        c.set_output(OutputKind::of(Some("dac"), ShowNet));
        assert_eq!(c.visible_items().count(), 8);
        c.set_output(Some(OutputKind::ShowNet));
        assert_eq!(c.visible_items().count(), 11);
        assert!(ids(&c).ends_with(&["shownet_hw_stop".into(), "shownet_wired".into(), "shownet_exclusive".into()]));
        assert!(!c.hw_stop_ok());
        c.check("shownet_wired", true, None).unwrap();
        assert!(!c.hw_stop_ok(), "only the hardware stop item opens it");
        c.check(HW_STOP_ITEM, true, None).unwrap();
        assert!(c.hw_stop_ok());
        c.check(HW_STOP_ITEM, false, None).unwrap();
        assert!(!c.hw_stop_ok(), "unticking closes it again");
        // The hidden ShowNET items can't be ticked for another output.
        c.set_output(None);
        assert!(c.check(HW_STOP_ITEM, true, None).is_err());
    }

    #[test]
    fn the_hw_stop_gate_holds_even_when_the_list_is_not_enforced() {
        let mut c = Checklist::in_memory(false);
        assert!(c.satisfied());
        c.set_output(Some(OutputKind::ShowNet));
        assert!(c.satisfied(), "the list itself stays optional");
        assert!(!c.hw_stop_ok());
        c.check(HW_STOP_ITEM, true, None).unwrap();
        assert!(c.hw_stop_ok());
        assert_eq!(c.view()["hw_stop_ok"], true);
        assert_eq!(c.view()["output"], "shownet");
    }

    #[test]
    fn a_completed_run_starts_again_when_shownet_items_appear() {
        let mut c = Checklist::in_memory(true);
        c.set_operator("Op").unwrap();
        tick_all(&mut c);
        c.complete().unwrap();
        assert!(!c.set_output(None), "unchanged");
        assert!(c.set_output(Some(OutputKind::ShowNet)), "three items to tick now");
        assert!(!c.is_complete() && !c.satisfied() && !c.hw_stop_ok());
        tick_all(&mut c);
        let detail = c.complete().unwrap();
        assert_eq!(detail["items"].as_array().unwrap().len(), 11);
        assert_eq!(detail["output"], "shownet");
        // Leaving the ShowNET hides items: the completed run stays.
        assert!(!c.set_output(None));
        assert!(c.is_complete());
    }

    #[test]
    fn the_shownet_items_cannot_be_removed_or_hidden() {
        let mut c = Checklist::in_memory(false);
        let mut cfg = c.config().clone();
        cfg.items.retain(|i| i.only_for.is_none());
        let wired = cfg.items.len();
        cfg.items.push(ChecklistItem { when: ItemWhen::Outdoor, only_for: None, ..shownet_items()[0].clone() });
        c.set_config(cfg, false).unwrap();
        let items = &c.config().items;
        assert_eq!(items.len(), wired + 3);
        let hw = items.iter().find(|i| i.id == HW_STOP_ITEM).unwrap();
        assert_eq!((hw.when, hw.only_for), (ItemWhen::Always, Some(OutputKind::ShowNet)));
        // A list saved before T-472 (no ShowNET items, no `only_for`) loads with them.
        let old: ChecklistConfig = serde_json::from_str(r#"{"items":[{"id":"a","text_fr":"x"}]}"#).unwrap();
        // And with the two audience scanning items (T-255).
        let fixed = normalise(old).unwrap();
        assert_eq!(fixed.items.len(), 6);
        assert!(AUDIENCE_ITEMS.iter().all(|id| fixed.items.iter().any(|i| i.id == *id && i.when == ItemWhen::AudienceScan)));
        // They don't count towards the list's size limit.
        let full = ChecklistConfig { items: (0..MAX_ITEMS).map(|n| ChecklistItem::new(&format!("i{n}"), "x", ItemWhen::Always)).collect(), ..Default::default() };
        let full = normalise(full).unwrap();
        assert_eq!(full.items.len(), MAX_ITEMS + 5);
        assert!(normalise(full).is_ok(), "loads again");
    }

    /// T-255: the audience items stay, keep their kind and their measured
    /// value, and only show when the show scans the audience.
    #[test]
    fn the_audience_items_stay_and_need_the_measure() {
        let mut c = Checklist::in_memory(false);
        assert!(!c.visible_items().any(|i| AUDIENCE_ITEMS.contains(&i.id.as_str())), "hidden by default");
        let mut cfg = c.config().clone();
        cfg.items.retain(|i| !AUDIENCE_ITEMS.contains(&i.id.as_str()));
        cfg.audience_scan = true;
        c.set_config(cfg, false).unwrap();
        let irradiance = c.config().items.iter().find(|i| i.id == AUDIENCE_ITEMS[0]).unwrap().clone();
        assert!(irradiance.needs_value && irradiance.when == ItemWhen::AudienceScan);
        assert!(c.check(AUDIENCE_ITEMS[0], true, None).is_err(), "the measured value is needed");
        c.check(AUDIENCE_ITEMS[0], true, Some("2.1")).unwrap();
        assert!(c.is_done(AUDIENCE_ITEMS[0]) && !c.is_done(AUDIENCE_ITEMS[1]));
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
