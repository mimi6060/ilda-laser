//! OSC feedback (T-416): the value of each external control sent back to
//! the clients that asked for it, so a touch surface or a console shows
//! what the studio really does.
//!
//! - One address per control the network may drive: the registry id under
//!   `/ls`, `/` for `.` (`master.size` → `/ls/master/size`). It is the
//!   address the OSC input accepts too. Arming, the hold-to-run pad, the
//!   e-stop reset and the safety settings have none (they are not
//!   external, or the gate never lets the network drive them); blackout
//!   and the e-stop do (stopping is always allowed).
//! - Off by default. Destinations are only those the operator typed (IP
//!   literal and port: no name lookup, no discovery, no scanning, never a
//!   broadcast or multicast address) and the clients that sent
//!   `/ls/subscribe <port>` (their own address; forgotten after 60 s of
//!   silence). Sent from the interface the OSC input listens on.
//! - Only control values go out: no file paths, no project content.
//! - Each tick (at most `max_rate_hz`) compares the base values (what the
//!   screen, MIDI, DMX or OSC set; never the LFOs' output) with what each
//!   destination was last sent, and sends what changed. A value a client
//!   just sent is recorded as sent to it (no echo back), and a value equal
//!   to the current one changes nothing (`same`).
//! - The engine never waits on the network: it `try_send`s into a bounded
//!   queue that the "osc-out" thread empties; a full queue drops (counted,
//!   and the value goes out at a later tick since it is not recorded).

use super::monitor::{self, Monitor};
use super::osc::{self, OscArg};
use super::{NetHub, NetInConfig};
use crate::controls::{self, ControlDesc, ControlInput, ControlKind, ControlRegistry, Unit};
use crate::external;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const PREFIX: &str = "/ls/";
/// `/ls/tempo/beat i n` on every beat (n = 1..4 in the bar).
pub const BEAT_ADDRESS: &str = "/ls/tempo/beat";
/// A subscriber silent this long is forgotten.
pub const SUBSCRIBER_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_TARGETS: usize = 8;
pub const MAX_SUBSCRIBERS: usize = 16;
pub const MIN_RATE_HZ: f32 = 1.0;
pub const MAX_RATE_HZ: f32 = 60.0;
/// Datagrams waiting for the "osc-out" thread at most.
pub const QUEUE_LEN: usize = 1024;
/// Messages queued per destination and tick at most (the full state of a
/// new subscriber goes out over a few ticks).
pub const MAX_PER_TICK: usize = 128;
/// Longest wait of the sending thread, so a shutdown is seen.
const POLL: Duration = Duration::from_millis(100);

/// In `<data-dir>/netin.json` (section `feedback`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OscFeedback {
    pub enabled: bool,
    /// Fixed destinations, `ip:port` (`192.168.1.20:9000`).
    pub targets: Vec<String>,
    /// Accept `/ls/subscribe <port>` from clients.
    pub allow_subscribe: bool,
    /// Messages per second and per control at most.
    pub max_rate_hz: f32,
    /// `/ls/tempo/beat` on every beat.
    pub beat_messages: bool,
}

impl Default for OscFeedback {
    fn default() -> Self {
        OscFeedback { enabled: false, targets: Vec::new(), allow_subscribe: true, max_rate_hz: 30.0, beat_messages: true }
    }
}

impl OscFeedback {
    pub fn validate(&self) -> Result<(), String> {
        if self.targets.len() > MAX_TARGETS {
            return Err(format!("trop de destinations de retour OSC ({MAX_TARGETS} au plus)"));
        }
        for t in &self.targets {
            parse_target(t)?;
        }
        if !(MIN_RATE_HZ..=MAX_RATE_HZ).contains(&self.max_rate_hz) {
            return Err(format!("débit du retour OSC invalide : {} (de {MIN_RATE_HZ} à {MAX_RATE_HZ} messages par seconde)", self.max_rate_hz));
        }
        Ok(())
    }
}

