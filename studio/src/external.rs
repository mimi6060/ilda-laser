//! The local gate for external inputs (T-408): the operator decides on
//! screen whether DMX (Art-Net / sACN), OSC and timecode may act, and on
//! what.
//!
//! Rules, in this order (`check`):
//! 1. `transport.blackout` and `safety.estop` are accepted from every
//!    source, at all times, gate closed or not, in scope or not.
//! 2. Never from outside: arming, the hold-to-run pad, anything that is
//!    not an `external` control, and any group outside the scope sets
//!    (transport, safety). Resetting an e-stop, opening this gate,
//!    editing scopes, safety settings, zones or calibration are not
//!    controls at all: no external path reaches them.
//! 3. The gate (`ExternalGate::open`) must be open. It starts closed, is
//!    never saved (not in a project, not in the preferences) and an
//!    emergency stop closes it; resetting the stop doesn't reopen it.
//! 4. The source's access must be raised when its scope asks for it: a
//!    DMX access channel ≥ 128, or `/ls/access 1` from that OSC client
//!    (dropped after `ACCESS_TIMEOUT` without a message from it).
//! 5. The control's group must be in the source's scope (default: the
//!    masters only; for timecode, the timeline only).
//!
//! Timecode (T-168, timecode/) drives the timeline's playhead only, never
//! a control: `timecode_allows` checks the gate and the *Timeline*
//! scope, and its access is the operator's TC-IN button.
//!
//! The receivers (netin/) never call `controls::apply` themselves: every
//! external write goes through `apply`, `play_cue`, `set_page`,
//! `release` or, for a DMX fixture instance (T-407), `instance_apply` and
//! `instance_play_cue` here. MIDI keeps its own rules (T-208) and is not gated.
//! Refusals are counted per source (the monitors show them), not logged
//! one by one.

use crate::controls::{self, ControlDesc, ControlInput, ControlKind};
use crate::cues::ClickMode;
use crate::interlock::ArmSource;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// An external source. Every protocol added later gets a variant here, so
/// the property test (`nothing_external_ever_arms`) covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Dmx,
    Osc,
    Timecode,
}

impl SourceKind {
    pub const ALL: [SourceKind; 3] = [SourceKind::Dmx, SourceKind::Osc, SourceKind::Timecode];

    pub fn label_fr(self) -> &'static str {
        match self {
            SourceKind::Dmx => "DMX",
            SourceKind::Osc => "OSC",
            SourceKind::Timecode => "Timecode",
        }
    }

    /// Who a stop from this source is credited to in the safety log.
    pub fn arm_source(self) -> ArmSource {
        match self {
            SourceKind::Dmx => ArmSource::Dmx,
            SourceKind::Osc => ArmSource::Osc,
            SourceKind::Timecode => ArmSource::Network,
        }
    }
}

/// What the operator ticks: a name for a few registry groups.
pub struct ScopeSet {
    pub id: &'static str,
    pub label_fr: &'static str,
    pub groups: &'static [&'static str],
}

pub const SCOPE_SETS: [ScopeSet; 5] = [
    ScopeSet { id: "masters", label_fr: "Maîtres", groups: &["master", "look", "layer"] },
    ScopeSet { id: "cues", label_fr: "Cues", groups: &["grid", "cue", "page"] },
    ScopeSet { id: "tempo", label_fr: "Tempo", groups: &["tempo"] },
    ScopeSet { id: "timeline", label_fr: "Timeline", groups: &["timeline"] },
    ScopeSet { id: "audio", label_fr: "Audio", groups: &["audio"] },
];

/// Accepted from everywhere, always.
pub const ALWAYS: [&str; 2] = ["transport.blackout", "safety.estop"];
/// Refused from outside even though they are `external` (MIDI may use
/// them): a lost UDP release would keep the hold-to-run pad pressed.
const NEVER: [&str; 2] = ["transport.arm", "safety.hold"];
/// An OSC client's access drops this long after its last message.
pub const ACCESS_TIMEOUT: Duration = Duration::from_secs(10);

