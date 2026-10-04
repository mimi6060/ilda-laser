//! The free DMX patch (T-406): any channel of an Art-Net or sACN universe
//! drives any external control, through the gate (external.rs) like every
//! other external write.
//!
//! - 8 bits (one channel) or 16 bits (`fine`: the low byte), optionally
//!   inverted, as `ControlInput::Norm`.
//! - Triggers fire on the way up past 127; momentary controls follow the
//!   channel (pressed above 127) and are released when the signal is lost
//!   or the gate closes.
//! - A value is applied only when it changes. With `pickup` (and always for
//!   a brightness, as for MIDI), a channel that the screen has overtaken
//!   must cross the current value again before it takes over.
//! - An optional access channel (≥ 128 = raised) is the DMX source's
//!   access for the gate's « Exiger l'accès ».
//! - *Apprendre DMX*: the first channel that moves by more than
//!   `LEARN_STEP` from where it was when learning started is mapped.
//!
//! Universe numbers are shared: universe N is Art-Net Port-Address N and
//! sACN universe N.

use super::{signal_log, NetInConfig, Proto};
use crate::controls::{self, ControlDesc, ControlInput, ControlKind};
use crate::dmx::fixture::DmxFixture;
use crate::external::{self, SourceKind};
use crate::interlock::ArmSource;
use crate::midi::mapping::pickup_catches;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

pub const UNIVERSE_LEN: usize = 512;
pub const MAX_UNIVERSE: u16 = 63999;
pub const MAX_MAPPINGS: usize = 512;
/// A channel must move this many steps to be learned.
pub const LEARN_STEP: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmxAddr {
    pub universe: u16,
    /// 1..=512.
    pub channel: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DmxMapping {
    pub universe: u16,
    /// 1..=512 (the coarse byte in 16 bits).
    pub channel: u16,
    /// The fine (low) byte's channel for 16 bits.
    pub fine: Option<u16>,
    /// A control id of the registry (controls.rs).
    pub control: String,
    pub invert: bool,
    pub pickup: bool,
}

impl Default for DmxMapping {
    fn default() -> Self {
        DmxMapping { universe: 1, channel: 1, fine: None, control: String::new(), invert: false, pickup: true }
    }
}

/// `netin.json` → `dmx`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PatchConfig {
    pub mappings: Vec<DmxMapping>,
    /// The DMX access channel (T-408); none: DMX never has access.
    pub access: Option<DmxAddr>,
    /// One fixture instance per projection zone (T-407).
    pub fixtures: Vec<DmxFixture>,
}

fn channel_ok(c: u16) -> bool {
    (1..=UNIVERSE_LEN as u16).contains(&c)
}