/// A destination the operator typed: `ip:port`, never a name.
pub fn parse_target(s: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = s.trim().parse().map_err(|_| format!("destination invalide : « {s} » (adresse IP:port, ex. 192.168.1.20:9000)"))?;
    check_dest(addr)?;
    Ok(addr)
}

/// One host only: no port 0, no broadcast (255.255.255.255 or a `.255`
/// directed broadcast), no multicast, no 0.0.0.0.
fn check_dest(addr: SocketAddr) -> Result<(), String> {
    if addr.port() == 0 {
        return Err(format!("destination invalide : {addr} (port 0)"));
    }
    let ip = addr.ip();
    let broadcast = matches!(ip, IpAddr::V4(v4) if v4.is_broadcast() || v4.octets()[3] == 255);
    if ip.is_unspecified() || ip.is_multicast() || broadcast {
        return Err(format!("destination invalide : {ip} (diffusion, multidiffusion ou adresse vide : une seule machine)"));
    }
    Ok(())
}

/// `master.size` → `/ls/master/size`.
pub fn address_of(control_id: &str) -> String {
    format!("{PREFIX}{}", control_id.replace('.', "/"))
}

/// `/ls/master/size` → `master.size`; `None` outside `/ls`.
pub fn control_of(address: &str) -> Option<String> {
    let rest = address.strip_prefix(PREFIX)?;
    (!rest.is_empty()).then(|| rest.replace('/', "."))
}

/// Whether a control has an address: the network may drive it (external,
/// in a scope group, not refused by the gate), or it stops (blackout,
/// e-stop).
pub fn has_address(d: &ControlDesc) -> bool {
    d.external && (!external::forbidden(d) || external::ALWAYS.contains(&d.id.as_str()))
}

#[derive(Clone, Debug, Serialize)]
pub struct OscAddress {
    pub address: String,
    pub control: String,
    pub label_fr: String,
    /// continuous, toggle, momentary, trigger or choice.
    pub kind: &'static str,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub unit: Option<Unit>,
}

fn kind_id(kind: &ControlKind) -> &'static str {
    match kind {
        ControlKind::Continuous { .. } => "continuous",
        ControlKind::Toggle { .. } => "toggle",
        ControlKind::Momentary => "momentary",
        ControlKind::Trigger => "trigger",
        ControlKind::Choice { .. } => "choice",
    }
}

/// `GET /api/osc/addresses`: generated from the registry.
pub fn addresses(reg: &ControlRegistry) -> Vec<OscAddress> {
    reg.list()
        .iter()
        .filter(|d| has_address(d))
        .map(|d| {
            let (min, max, unit) = match &d.kind {
                ControlKind::Continuous { min, max, unit, .. } => (Some(*min), Some(*max), Some(*unit)),
                ControlKind::Toggle { .. } | ControlKind::Momentary => (Some(0.0), Some(1.0), None),
                ControlKind::Choice { options, .. } => (Some(0.0), Some(options.len().saturating_sub(1) as f32), None),
                ControlKind::Trigger => (None, None, None),
            };
            OscAddress { address: address_of(&d.id), control: d.id.clone(), label_fr: d.label_fr.clone(), kind: kind_id(&d.kind), min, max, unit }
        })
        .collect()
}