fn group_allowed(group: &str) -> bool {
    SCOPE_SETS.iter().any(|set| set.groups.contains(&scope_group(group)))
}

/// The scope group a registry group is checked against: the zone group
/// masters (T-420, registry group `group`) are dimmers like the layer
/// dimmers and go with them (« Maîtres »), so scopes saved before them
/// keep their meaning.
fn scope_group(group: &str) -> &str {
    if group == "group" {
        "layer"
    } else {
        group
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceScope {
    pub source: SourceKind,
    /// Registry groups (`ControlDesc.group`) this source may touch.
    pub groups: Vec<String>,
    /// The source's access (DMX channel, `/ls/access`) must be raised.
    pub require_access: bool,
}

impl SourceScope {
    pub fn default_for(source: SourceKind) -> Self {
        // Timecode can only ever move the timeline (T-168).
        let set = if source == SourceKind::Timecode { 3 } else { 0 };
        SourceScope { source, groups: SCOPE_SETS[set].groups.iter().map(|g| g.to_string()).collect(), require_access: true }
    }
}

impl Default for SourceScope {
    fn default() -> Self {
        Self::default_for(SourceKind::Dmx)
    }
}

/// `<data-dir>/external.json`: the scopes. Never the gate itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalConfig {
    pub scopes: Vec<SourceScope>,
}

impl ExternalConfig {
    pub fn scope(&self, src: SourceKind) -> SourceScope {
        self.scopes.iter().find(|s| s.source == src).cloned().unwrap_or_else(|| SourceScope::default_for(src))
    }

    pub fn validate(&self) -> Result<(), String> {
        for (i, sc) in self.scopes.iter().enumerate() {
            if self.scopes[..i].iter().any(|o| o.source == sc.source) {
                return Err(format!("source en double : {}", sc.source.label_fr()));
            }
            if let Some(g) = sc.groups.iter().find(|g| !group_allowed(g)) {
                return Err(format!("groupe de contrôles non autorisé depuis l'extérieur : {g}"));
            }
        }
        Ok(())
    }

    /// One entry per source, groups sorted and without duplicates.
    pub fn normalized(&self) -> Self {
        let scopes = SourceKind::ALL
            .iter()
            .map(|&src| {
                let mut sc = self.scope(src);
                sc.groups.sort();
                sc.groups.dedup();
                sc
            })
            .collect();
        ExternalConfig { scopes }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    GateClosed,
    NoAccess,
    OutOfScope,
    Forbidden,
    Unknown,
}

impl Refusal {
    pub fn label_fr(self) -> &'static str {
        match self {
            Refusal::GateClosed => "refusé : entrées externes fermées",
            Refusal::NoAccess => "refusé : accès de la source non levé",
            Refusal::OutOfScope => "refusé : hors du périmètre de cette source",
            Refusal::Forbidden => "refusé : ne se pilote jamais depuis l'extérieur",
            Refusal::Unknown => "refusé : contrôle inconnu",
        }
    }
}

/// Runtime state: memory only, closed at every start.
#[derive(Debug, Default)]
pub struct ExternalGate {
    pub open: bool,
    /// Access key (`dmx`, `osc:<ip>`, `timecode`) → last time it was
    /// raised or refreshed.
    pub access: HashMap<String, Instant>,
    pub refused: HashMap<SourceKind, u64>,
    pub accepted: HashMap<SourceKind, u64>,
    /// Last refusal per source, for the monitors.
    pub last_refusal: HashMap<SourceKind, (String, Refusal)>,
    /// `EStop::trips` when last looked at.
    estop_trips: u64,
}

/// `Shared.external`: the gate and the scopes.
pub struct External {
    pub gate: ExternalGate,
    config: ExternalConfig,
    path: Option<PathBuf>,
}

impl External {
    pub fn load(path: PathBuf) -> Self {
        let mut config: ExternalConfig = crate::load_json(&path);
        if let Err(e) = config.validate() {
            log::warn!("external.json : {e} ; périmètres par défaut (maîtres seulement)");
            config = ExternalConfig::default();
        }
        External { gate: ExternalGate::default(), config: config.normalized(), path: Some(path) }
    }

    pub fn in_memory() -> Self {
        External { gate: ExternalGate::default(), config: ExternalConfig::default().normalized(), path: None }
    }

    pub fn config(&self) -> &ExternalConfig {
        &self.config
    }

    /// What to write, out of the `Shared` lock.
    pub fn save_job(&self) -> Option<(PathBuf, ExternalConfig)> {
        self.path.clone().map(|p| (p, self.config.clone()))
    }

    #[cfg(test)]
    pub fn set_config(&mut self, cfg: ExternalConfig) {
        cfg.validate().unwrap();
        self.config = cfg.normalized();
    }
}

pub fn access_key(src: SourceKind, client: &str) -> String {
    match src {
        SourceKind::Osc => format!("osc:{client}"),
        _ => serde_json::to_value(src).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
    }
}

fn has_access(gate: &ExternalGate, key: &str, now: Instant) -> bool {
    gate.access.get(key).is_some_and(|&t| now.saturating_duration_since(t) <= ACCESS_TIMEOUT)
}

/// Raises or lowers a source's access (DMX channel, `/ls/access`).
pub fn set_access(s: &mut Shared, src: SourceKind, client: &str, raised: bool, now: Instant) {
    let key = access_key(src, client);
    if raised {
        s.external.gate.access.insert(key, now);
    } else {
        s.external.gate.access.remove(&key);
    }
}

/// A message from a client keeps its raised access alive.
pub fn touch(s: &mut Shared, src: SourceKind, client: &str, now: Instant) {
    let key = access_key(src, client);
    let gate = &mut s.external.gate;
    if has_access(gate, &key, now) {
        gate.access.insert(key, now);
    } else {
        gate.access.remove(&key);
    }
}

/// Closes the gate after an emergency stop, however it was tripped (the
/// HTTP fast path and the listeners trip the latch without the lock).
pub fn sync(s: &mut Shared) {
    let trips = s.estop.trips();
    if trips != s.external.gate.estop_trips || s.estop.is_latched() {
        s.external.gate.estop_trips = trips;
        if s.external.gate.open {
            close(s, ArmSource::System, "estop");
        }
    }
}

fn close(s: &mut Shared, src: ArmSource, reason: &str) {
    s.external.gate.open = false;
    log::info!("Entrées externes fermées ({})", if reason == "estop" { "arrêt d'urgence" } else { src.label_fr() });
    s.gate.log().record("external_gate", Some(src.id()), json!({ "open": false, "reason": reason }));
}

/// The operator opens or closes the gate (the top-bar button). Opening is
/// refused while the emergency stop is latched.
pub fn set_open(s: &mut Shared, open: bool, src: ArmSource) -> Result<(), &'static str> {
    sync(s);
    if open == s.external.gate.open {
        return Ok(());
    }
    if !open {
        close(s, src, "operator");
        return Ok(());
    }
    if s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : réinitialisez-le avant d'ouvrir les entrées externes");
    }
    s.external.gate.open = true;
    log::info!("Entrées externes ouvertes ({})", src.label_fr());
    s.gate.log().record("external_gate", Some(src.id()), json!({ "open": true }));
    Ok(())
}