impl PatchConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.mappings.len() > MAX_MAPPINGS {
            return Err(format!("trop d'affectations DMX ({MAX_MAPPINGS} au plus)"));
        }
        for m in &self.mappings {
            if m.universe > MAX_UNIVERSE {
                return Err(format!("univers DMX invalide : {} (0 à {MAX_UNIVERSE})", m.universe));
            }
            if !channel_ok(m.channel) || m.fine.is_some_and(|f| !channel_ok(f) || f == m.channel) {
                return Err(format!("canal DMX invalide pour {} (1 à 512, canal fin différent)", m.control));
            }
        }
        if let Some(a) = self.access {
            if a.universe > MAX_UNIVERSE || !channel_ok(a.channel) {
                return Err(format!("canal d'accès DMX invalide : univers {}, canal {}", a.universe, a.channel));
            }
        }
        crate::dmx::fixture::check(&self.fixtures, |z| format!("Zone {z}"))
    }

    /// The universes the patch and the fixture instances listen to.
    pub fn universes(&self) -> Vec<u16> {
        let fixtures = self.fixtures.iter().filter(|f| f.enabled).map(|f| f.universe);
        let mut v: Vec<u16> = self.mappings.iter().map(|m| m.universe).chain(self.access.map(|a| a.universe)).chain(fixtures).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// One universe as a listener hands it over (sACN already merged).
#[derive(Debug)]
pub struct UniverseEvent {
    pub proto: Proto,
    pub universe: u16,
    pub from: IpAddr,
    /// The sACN source name, or the sender's address for Art-Net.
    pub name: String,
    pub data: Vec<u8>,
}

/// The latest values of one universe.
#[derive(Debug)]
pub struct Feed {
    pub data: Vec<u8>,
    pub at: Instant,
    pub proto: Proto,
    pub from: IpAddr,
    pub name: String,
    pub lost: bool,
    pub packets: u64,
    /// Packets per second (smoothed).
    pub rate: f32,
}

#[derive(Debug, Default)]
struct MapState {
    /// Last raw value seen (16 bits); `None` until the first, and after a loss.
    raw: Option<u16>,
    /// Pickup: the channel's previous position (0..1).
    prev: Option<f32>,
    engaged: bool,
    /// What this channel last wrote (0..1), to see the screen move it.
    written: Option<f32>,
    /// Above 127 (triggers and momentary controls).
    high: bool,
}

#[derive(Debug)]
pub struct Learn {
    pub control: String,
    /// Each universe's values when it was first seen while learning.
    base: HashMap<u16, Vec<u8>>,
}

/// `NetInState.patch`: memory only.
#[derive(Debug, Default)]
pub struct PatchState {
    pub feeds: BTreeMap<u16, Feed>,
    maps: HashMap<String, MapState>,
    /// Momentary controls DMX holds pressed.
    pub held: HashSet<String>,
    pub learn: Option<Learn>,
    /// The last mapping a learn made.
    pub learned: Option<DmxMapping>,
}

fn key(m: &DmxMapping) -> String {
    format!("{}/{}/{:?}/{}", m.universe, m.channel, m.fine, m.control)
}

/// A mapping's channel(s) as 0..1, inverted if asked; `None` past the end
/// of a short packet. Also the raw 16-bit value, for change detection.
pub fn read(m: &DmxMapping, data: &[u8]) -> Option<(u16, f32)> {
    let coarse = *data.get(m.channel as usize - 1)? as u16;
    let raw = match m.fine {
        Some(f) => coarse << 8 | *data.get(f as usize - 1)? as u16,
        None => coarse << 8 | coarse,
    };
    let n = raw as f32 / 65535.0;
    Some((raw, if m.invert { 1.0 - n } else { n }))
}

/// A continuous control's current value as 0..1.
fn norm_now(s: &Shared, desc: &ControlDesc) -> Option<f32> {
    let ControlKind::Continuous { min, max, .. } = desc.kind else { return None };
    let v = controls::current(s, desc)?.as_f64()? as f32;
    Some(if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 })
}

/// One universe (the newest packet of the frame for it).
pub fn universe(s: &mut Shared, cfg: &NetInConfig, ev: UniverseEvent, now: Instant) {
    let UniverseEvent { proto, universe, from, name, data } = ev;
    let feeds = &mut s.netin.patch.feeds;
    let back = feeds.get(&universe).is_some_and(|f| f.lost);
    let (rate, packets) = match feeds.get(&universe) {
        Some(f) if !f.lost => {
            let dt = now.saturating_duration_since(f.at).as_secs_f32().max(1e-3);
            (f.rate * 0.8 + 0.2 / dt, f.packets + 1)
        }
        Some(f) => (0.0, f.packets + 1),
        None => (0.0, 1),
    };
    if back {
        log::info!("DMX univers {universe} : signal retrouvé ({name})");
        signal_log(s, "input_back", universe, &name, None);
    }
    s.netin.patch.feeds.insert(universe, Feed { data: data.clone(), at: now, proto, from, name, lost: false, packets, rate });

    if let Some(a) = cfg.dmx.access.filter(|a| a.universe == universe) {
        let raised = data.get(a.channel as usize - 1).is_some_and(|&v| v >= 128);
        external::set_access(s, SourceKind::Dmx, "", raised, now);
    }
    learn(s, universe, &data);
    for m in cfg.dmx.mappings.iter().filter(|m| m.universe == universe) {
        mapping(s, m, &data, now);
    }
    crate::dmx::fixture::universe(s, cfg, universe, &data, now);
}