/// A control's base value as a number (bool: 0/1); triggers have none.
pub fn value_of(s: &Shared, d: &ControlDesc) -> Option<f32> {
    match controls::current(s, d)? {
        Value::Bool(b) => Some(if b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64().map(|v| v as f32).filter(|v| v.is_finite()),
        _ => None,
    }
}

/// What an input would set, in native units (`controls::apply`'s rules);
/// `None` for pads and triggers, which have no value to compare.
pub fn native(kind: &ControlKind, input: ControlInput) -> Option<f32> {
    let raw = match input {
        ControlInput::Value(v) | ControlInput::Norm(v) => v,
    };
    match kind {
        ControlKind::Continuous { min, max, .. } => Some(match input {
            ControlInput::Value(v) => v.clamp(*min, *max),
            ControlInput::Norm(n) => min + (max - min) * n.clamp(0.0, 1.0),
        }),
        ControlKind::Toggle { .. } => Some(if raw >= 0.5 { 1.0 } else { 0.0 }),
        ControlKind::Choice { options, .. } => {
            let n = options.len().max(1);
            let i = match input {
                ControlInput::Value(v) => v.round().max(0.0) as usize,
                ControlInput::Norm(x) => (x.clamp(0.0, 1.0) * n as f32) as usize,
            };
            Some(i.min(n - 1) as f32)
        }
        ControlKind::Momentary | ControlKind::Trigger => None,
    }
}

/// Equal at the control's precision: 1/10 000 of a continuous range,
/// the same step for the others.
pub fn same(kind: &ControlKind, a: f32, b: f32) -> bool {
    let quantum = match kind {
        ControlKind::Continuous { min, max, .. } => ((max - min) * 1e-4).max(1e-6),
        _ => 0.5,
    };
    (a - b).abs() < quantum
}

fn arg(kind: &ControlKind, v: f32) -> OscArg {
    match kind {
        ControlKind::Continuous { .. } => OscArg::Float(v),
        _ => OscArg::Int(v.round() as i32),
    }
}

/// A client that subscribed, or a fixed destination.
#[derive(Debug)]
pub struct Subscriber {
    pub addr: SocketAddr,
    /// Typed in the settings: never expires.
    pub fixed: bool,
    pub last_seen: Instant,
    /// Control id → value last queued for it.
    pub last_sent: HashMap<String, f32>,
}

/// One datagram for the "osc-out" thread.
#[derive(Debug)]
pub struct Outgoing {
    /// The OSC input's interface (sent from there).
    pub bind: IpAddr,
    pub to: SocketAddr,
    pub data: Vec<u8>,
}

/// What the sending thread reports (`NetHub`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct FeedbackStatus {
    pub local: Option<String>,
    pub sent: u64,
    pub errors: u64,
    pub error: Option<String>,
}

/// The engine's side (`Shared.netin.feedback`): never saved.
pub struct FeedbackState {
    pub subscribers: Vec<Subscriber>,
    tx: SyncSender<Outgoing>,
    rx: Option<Receiver<Outgoing>>,
    last_tick: Option<Instant>,
    last_beat: Option<i64>,
    /// Control id → address, built once per control.
    addresses: HashMap<String, String>,
    /// Datagrams queued, and dropped because the queue was full.
    pub queued: u64,
    pub dropped: u64,
}

impl Default for FeedbackState {
    fn default() -> Self {
        let (tx, rx) = sync_channel(QUEUE_LEN);
        FeedbackState { subscribers: Vec::new(), tx, rx: Some(rx), last_tick: None, last_beat: None, addresses: HashMap::new(), queued: 0, dropped: 0 }
    }
}

impl FeedbackState {
    /// For the "osc-out" thread (once).
    pub fn take_receiver(&mut self) -> Option<Receiver<Outgoing>> {
        self.rx.take()
    }

    /// Any message from this client keeps its subscriptions alive.
    pub fn seen(&mut self, ip: IpAddr, now: Instant) {
        for s in self.subscribers.iter_mut().filter(|s| s.addr.ip() == ip) {
            s.last_seen = now;
        }
    }

    /// `/ls/subscribe <port>`: feedback to `addr`; the full state goes out
    /// at the next ticks (again when an existing subscriber re-subscribes).
    pub fn subscribe(&mut self, cfg: &OscFeedback, addr: SocketAddr, now: Instant) -> Result<(), String> {
        if !cfg.enabled {
            return Err("retour OSC désactivé".into());
        }
        if !cfg.allow_subscribe {
            return Err("abonnements refusés (« Accepter les abonnements » est décoché)".into());
        }
        check_dest(addr)?;
        if let Some(s) = self.subscribers.iter_mut().find(|s| s.addr == addr) {
            s.last_seen = now;
            s.last_sent.clear();
            return Ok(());
        }
        if self.subscribers.iter().filter(|s| !s.fixed).count() >= MAX_SUBSCRIBERS {
            return Err(format!("trop d'abonnés ({MAX_SUBSCRIBERS} au plus)"));
        }
        self.subscribers.push(Subscriber { addr, fixed: false, last_seen: now, last_sent: HashMap::new() });
        Ok(())
    }