/// Whether a control may never be driven from outside (the stops aside).
pub fn forbidden(desc: &ControlDesc) -> bool {
    !desc.external || NEVER.contains(&desc.id.as_str()) || !group_allowed(desc.group)
}

/// Gate, access and scope for one group (rules 3 to 5).
fn check_group(s: &mut Shared, src: SourceKind, client: &str, group: &str, now: Instant) -> Result<(), Refusal> {
    sync(s);
    if !s.external.gate.open {
        return Err(Refusal::GateClosed);
    }
    let scope = s.external.config.scope(src);
    if scope.require_access && !has_access(&s.external.gate, &access_key(src, client), now) {
        return Err(Refusal::NoAccess);
    }
    if !scope.groups.iter().any(|g| g == group) {
        return Err(Refusal::OutOfScope);
    }
    Ok(())
}

/// Every rule for one control, without applying it.
pub fn check(s: &mut Shared, src: SourceKind, client: &str, desc: &ControlDesc, now: Instant) -> Result<(), Refusal> {
    if ALWAYS.contains(&desc.id.as_str()) {
        return Ok(());
    }
    if forbidden(desc) {
        return Err(Refusal::Forbidden);
    }
    check_group(s, src, client, scope_group(desc.group), now)
}

fn count(s: &mut Shared, src: SourceKind, what: &str, r: Result<(), Refusal>) -> Result<(), Refusal> {
    let gate = &mut s.external.gate;
    match r {
        Ok(()) => *gate.accepted.entry(src).or_default() += 1,
        Err(why) => {
            *gate.refused.entry(src).or_default() += 1;
            gate.last_refusal.insert(src, (what.to_string(), why));
            log::debug!("{} {what} : {}", src.label_fr(), why.label_fr());
        }
    }
    r
}

