//! External inputs over the network (T-013): OSC and Art-Net, for a
//! lighting console or another app. Both drive the stable control ids
//! (controls.rs) through `controls::apply_as(…, ArmSource::Network)`, the
//! entry point MIDI and the UI use: there is no network-only control path.
//!
//! Safety:
//! - Nothing from the network arms the laser. `transport.arm` is not an
//!   external control and `/ls/arm` is refused by the gate (and logged);
//!   the hold-to-run pad (`safety.hold`) is refused too, since a lost UDP
//!   release would keep the beam on. Blackout and the emergency stop are
//!   accepted at all times: the listener trips the e-stop latch itself,
//!   without waiting for the engine, the queue or the `Shared` lock.
//! - Off by default, listening on 127.0.0.1 by default; the operator picks
//!   another address (a console on the LAN) in RÉGLAGES or with `--osc` /
//!   `--artnet`. Listen only: nothing is ever sent (no ArtPollReply).
//! - Signal loss (Art-Net is a stream): no ArtDmx for `loss.timeout_ms`
//!   applies `loss.policy`: « neutre » (default) puts the chart's channels
//!   back to neutral — intensity 0, size 100 %, centred, rotation stopped,
//!   normal speed — and leaves cues, page and colour as they are; « garder »
//!   keeps the last values; « blackout » latches the emergency stop. The
//!   next packet takes over again with every channel; arming never
//!   changes. OSC sends commands, not a stream, so it has nothing to lose;
//!   pads it holds pressed are released when OSC is switched off.
//!
//! Threads: one listener per protocol (listener.rs) decodes datagrams and
//! pushes them into a bounded queue (dropped when full, counted); the
//! engine thread drains it once per frame (`frame`), applying at most the
//! newest DMX packet and `MAX_EVENTS_PER_FRAME` OSC messages. Neither the
//! engine nor the HTTP worker ever waits on a socket.

pub mod artnet;
pub mod listener;
pub mod osc;