    /// `/ls/unsubscribe`: every subscription of this client (not the
    /// fixed destinations). False: there was none.
    pub fn unsubscribe(&mut self, ip: IpAddr) -> bool {
        let before = self.subscribers.len();
        self.subscribers.retain(|s| s.fixed || s.addr.ip() != ip);
        self.subscribers.len() != before
    }

    /// A client set this value: it has it already, nothing goes back to it.
    pub fn echoed(&mut self, ip: IpAddr, id: &str, value: f32) {
        for s in self.subscribers.iter_mut().filter(|s| s.addr.ip() == ip) {
            s.last_sent.insert(id.to_string(), value);
        }
    }

    /// The fixed destinations follow the settings; silent subscribers go.
    fn reconcile(&mut self, cfg: &OscFeedback, now: Instant) {
        let targets: Vec<SocketAddr> = cfg.targets.iter().filter_map(|t| parse_target(t).ok()).collect();
        self.subscribers.retain(|s| {
            if s.fixed {
                targets.contains(&s.addr)
            } else {
                cfg.allow_subscribe && now.saturating_duration_since(s.last_seen) <= SUBSCRIBER_TIMEOUT && !targets.contains(&s.addr)
            }
        });
        for t in targets {
            if !self.subscribers.iter().any(|s| s.addr == t) {
                self.subscribers.push(Subscriber { addr: t, fixed: true, last_seen: now, last_sent: HashMap::new() });
            }
        }
    }

    fn reset(&mut self) {
        self.subscribers.clear();
        (self.last_tick, self.last_beat) = (None, None);
    }
}

/// Queues one datagram; false when the queue is full (dropped).
fn queue(tx: &SyncSender<Outgoing>, monitor: &mut Monitor, bind: IpAddr, to: SocketAddr, address: &str, args: &[OscArg]) -> bool {
    let data = osc::encode(address, args);
    let ok = tx.try_send(Outgoing { bind, to, data }).is_ok();
    monitor.push(false, to.to_string(), address, &monitor::args_text(args), if ok { monitor::SENT } else { monitor::DROPPED }, None);
    ok
}

/// Once per engine frame, after the inputs: send what changed.
pub fn tick(s: &mut Shared, cfg: &NetInConfig, now: Instant) {
    let fb = &cfg.feedback;
    if !fb.enabled {
        if !s.netin.feedback.subscribers.is_empty() || s.netin.feedback.last_tick.is_some() {
            s.netin.feedback.reset();
        }
        return;
    }
    s.netin.feedback.reconcile(fb, now);
    if s.netin.feedback.subscribers.is_empty() {
        s.netin.feedback.last_beat = None;
        return;
    }
    let interval = Duration::from_secs_f32(1.0 / fb.max_rate_hz.clamp(MIN_RATE_HZ, MAX_RATE_HZ));
    if s.netin.feedback.last_tick.is_some_and(|t| now.saturating_duration_since(t) < interval) {
        return;
    }
    s.netin.feedback.last_tick = Some(now);
    let Ok(bind) = super::parse_bind(&cfg.osc.bind, 0).map(|a| a.ip()) else { return };
    // The base values at this instant (index in the registry, value).
    let values: Vec<(usize, f32)> =
        s.controls.list().iter().enumerate().filter(|(_, d)| has_address(d)).filter_map(|(i, d)| value_of(s, d).map(|v| (i, v))).collect();
    let beat = fb.beat_messages.then(|| s.tempo.beat_at(now.saturating_duration_since(s.epoch).as_secs_f64()).floor() as i64);
    let netin = &mut s.netin;
    let list = s.controls.list();
    let state = &mut netin.feedback;
    let new_beat = match (state.last_beat, beat) {
        (Some(was), Some(b)) if b > was => Some(b.rem_euclid(4) as i32 + 1),
        _ => None,
    };
    state.last_beat = beat;
    let FeedbackState { subscribers, tx, addresses, queued, dropped, .. } = state;
    let monitor = &mut netin.monitor;
    'subs: for sub in subscribers.iter_mut() {
        if let Some(n) = new_beat {
            if !queue(tx, monitor, bind, sub.addr, BEAT_ADDRESS, &[OscArg::Int(n)]) {
                *dropped += 1;
                break 'subs;
            }
            *queued += 1;
        }
        let mut budget = MAX_PER_TICK;
        for &(i, v) in &values {
            let d = &list[i];
            if sub.last_sent.get(&d.id).is_some_and(|&old| same(&d.kind, old, v)) {
                continue;
            }
            if budget == 0 {
                break;
            }
            let address = addresses.entry(d.id.clone()).or_insert_with(|| address_of(&d.id));
            if !queue(tx, monitor, bind, sub.addr, address, &[arg(&d.kind, v)]) {
                *dropped += 1;
                break 'subs;
            }
            *queued += 1;
            sub.last_sent.insert(d.id.clone(), v);
            budget -= 1;
        }
    }
}