/// The single entry point for an external control write.
pub fn apply(s: &mut Shared, src: SourceKind, client: &str, id: &str, input: ControlInput, now: Instant) -> Result<(), Refusal> {
    let Some(desc) = s.controls.get(id).cloned() else {
        return count(s, src, id, Err(Refusal::Unknown));
    };
    let r = check(s, src, client, &desc, now).and_then(|()| controls::apply_as(s, &desc.id, input, src.arm_source()).map_err(|_| Refusal::Forbidden));
    count(s, src, &desc.id, r)
}

/// Plays a cue from the start (OSC `/ls/cue/play`, the DMX cue channel):
/// the *Cues* scope. `Ok(false)`: no such cue.
pub fn play_cue(s: &mut Shared, src: SourceKind, client: &str, cue: &str, now: Instant) -> Result<bool, Refusal> {
    let r = check_group(s, src, client, "cue", now);
    count(s, src, cue, r)?;
    Ok(controls::press_cue_from(s, cue, Some(ClickMode::Restart), true, Some(src.arm_source().id())))
}

/// Selects the cue page (the DMX page channel): the *Cues* scope.
pub fn set_page(s: &mut Shared, src: SourceKind, client: &str, page: usize, now: Instant) -> Result<(), Refusal> {
    let r = check_group(s, src, client, "page", now);
    count(s, src, "page", r)?;
    s.cue_page = page;
    Ok(())
}

/// Gate, access and scope for a DMX fixture instance (T-407): its own
/// *Accès* channel is the access, and it is always required.
fn check_instance(s: &mut Shared, group: &str, access: bool) -> Result<(), Refusal> {
    sync(s);
    if !s.external.gate.open {
        return Err(Refusal::GateClosed);
    }
    if !access {
        return Err(Refusal::NoAccess);
    }
    if !s.external.config.scope(SourceKind::Dmx).groups.iter().any(|g| g == group) {
        return Err(Refusal::OutOfScope);
    }
    Ok(())
}

/// Whether timecode may drive the timeline (T-168): the gate open and
/// *Timeline* in the source's scope. TC-IN, its access, is checked by
/// the caller. Checked every frame, so not counted.
pub fn timecode_allows(s: &mut Shared) -> Result<(), Refusal> {
    sync(s);
    if !s.external.gate.open {
        return Err(Refusal::GateClosed);
    }
    if !s.external.config.scope(SourceKind::Timecode).groups.iter().any(|g| g == "timeline") {
        return Err(Refusal::OutOfScope);
    }
    Ok(())
}

/// Whether an instance with its access raised may shape its zone's image
/// (`group`: « master »). Checked every frame, so not counted.
pub fn instance_allows(s: &mut Shared, group: &str) -> Result<(), Refusal> {
    check_instance(s, group, true)
}