use crate::controls::{self, ControlError, ControlInput, ControlKind};
use crate::cues::ClickMode;
use crate::interlock::{ArmSource, EStop};
use crate::presets::CATEGORIES;
use crate::Shared;
use artnet::{Function, CHART};
use osc::{OscArg, OscCommand, OscMessage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Events waiting for the engine at most; more are dropped (and counted).
pub const QUEUE_LEN: usize = 256;
/// Events the engine takes per frame at most (the rest wait a frame).
pub const MAX_EVENTS_PER_FRAME: usize = 128;
/// OSC messages kept for the monitor.
pub const RECENT_LEN: usize = 20;
pub const DEFAULT_BIND: &str = "127.0.0.1";
pub const DEFAULT_OSC_PORT: u16 = 8000;
pub const LOSS_TIMEOUT_MIN_MS: u64 = 500;
pub const LOSS_TIMEOUT_MAX_MS: u64 = 30_000;
/// Controls the network may never drive, even if they are external.
const NETWORK_REFUSED: [&str; 2] = ["transport.arm", "safety.hold"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    Osc,
    ArtNet,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Osc => "OSC",
            Proto::ArtNet => "Art-Net",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OscConfig {
    pub enabled: bool,
    /// The local address to listen on (an IP of this Mac, 0.0.0.0 for all).
    pub bind: String,
    /// 0: the system picks one (tests); the status shows which.
    pub port: u16,
}

impl Default for OscConfig {
    fn default() -> Self {
        OscConfig { enabled: false, bind: DEFAULT_BIND.into(), port: DEFAULT_OSC_PORT }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtNetConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    /// 15-bit Port-Address (Net, Sub-Net, Universe).
    pub universe: u16,
    /// DMX start address of the chart, 1-based.
    pub address: u16,
}

impl Default for ArtNetConfig {
    fn default() -> Self {
        ArtNetConfig { enabled: false, bind: DEFAULT_BIND.into(), port: artnet::PORT, universe: 0, address: 1 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossPolicy {
    Hold,
    #[default]
    Neutral,
    Blackout,
}

pub const LOSS_POLICIES: [LossPolicy; 3] = [LossPolicy::Neutral, LossPolicy::Hold, LossPolicy::Blackout];

impl LossPolicy {
    pub fn label_fr(self) -> &'static str {
        match self {
            LossPolicy::Hold => "garder les dernières valeurs",
            LossPolicy::Neutral => "revenir au neutre (intensité 0)",
            LossPolicy::Blackout => "blackout (arrêt d'urgence)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LossConfig {
    pub policy: LossPolicy,
    pub timeout_ms: u64,
}

impl Default for LossConfig {
    fn default() -> Self {
        LossConfig { policy: LossPolicy::Neutral, timeout_ms: 2500 }
    }
}

/// `<data-dir>/netin.json`: a setting of this machine, never in a project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetInConfig {
    pub osc: OscConfig,
    pub artnet: ArtNetConfig,
    pub loss: LossConfig,
}

/// A local address to listen on: an IP literal, never a name, never a
/// multicast or broadcast address.
pub fn parse_bind(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let ip: IpAddr = bind.trim().parse().map_err(|_| format!("adresse d'écoute invalide : « {bind} » (une adresse IP de ce Mac, ex. 127.0.0.1)"))?;
    let broadcast = matches!(ip, IpAddr::V4(v4) if v4.is_broadcast());
    if ip.is_multicast() || broadcast {
        return Err(format!("adresse d'écoute invalide : {ip} (multidiffusion ou diffusion)"));
    }
    Ok(SocketAddr::new(ip, port))
}

/// `--osc` / `--artnet` values: `ip:port`, an IP alone, or a port alone
/// (on 127.0.0.1).
pub fn parse_cli_addr(s: &str, default_port: u16) -> Result<SocketAddr, String> {
    if let Ok(port) = s.parse::<u16>() {
        return parse_bind(DEFAULT_BIND, port);
    }
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return parse_bind(&addr.ip().to_string(), addr.port());
    }
    parse_bind(s, default_port)
}

impl NetInConfig {
    pub fn validate(&self) -> Result<(), String> {
        parse_bind(&self.osc.bind, self.osc.port)?;
        parse_bind(&self.artnet.bind, self.artnet.port)?;
        if self.artnet.universe > artnet::MAX_UNIVERSE {
            return Err(format!("univers Art-Net invalide : {} (0 à {})", self.artnet.universe, artnet::MAX_UNIVERSE));
        }
        if !(1..=artnet::MAX_ADDRESS).contains(&self.artnet.address) {
            return Err(format!("adresse DMX invalide : {} (1 à {}, {} canaux)", self.artnet.address, artnet::MAX_ADDRESS, CHART.len()));
        }
        if !(LOSS_TIMEOUT_MIN_MS..=LOSS_TIMEOUT_MAX_MS).contains(&self.loss.timeout_ms) {
            return Err(format!("délai de perte du signal invalide : {} ms ({} à {} ms)", self.loss.timeout_ms, LOSS_TIMEOUT_MIN_MS, LOSS_TIMEOUT_MAX_MS));
        }
        Ok(())
    }

    /// Where a protocol listens, if it is on.
    pub fn listen(&self, proto: Proto) -> Option<SocketAddr> {
        match proto {
            Proto::Osc => self.osc.enabled.then(|| parse_bind(&self.osc.bind, self.osc.port).ok()).flatten(),
            Proto::ArtNet => self.artnet.enabled.then(|| parse_bind(&self.artnet.bind, self.artnet.port).ok()).flatten(),
        }
    }
}

/// `--osc` / `--artnet`: on for this run, at that address; not saved.
#[derive(Clone, Copy, Debug, Default)]
pub struct CliOverride {
    pub osc: Option<SocketAddr>,
    pub artnet: Option<SocketAddr>,
}

/// One listener's state, for the UI.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ListenStatus {
    /// Bound address (with the real port when 0 was asked).
    pub local: Option<String>,
    pub error: Option<String>,
    /// Packets accepted.
    pub packets: u64,
    /// Not OSC / not Art-Net, or malformed.
    pub rejected: u64,
    /// Lost because the engine's queue was full.
    pub dropped: u64,
    /// Art-Net only: packets for another universe.
    pub ignored: u64,
    pub last_from: Option<String>,
    #[serde(skip)]
    pub last_at: Option<Instant>,
}

struct HubInner {
    file: NetInConfig,
    cli: CliOverride,
    path: Option<PathBuf>,
    osc: ListenStatus,
    artnet: ListenStatus,
}

/// Shared with the listener threads (its own short lock, never the
/// `Shared` one): the configuration, their status, and the e-stop latch
/// for the blackout fast path.
pub struct NetHub {
    inner: Mutex<HubInner>,
    pub estop: Arc<EStop>,
}

impl NetHub {
    pub fn load(path: PathBuf, cli: CliOverride, estop: Arc<EStop>) -> Self {
        let mut file: NetInConfig = crate::load_json(&path);
        if let Err(e) = file.validate() {
            log::warn!("netin.json : {e} ; entrées réseau par défaut (coupées)");
            file = NetInConfig::default();
        }
        Self::with(file, cli, Some(path), estop)
    }

    pub fn in_memory(estop: Arc<EStop>) -> Self {
        Self::with(NetInConfig::default(), CliOverride::default(), None, estop)
    }

    fn with(file: NetInConfig, cli: CliOverride, path: Option<PathBuf>, estop: Arc<EStop>) -> Self {
        let inner = HubInner { file, cli, path, osc: ListenStatus::default(), artnet: ListenStatus::default() };
        NetHub { inner: Mutex::new(inner), estop }
    }

    fn lock(&self) -> MutexGuard<'_, HubInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What is saved (and what the UI edits).
    pub fn file(&self) -> NetInConfig {
        self.lock().file.clone()
    }

    pub fn cli(&self) -> CliOverride {
        self.lock().cli
    }

    /// The saved settings with the command line on top.
    pub fn effective(&self) -> NetInConfig {
        let inner = self.lock();
        let mut c = inner.file.clone();
        if let Some(a) = inner.cli.osc {
            (c.osc.enabled, c.osc.bind, c.osc.port) = (true, a.ip().to_string(), a.port());
        }
        if let Some(a) = inner.cli.artnet {
            (c.artnet.enabled, c.artnet.bind, c.artnet.port) = (true, a.ip().to_string(), a.port());
        }
        c
    }

    /// Validates, keeps and saves new settings.
    pub fn set_file(&self, cfg: NetInConfig) -> Result<(), String> {
        self.store(cfg)?;
        self.save();
        Ok(())
    }

    /// Validates and keeps new settings (the listeners follow at once);
    /// `save` writes them, out of the `Shared` lock.
    pub fn store(&self, cfg: NetInConfig) -> Result<(), String> {
        cfg.validate()?;
        self.lock().file = cfg;
        Ok(())
    }

    /// Writes `netin.json` (nothing in memory-only tests).
    pub fn save(&self) {
        let (path, cfg) = {
            let inner = self.lock();
            (inner.path.clone(), inner.file.clone())
        };
        if let Some(path) = path {
            crate::save_json(&path, &cfg);
        }
    }

    pub fn status(&self, proto: Proto) -> ListenStatus {
        let inner = self.lock();
        match proto {
            Proto::Osc => inner.osc.clone(),
            Proto::ArtNet => inner.artnet.clone(),
        }
    }

    pub fn update_status(&self, proto: Proto, f: impl FnOnce(&mut ListenStatus)) {
        let mut inner = self.lock();
        f(match proto {
            Proto::Osc => &mut inner.osc,
            Proto::ArtNet => &mut inner.artnet,
        });
    }
}

/// What a listener hands to the engine.
#[derive(Debug)]
pub enum NetEvent {
    Osc { from: IpAddr, msg: OscMessage, cmd: Result<OscCommand, String> },
    /// The chart's channels from one ArtDmx of our universe.
    Dmx { from: IpAddr, values: [Option<u8>; CHART.len()] },
}

/// One line of the OSC monitor.
#[derive(Clone, Debug)]
struct OscLog {
    at: Instant,
    from: IpAddr,
    address: String,
    args: Vec<OscArg>,
    /// `None`: done; else why not (French).
    error: Option<String>,
}

#[derive(Debug, Default)]
struct DmxState {
    /// Last chart values applied; `None` until the first packet.
    last: Option<[Option<u8>; CHART.len()]>,
    at: Option<Instant>,
    from: Option<IpAddr>,
    lost: bool,
}

/// `Shared.netin`: never saved in a look or a project.
pub struct NetInState {
    pub hub: Arc<NetHub>,
    tx: SyncSender<NetEvent>,
    rx: Receiver<NetEvent>,
    recent: VecDeque<OscLog>,
    dmx: DmxState,
    /// Momentary controls an OSC sender holds pressed.
    held: HashSet<String>,
}

impl NetInState {
    pub fn new(hub: NetHub) -> Self {
        let (tx, rx) = sync_channel(QUEUE_LEN);
        NetInState { hub: Arc::new(hub), tx, rx, recent: VecDeque::new(), dmx: DmxState::default(), held: HashSet::new() }
    }

    /// For the listener threads.
    pub fn sender(&self) -> SyncSender<NetEvent> {
        self.tx.clone()
    }
}

/// Once per engine frame: apply what arrived, then watch for signal loss.
pub fn frame(s: &mut Shared, now: Instant) {
    let cfg = s.netin.hub.effective();
    let mut dmx = None;
    for _ in 0..MAX_EVENTS_PER_FRAME {
        match s.netin.rx.try_recv() {
            Ok(NetEvent::Osc { from, msg, cmd }) if cfg.osc.enabled => osc_message(s, from, msg, cmd, now),
            // Only the newest packet counts: each one carries every channel.
            Ok(NetEvent::Dmx { from, values }) if cfg.artnet.enabled => dmx = Some((from, values)),
            Ok(_) => {} // arrived just before it was switched off
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    if let Some((from, values)) = dmx {
        dmx_packet(s, from, values, now);
    }
    if !cfg.artnet.enabled {
        // Switched off by hand: nothing is lost, values stay as they are.
        s.netin.dmx = DmxState::default();
    } else if let Some(at) = s.netin.dmx.at {
        if !s.netin.dmx.lost && now.saturating_duration_since(at) > Duration::from_millis(cfg.loss.timeout_ms) {
            signal_lost(s, cfg.loss.policy);
        }
    }
    if !cfg.osc.enabled && !s.netin.held.is_empty() {
        release_held(s);
    }
}

/// Every network write goes through here: an external caller with the
/// network as source, minus what the network may never touch.
fn send(s: &mut Shared, id: &str, input: ControlInput) -> Result<(), String> {
    let desc = s.controls.get(id).ok_or_else(|| format!("contrôle inconnu : {id}"))?;
    if !desc.external || NETWORK_REFUSED.contains(&desc.id.as_str()) {
        return Err(format!("« {} » ne se pilote pas depuis le réseau", desc.id));
    }
    controls::apply_as(s, id, input, ArmSource::Network).map_err(|e| match e {
        ControlError::Unknown(id) => format!("contrôle inconnu : {id}"),
        ControlError::Refused(why) => why.to_string(),
    })
}

fn osc_message(s: &mut Shared, from: IpAddr, msg: OscMessage, cmd: Result<OscCommand, String>, now: Instant) {
    let error = cmd.and_then(|c| osc_command(s, c)).err();
    if let Some(e) = &error {
        log::debug!("OSC {} : {e}", msg.address);
    }
    let recent = &mut s.netin.recent;
    if recent.len() >= RECENT_LEN {
        recent.pop_front();
    }
    recent.push_back(OscLog { at: now, from, address: msg.address, args: msg.args, error });
}

fn osc_command(s: &mut Shared, cmd: OscCommand) -> Result<(), String> {
    match cmd {
        OscCommand::Control { id, input } => {
            let desc = s.controls.get(&id).ok_or_else(|| format!("contrôle inconnu : {id}"))?;
            let canonical = desc.id.clone();
            let momentary = desc.kind == ControlKind::Momentary;
            send(s, &id, input)?;
            if momentary {
                let pressed = matches!(input, ControlInput::Value(v) | ControlInput::Norm(v) if v >= 0.5);
                if pressed {
                    s.netin.held.insert(canonical);
                } else {
                    s.netin.held.remove(&canonical);
                }
            }
            Ok(())
        }
        OscCommand::PlayCue(id) => {
            if controls::press_cue_from(s, &id, Some(ClickMode::Restart), true, Some(ArmSource::Network.id())) {
                Ok(())
            } else {
                Err(format!("cue inconnu : {id}"))
            }
        }
        OscCommand::StopCues => send(s, "cue.stop_all", ControlInput::Value(1.0)),
        OscCommand::Blackout => {
            s.emergency_stop(ArmSource::Network);
            Ok(())
        }
        OscCommand::Arm => {
            // Through the gate, which refuses the network and logs it.
            let _ = s.gate.request_arm(ArmSource::Network);
            Err("refusé : l'armement ne se fait jamais depuis le réseau".into())
        }
        OscCommand::Ping => Ok(()),
    }
}

/// OSC was switched off: the pads it held come up.
fn release_held(s: &mut Shared) {
    for id in std::mem::take(&mut s.netin.held) {
        let _ = send(s, &id, ControlInput::Value(0.0));
    }
}

/// One ArtDmx (the newest of the frame). After the first packet, or after
/// a loss, every channel is applied; otherwise only those that changed, so
/// a console holding still doesn't fight the UI 40 times a second.
fn dmx_packet(s: &mut Shared, from: IpAddr, values: [Option<u8>; CHART.len()], now: Instant) {
    let d = &mut s.netin.dmx;
    let fresh = d.last.is_none() || d.lost;
    if d.lost {
        log::info!("Art-Net : signal retrouvé ({from})");
        s.gate.log().record("input_back", Some(ArmSource::Network.id()), json!({ "protocol": "artnet", "protocol_fr": "Art-Net" }));
    }
    let prev = if fresh { [None; CHART.len()] } else { d.last.unwrap_or([None; CHART.len()]) };
    (d.last, d.at, d.from, d.lost) = (Some(values), Some(now), Some(from), false);

    let mut cue_changed = false;
    for ch in CHART {
        let Some(v) = values[ch.offset] else { continue };
        if prev[ch.offset] == Some(v) {
            continue;
        }
        match ch.function {
            Function::Control { id, centred, .. } => {
                if let Err(e) = send(s, id, artnet::input(v, centred)) {
                    log::debug!("Art-Net {id} : {e}");
                }
            }
            Function::Page => {
                s.cue_page = artnet::page(v, CATEGORIES.len());
                cue_changed = true;
            }
            Function::Cue => cue_changed = true,
            // The listener has already tripped it; same rule here.
            Function::Blackout => {
                let was_on = prev[ch.offset].is_some_and(artnet::blackout_on);
                if artnet::blackout_on(v) && !was_on {
                    s.emergency_stop(ArmSource::Network);
                }
            }
        }
    }
    if cue_changed {
        dmx_cue(s, values[2], fresh);
    }
}

/// Channel 3: 0 stops the cues (not on the first packet: a console that
/// connects with it at 0 leaves the operator's cues alone), 1–40 plays
/// that cell of the current page from the start.
fn dmx_cue(s: &mut Shared, value: Option<u8>, fresh: bool) {
    let cells = controls::GRID_ROWS * controls::GRID_COLS;
    match value {
        Some(0) if !fresh => {
            let _ = send(s, "cue.stop_all", ControlInput::Value(1.0));
        }
        Some(v) if (1..=cells).contains(&(v as usize)) => {
            let i = v as usize - 1;
            let cell = format!("{}.{}.{}", s.cue_page + 1, i / controls::GRID_COLS + 1, i % controls::GRID_COLS + 1);
            if let Some(id) = controls::grid_cell_preset(&s.presets, &cell) {
                controls::press_cue_from(s, &id, Some(ClickMode::Restart), true, Some(ArmSource::Network.id()));
            }
        }
        _ => {}
    }
}

fn signal_lost(s: &mut Shared, policy: LossPolicy) {
    s.netin.dmx.lost = true;
    log::warn!("Art-Net : signal perdu, {}", policy.label_fr());
    s.gate.log().record(
        "input_lost",
        Some(ArmSource::Network.id()),
        json!({ "protocol": "artnet", "protocol_fr": "Art-Net", "policy": policy, "policy_fr": policy.label_fr() }),
    );
    match policy {
        LossPolicy::Hold => {}
        LossPolicy::Neutral => {
            for ch in CHART {
                if let Function::Control { id, neutral: Some(v), .. } = ch.function {
                    let _ = send(s, id, ControlInput::Value(v));
                }
            }
        }
        LossPolicy::Blackout => s.emergency_stop(ArmSource::Network),
    }
}

/// « off », « waiting » (on, nothing yet), « ok » or « lost ».
fn signal(s: &Shared, enabled: bool) -> &'static str {
    match (&s.netin.dmx, enabled) {
        (_, false) => "off",
        (d, _) if d.lost => "lost",
        (d, _) if d.at.is_some() => "ok",
        _ => "waiting",
    }
}

fn status_json(st: &ListenStatus, now: Instant) -> Value {
    let mut v = json!(st);
    v["activity_ms"] = json!(st.last_at.map(|t| now.saturating_duration_since(t).as_millis() as u64));
    v
}

/// `GET /api/netin`.
pub fn view(s: &Shared, now: Instant) -> Value {
    let (file, eff, cli) = (s.netin.hub.file(), s.netin.hub.effective(), s.netin.hub.cli());
    let recent: Vec<Value> = s
        .netin
        .recent
        .iter()
        .rev()
        .map(|l| json!({ "from": l.from.to_string(), "address": l.address, "args": l.args, "error": l.error, "age_ms": now.saturating_duration_since(l.at).as_millis() as u64 }))
        .collect();
    let values = s.netin.dmx.last.unwrap_or([None; CHART.len()]);
    let channels: Vec<Value> = CHART
        .iter()
        .map(|c| json!({ "channel": eff.artnet.address as usize + c.offset, "label": c.label_fr, "value": values[c.offset] }))
        .collect();
    json!({
        "config": file,
        "effective": eff,
        "cli": { "osc": cli.osc.is_some(), "artnet": cli.artnet.is_some() },
        "osc": { "status": status_json(&s.netin.hub.status(Proto::Osc), now), "recent": recent },
        "artnet": {
            "status": status_json(&s.netin.hub.status(Proto::ArtNet), now),
            "signal": signal(s, eff.artnet.enabled),
            "source": s.netin.dmx.from.map(|ip| ip.to_string()),
            "channels": channels,
        },
        "policies": LOSS_POLICIES.iter().map(|p| json!({ "id": p, "label": p.label_fr() })).collect::<Vec<_>>(),
    })
}

/// `POST /api/netin`: a partial update (`{"osc": {"enabled": true}}`);
/// missing fields keep their value. Returns the names of what changed;
/// the caller saves (`NetHub::save`) once the `Shared` lock is released.
pub fn update(s: &mut Shared, patch: &Value) -> Result<Vec<String>, String> {
    let Value::Object(patch) = patch else { return Err("objet JSON attendu".into()) };
    let before = s.netin.hub.file();
    let mut merged = json!(before);
    for (section, fields) in patch {
        let (Some(Value::Object(target)), Value::Object(fields)) = (merged.get_mut(section), fields) else {
            return Err(format!("réglage inconnu : {section}"));
        };
        for (k, v) in fields {
            if !target.contains_key(k) {
                return Err(format!("réglage inconnu : {section}.{k}"));
            }
            target.insert(k.clone(), v.clone());
        }
    }
    let cfg: NetInConfig = serde_json::from_value(merged).map_err(|e| format!("réglage invalide : {e}"))?;
    let changed = changed_keys(&json!(before), &json!(cfg));
    if changed.is_empty() {
        cfg.validate()?;
        return Ok(changed);
    }
    s.netin.hub.store(cfg.clone())?;
    s.gate.log().record("netin_settings", Some(ArmSource::Ui.id()), json!({ "changed": changed, "before": before, "after": cfg }));
    Ok(changed)
}

fn changed_keys(a: &Value, b: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let (Value::Object(a), Value::Object(b)) = (a, b) {
        for (section, av) in a {
            if let (Value::Object(af), Some(Value::Object(bf))) = (av, b.get(section)) {
                out.extend(af.iter().filter(|(k, v)| bf.get(*k) != Some(v)).map(|(k, _)| format!("{section}.{k}")));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;

    fn ip() -> IpAddr {
        "127.0.0.1".parse().unwrap()
    }

    /// A studio whose inputs are both on (in memory, nothing bound).
    fn studio() -> Shared {
        let mut s = shared();
        s.netin = NetInState::new(NetHub::in_memory(Arc::clone(&s.estop)));
        let mut cfg = NetInConfig::default();
        (cfg.osc.enabled, cfg.artnet.enabled) = (true, true);
        s.netin.hub.set_file(cfg).unwrap();
        s
    }

    fn osc(s: &mut Shared, address: &str, args: &[OscArg]) {
        let msg = OscMessage { address: address.into(), args: args.to_vec() };
        let cmd = osc::command(&msg);
        s.netin.sender().try_send(NetEvent::Osc { from: ip(), msg, cmd }).unwrap();
        frame(s, Instant::now());
    }

    fn last_error(s: &Shared) -> Option<String> {
        s.netin.recent.back().unwrap().error.clone()
    }

    fn dmx_at(s: &mut Shared, values: &[u8], at: Instant) {
        let values = std::array::from_fn(|i| values.get(i).copied());
        s.netin.sender().try_send(NetEvent::Dmx { from: ip(), values }).unwrap();
        frame(s, at);
    }

    /// Chart values: intensity, page, cue, size, x, y, rotation, speed, mode, hue, blackout.
    const NEUTRAL: [u8; 11] = [255, 0, 0, 128, 128, 128, 128, 64, 0, 0, 0];

    #[test]
    fn an_osc_message_plays_a_cue() {
        let mut s = studio();
        let cue = s.presets[0].id.clone();
        osc(&mut s, "/ls/cue/play", &[OscArg::Str(cue.clone())]);
        assert_eq!(last_error(&s), None);
        assert_eq!(s.active_cue.as_deref(), Some(cue.as_str()));
        osc(&mut s, "/ls/cue/stop", &[]);
        assert!(s.deck.active.is_empty());
        osc(&mut s, "/ls/cue/play", &[OscArg::Str("nope".into())]);
        assert_eq!(last_error(&s).unwrap(), "cue inconnu : nope");
        // A grid cell as a control id (the MIDI path).
        osc(&mut s, "/ls/control/grid/1/1/1", &[OscArg::Int(1)]);
        assert!(!s.deck.active.is_empty() && s.netin.held.contains("grid.1.1.1"));
        osc(&mut s, "/ls/control/grid.1.1.1", &[OscArg::Int(0)]);
        assert!(s.netin.held.is_empty());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn osc_drives_controls_in_native_or_normalised_units() {
        let mut s = studio();
        osc(&mut s, "/ls/control/master/brightness", &[OscArg::Float(0.25)]);
        assert_eq!(s.live.brightness, 0.25);
        osc(&mut s, "/ls/norm/master.size", &[OscArg::Float(1.0)]);
        assert_eq!(s.live.size, 2.0);
        osc(&mut s, "/ls/control/live.size", &[OscArg::Float(5.0)]);
        assert_eq!(s.live.size, 2.0, "clamped, alias accepted");
        osc(&mut s, "/ls/control/no.such", &[OscArg::Float(1.0)]);
        assert!(last_error(&s).unwrap().contains("inconnu"));
        assert_eq!(s.netin.recent.len(), 4);
    }

    #[test]
    fn a_dmx_channel_changes_the_brightness() {
        let mut s = studio();
        let t0 = Instant::now();
        dmx_at(&mut s, &NEUTRAL, t0);
        assert_eq!(s.live.brightness, 1.0);
        assert_eq!((s.live.size, s.live.pos_x, s.live.pos_y, s.live.rot_speed[2]), (1.0, 0.0, 0.0, 0.0), "128 is the true centre");
        let mut v = NEUTRAL;
        v[0] = 51;
        dmx_at(&mut s, &v, t0);
        assert!((s.live.brightness - 0.2).abs() < 1e-6);
        // A value the console holds doesn't fight the UI: only changes apply.
        s.live.brightness = 0.7;
        v[3] = 255;
        dmx_at(&mut s, &v, t0);
        assert_eq!((s.live.brightness, s.live.size), (0.7, 2.0));
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn dmx_start_address_universe_and_cue_channel() {
        let mut s = studio();
        let t0 = Instant::now();
        // Page 1, cell 3, then 0 stops it.
        let mut v = NEUTRAL;
        v[2] = 3;
        dmx_at(&mut s, &v, t0);
        let want = controls::grid_cell_preset(&s.presets, "1.1.3").unwrap();
        assert_eq!(s.active_cue.as_deref(), Some(want.as_str()));
        v[1] = 255; // last page
        dmx_at(&mut s, &v, t0);
        assert_eq!(s.cue_page, CATEGORIES.len() - 1);
        v[2] = 0;
        dmx_at(&mut s, &v, t0);
        assert!(s.deck.active.is_empty());
        // First packet with the cue channel at 0: the operator's cues stay.
        let mut s = studio();
        let cue = s.presets[0].id.clone();
        controls::press_cue(&mut s, &cue, None, true);
        dmx_at(&mut s, &NEUTRAL, t0);
        assert_eq!(s.deck.active.len(), 1);
    }

    #[test]
    fn signal_loss_goes_neutral_then_recovers_without_arming() {
        let mut s = studio();
        let t0 = Instant::now();
        let mut v = NEUTRAL;
        (v[3], v[4], v[7]) = (200, 10, 200);
        dmx_at(&mut s, &v, t0);
        let cue = s.presets[0].id.clone();
        controls::press_cue(&mut s, &cue, None, true);
        frame(&mut s, t0 + Duration::from_millis(2400));
        assert_eq!(s.live.brightness, 1.0, "not before the timeout");
        frame(&mut s, t0 + Duration::from_millis(2600));
        assert_eq!((s.live.brightness, s.live.size, s.live.pos_x, s.live.speed), (0.0, 1.0, 0.0, 1.0));
        assert_eq!(s.deck.active.len(), 1, "cues are left alone");
        assert_eq!(signal(&s, true), "lost");
        assert!(!s.estop.is_latched() && !s.gate.is_armed());
        // The console comes back: every channel again, even unchanged ones.
        dmx_at(&mut s, &v, t0 + Duration::from_secs(5));
        assert_eq!(s.live.brightness, 1.0);
        assert!(s.live.size > 1.5);
        assert_eq!(signal(&s, true), "ok");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn loss_policies_hold_and_blackout() {
        for (policy, dark, latched) in [(LossPolicy::Hold, false, false), (LossPolicy::Blackout, false, true)] {
            let mut s = studio();
            let mut cfg = s.netin.hub.file();
            cfg.loss.policy = policy;
            s.netin.hub.set_file(cfg).unwrap();
            let t0 = Instant::now();
            dmx_at(&mut s, &NEUTRAL, t0);
            frame(&mut s, t0 + Duration::from_secs(3));
            assert_eq!(s.live.brightness == 0.0, dark, "{policy:?}");
            assert_eq!(s.estop.is_latched(), latched, "{policy:?}");
        }
        // Switched off by hand: no policy, nothing lost.
        let mut s = studio();
        let t0 = Instant::now();
        dmx_at(&mut s, &NEUTRAL, t0);
        update(&mut s, &json!({ "artnet": { "enabled": false } })).unwrap();
        frame(&mut s, t0 + Duration::from_secs(10));
        assert_eq!(s.live.brightness, 1.0);
        assert_eq!(signal(&s, false), "off");
    }

    #[test]
    fn blackout_is_accepted_from_both_on_the_way_up() {
        let mut s = studio();
        osc(&mut s, "/ls/blackout", &[]);
        assert_eq!(s.gate.status(&s.estop).estop.unwrap().source, ArmSource::Network);
        let mut s = studio();
        let t0 = Instant::now();
        dmx_at(&mut s, &NEUTRAL, t0);
        assert!(!s.estop.is_latched());
        let mut v = NEUTRAL;
        v[10] = 255;
        dmx_at(&mut s, &v, t0);
        assert!(s.estop.is_latched());
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        assert!(!s.estop.is_latched());
        dmx_at(&mut s, &v, t0);
        assert!(!s.estop.is_latched(), "held high: no new edge after a reset");
    }

    #[test]
    fn nothing_from_the_network_ever_arms() {
        let mut s = studio();
        osc(&mut s, "/ls/arm", &[]);
        assert!(last_error(&s).unwrap().contains("armement"));
        for addr in ["/ls/control/transport.arm", "/ls/norm/transport.arm", "/ls/control/safety.hold"] {
            osc(&mut s, addr, &[OscArg::Float(1.0)]);
            assert!(last_error(&s).is_some(), "{addr}");
        }
        assert!(!s.presence.status().midi_hold, "no hold-to-run over UDP");
        // Every control id, every value, both protocols. The stops are left
        // out so the gate stays armable: the UI can still arm at the end.
        let ids: Vec<String> = s.controls.list().iter().map(|d| d.id.clone()).filter(|id| id != "transport.blackout" && id != "safety.estop").collect();
        let t0 = Instant::now();
        for id in &ids {
            for v in [0.0, 0.5, 1.0, 1e6] {
                osc(&mut s, &format!("/ls/control/{id}"), &[OscArg::Float(v)]);
                osc(&mut s, &format!("/ls/norm/{id}"), &[OscArg::Float(v)]);
                assert!(!s.gate.is_armed(), "{id} {v}");
            }
        }
        for b in 0..128u8 {
            dmx_at(&mut s, &[b; 11], t0);
            assert!(!s.gate.is_armed());
        }
        assert!(!s.estop.is_latched());
        assert!(s.request_arm(ArmSource::Ui).is_ok(), "the gate was armable all along");
        for b in 128..=255u8 {
            dmx_at(&mut s, &[b; 11], t0);
        }
        assert!(!s.gate.is_armed(), "the blackout channel disarmed it");
    }

    #[test]
    fn switching_osc_off_releases_held_pads() {
        let mut s = studio();
        osc(&mut s, "/ls/control/master.rot.reverse", &[OscArg::Int(1)]);
        assert!(s.live.rot_reverse);
        update(&mut s, &json!({ "osc": { "enabled": false } })).unwrap();
        frame(&mut s, Instant::now());
        assert!(!s.live.rot_reverse && s.netin.held.is_empty());
        // Messages that arrive while off are dropped.
        osc(&mut s, "/ls/control/master.brightness", &[OscArg::Float(0.1)]);
        assert_eq!(s.live.brightness, 1.0);
    }

    #[test]
    fn the_queue_is_bounded_and_the_frame_takes_a_bounded_share() {
        let mut s = studio();
        let tx = s.netin.sender();
        let mut sent = 0;
        while tx.try_send(NetEvent::Dmx { from: ip(), values: [Some(1); 11] }).is_ok() {
            sent += 1;
        }
        assert_eq!(sent, QUEUE_LEN);
        frame(&mut s, Instant::now());
        assert_eq!(s.netin.rx.try_iter().count(), QUEUE_LEN - MAX_EVENTS_PER_FRAME);
    }

    #[test]
    fn settings_validate_merge_and_log() {
        let mut s = studio();
        assert_eq!(update(&mut s, &json!({ "artnet": { "universe": 3, "address": 100 } })).unwrap(), ["artnet.address", "artnet.universe"]);
        assert_eq!(s.netin.hub.file().artnet.universe, 3);
        assert_eq!(update(&mut s, &json!({ "artnet": { "universe": 3 } })).unwrap(), Vec::<String>::new());
        for bad in [
            json!({ "osc": { "bind": "example.com" } }),
            json!({ "osc": { "bind": "224.0.0.1" } }),
            json!({ "artnet": { "bind": "255.255.255.255" } }),
            json!({ "artnet": { "address": 0 } }),
            json!({ "artnet": { "address": 503 } }),
            json!({ "artnet": { "universe": 40000 } }),
            json!({ "loss": { "timeout_ms": 10 } }),
            json!({ "loss": { "policy": "explode" } }),
            json!({ "osc": { "colour": 1 } }),
            json!({ "midi": {} }),
            json!([1]),
        ] {
            assert!(update(&mut s, &bad).is_err(), "{bad}");
        }
        assert_eq!(s.netin.hub.file().artnet.address, 100, "a refused update changes nothing");
    }

    #[test]
    fn defaults_are_off_on_localhost_and_cli_overrides_are_not_saved() {
        let c = NetInConfig::default();
        assert!(!c.osc.enabled && !c.artnet.enabled);
        assert_eq!((c.osc.bind.as_str(), c.artnet.bind.as_str(), c.artnet.port), ("127.0.0.1", "127.0.0.1", 6454));
        assert_eq!(c.listen(Proto::Osc), None);
        let dir = crate::datafile::tests::temp_dir("netin-cli");
        let path = dir.join("netin.json");
        let cli = CliOverride { osc: Some(parse_cli_addr("9001", 8000).unwrap()), artnet: None };
        let hub = NetHub::load(path.clone(), cli, Arc::default());
        assert_eq!(hub.effective().listen(Proto::Osc), Some("127.0.0.1:9001".parse().unwrap()));
        assert!(!hub.file().osc.enabled);
        hub.set_file(hub.file()).unwrap();
        let saved: NetInConfig = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!saved.osc.enabled, "the command line is for this run only");
        assert_eq!(parse_cli_addr("0.0.0.0:7000", 1).unwrap(), "0.0.0.0:7000".parse().unwrap());
        assert_eq!(parse_cli_addr("10.0.0.2", 6454).unwrap(), "10.0.0.2:6454".parse().unwrap());
        assert!(parse_cli_addr("laser.local", 1).is_err());
        // A damaged or invalid file: off.
        std::fs::write(&path, r#"{"osc":{"enabled":true,"bind":"nope"}}"#).unwrap();
        assert!(!NetHub::load(path, CliOverride::default(), Arc::default()).effective().osc.enabled);
    }
}