fn mapping(s: &mut Shared, m: &DmxMapping, data: &[u8], now: Instant) {
    let Some((raw, n)) = read(m, data) else { return };
    let Some(desc) = s.controls.get(&m.control).cloned() else { return };
    let k = key(m);
    let st = s.netin.patch.maps.entry(k.clone()).or_default();
    if st.raw == Some(raw) {
        return;
    }
    st.raw = Some(raw);
    let high = n > 127.5 / 255.0;
    let was_high = std::mem::replace(&mut st.high, high);
    let id = desc.id.clone();
    match desc.kind {
        ControlKind::Trigger => {
            if high && !was_high {
                let _ = external::apply(s, SourceKind::Dmx, "", &id, ControlInput::Value(1.0), now);
            }
        }
        ControlKind::Momentary => {
            if high != was_high {
                if high {
                    if external::apply(s, SourceKind::Dmx, "", &id, ControlInput::Value(1.0), now).is_ok() {
                        s.netin.patch.held.insert(id);
                    }
                } else if s.netin.patch.held.remove(&id) {
                    external::release(s, SourceKind::Dmx, &id);
                }
            }
        }
        ControlKind::Toggle { .. } | ControlKind::Choice { .. } => {
            let _ = external::apply(s, SourceKind::Dmx, "", &id, ControlInput::Norm(n), now);
        }
        ControlKind::Continuous { .. } => {
            if m.pickup || crate::midi::safety::is_brightness(&id) {
                let current = norm_now(s, &desc);
                let st = s.netin.patch.maps.get_mut(&k).expect("inserted above");
                if st.engaged && st.written.zip(current).is_some_and(|(w, c)| (w - c).abs() > 0.01) {
                    st.engaged = false; // the screen moved it
                }
                if !st.engaged {
                    st.engaged = current.is_none_or(|c| pickup_catches(st.prev, n, c));
                }
                st.prev = Some(n);
                if !st.engaged {
                    return;
                }
            }
            if external::apply(s, SourceKind::Dmx, "", &id, ControlInput::Norm(n), now).is_ok() {
                let written = norm_now(s, &desc);
                if let Some(st) = s.netin.patch.maps.get_mut(&k) {
                    st.written = written;
                }
            }
        }
    }
}

/// While learning: the first channel that moved far enough is mapped.
fn learn(s: &mut Shared, universe: u16, data: &[u8]) {
    let Some(learn) = &mut s.netin.patch.learn else { return };
    let base = learn.base.entry(universe).or_insert_with(|| data.to_vec());
    let Some(ch) = base.iter().zip(data).position(|(a, b)| a.abs_diff(*b) > LEARN_STEP) else { return };
    let control = learn.control.clone();
    s.netin.patch.learn = None;
    let m = DmxMapping { universe, channel: ch as u16 + 1, control: control.clone(), ..DmxMapping::default() };
    let mut file = s.netin.hub.file();
    file.dmx.mappings.retain(|o| o.control != control && !(o.universe == universe && (o.channel == m.channel || o.fine == Some(m.channel))));
    file.dmx.mappings.push(m.clone());
    if let Err(e) = s.netin.hub.store(file) {
        log::warn!("Apprendre DMX : {e}");
        return;
    }
    log::info!("Apprendre DMX : univers {universe}, canal {} → {control}", m.channel);
    s.gate.log().record("netin_settings", Some(ArmSource::Ui.id()), json!({ "changed": ["dmx.mappings"], "learned": m }));
    // The file is written off the engine thread.
    let hub = Arc::clone(&s.netin.hub);
    if std::thread::Builder::new().name("netin-save".into()).spawn(move || hub.save()).is_err() {
        log::warn!("Apprendre DMX : enregistrement différé impossible");
    }
    s.netin.patch.learned = Some(m);
}