/// `apply` for a DMX fixture instance: the same rules, with the
/// instance's access channel as the source's access.
pub fn instance_apply(s: &mut Shared, access: bool, id: &str, input: ControlInput) -> Result<(), Refusal> {
    let Some(desc) = s.controls.get(id).cloned() else {
        return count(s, SourceKind::Dmx, id, Err(Refusal::Unknown));
    };
    let r = if ALWAYS.contains(&desc.id.as_str()) {
        Ok(())
    } else if forbidden(&desc) {
        Err(Refusal::Forbidden)
    } else {
        check_instance(s, scope_group(desc.group), access)
    };
    let r = r.and_then(|()| controls::apply_as(s, &desc.id, input, ArmSource::Dmx).map_err(|_| Refusal::Forbidden));
    count(s, SourceKind::Dmx, &desc.id, r)
}

/// `play_cue` for a DMX fixture instance (the *Cues* scope).
pub fn instance_play_cue(s: &mut Shared, access: bool, cue: &str) -> Result<bool, Refusal> {
    let r = check_instance(s, "cue", access);
    count(s, SourceKind::Dmx, cue, r)?;
    Ok(controls::press_cue_from(s, cue, Some(ClickMode::Restart), true, Some(ArmSource::Dmx.id())))
}

/// Lets go of a momentary control a source held pressed. Always allowed
/// (it only brings the control back to rest), so a pad never stays held
/// because the gate closed in between.
pub fn release(s: &mut Shared, src: SourceKind, id: &str) {
    let Some(desc) = s.controls.get(id).cloned() else { return };
    if desc.kind == ControlKind::Momentary && !forbidden(&desc) {
        let _ = controls::apply_as(s, &desc.id, ControlInput::Value(0.0), src.arm_source());
    }
}

/// `GET /api/external`.
pub fn view(s: &mut Shared, now: Instant) -> Value {
    sync(s);
    let cfg = s.external.config.clone();
    let gate = &s.external.gate;
    let sources: Vec<Value> = SourceKind::ALL
        .iter()
        .map(|&src| {
            let scope = cfg.scope(src);
            let clients: Vec<String> = match src {
                SourceKind::Osc => {
                    gate.access.keys().filter(|k| has_access(gate, k, now)).filter_map(|k| k.strip_prefix("osc:").map(str::to_string)).collect()
                }
                _ => Vec::new(),
            };
            let access = match src {
                SourceKind::Osc => !clients.is_empty(),
                // TC-IN (T-168).
                SourceKind::Timecode => s.timecode.armed,
                _ => has_access(gate, &access_key(src, ""), now),
            };
            let sets: Vec<&str> = SCOPE_SETS.iter().filter(|set| set.groups.iter().all(|g| scope.groups.iter().any(|h| h == g))).map(|set| set.id).collect();
            json!({
                "source": src,
                "label": src.label_fr(),
                "groups": scope.groups,
                "sets": sets,
                "require_access": scope.require_access,
                "access": access,
                "clients": clients,
                "refused": gate.refused.get(&src).copied().unwrap_or(0),
                "accepted": gate.accepted.get(&src).copied().unwrap_or(0),
                "last_refusal": gate.last_refusal.get(&src).map(|(what, why)| json!({ "what": what, "why": why, "why_fr": why.label_fr() })),
            })
        })
        .collect();
    json!({
        "open": gate.open,
        "estop": s.estop.is_latched(),
        "sources": sources,
        "sets": SCOPE_SETS.iter().map(|set| json!({ "id": set.id, "label": set.label_fr, "groups": set.groups })).collect::<Vec<_>>(),
        "always": ALWAYS,
    })
}