/// Starts the "osc-out" thread. None: it could not be created (logged);
/// the queue then fills and drops, the engine is not affected.
pub fn spawn(rx: Receiver<Outgoing>, hub: Arc<NetHub>, running: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("osc-out".into())
        .spawn(move || run(rx, &hub, &running))
        .map_err(|e| log::warn!("Retour OSC : impossible de démarrer le thread : {e}"))
        .ok()
}

fn run(rx: Receiver<Outgoing>, hub: &NetHub, running: &AtomicBool) {
    let mut sock: Option<(IpAddr, UdpSocket)> = None;
    while running.load(Ordering::SeqCst) {
        let p = match rx.recv_timeout(POLL) {
            Ok(p) => p,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if sock.as_ref().map(|(ip, _)| *ip) != Some(p.bind) {
            // Broadcast stays off (the system default): one host per datagram.
            sock = match UdpSocket::bind(SocketAddr::new(p.bind, 0)) {
                Ok(s) => {
                    let local = s.local_addr().map(|a| a.to_string()).ok();
                    hub.update_feedback(|st| (st.local, st.error) = (local, None));
                    Some((p.bind, s))
                }
                Err(e) => {
                    let error = format!("impossible d'envoyer depuis {} : {e}", p.bind);
                    log::debug!("Retour OSC : {error}");
                    hub.update_feedback(|st| (st.local, st.errors, st.error) = (None, st.errors + 1, Some(error)));
                    None
                }
            };
        }
        let Some((_, s)) = &sock else { continue };
        match s.send_to(&p.data, p.to) {
            Ok(_) => hub.update_feedback(|st| st.sent += 1),
            Err(e) => {
                let error = format!("envoi vers {} : {e}", p.to);
                log::debug!("Retour OSC : {error}");
                hub.update_feedback(|st| (st.errors, st.error) = (st.errors + 1, Some(error)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netin::{NetHub, NetInState};
    use crate::test_support::shared;

    fn studio(feedback: OscFeedback) -> Shared {
        let mut s = shared();
        s.netin = NetInState::new(NetHub::in_memory(Arc::clone(&s.estop)));
        let mut cfg = NetInConfig::default();
        cfg.osc.enabled = true;
        cfg.feedback = feedback;
        s.netin.hub.set_file(cfg).unwrap();
        s
    }

    fn on() -> OscFeedback {
        OscFeedback { enabled: true, beat_messages: false, ..OscFeedback::default() }
    }

    fn client(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    /// Everything queued so far: (destination, address, args).
    fn drain(s: &mut Shared) -> Vec<(SocketAddr, String, Vec<OscArg>)> {
        let rx = s.netin.feedback.rx.as_ref().unwrap();
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|p| {
                let m = osc::decode(&p.data).unwrap().remove(0);
                (p.to, m.address, m.args)
            })
            .collect()
    }

    fn tick_at(s: &mut Shared, now: Instant) {
        let cfg = s.netin.hub.effective();
        tick(s, &cfg, now);
    }

    /// Ticks until the full state has gone out, then empties the queue.
    fn settle(s: &mut Shared, mut now: Instant) -> Instant {
        for _ in 0..50 {
            now += Duration::from_millis(100);
            tick_at(s, now);
            if drain(s).is_empty() {
                break;
            }
        }
        now
    }

    #[test]
    fn addresses_and_ids_are_inverse_for_the_whole_registry() {
        let s = shared();
        for d in s.controls.list() {
            assert!(!d.id.contains('/'), "{}", d.id);
            assert_eq!(control_of(&address_of(&d.id)).as_deref(), Some(d.id.as_str()));
        }
        assert_eq!(address_of("master.size"), "/ls/master/size");
        assert_eq!(address_of("grid.1.2.3"), "/ls/grid/1/2/3");
        assert_eq!(control_of("/ls/tempo/bpm").as_deref(), Some("tempo.bpm"));
        assert_eq!(control_of("/beyond/master"), None);
        assert_eq!(control_of("/ls/"), None);
    }

    #[test]
    fn only_controls_the_network_may_drive_have_an_address() {
        let s = shared();
        let list = addresses(&s.controls);
        let ids: Vec<&str> = list.iter().map(|a| a.control.as_str()).collect();
        for id in ["master.size", "master.brightness", "tempo.bpm", "tempo.resync", "transport.blackout", "safety.estop", "grid.1.1.1"] {
            assert!(ids.contains(&id), "{id}");
        }
        for id in ["transport.arm", "safety.hold", "safety.sky_off", "transport.freeze", "transport.black_hold"] {
            assert!(!ids.contains(&id), "{id}");
        }
        for a in &list {
            let d = s.controls.get(&a.control).unwrap();
            assert!(d.external, "{}", a.control);
            assert!(!a.control.starts_with("safety.") || a.control == "safety.estop", "{}: no safety setting", a.control);
            assert!(!a.control.contains("estop") || a.control == "safety.estop", "{}: no e-stop reset", a.control);
            assert!(!a.control.ends_with(".reset") || a.control == "master.reset", "{}", a.control);
            assert!(!a.address.contains("arm"), "{}", a.address);
            let rest = &a.address[PREFIX.len()..];
            assert!(!osc::RESERVED.iter().any(|r| rest == *r || rest.starts_with(&format!("{r}/"))), "{} collides with a command", a.address);
        }
        assert!(s.controls.list().iter().filter(|d| !d.external).all(|d| !ids.contains(&d.id.as_str())));
        let size = list.iter().find(|a| a.control == "master.size").unwrap();
        assert_eq!((size.address.as_str(), size.kind, size.min, size.max), ("/ls/master/size", "continuous", Some(0.0), Some(2.0)));
        let bpm = list.iter().find(|a| a.control == "tempo.bpm").unwrap();
        assert_eq!(bpm.unit, Some(Unit::Bpm));
    }

    #[test]
    fn targets_are_validated() {
        assert!(parse_target("192.168.1.20:9000").is_ok());
        assert!(parse_target("127.0.0.1:9000").is_ok());
        for bad in ["console.local:9000", "192.168.1.20", "192.168.1.20:0", "255.255.255.255:9000", "192.168.1.255:9000", "239.255.0.1:9000", "0.0.0.0:9000", "[::]:9000", ""] {
            assert!(parse_target(bad).is_err(), "{bad}");
        }
        let mut fb = on();
        fb.targets = (0..=MAX_TARGETS).map(|i| format!("127.0.0.1:{}", 9000 + i)).collect();
        assert!(fb.validate().is_err());
        fb.targets.truncate(1);
        assert!(fb.validate().is_ok());
        fb.max_rate_hz = 0.0;
        assert!(fb.validate().is_err());
        fb.max_rate_hz = f32::NAN;
        assert!(fb.validate().is_err());
    }

    #[test]
    fn off_sends_nothing() {
        let mut s = studio(OscFeedback { targets: vec!["127.0.0.1:9000".into()], ..OscFeedback::default() });
        let now = Instant::now();
        assert!(s.netin.feedback.subscribe(&s.netin.hub.effective().feedback, client(9001), now).is_err(), "no subscription while off");
        for i in 0..5 {
            s.live.size = 0.5 + i as f32 * 0.1;
            tick_at(&mut s, now + Duration::from_secs(i));
        }
        assert!(drain(&mut s).is_empty());
        assert_eq!(s.netin.feedback.queued, 0);
        assert!(s.netin.feedback.subscribers.is_empty());
    }

    #[test]
    fn a_change_goes_once_to_each_subscriber_never_back_to_its_sender() {
        let mut s = studio(OscFeedback { targets: vec!["127.0.0.1:9100".into()], ..on() });
        let t0 = Instant::now();
        let cfg = s.netin.hub.effective().feedback;
        s.netin.feedback.subscribe(&cfg, client(9001), t0).unwrap();
        // Subscribing sends the full state (in a few ticks).
        tick_at(&mut s, t0);
        let first = drain(&mut s);
        assert!(first.iter().any(|(to, a, _)| *to == client(9001) && a == "/ls/master/size"));
        assert!(first.iter().any(|(to, a, _)| *to == client(9100) && a == "/ls/master/size"), "fixed destination too");
        assert!(first.iter().all(|(_, a, _)| !a.contains("arm") && a.starts_with("/ls/")));
        let mut now = settle(&mut s, t0);

        // One change: one message per destination.
        s.live.size = 1.5;
        now += Duration::from_millis(100);
        tick_at(&mut s, now);
        let got = drain(&mut s);
        let size: Vec<_> = got.iter().filter(|(_, a, _)| a == "/ls/master/size").collect();
        assert_eq!(size.len(), 2, "{got:?}");
        assert!(size.iter().all(|(_, _, args)| args == &vec![OscArg::Float(1.5)]));
        now += Duration::from_millis(100);
        tick_at(&mut s, now);
        assert!(drain(&mut s).iter().all(|(_, a, _)| a != "/ls/master/size"), "once per change");

        // A value set by the subscriber (127.0.0.1) is not sent back to it.
        s.live.size = 0.7;
        s.netin.feedback.echoed(client(1).ip(), "master.size", 0.7);
        now += Duration::from_millis(100);
        tick_at(&mut s, now);
        assert!(drain(&mut s).iter().all(|(_, a, _)| a != "/ls/master/size"), "both destinations are on 127.0.0.1");

        // An identical echo changes nothing: no message.
        s.live.size = 0.7 + 1e-6;
        now += Duration::from_millis(100);
        tick_at(&mut s, now);
        assert!(drain(&mut s).is_empty());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn the_rate_is_limited() {
        let mut s = studio(OscFeedback { max_rate_hz: 10.0, ..on() });
        let t0 = Instant::now();
        let cfg = s.netin.hub.effective().feedback;
        s.netin.feedback.subscribe(&cfg, client(9001), t0).unwrap();
        let mut now = settle(&mut s, t0);
        let mut sent = 0;
        // 60 changes in one second (one per frame): 10 messages at most.
        for i in 0..60 {
            now += Duration::from_micros(16_667);
            s.live.size = 0.01 * (i + 1) as f32;
            tick_at(&mut s, now);
            sent += drain(&mut s).iter().filter(|(_, a, _)| a == "/ls/master/size").count();
        }
        assert!((9..=11).contains(&sent), "{sent}");
        // The last value always arrives.
        now += Duration::from_millis(200);
        tick_at(&mut s, now);
        assert!((s.netin.feedback.subscribers[0].last_sent["master.size"] - 0.6).abs() < 1e-5);
    }

    #[test]
    fn a_silent_subscriber_expires_after_60_s() {
        let mut s = studio(on());
        let t0 = Instant::now();
        let cfg = s.netin.hub.effective().feedback;
        s.netin.feedback.subscribe(&cfg, client(9001), t0).unwrap();
        s.netin.feedback.seen(client(5).ip(), t0 + Duration::from_secs(30));
        tick_at(&mut s, t0 + Duration::from_secs(80));
        assert_eq!(s.netin.feedback.subscribers.len(), 1, "seen 50 s ago");
        drain(&mut s);
        tick_at(&mut s, t0 + Duration::from_secs(91));
        assert!(s.netin.feedback.subscribers.is_empty());
        s.live.size = 1.9;
        tick_at(&mut s, t0 + Duration::from_secs(92));
        assert!(drain(&mut s).is_empty(), "nothing for a forgotten subscriber");
        // Unsubscribe, and the subscriber cap.
        for p in 0..MAX_SUBSCRIBERS as u16 {
            s.netin.feedback.subscribe(&cfg, client(10_000 + p), t0).unwrap();
        }
        assert!(s.netin.feedback.subscribe(&cfg, client(20_000), t0).is_err());
        assert!(s.netin.feedback.unsubscribe(client(1).ip()));
        assert!(s.netin.feedback.subscribers.is_empty());
        let closed = OscFeedback { allow_subscribe: false, ..on() };
        assert!(s.netin.feedback.subscribe(&closed, client(9001), t0).is_err());
        assert!(s.netin.feedback.subscribe(&cfg, SocketAddr::from(([255, 255, 255, 255], 9000)), t0).is_err(), "never broadcast");
    }

    #[test]
    fn beats_go_out_once_each() {
        let mut s = studio(OscFeedback { beat_messages: true, ..on() });
        let t0 = s.epoch;
        let cfg = s.netin.hub.effective().feedback;
        s.netin.feedback.subscribe(&cfg, client(9001), t0).unwrap();
        s.tempo.set_bpm(120.0, 0.0);
        let mut beats = Vec::new();
        // Two seconds at 120 BPM, 60 frames a second: four beats.
        for f in 1..=120 {
            tick_at(&mut s, t0 + Duration::from_micros(16_667 * f));
            beats.extend(drain(&mut s).into_iter().filter(|(_, a, _)| a == BEAT_ADDRESS).map(|(_, _, args)| args));
        }
        assert!((3..=4).contains(&beats.len()), "{beats:?}");
    }

    /// Loopback only (127.0.0.1, ports picked by the system): the sending
    /// thread delivers what the engine queued.
    #[test]
    fn the_sending_thread_delivers_on_loopback() {
        let mut s = studio(on());
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let addr = receiver.local_addr().unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let thread = spawn(s.netin.feedback.take_receiver().unwrap(), Arc::clone(&s.netin.hub), Arc::clone(&running)).unwrap();
        let now = Instant::now();
        let cfg = s.netin.hub.effective().feedback;
        s.netin.feedback.subscribe(&cfg, addr, now).unwrap();
        s.live.size = 1.25;
        tick_at(&mut s, now);
        let mut buf = [0u8; 2048];
        let mut seen = false;
        for _ in 0..MAX_PER_TICK {
            let (n, from) = receiver.recv_from(&mut buf).unwrap();
            assert!(from.ip().is_loopback(), "sent from the OSC interface");
            let m = osc::decode(&buf[..n]).unwrap().remove(0);
            if m.address == "/ls/master/size" {
                assert_eq!(m.args, vec![OscArg::Float(1.25)]);
                seen = true;
                break;
            }
        }
        assert!(seen);
        running.store(false, Ordering::SeqCst);
        thread.join().unwrap();
        assert!(s.netin.hub.feedback_status().sent > 0);
    }
}