/// Starts (`Some(control)`) or cancels learning.
pub fn start_learn(s: &mut Shared, control: Option<&str>) -> Result<(), String> {
    let Some(id) = control else {
        s.netin.patch.learn = None;
        return Ok(());
    };
    let desc = s.controls.get(id).ok_or_else(|| format!("contrôle inconnu : {id}"))?;
    if !mappable(desc) {
        return Err(format!("« {} » ne se pilote pas depuis l'extérieur", desc.id));
    }
    s.netin.patch.learn = Some(Learn { control: desc.id.clone(), base: HashMap::new() });
    s.netin.patch.learned = None;
    Ok(())
}

/// Controls a DMX channel may be patched to.
pub fn mappable(desc: &ControlDesc) -> bool {
    external::ALWAYS.contains(&desc.id.as_str()) || !external::forbidden(desc)
}

/// Universes silent for longer than the timeout are lost: logged, their
/// held pads released, the access dropped, and the e-stop latched under
/// the blackout policy (the fade and the dark policies act through the
/// output gain, `super::output_gain`).
pub fn check_loss(s: &mut Shared, cfg: &NetInConfig, now: Instant) {
    let timeout = std::time::Duration::from_millis(cfg.loss.timeout_ms);
    let newly: Vec<(u16, String)> =
        s.netin.patch.feeds.iter().filter(|(_, f)| !f.lost && now.saturating_duration_since(f.at) > timeout).map(|(u, f)| (*u, f.name.clone())).collect();
    for (universe, name) in newly {
        if let Some(f) = s.netin.patch.feeds.get_mut(&universe) {
            f.lost = true;
            f.rate = 0.0;
        }
        log::warn!("DMX univers {universe} : signal perdu ({name}), {}", cfg.loss.policy.label_fr());
        signal_log(s, "input_lost", universe, &name, Some(cfg.loss.policy));
        // Every channel takes over again when the signal comes back.
        forget_universe(s, universe);
        if cfg.dmx.access.is_some_and(|a| a.universe == universe) {
            external::set_access(s, SourceKind::Dmx, "", false, now);
        }
        release_held(s);
        if cfg.loss.policy == super::LossPolicy::Blackout {
            s.emergency_stop(ArmSource::Dmx);
        }
    }
}

/// Forgets the universes of a protocol switched off (and every channel's
/// last value with them, so it applies again when it comes back).
pub fn drop_disabled(s: &mut Shared, cfg: &NetInConfig) {
    let gone: Vec<u16> = s.netin.patch.feeds.iter().filter(|(_, f)| !cfg.enabled(f.proto)).map(|(u, _)| *u).collect();
    for u in gone {
        s.netin.patch.feeds.remove(&u);
        forget_universe(s, u);
    }
    if !cfg.artnet.enabled && !cfg.sacn.enabled {
        s.netin.patch.maps.clear();
        release_held(s);
    }
}

fn forget_universe(s: &mut Shared, universe: u16) {
    let prefix = format!("{universe}/");
    s.netin.patch.maps.retain(|k, _| !k.starts_with(&prefix));
}

/// Lets go of the pads DMX holds.
pub fn release_held(s: &mut Shared) {
    for id in std::mem::take(&mut s.netin.patch.held) {
        external::release(s, SourceKind::Dmx, &id);
    }
}

/// The universes lost right now.
pub fn lost(s: &Shared) -> impl Iterator<Item = (&u16, &Feed)> {
    s.netin.patch.feeds.iter().filter(|(_, f)| f.lost)
}