/// `POST /api/external/scopes` `{ "scopes": [{ "source": "osc", "groups":
/// […], "require_access": true }] }`: the sources given are replaced, the
/// others kept. Returns the sources that changed; the caller saves.
pub fn update_scopes(s: &mut Shared, body: &Value) -> Result<Vec<String>, String> {
    let list = body.get("scopes").and_then(Value::as_array).ok_or("« scopes » attendu (liste)")?;
    let mut cfg = s.external.config.clone();
    for item in list {
        let sc: SourceScope = serde_json::from_value(item.clone()).map_err(|e| format!("périmètre invalide : {e}"))?;
        if item.get("source").is_none() {
            return Err("périmètre invalide : « source » manquante".into());
        }
        cfg.scopes.retain(|o| o.source != sc.source);
        cfg.scopes.push(sc);
    }
    cfg.validate()?;
    let cfg = cfg.normalized();
    let before = s.external.config.clone();
    let changed: Vec<String> =
        cfg.scopes.iter().filter(|sc| before.scope(sc.source) != **sc).map(|sc| sc.source.label_fr().to_string()).collect();
    if !changed.is_empty() {
        s.gate.log().record("external_scopes", Some(ArmSource::Ui.id()), json!({ "changed": changed, "before": before, "after": cfg }));
        s.external.config = cfg;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;

    fn full_scope(require_access: bool) -> ExternalConfig {
        let groups: Vec<String> = SCOPE_SETS.iter().flat_map(|set| set.groups.iter().map(|g| g.to_string())).collect();
        ExternalConfig { scopes: SourceKind::ALL.iter().map(|&source| SourceScope { source, groups: groups.clone(), require_access }).collect() }
    }

    fn open(s: &mut Shared) {
        set_open(s, true, ArmSource::Ui).unwrap();
    }

    #[test]
    fn defaults_closed_masters_only_access_required() {
        let s = shared();
        assert!(!s.external.gate.open);
        for src in SourceKind::ALL {
            let sc = s.external.config().scope(src);
            let want: &[&str] = if src == SourceKind::Timecode { &["timeline"] } else { &["layer", "look", "master"] };
            assert_eq!(sc.groups, want);
            assert!(sc.require_access);
        }
    }

    /// Gate × access × scope × control.
    #[test]
    fn truth_table() {
        let now = Instant::now();
        // (gate open, access raised, control, accepted?)
        let cases = [
            (false, false, "master.size", Err(Refusal::GateClosed)),
            (false, true, "master.size", Err(Refusal::GateClosed)),
            (true, false, "master.size", Err(Refusal::NoAccess)),
            (true, true, "master.size", Ok(())),
            (true, true, "cue.stop_all", Err(Refusal::OutOfScope)),
            (true, true, "tempo.tap", Err(Refusal::OutOfScope)),
            (true, true, "transport.arm", Err(Refusal::Forbidden)),
            (true, true, "safety.hold", Err(Refusal::Forbidden)),
            (true, true, "no.such", Err(Refusal::Unknown)),
        ];
        for (gate, access, id, want) in cases {
            for src in [SourceKind::Dmx, SourceKind::Osc, SourceKind::Timecode] {
                let mut s = shared();
                if src == SourceKind::Timecode {
                    // Its default is the timeline only: given the masters
                    // here, the same rules hold.
                    update_scopes(&mut s, &json!({ "scopes": [{ "source": "timecode", "groups": ["master", "look", "layer"], "require_access": true }] })).unwrap();
                }
                if gate {
                    open(&mut s);
                }
                set_access(&mut s, src, "10.0.0.9", access, now);
                let got = apply(&mut s, src, "10.0.0.9", id, ControlInput::Norm(0.25), now);
                assert_eq!(got, want, "{src:?} gate={gate} access={access} {id}");
                assert_eq!(s.live.size == 0.5, want.is_ok() && id == "master.size", "{src:?} {id}");
                assert_eq!(s.external.gate.refused.get(&src).copied().unwrap_or(0), want.is_err() as u64);
            }
        }
    }

    /// T-420: a zone group's master goes with the « Maîtres » scope (as
    /// the layer dimmers do), even in a scope saved before it existed;
    /// out of that scope it is refused; it never arms.
    #[test]
    fn group_masters_follow_the_masters_scope() {
        use crate::projection::{ProjectionConfig, ZoneGroup};
        let now = Instant::now();
        for src in [SourceKind::Dmx, SourceKind::Osc] {
            let mut s = shared();
            let cfg = ProjectionConfig { groups: vec![ZoneGroup { name: "Côtés".into(), zones: vec![1], master: 1.0 }], ..Default::default() };
            s.projection.set(cfg).unwrap();
            open(&mut s);
            set_access(&mut s, src, "c", true, now);
            assert_eq!(apply(&mut s, src, "c", "group.1.master", ControlInput::Norm(0.25), now), Ok(()));
            assert_eq!(s.projection.master(1), Some(0.25));
            assert_eq!(apply(&mut s, src, "c", "group.all_full", ControlInput::Value(1.0), now), Ok(()));
            assert_eq!(s.projection.master(1), Some(1.0));
            update_scopes(&mut s, &json!({ "scopes": [{ "source": src, "groups": ["master", "look"], "require_access": true }] })).unwrap();
            assert_eq!(apply(&mut s, src, "c", "group.1.master", ControlInput::Norm(0.0), now), Err(Refusal::OutOfScope));
            assert_eq!(s.projection.master(1), Some(1.0));
            assert!(!s.gate.is_armed());
        }
    }

    #[test]
    fn stops_are_accepted_from_everywhere_always() {
        let now = Instant::now();
        for id in ALWAYS {
            for src in SourceKind::ALL {
                let mut s = shared();
                assert_eq!(apply(&mut s, src, "x", id, ControlInput::Value(1.0), now), Ok(()), "{id} closed gate");
                assert!(s.estop.is_latched());
                assert_eq!(s.gate.status(&s.estop).estop.unwrap().source, src.arm_source());
            }
        }
    }

    #[test]
    fn a_masters_scope_cannot_start_a_cue() {
        let now = Instant::now();
        let mut s = shared();
        open(&mut s);
        set_access(&mut s, SourceKind::Osc, "c", true, now);
        let cue = s.presets[0].id.clone();
        assert_eq!(play_cue(&mut s, SourceKind::Osc, "c", &cue, now), Err(Refusal::OutOfScope));
        assert_eq!(apply(&mut s, SourceKind::Osc, "c", "grid.1.1.1", ControlInput::Value(1.0), now), Err(Refusal::OutOfScope));
        assert_eq!(set_page(&mut s, SourceKind::Osc, "c", 2, now), Err(Refusal::OutOfScope));
        assert!(s.deck.active.is_empty() && s.cue_page == 0);
        // With the Cues set ticked, it can.
        update_scopes(&mut s, &json!({ "scopes": [{ "source": "osc", "groups": ["master", "grid", "cue", "page"], "require_access": true }] })).unwrap();
        assert_eq!(play_cue(&mut s, SourceKind::Osc, "c", &cue, now), Ok(true));
        assert_eq!(s.active_cue.as_deref(), Some(cue.as_str()));
    }

    #[test]
    fn osc_access_expires_and_drops() {
        let t0 = Instant::now();
        let mut s = shared();
        open(&mut s);
        set_access(&mut s, SourceKind::Osc, "a", true, t0);
        let size = |s: &mut Shared, at| apply(s, SourceKind::Osc, "a", "master.size", ControlInput::Value(1.5), at);
        assert_eq!(size(&mut s, t0 + Duration::from_secs(9)), Ok(()));
        // Another client has no access of its own.
        assert_eq!(apply(&mut s, SourceKind::Osc, "b", "master.size", ControlInput::Value(1.0), t0), Err(Refusal::NoAccess));
        // A message keeps it alive...
        touch(&mut s, SourceKind::Osc, "a", t0 + Duration::from_secs(9));
        assert_eq!(size(&mut s, t0 + Duration::from_secs(18)), Ok(()));
        // ...10 s of silence drops it, and a late message doesn't bring it back.
        assert_eq!(size(&mut s, t0 + Duration::from_secs(30)), Err(Refusal::NoAccess));
        touch(&mut s, SourceKind::Osc, "a", t0 + Duration::from_secs(30));
        assert!(s.external.gate.access.is_empty());
        set_access(&mut s, SourceKind::Osc, "a", true, t0);
        set_access(&mut s, SourceKind::Osc, "a", false, t0);
        assert_eq!(size(&mut s, t0), Err(Refusal::NoAccess));
    }

    #[test]
    fn an_estop_closes_the_gate_and_a_reset_does_not_reopen_it() {
        let mut s = shared();
        open(&mut s);
        s.emergency_stop(ArmSource::Keyboard);
        assert!(!s.external.gate.open);
        assert!(set_open(&mut s, true, ArmSource::Ui).is_err(), "not while latched");
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        sync(&mut s);
        assert!(!s.external.gate.open);
        open(&mut s);
        // Tripped and reset behind the lock's back (HTTP fast path): still closes.
        s.estop.trip(ArmSource::Api);
        s.gate.reset_estop(&estop);
        assert_eq!(apply(&mut s, SourceKind::Dmx, "", "master.size", ControlInput::Value(1.0), Instant::now()), Err(Refusal::GateClosed));
        assert!(!s.external.gate.open);
    }

    use std::sync::Arc;

    /// The guard of every external input: each control, each source,
    /// several values, gate open, full scope, no access needed. Nothing
    /// arms, safety settings and calibration never change, and a latched
    /// stop is never reset.
    #[test]
    fn nothing_external_ever_arms() {
        let mut s = shared();
        s.external.set_config(full_scope(false));
        let (safety, calibration) = (s.safety.get(), s.calibration);
        let ids: Vec<String> = s.controls.list().iter().map(|d| d.id.clone()).collect();
        let now = Instant::now();
        for id in &ids {
            for src in SourceKind::ALL {
                for v in [0.0, 0.5, 1.0] {
                    for input in [ControlInput::Norm(v), ControlInput::Value(v)] {
                        let estop = Arc::clone(&s.estop);
                        if estop.is_latched() {
                            // The operator, never the input, resets and reopens.
                            s.gate.reset_estop(&estop);
                        }
                        open(&mut s);
                        let _ = apply(&mut s, src, "c", id, input, now);
                        let _ = play_cue(&mut s, src, "c", id, now);
                        assert!(!s.gate.is_armed(), "{id} {src:?} {v}");
                        if ALWAYS.contains(&id.as_str()) {
                            assert!(s.estop.is_latched() && !s.external.gate.open, "{id} stops and closes");
                        }
                        assert_eq!(s.safety.get(), safety, "{id}");
                        assert_eq!(s.calibration, calibration, "{id}");
                    }
                }
            }
        }
        // A latched stop stays latched whatever comes in.
        s.emergency_stop(ArmSource::Ui);
        for id in &ids {
            for src in SourceKind::ALL {
                let _ = apply(&mut s, src, "c", id, ControlInput::Value(1.0), now);
                assert!(s.estop.is_latched(), "{id}");
            }
        }
        assert!(set_open(&mut s, true, ArmSource::Ui).is_err());
    }

    #[test]
    fn scopes_validate_save_and_never_carry_the_gate() {
        let mut s = shared();
        open(&mut s);
        for bad in [
            json!({ "scopes": [{ "source": "dmx", "groups": ["transport"] }] }),
            json!({ "scopes": [{ "source": "dmx", "groups": ["safety"] }] }),
            json!({ "scopes": [{ "source": "midi", "groups": [] }] }),
            json!({ "scopes": [{ "groups": [] }] }),
            json!({ "nope": 1 }),
        ] {
            assert!(update_scopes(&mut s, &bad).is_err(), "{bad}");
        }
        let changed = update_scopes(&mut s, &json!({ "scopes": [{ "source": "dmx", "groups": ["tempo", "master", "tempo"], "require_access": false }] })).unwrap();
        assert_eq!(changed, ["DMX"]);
        assert_eq!(s.external.config().scope(SourceKind::Dmx).groups, ["master", "tempo"]);
        let saved = serde_json::to_string(s.external.config()).unwrap();
        assert!(!saved.contains("open"), "the gate is never saved: {saved}");
        // Nor in a project, and opening one leaves the gate as it is.
        let project = serde_json::to_string(&crate::project::snapshot(&s)).unwrap();
        assert!(!project.contains("require_access") && !project.contains("\"open\""));
        // A restart (a fresh load) is closed.
        let dir = crate::datafile::tests::temp_dir("external");
        let path = dir.join("external.json");
        crate::save_json(&path, s.external.config());
        let back = External::load(path);
        assert!(!back.gate.open);
        assert!(!back.config().scope(SourceKind::Dmx).require_access);
    }
}
