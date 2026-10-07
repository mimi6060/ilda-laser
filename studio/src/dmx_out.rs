//! DMX output (T-453): a few named channels of one Art-Net or sACN
//! universe, for a haze machine and one or two simple lights, driven by
//! the *Brouillard* panel and by the timeline's DMX tracks. Not a
//! lighting console: no fixture library, at most `MAX_CHANNELS` channels.
//!
//! What it does, and what it never does:
//! - **Off by default** (`<data-dir>/dmx_out.json`, a setting of this
//!   machine, never in a project). No socket exists while it is off. It
//!   sends only from the interface the operator picked to the one host
//!   they typed: no discovery, no ArtPoll, no broadcast, no multicast.
//! - **Never the laser**: nothing here arms, disarms or reaches the
//!   laser's output, and no channel stands for the arm state. A target
//!   that is the laser's own address (the ShowNET card output, or the
//!   connected output's) is refused (409).
//! - **No self-drive**: the output universe may not be one the studio
//!   listens to on the same protocol (T-406's chart, patch and fixtures),
//!   both ways (409); the engine also holds the output back if that
//!   happens later (a `--artnet` start).
//! - **Stops**: while the emergency stop is latched every channel is 0;
//!   the sending thread reads the latch itself, so it needs neither the
//!   engine nor the `Shared` lock to go dark. Disabling the output, a
//!   changed destination, a send error and the studio's shutdown (Ctrl+C,
//!   SIGTERM, an engine panic) send `OFF_FRAMES` frames of zeros before
//!   the socket closes: a node that holds its last values holds zeros.
//!   An engine that stops feeding the thread (`STALE`) gets zeros too.
//! - **Disarm and blackout** (any reason: the operator, « Noir », an
//!   interlock, the presence, the watchdog, a profile, the agenda's
//!   « Arrêt et noir », a lost output…, seen as a new `last_disarm` of the
//!   gate): every channel goes back to its rest value at once, the pump
//!   and the fan to 0, the haze button lets go and the fan slider goes to
//!   0. The output keeps sending those values (`Hold`): the timeline's
//!   DMX events come back only once the show is started again (*Lecture*)
//!   with the laser re-armed, the haze only from a new press. The thread
//!   sends such a change at once, not at its next tick.
//! - The engine never waits: once per frame it `try_send`s the universe
//!   it computed into a bounded queue; the "dmx-out" thread sends the
//!   newest at `rate_hz` (30 Hz by default).
//!
//! Values: the timeline's DMX events (`Player::dmx`) mixed with each
//! channel's rest value, then the *Brouillard* panel on top (the maximum
//! of the two). Every value is a `u8`, so it is 0–255 by construction; a
//! float on the way (fan slider, fades) is refused if not finite.
//!
//! Wire formats from the public specifications: ArtDmx (Art-Net 4, the
//! encoder of artnet.rs) and the E1.31 data packet (ANSI E1.31), both
//! read back by the input's decoders (netin/) in the tests.

use crate::interlock::{ArmSource, EStop};
use crate::netin::NetInConfig;
use crate::timeline::{Clock, EventSource, TrackKind};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Named channels at most.
pub const MAX_CHANNELS: usize = 16;
/// Channels of one universe.
pub const UNIVERSE_LEN: usize = 512;
pub const ARTNET_PORT: u16 = 6454;
pub const SACN_PORT: u16 = 5568;
/// Snapshots waiting for the "dmx-out" thread at most (it keeps the newest).
pub const QUEUE_LEN: usize = 8;
/// Frames of zeros sent before the socket closes.
pub const OFF_FRAMES: usize = 3;
/// No snapshot from the engine for this long: zeros.
pub const STALE: Duration = Duration::from_millis(250);
/// Longest sleep of the thread, so an e-stop or a shutdown is seen fast.
const POLL: Duration = Duration::from_millis(10);
/// A socket that could not be opened is tried again after this.
const RETRY: Duration = Duration::from_secs(2);
pub const MIN_RATE_HZ: u8 = 1;
pub const MAX_RATE_HZ: u8 = 44;
pub const MAX_BURST_S: f32 = 60.0;
/// Longest channel name.
pub const MAX_NAME: usize = 40;
/// Our sACN source name and priority (the E1.31 default).
const SACN_SOURCE: &str = "Laser Studio";
const SACN_PRIORITY: u8 = 100;

pub const SELF_DRIVE: &str = "L'univers de sortie doit être différent des univers d'entrée, sinon le studio se pilote lui-même";
pub const HELD: &str = "Laser désarmé : canaux DMX au repos jusqu'à une nouvelle lecture, laser réarmé";
pub const LASER_TARGET: &str = "Cette adresse est celle de la sortie laser : la sortie DMX ne pilote jamais le laser";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmxProtocol {
    #[default]
    ArtNet,
    Sacn,
}

impl DmxProtocol {
    pub fn label(self) -> &'static str {
        match self {
            DmxProtocol::ArtNet => "Art-Net",
            DmxProtocol::Sacn => "sACN",
        }
    }

    fn port(self) -> u16 {
        match self {
            DmxProtocol::ArtNet => ARTNET_PORT,
            DmxProtocol::Sacn => SACN_PORT,
        }
    }

    /// The universes the protocol can address.
    fn universes(self) -> (u16, u16) {
        match self {
            DmxProtocol::ArtNet => (0, crate::artnet::MAX_UNIVERSE),
            DmxProtocol::Sacn => (crate::netin::sacn::MIN_UNIVERSE, crate::netin::sacn::MAX_UNIVERSE),
        }
    }
}

/// `<data-dir>/dmx_out.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DmxOutSettings {
    pub enabled: bool,
    pub protocol: DmxProtocol,
    pub universe: u16,
    /// The address of this Mac to send from; None: nothing is sent.
    pub interface: Option<String>,
    /// `ip` or `ip:port` of the one node to send to (the protocol's port
    /// by default); None: nothing is sent.
    pub target: Option<String>,
    pub rate_hz: u8,
    pub channels: Vec<DmxChannel>,
    pub haze: HazePatch,
}

impl Default for DmxOutSettings {
    fn default() -> Self {
        DmxOutSettings { enabled: false, protocol: DmxProtocol::ArtNet, universe: 1, interface: None, target: None, rate_hz: 30, channels: Vec::new(), haze: HazePatch::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DmxChannel {
    pub name: String,
    /// 1..=512.
    pub address: u16,
    /// Its value outside any event, and with the haze off.
    pub rest: u8,
}

impl Default for DmxChannel {
    fn default() -> Self {
        DmxChannel { name: String::new(), address: 1, rest: 0 }
    }
}

/// The *Brouillard* panel: which named channels are the pump and the fan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HazePatch {
    pub pump: Option<String>,
    pub fan: Option<String>,
    /// The pump's value while the button is held.
    pub pump_level: u8,
    /// Longest burst: then the pump goes back to 0, button held or not.
    pub max_burst_s: f32,
}

impl Default for HazePatch {
    fn default() -> Self {
        HazePatch { pump: None, fan: None, pump_level: 255, max_burst_s: 10.0 }
    }
}

/// A refused request: the HTTP status and the French message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub message: String,
}

impl Refused {
    fn bad(message: impl Into<String>) -> Self {
        Refused { status: 400, message: message.into() }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Refused { status: 409, message: message.into() }
    }
}

impl From<String> for Refused {
    fn from(message: String) -> Self {
        Refused::bad(message)
    }
}

/// The address to send from: an IP of this Mac, never 0.0.0.0, a
/// broadcast or a multicast address.
fn parse_interface(s: &str) -> Result<IpAddr, String> {
    let ip: IpAddr = s.trim().parse().map_err(|_| format!("interface réseau invalide : « {s} » (une adresse IP de ce Mac)"))?;
    let broadcast = matches!(ip, IpAddr::V4(v4) if v4.is_broadcast());
    if ip.is_unspecified() || ip.is_multicast() || broadcast {
        return Err(format!("interface réseau invalide : {ip} (une adresse IP de ce Mac)"));
    }
    Ok(ip)
}

/// The node to send to: one IPv4 host, `ip` or `ip:port`. Never a name
/// (nothing is looked up), 0.0.0.0, a broadcast (.255) or a multicast
/// address, never port 0.
pub fn parse_target(s: &str, protocol: DmxProtocol) -> Result<SocketAddr, String> {
    let s = s.trim();
    let addr = match s.parse::<SocketAddr>() {
        Ok(a) => a,
        Err(_) => SocketAddr::new(s.parse::<IpAddr>().map_err(|_| format!("adresse de destination invalide : « {s} » (adresse IP ou IP:port, ex. 192.168.1.30)"))?, protocol.port()),
    };
    let IpAddr::V4(ip) = addr.ip() else { return Err(format!("adresse de destination invalide : {addr} (IPv4 seulement)")) };
    if addr.port() == 0 || ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || ip.octets()[3] == 255 {
        return Err(format!("adresse de destination invalide : {addr} (une seule machine : ni diffusion, ni multidiffusion)"));
    }
    Ok(addr)
}

/// Where the frames go: from, to, how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub bind: IpAddr,
    pub dest: SocketAddr,
    pub protocol: DmxProtocol,
    pub universe: u16,
}

impl DmxOutSettings {
    pub fn validate(&self) -> Result<(), String> {
        let (lo, hi) = self.protocol.universes();
        if !(lo..=hi).contains(&self.universe) {
            return Err(format!("univers {} invalide ({lo} à {hi})", self.protocol.label()));
        }
        if !(MIN_RATE_HZ..=MAX_RATE_HZ).contains(&self.rate_hz) {
            return Err(format!("cadence invalide : {} trames/s ({MIN_RATE_HZ} à {MAX_RATE_HZ})", self.rate_hz));
        }
        if let Some(i) = &self.interface {
            parse_interface(i)?;
        }
        if let Some(t) = &self.target {
            parse_target(t, self.protocol)?;
        }
        if self.channels.len() > MAX_CHANNELS {
            return Err(format!("trop de canaux ({MAX_CHANNELS} au plus)"));
        }
        for (i, c) in self.channels.iter().enumerate() {
            let name = c.name.trim();
            if name.is_empty() || name.chars().count() > MAX_NAME || name != c.name {
                return Err(format!("canal {} : nom invalide (1 à {MAX_NAME} caractères)", i + 1));
            }
            if !(1..=UNIVERSE_LEN as u16).contains(&c.address) {
                return Err(format!("canal « {name} » : adresse {} invalide (1 à 512)", c.address));
            }
            if self.channels[..i].iter().any(|o| o.name == c.name) {
                return Err(format!("deux canaux s'appellent « {name} »"));
            }
            if let Some(o) = self.channels[..i].iter().find(|o| o.address == c.address) {
                return Err(format!("« {name} » et « {} » ont la même adresse {}", o.name, c.address));
            }
        }
        let h = &self.haze;
        for (what, name) in [("pompe", &h.pump), ("ventilateur", &h.fan)] {
            if let Some(n) = name {
                if !self.channels.iter().any(|c| &c.name == n) {
                    return Err(format!("brouillard : le canal de la {what} « {n} » n'existe pas"));
                }
            }
        }
        if h.pump.is_some() && h.pump == h.fan {
            return Err("brouillard : la pompe et le ventilateur sont deux canaux différents".into());
        }
        if !h.max_burst_s.is_finite() || !(0.5..=MAX_BURST_S).contains(&h.max_burst_s) {
            return Err(format!("brouillard : durée max d'un jet invalide (0,5 à {MAX_BURST_S} s)"));
        }
        if self.enabled && (self.interface.is_none() || self.target.is_none()) {
            return Err("choisissez l'interface réseau et l'adresse de destination avant d'activer la sortie DMX".into());
        }
        Ok(())
    }

    /// The route the settings ask for (None: off, or nowhere to send).
    pub fn route(&self) -> Option<Route> {
        if !self.enabled {
            return None;
        }
        let bind = parse_interface(self.interface.as_deref()?).ok()?;
        let dest = parse_target(self.target.as_deref()?, self.protocol).ok()?;
        Some(Route { bind, dest, protocol: self.protocol, universe: self.universe })
    }

    fn channel(&self, name: &str) -> Option<&DmxChannel> {
        self.channels.iter().find(|c| c.name == name)
    }
}

/// The universes the studio listens to on `protocol` (T-013, T-406): the
/// Art-Net chart's, the sACN list, the free patch and the fixtures, for a
/// protocol that is on. Universe numbers are shared between the two.
pub fn input_universes(cfg: &NetInConfig, protocol: DmxProtocol) -> Vec<u16> {
    let mut v = match protocol {
        DmxProtocol::ArtNet if cfg.artnet.enabled => vec![cfg.artnet.universe],
        DmxProtocol::Sacn if cfg.sacn.enabled => cfg.sacn.universes.clone(),
        _ => return Vec::new(),
    };
    v.extend(cfg.dmx.universes());
    v.sort_unstable();
    v.dedup();
    v
}

/// The self-drive trap: the output universe is one the input listens to.
pub fn conflicts(out: &DmxOutSettings, input: &NetInConfig) -> Option<&'static str> {
    (out.enabled && input_universes(input, out.protocol).contains(&out.universe)).then_some(SELF_DRIVE)
}

/// T-406's side of the trap: new input settings (`file`, the command
/// line's `--artnet` on top) that would listen to the output's universe.
pub fn refuses_input(s: &Shared, file: &NetInConfig) -> Option<&'static str> {
    let mut input = file.clone();
    if s.netin.hub.cli().artnet.is_some() {
        input.artnet.enabled = true;
    }
    conflicts(&s.dmx_out.cfg, &input)
}

/// IPv4 addresses written in `s` (an output id such as `idn:10.0.0.5`).
fn ipv4s_in(s: &str) -> Vec<Ipv4Addr> {
    s.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter_map(|t| t.parse().ok()).collect()
}

/// The laser's own addresses: the ShowNET card output (T-301) and the
/// output selected now or remembered.
fn laser_ips(s: &Shared) -> Vec<Ipv4Addr> {
    let mut v = Vec::new();
    if let Some(t) = s.shownet_dmx.get().target {
        v.extend(ipv4s_in(&t));
    }
    for id in [s.output_mgr.selected(), s.output_mgr.prefs().selected].into_iter().flatten() {
        v.extend(ipv4s_in(&id));
    }
    v
}

/// One ArtDmx packet of the whole universe (artnet.rs's encoder).
pub fn artnet_packet(universe: u16, data: &[u8; UNIVERSE_LEN]) -> Vec<u8> {
    crate::artnet::encode_dmx(universe, data).to_vec()
}

/// Offset of the START code in an E1.31 data packet.
const SACN_HEADER: usize = 125;
/// E1.31 options: the source stops sending.
const SACN_TERMINATED: u8 = 0x40;

/// One E1.31 data packet of the whole universe (START code 0).
pub fn sacn_packet(cid: &[u8; 16], universe: u16, sequence: u8, terminated: bool, data: &[u8; UNIVERSE_LEN]) -> Vec<u8> {
    let len = SACN_HEADER + 1 + UNIVERSE_LEN;
    let flags = |n: usize| (0x7000 | n as u16).to_be_bytes();
    let mut p = Vec::with_capacity(len);
    // Root layer.
    p.extend_from_slice(&0x0010u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(b"ASC-E1.17\0\0\0");
    p.extend_from_slice(&flags(len - 16));
    p.extend_from_slice(&4u32.to_be_bytes()); // VECTOR_ROOT_E131_DATA
    p.extend_from_slice(cid);
    // Framing layer.
    p.extend_from_slice(&flags(len - 38));
    p.extend_from_slice(&2u32.to_be_bytes()); // VECTOR_E131_DATA_PACKET
    let mut name = [0u8; 64];
    name[..SACN_SOURCE.len()].copy_from_slice(SACN_SOURCE.as_bytes());
    p.extend_from_slice(&name);
    p.push(SACN_PRIORITY);
    p.extend_from_slice(&0u16.to_be_bytes()); // no synchronisation
    p.push(sequence);
    p.push(if terminated { SACN_TERMINATED } else { 0 });
    p.extend_from_slice(&universe.to_be_bytes());
    // DMP layer.
    p.extend_from_slice(&flags(len - 115));
    p.extend_from_slice(&[0x02, 0xA1, 0, 0, 0, 1]);
    p.extend_from_slice(&((UNIVERSE_LEN + 1) as u16).to_be_bytes());
    p.push(0); // START code
    p.extend_from_slice(data);
    debug_assert_eq!(p.len(), len);
    p
}

/// This run's sACN component id: random enough to tell two studios apart.
fn make_cid() -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let digest = Sha256::digest(format!("laser-studio dmx-out {} {nanos}", std::process::id()).as_bytes());
    let mut cid = [0u8; 16];
    cid.copy_from_slice(&digest[..16]);
    cid
}

/// What the engine hands the "dmx-out" thread, once per frame.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub cfg: DmxOutSettings,
    /// Why nothing may be sent now (self-drive).
    pub blocked: Option<&'static str>,
    pub universe: Box<[u8; UNIVERSE_LEN]>,
    /// The e-stop is latched: zeros.
    pub zero: bool,
    /// Counts the disarms and blackouts seen: a new one is sent at once.
    pub holds: u64,
    pub at: Instant,
}

impl Snapshot {
    fn route(&self) -> Option<Route> {
        if self.blocked.is_some() {
            return None;
        }
        self.cfg.route()
    }
}

/// What the "dmx-out" thread reports.
#[derive(Clone, Debug, Default, Serialize)]
pub struct OutStatus {
    /// A socket is open and frames go out.
    pub sending: bool,
    /// Where (`ip:port`).
    pub to: Option<String>,
    pub frames: u64,
    pub errors: u64,
    pub error: Option<String>,
    /// The last frame sent was zeros (e-stop, stale engine, closing).
    pub dark: bool,
}

/// The engine's side, in `Shared` (`s.dmx_out`).
pub struct DmxOutState {
    path: Option<PathBuf>,
    cfg: DmxOutSettings,
    tx: SyncSender<Snapshot>,
    rx: Option<Receiver<Snapshot>>,
    pub status: Arc<Mutex<OutStatus>>,
    /// Snapshots the full queue refused.
    pub dropped: u64,
    idle_sent: bool,
    /// The *Brouillard* button is held.
    haze_held: bool,
    /// When the burst started (`Clock::t`); set by the first frame held.
    burst_from: Option<f64>,
    /// The fan slider, 0..1.
    pub fan: f32,
    /// This frame's values, by channel (for the panels and `/api/state`).
    pub values: Vec<(String, u8)>,
    pub pump_on: bool,
    pub blocked: Option<&'static str>,
    pub warning: Option<String>,
    /// The gate's last disarm seen (None: not looked at yet; the start-up
    /// one is not a new disarm).
    disarm_seen: Option<Option<std::time::SystemTime>>,
    /// After a disarm or a blackout: the timeline's DMX events are held at
    /// rest until `Player::plays` moves on while armed.
    timeline_held: bool,
    held_plays: u64,
    /// Disarms and blackouts seen.
    holds: u64,
}

impl DmxOutState {
    fn with(path: Option<PathBuf>, cfg: DmxOutSettings) -> Self {
        let (tx, rx) = sync_channel(QUEUE_LEN);
        DmxOutState {
            path,
            cfg,
            tx,
            rx: Some(rx),
            status: Arc::default(),
            dropped: 0,
            idle_sent: false,
            haze_held: false,
            burst_from: None,
            fan: 0.0,
            values: Vec::new(),
            pump_on: false,
            blocked: None,
            warning: None,
            disarm_seen: None,
            timeline_held: false,
            held_plays: 0,
            holds: 0,
        }
    }

    /// A missing file: off (the default). A damaged or invalid one is set
    /// aside (datafile.rs) or replaced by the default; never enabled then.
    pub fn load(path: PathBuf) -> Self {
        let cfg: DmxOutSettings = crate::load_json(&path);
        let cfg = if cfg.validate().is_ok() { cfg } else { DmxOutSettings::default() };
        Self::with(Some(path), cfg)
    }

    pub fn in_memory() -> Self {
        Self::with(None, DmxOutSettings::default())
    }

    pub fn config(&self) -> &DmxOutSettings {
        &self.cfg
    }

    pub fn take_receiver(&mut self) -> Option<Receiver<Snapshot>> {
        self.rx.take()
    }

    pub fn status(&self) -> OutStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The file and its contents to write (outside the `Shared` lock).
    pub fn save_job(&self) -> Option<(PathBuf, DmxOutSettings)> {
        Some((self.path.clone()?, self.cfg.clone()))
    }

    fn publish(&mut self, snap: Snapshot) {
        let off = snap.route().is_none();
        if off && self.idle_sent {
            return;
        }
        match self.tx.try_send(snap) {
            Ok(()) => self.idle_sent = off,
            Err(_) => self.dropped += 1,
        }
    }
}

impl Default for DmxOutState {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// The *Brouillard* button (UI, Alt+B, MIDI `haze.burst`): held or let go.
/// A new press starts a new burst.
pub fn set_haze(s: &mut Shared, on: bool) {
    let d = &mut s.dmx_out;
    if on && !d.haze_held {
        d.burst_from = None;
    }
    d.haze_held = on && !s.estop.is_latched();
}

/// The fan slider (UI, MIDI `haze.fan`), 0..1; not finite is refused.
pub fn set_fan(s: &mut Shared, v: f32) -> Result<(), &'static str> {
    if !v.is_finite() {
        return Err("valeur du ventilateur invalide");
    }
    s.dmx_out.fan = v.clamp(0.0, 1.0);
    Ok(())
}

/// The emergency stop (`Shared::emergency_stop`): the haze button is let
/// go and the fan slider goes to 0, so nothing starts again on reset.
pub fn estop(s: &mut Shared) {
    let d = &mut s.dmx_out;
    d.haze_held = false;
    d.burst_from = None;
    d.fan = 0.0;
}

/// A disarm or a blackout: everything back to rest, the haze off, and the
/// timeline's DMX events held until the show is started again armed.
fn hold(s: &mut Shared) {
    estop(s);
    let plays = s.timeline.plays();
    let d = &mut s.dmx_out;
    d.timeline_held = true;
    d.held_plays = plays;
    d.holds += 1;
}

/// Watches the gate (once per frame and from `frame`): a new disarm, of
/// any reason, holds the output at rest. A start of the show while armed,
/// after the hold, lets the timeline's events through again.
fn follow_gate(s: &mut Shared) {
    let last = s.gate.last_disarm().map(|(_, at)| at);
    let seen = s.dmx_out.disarm_seen.replace(last);
    if seen.is_some_and(|before| before != last) {
        hold(s);
    }
    let (armed, plays) = (s.gate.is_armed(), s.timeline.plays());
    let d = &mut s.dmx_out;
    if d.timeline_held {
        if !armed {
            // A start while disarmed doesn't count: only one after re-arming.
            d.held_plays = plays;
        } else if plays != d.held_plays {
            d.timeline_held = false;
        }
    }
}

pub fn haze_held(s: &Shared) -> bool {
    s.dmx_out.haze_held
}

/// A float level as a DMX value; not finite: 0.
fn level(v: f32) -> u8 {
    if v.is_finite() {
        v.round().clamp(0.0, 255.0) as u8
    } else {
        0
    }
}

/// The values of the named channels: the timeline's levels mixed with the
/// rest value (`rest + (value - rest) × gain`; several DMX tracks on one
/// channel: the maximum), then the haze panel's pump and fan (maximum).
pub fn mix(cfg: &DmxOutSettings, timeline: &[crate::timeline::DmxLevel], pump: u8, fan: u8) -> Vec<(String, u8)> {
    cfg.channels
        .iter()
        .map(|c| {
            let rest = c.rest as f32;
            let from_timeline = timeline.iter().filter(|l| l.channel == c.name && l.value.is_finite() && l.gain.is_finite()).map(|l| level(rest + (l.value - rest) * l.gain.clamp(0.0, 1.0))).max();
            let mut v = from_timeline.unwrap_or(c.rest);
            if cfg.haze.pump.as_ref() == Some(&c.name) {
                v = v.max(pump);
            }
            if cfg.haze.fan.as_ref() == Some(&c.name) {
                v = v.max(fan);
            }
            (c.name.clone(), v)
        })
        .collect()
}

/// The universe of `values`: every channel not named is 0.
pub fn universe_of(cfg: &DmxOutSettings, values: &[(String, u8)]) -> Box<[u8; UNIVERSE_LEN]> {
    let mut u = Box::new([0u8; UNIVERSE_LEN]);
    for (name, v) in values {
        if let Some(c) = cfg.channel(name) {
            u[c.address as usize - 1] = *v;
        }
    }
    u
}

/// Why the output is held back now (the self-drive trap appeared after it
/// was enabled, e.g. `--artnet`).
fn blocked(s: &Shared) -> Option<&'static str> {
    conflicts(&s.dmx_out.cfg, &s.netin.hub.effective())
}

/// The warning for a show with DMX events the output can't play.
fn show_warning(s: &Shared) -> Option<String> {
    let show = s.timeline.show.as_ref()?;
    let names: Vec<&str> = show
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Dmx)
        .flat_map(|t| &t.events)
        .filter_map(|e| match &e.source {
            EventSource::Dmx { channel, .. } => Some(channel.as_str()),
            _ => None,
        })
        .collect();
    if names.is_empty() {
        return None;
    }
    let cfg = &s.dmx_out.cfg;
    if !cfg.enabled {
        return Some("Le show contient des événements DMX : sortie DMX désactivée, ils sont ignorés".into());
    }
    let mut unknown: Vec<&str> = names.into_iter().filter(|n| cfg.channel(n).is_none()).collect();
    unknown.sort_unstable();
    unknown.dedup();
    (!unknown.is_empty()).then(|| format!("Canaux DMX inconnus dans le show, ignorés : {}", unknown.join(", ")))
}

/// Once per engine frame, after the playhead moved (main.rs): the values
/// of this frame, handed to the "dmx-out" thread.
pub fn frame(s: &mut Shared, c: &Clock) {
    let latched = s.estop.is_latched();
    if latched {
        estop(s);
    }
    follow_gate(s);
    let levels = if s.dmx_out.timeline_held { Vec::new() } else { s.timeline.dmx(c) };
    let why = blocked(s);
    let warning = show_warning(s);
    let d = &mut s.dmx_out;
    let haze = &d.cfg.haze;
    if d.haze_held && d.burst_from.is_none() {
        d.burst_from = Some(c.t);
    }
    let pump_on = haze.pump.is_some() && d.haze_held && d.burst_from.is_some_and(|t0| c.t - t0 < haze.max_burst_s as f64);
    let pump = if pump_on { haze.pump_level } else { 0 };
    let fan = level(d.fan * 255.0);
    let values = if latched { d.cfg.channels.iter().map(|ch| (ch.name.clone(), 0)).collect() } else { mix(&d.cfg, &levels, pump, fan) };
    let universe = if latched { Box::new([0u8; UNIVERSE_LEN]) } else { universe_of(&d.cfg, &values) };
    (d.values, d.pump_on, d.blocked, d.warning) = (values, pump_on && !latched, why, warning);
    let snap = Snapshot { cfg: d.cfg.clone(), blocked: why, universe, zero: latched, holds: d.holds, at: Instant::now() };
    d.publish(snap);
}

/// The open socket.
struct Sink {
    route: Route,
    sock: UdpSocket,
    sequence: u8,
    /// Sends failed since the last success.
    lost: bool,
}

/// Testing only: a test build never sends outside the loopback, whatever
/// it is asked (like artnet.rs).
fn check_route(r: &Route) -> Result<(), String> {
    if cfg!(test) && !r.dest.ip().is_loopback() {
        return Err(format!("tests : DMX vers {} refusé (127.0.0.1 seulement)", r.dest));
    }
    Ok(())
}

impl Sink {
    fn open(route: Route) -> Result<Sink, String> {
        check_route(&route)?;
        let sock = UdpSocket::bind(SocketAddr::new(route.bind, 0)).map_err(|e| format!("impossible d'envoyer depuis {} : {e}", route.bind))?;
        // A full send buffer drops a frame rather than waiting.
        sock.set_nonblocking(true).map_err(|e| format!("socket DMX : {e}"))?;
        Ok(Sink { route, sock, sequence: 0, lost: false })
    }

    fn send(&mut self, cid: &[u8; 16], data: &[u8; UNIVERSE_LEN], terminated: bool) -> std::io::Result<()> {
        let r = &self.route;
        let packet = match r.protocol {
            DmxProtocol::ArtNet => artnet_packet(r.universe, data),
            DmxProtocol::Sacn => {
                // E1.31: sequence numbers count up from 1 and wrap.
                self.sequence = self.sequence.wrapping_add(1);
                sacn_packet(cid, r.universe, self.sequence, terminated, data)
            }
        };
        self.sock.send_to(&packet, r.dest).map(|_| ())
    }

    /// Zeros, `OFF_FRAMES` times (then sACN's stream-terminated packets),
    /// before the socket closes.
    fn close(mut self, cid: &[u8; 16], status: &Mutex<OutStatus>) {
        let zero = [0u8; UNIVERSE_LEN];
        let mut sent = 0;
        for _ in 0..OFF_FRAMES {
            if self.send(cid, &zero, false).is_ok() {
                sent += 1;
            }
        }
        if self.route.protocol == DmxProtocol::Sacn {
            for _ in 0..OFF_FRAMES {
                let _ = self.send(cid, &zero, true);
            }
        }
        log::info!("Sortie DMX : {sent} trames à 0 envoyées vers {}, socket fermée", self.route.dest);
        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
        st.frames += sent;
        (st.sending, st.to, st.dark) = (false, None, true);
    }
}

fn set_error(status: &Mutex<OutStatus>, error: String) {
    let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
    st.errors += 1;
    if st.error.as_ref() != Some(&error) {
        log::warn!("Sortie DMX : {error}");
    }
    st.error = Some(error);
}

/// Starts the "dmx-out" thread. It opens nothing until a snapshot asks
/// for a route. None: it could not be created (logged); the queue then
/// fills and drops, the engine is not affected.
pub fn spawn(rx: Receiver<Snapshot>, status: Arc<Mutex<OutStatus>>, estop: Arc<EStop>, running: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("dmx-out".into())
        .spawn(move || run(rx, &status, &estop, &running))
        .map_err(|e| log::warn!("Sortie DMX : impossible de démarrer le thread : {e}"))
        .ok()
}

fn run(rx: Receiver<Snapshot>, status: &Mutex<OutStatus>, estop: &EStop, running: &AtomicBool) {
    let cid = make_cid();
    let mut snap: Option<Snapshot> = None;
    let mut sink: Option<Sink> = None;
    let mut retry: Option<(Route, Instant)> = None;
    let mut next_send = Instant::now();
    let mut was_dark = false;
    let mut holds_sent = 0;
    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(POLL) {
            Ok(s) => snap = Some(rx.try_iter().last().unwrap_or(s)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        let want = snap.as_ref().and_then(Snapshot::route);
        // Off, or a new destination: zeros to the old one first.
        if sink.as_ref().map(|k| k.route) != want {
            if let Some(old) = sink.take() {
                old.close(&cid, status);
            }
            if let Some(r) = want {
                if !retry.is_some_and(|(rr, at)| rr == r && now < at) {
                    match Sink::open(r) {
                        Ok(k) => {
                            log::info!("Sortie DMX : {} univers {} vers {}", r.protocol.label(), r.universe, r.dest);
                            let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                            (st.to, st.error) = (Some(r.dest.to_string()), None);
                            sink = Some(k);
                            retry = None;
                            next_send = now;
                        }
                        Err(e) => {
                            set_error(status, e);
                            retry = Some((r, now + RETRY));
                        }
                    }
                }
            }
        }
        let (Some(k), Some(s)) = (sink.as_mut(), snap.as_ref()) else {
            let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
            st.sending = false;
            if want.is_none() {
                st.error = None;
            }
            continue;
        };
        // The latch is read here, without the engine: zeros at once.
        let dark = estop.is_latched() || s.zero || now.saturating_duration_since(s.at) > STALE;
        // A new e-stop, disarm or blackout goes out now, not at the next tick.
        if now < next_send && !(dark && !was_dark) && s.holds == holds_sent {
            continue;
        }
        holds_sent = s.holds;
        let period = Duration::from_secs_f64(1.0 / s.cfg.rate_hz.clamp(MIN_RATE_HZ, MAX_RATE_HZ) as f64);
        next_send = now + period;
        let zero = [0u8; UNIVERSE_LEN];
        // Back after a loss: zeros first, the values on the next frame.
        let data: &[u8; UNIVERSE_LEN] = if dark || k.lost { &zero } else { &s.universe };
        match k.send(&cid, data, false) {
            Ok(()) => {
                k.lost = false;
                let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                st.frames += 1;
                (st.sending, st.dark, st.error) = (true, dark, None);
                was_dark = dark;
            }
            Err(e) => {
                k.lost = true;
                set_error(status, format!("envoi vers {} : {e}", k.route.dest));
                status.lock().unwrap_or_else(|e| e.into_inner()).sending = false;
            }
        }
    }
    // Shutdown (Ctrl+C, SIGTERM, a panic winding the loops down): zeros.
    if let Some(k) = sink {
        k.close(&cid, status);
    }
}

/// The addresses of this Mac's interfaces (IPv4).
fn local_ips() -> Vec<String> {
    crate::netin::interfaces().iter().filter_map(|i| i["ip"].as_str().map(str::to_string)).collect()
}

/// Under `dmx_out` of `/api/state` and `/api/frame`: on, sending, the
/// values by channel name, the haze panel.
pub fn summary(s: &Shared) -> Value {
    let d = &s.dmx_out;
    let st = d.status();
    let values: serde_json::Map<String, Value> = d.values.iter().map(|(n, v)| (n.clone(), json!(v))).collect();
    json!({
        "enabled": d.cfg.enabled,
        "sending": d.cfg.enabled && st.sending,
        "rate_hz": d.cfg.rate_hz,
        "values": values,
        "estop": s.estop.is_latched(),
        "blocked": d.blocked,
        "held": d.timeline_held.then_some(HELD),
        "warning": d.warning,
        "error": st.error,
        "haze": {
            "available": d.cfg.haze.pump.is_some(),
            "held": d.haze_held,
            "pump_on": d.pump_on,
            "fan": d.fan,
            "has_fan": d.cfg.haze.fan.is_some(),
        },
    })
}

/// `GET /api/dmx-out`.
pub fn view(s: &Shared) -> Value {
    json!({
        "config": s.dmx_out.cfg,
        "status": s.dmx_out.status(),
        "summary": summary(s),
        "interfaces": crate::netin::interfaces(),
        "max_channels": MAX_CHANNELS,
    })
}

/// `POST /api/dmx-out` `{ "enabled": true, "channels": [...], … }`: the
/// fields given replace the current ones (`haze` is merged field by
/// field). The interface must be an address of this Mac, the target not
/// the laser's; the self-drive trap is refused (409). Returns whether
/// anything changed (the caller saves `dmx_out.json`).
pub fn update(s: &mut Shared, patch: &Value) -> Result<bool, Refused> {
    let obj = patch.as_object().ok_or_else(|| Refused::bad("objet JSON attendu"))?;
    let before = s.dmx_out.cfg.clone();
    let mut v = json!(before);
    for (k, val) in obj {
        if v.get(k).is_none() {
            return Err(Refused::bad(format!("réglage inconnu : {k}")));
        }
        match (k.as_str(), val) {
            ("haze", Value::Object(fields)) => {
                for (hk, hv) in fields {
                    if v["haze"].get(hk).is_none() {
                        return Err(Refused::bad(format!("réglage inconnu : haze.{hk}")));
                    }
                    v["haze"][hk] = hv.clone();
                }
            }
            _ => v[k] = val.clone(),
        }
    }
    let mut cfg: DmxOutSettings = serde_json::from_value(v).map_err(|e| Refused::bad(format!("réglage invalide : {e}")))?;
    // Empty strings mean « none ».
    for f in [&mut cfg.interface, &mut cfg.target, &mut cfg.haze.pump, &mut cfg.haze.fan] {
        *f = f.take().map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    }
    for c in &mut cfg.channels {
        c.name = c.name.trim().to_string();
    }
    cfg.validate()?;
    if let Some(i) = cfg.interface.as_ref().filter(|i| before.interface.as_ref() != Some(*i)) {
        let ip = parse_interface(i)?;
        if !local_ips().iter().any(|l| l.parse::<IpAddr>().ok() == Some(ip)) {
            return Err(Refused::bad(format!("{i} n'est pas une adresse de ce Mac")));
        }
    }
    if let Some(t) = &cfg.target {
        let dest = parse_target(t, cfg.protocol)?;
        if matches!(dest.ip(), IpAddr::V4(ip) if laser_ips(s).contains(&ip)) {
            return Err(Refused::conflict(LASER_TARGET));
        }
    }
    if let Some(why) = conflicts(&cfg, &s.netin.hub.effective()) {
        return Err(Refused::conflict(why));
    }
    if cfg == before {
        return Ok(false);
    }
    if let Some(change) = crate::safety_log::settings_change(&before, &cfg) {
        s.gate.log().record("dmx_out_settings", Some(ArmSource::Ui.id()), change);
    }
    if cfg.enabled != before.enabled {
        log::info!("Sortie DMX {}", if cfg.enabled { "activée" } else { "désactivée" });
    }
    // A channel that is no longer the pump: the button lets go.
    if cfg.haze.pump.is_none() {
        s.dmx_out.haze_held = false;
    }
    s.dmx_out.cfg = cfg;
    s.dmx_out.idle_sent = false;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;
    use crate::timeline::{Dur, Event, Player, Show, TimeBase, Track};

    fn clock(t: f64) -> Clock {
        Clock { t, beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 }
    }

    fn ch(name: &str, address: u16, rest: u8) -> DmxChannel {
        DmxChannel { name: name.into(), address, rest }
    }

    fn rig() -> DmxOutSettings {
        DmxOutSettings {
            channels: vec![ch("Pompe", 1, 0), ch("Ventilateur", 2, 0), ch("Face", 10, 20)],
            haze: HazePatch { pump: Some("Pompe".into()), fan: Some("Ventilateur".into()), pump_level: 200, max_burst_s: 10.0 },
            ..Default::default()
        }
    }

    fn loopback(port: u16, protocol: DmxProtocol) -> DmxOutSettings {
        DmxOutSettings { enabled: true, protocol, interface: Some("127.0.0.1".into()), target: Some(format!("127.0.0.1:{port}")), ..rig() }
    }

    fn dmx_event(id: u64, start: f64, len: f64, channel: &str, value: u8) -> Event {
        Event { id, start, len, source: EventSource::Dmx { channel: channel.into(), value }, ..Default::default() }
    }

    fn dmx_show(events: Vec<Event>) -> Show {
        Show { name: "dmx".into(), time_base: TimeBase::Seconds, tracks: vec![Track { name: "DMX".into(), kind: TrackKind::Dmx, events, ..Default::default() }], ..Default::default() }
    }

    fn value(s: &Shared, name: &str) -> u8 {
        s.dmx_out.values.iter().find(|(n, _)| n == name).map(|(_, v)| *v).unwrap()
    }

    #[test]
    fn artnet_and_sacn_frames_read_back_with_the_input_decoders() {
        let mut data = [0u8; UNIVERSE_LEN];
        (data[0], data[9], data[511]) = (255, 20, 7);
        for universe in [0u16, 1, 300, 0x7FFF] {
            match crate::netin::artnet::decode(&artnet_packet(universe, &data)).unwrap() {
                crate::netin::artnet::Packet::Dmx { universe: u, data: d } => assert_eq!((u, d.as_slice()), (universe, &data[..])),
                other => panic!("{other:?}"),
            }
        }
        let cid = make_cid();
        for universe in [1u16, 2, 63999] {
            let p = sacn_packet(&cid, universe, 42, false, &data);
            let crate::netin::sacn::Packet::Data(d) = crate::netin::sacn::decode(&p).unwrap() else { panic!() };
            assert_eq!((d.universe, d.sequence, d.priority, d.start_code, d.name.as_str()), (universe, 42, 100, 0, "Laser Studio"));
            assert_eq!((d.cid, d.slots.as_slice(), d.terminated, d.preview), (cid, &data[..], false, false));
            let crate::netin::sacn::Packet::Data(t) = crate::netin::sacn::decode(&sacn_packet(&cid, universe, 43, true, &[0; UNIVERSE_LEN])).unwrap() else { panic!() };
            assert!(t.terminated);
        }
    }

    #[test]
    fn settings_are_checked_and_off_by_default() {
        let d = DmxOutSettings::default();
        assert!(!d.enabled && d.route().is_none() && d.validate().is_ok());
        assert_eq!((d.universe, d.rate_hz, d.haze.pump_level, d.haze.max_burst_s), (1, 30, 255, 10.0));
        // An absent or empty file is the default.
        assert_eq!(serde_json::from_str::<DmxOutSettings>("{}").unwrap(), d);
        let missing = DmxOutState::load(crate::test_support::scratch_dir().join("dmx_out.json"));
        assert_eq!(missing.config(), &d);
        assert!(rig().validate().is_ok());
        let bad = |f: &dyn Fn(&mut DmxOutSettings)| {
            let mut c = rig();
            f(&mut c);
            c.validate().is_err()
        };
        assert!(bad(&|c| c.channels.push(ch("Pompe", 5, 0))), "same name");
        assert!(bad(&|c| c.channels.push(ch("Autre", 1, 0))), "same address");
        assert!(bad(&|c| c.channels.push(ch("Trop loin", 513, 0))));
        assert!(bad(&|c| c.channels.push(ch("Zéro", 0, 0))));
        assert!(bad(&|c| c.channels.push(ch("", 30, 0))));
        assert!(bad(&|c| c.channels = (1..=17).map(|i| ch(&format!("C{i}"), i, 0)).collect()));
        assert!(bad(&|c| c.haze.pump = Some("Inconnu".into())));
        assert!(bad(&|c| c.haze.fan = Some("Pompe".into())));
        assert!(bad(&|c| c.haze.max_burst_s = f32::NAN));
        assert!(bad(&|c| c.haze.max_burst_s = 600.0));
        assert!(bad(&|c| c.rate_hz = 0));
        assert!(bad(&|c| c.enabled = true), "nowhere to send");
        assert!(bad(&|c| (c.protocol, c.universe) = (DmxProtocol::Sacn, 0)));
        assert!(bad(&|c| c.universe = 0x8000));
        for t in ["0.0.0.0", "255.255.255.255", "192.168.1.255", "239.255.0.1", "nom.local", "10.0.0.2:0", "::1"] {
            assert!(parse_target(t, DmxProtocol::ArtNet).is_err(), "{t}");
        }
        assert_eq!(parse_target("10.0.0.2", DmxProtocol::Sacn).unwrap(), "10.0.0.2:5568".parse().unwrap());
        assert_eq!(parse_target("10.0.0.2", DmxProtocol::ArtNet).unwrap(), "10.0.0.2:6454".parse().unwrap());
        for i in ["0.0.0.0", "224.0.0.1", "255.255.255.255", "en0"] {
            assert!(parse_interface(i).is_err(), "{i}");
        }
        // A NaN or out-of-range value never reaches the wire: u8 by type.
        assert!(serde_json::from_str::<DmxChannel>(r#"{"name":"x","rest":256}"#).is_err());
        assert!(serde_json::from_str::<DmxChannel>(r#"{"name":"x","rest":-1}"#).is_err());
        assert_eq!((level(f32::NAN), level(300.0), level(-5.0), level(127.6)), (0, 255, 0, 128));
    }

    #[test]
    fn timeline_events_fade_in_and_out_then_rest() {
        let mut e = dmx_event(1, 2.0, 4.0, "Face", 220);
        (e.fade_in, e.fade_out) = (Dur::Seconds(1.0), Dur::Seconds(2.0));
        let mut p = Player::default();
        p.load(dmx_show(vec![e]));
        let cfg = rig();
        let at = |p: &mut Player, t: f64| {
            let c = clock(t);
            p.frame(&c);
            mix(&cfg, &p.dmx(&c), 0, 0).into_iter().find(|(n, _)| n == "Face").unwrap().1
        };
        // Stopped: rest.
        assert_eq!(at(&mut p, 0.0), 20);
        p.play(&clock(0.0));
        assert_eq!(at(&mut p, 1.0), 20, "before the event: rest");
        assert_eq!(at(&mut p, 2.0), 20, "fade in from rest");
        assert_eq!(at(&mut p, 2.5), 120);
        assert_eq!(at(&mut p, 3.0), 220);
        assert_eq!(at(&mut p, 4.0), 220, "the fade out starts at 4 s");
        assert_eq!(at(&mut p, 5.0), 120);
        assert_eq!(at(&mut p, 6.0), 20, "after: rest");
        // Paused mid-fade: frozen (the show ended at 6 s: play it again).
        p.seek(2.5, &clock(10.0));
        p.play(&clock(10.0));
        assert_eq!(at(&mut p, 10.0), 120);
        p.pause(&clock(10.0));
        assert_eq!(at(&mut p, 20.0), 120);
        assert_eq!(at(&mut p, 30.0), 120);
        // Stopped: back to rest.
        p.stop();
        assert_eq!(at(&mut p, 31.0), 20);
        // A muted track plays nothing.
        let mut show = dmx_show(vec![dmx_event(1, 0.0, 10.0, "Face", 255)]);
        show.tracks[0].mute = true;
        p.load(show);
        p.play(&clock(40.0));
        assert_eq!(at(&mut p, 41.0), 20);
    }

    #[test]
    fn the_last_event_started_wins_on_a_track_and_tracks_and_haze_mix_by_maximum() {
        let mut show = dmx_show(vec![dmx_event(1, 0.0, 10.0, "Face", 200), dmx_event(2, 1.0, 2.0, "Face", 50)]);
        show.tracks.push(Track { name: "DMX 2".into(), kind: TrackKind::Dmx, events: vec![dmx_event(3, 0.0, 10.0, "Pompe", 30)], ..Default::default() });
        let mut p = Player::default();
        p.load(show);
        p.play(&clock(0.0));
        let cfg = rig();
        let vals = |p: &mut Player, t: f64, pump: u8| {
            let c = clock(t);
            p.frame(&c);
            mix(&cfg, &p.dmx(&c), pump, 0)
        };
        assert_eq!(vals(&mut p, 0.5, 0)[2].1, 200);
        assert_eq!(vals(&mut p, 1.5, 0)[2].1, 50, "the later start wins, even lower");
        assert_eq!(vals(&mut p, 3.5, 0)[2].1, 200, "the first one again once the second is over");
        // Haze and timeline on the pump: the maximum.
        assert_eq!(vals(&mut p, 4.0, 0)[0].1, 30);
        assert_eq!(vals(&mut p, 4.0, 200)[0].1, 200);
        // A second track on the same channel: the maximum too.
        let mut show = dmx_show(vec![dmx_event(1, 0.0, 10.0, "Face", 60)]);
        show.tracks.push(Track { kind: TrackKind::Dmx, events: vec![dmx_event(2, 0.0, 10.0, "Face", 90)], ..Default::default() });
        p.load(show);
        p.play(&clock(0.0));
        assert_eq!(vals(&mut p, 1.0, 0)[2].1, 90);
        // A DMX event never becomes a cue to draw.
        assert!(p.frame(&clock(1.5)).is_empty());
    }

    #[test]
    fn the_haze_burst_ends_by_itself_after_the_max_time() {
        let mut s = shared();
        s.dmx_out.cfg = rig();
        frame(&mut s, &clock(0.0));
        assert_eq!(value(&s, "Pompe"), 0);
        set_haze(&mut s, true);
        frame(&mut s, &clock(1.0));
        assert_eq!(value(&s, "Pompe"), 200);
        frame(&mut s, &clock(10.9));
        assert_eq!(value(&s, "Pompe"), 200);
        // Held past 10 s: back to 0 while still held.
        frame(&mut s, &clock(11.0));
        assert_eq!(value(&s, "Pompe"), 0);
        assert!(haze_held(&s));
        frame(&mut s, &clock(60.0));
        assert_eq!(value(&s, "Pompe"), 0);
        // Let go and pressed again: a new burst.
        set_haze(&mut s, false);
        frame(&mut s, &clock(61.0));
        set_haze(&mut s, true);
        frame(&mut s, &clock(62.0));
        assert_eq!(value(&s, "Pompe"), 200);
        set_fan(&mut s, 0.5).unwrap();
        assert!(set_fan(&mut s, f32::NAN).is_err());
        frame(&mut s, &clock(63.0));
        assert_eq!(value(&s, "Ventilateur"), 128);
        // Without a pump channel the button does nothing.
        s.dmx_out.cfg.haze.pump = None;
        frame(&mut s, &clock(64.0));
        assert_eq!(value(&s, "Pompe"), 0);
    }

    #[test]
    fn the_emergency_stop_zeroes_every_channel_and_releases_the_haze() {
        let mut s = shared();
        s.dmx_out.cfg = rig();
        s.dmx_out.cfg.channels[2].rest = 255;
        set_haze(&mut s, true);
        set_fan(&mut s, 1.0).unwrap();
        frame(&mut s, &clock(0.0));
        assert_eq!(s.dmx_out.values.iter().map(|v| v.1).collect::<Vec<_>>(), [200, 255, 255]);
        let armed_before = s.gate.is_armed();
        s.emergency_stop(ArmSource::Ui);
        frame(&mut s, &clock(0.1));
        assert!(s.dmx_out.values.iter().all(|v| v.1 == 0));
        assert_eq!(summary(&s)["values"]["Face"], 0);
        // While latched, the button does nothing.
        set_haze(&mut s, true);
        frame(&mut s, &clock(0.2));
        assert!(s.dmx_out.values.iter().all(|v| v.1 == 0));
        assert!(!haze_held(&s) && s.dmx_out.fan == 0.0);
        // Nothing here arms.
        assert_eq!(s.gate.is_armed(), armed_before);
    }

    use crate::interlock::DisarmReason;

    /// The rig with a 10 s DMX event on « Face » (rest 20) playing, armed,
    /// the haze held and the fan on: Pompe 200, Ventilateur 255, Face 220.
    fn lit_rig(t: f64) -> Shared {
        let mut s = shared();
        s.dmx_out.cfg = rig();
        frame(&mut s, &clock(t));
        s.gate.request_arm(ArmSource::Ui).unwrap();
        s.timeline.load(dmx_show(vec![dmx_event(1, 0.0, 100.0, "Face", 220)]));
        s.timeline.play(&clock(t));
        set_haze(&mut s, true);
        set_fan(&mut s, 1.0).unwrap();
        step(&mut s, t);
        assert_eq!(all(&s), [200, 255, 220]);
        s
    }

    /// One engine frame, as `timeline_cues` runs it (the e-stop halts the timeline).
    fn step(s: &mut Shared, t: f64) {
        let c = clock(t);
        if s.estop.is_latched() {
            s.timeline.halt(&c);
        }
        s.timeline.frame(&c);
        frame(s, &c);
    }

    fn all(s: &Shared) -> Vec<u8> {
        s.dmx_out.values.iter().map(|v| v.1).collect()
    }

    /// Rest values (Face 20), pump and fan 0, the haze button let go.
    fn at_rest(s: &Shared, why: &str) {
        assert_eq!(all(s), [0, 0, 20], "{why}");
        assert!(!haze_held(s) && s.dmx_out.fan == 0.0, "{why}");
        assert_eq!(summary(s)["held"], HELD, "{why}");
    }

    #[test]
    fn every_disarm_sends_the_rest_values_and_holds_them_until_a_new_explicit_action() {
        let reasons = [
            DisarmReason::User,
            DisarmReason::UiLost,
            DisarmReason::HoldReleased,
            DisarmReason::Interlock("presence".into()),
            DisarmReason::ProfileChange,
            DisarmReason::OutputChanged,
            DisarmReason::OutputLost,
            DisarmReason::Shutdown,
            DisarmReason::Schedule,
            DisarmReason::EngineStall,
        ];
        for reason in reasons {
            let why = reason.id();
            let mut s = lit_rig(0.0);
            s.gate.disarm(reason, ArmSource::System);
            step(&mut s, 0.1);
            at_rest(&s, &why);
            // Held, whatever the timeline does by itself.
            for t in [0.5, 1.0, 5.0] {
                step(&mut s, t);
                at_rest(&s, &why);
            }
            // Re-arming alone brings nothing back; starting again while
            // disarmed neither.
            s.timeline.stop();
            s.timeline.play(&clock(5.1));
            step(&mut s, 5.2);
            at_rest(&s, &why);
            s.gate.request_arm(ArmSource::Ui).unwrap();
            step(&mut s, 5.3);
            at_rest(&s, &why);
            // Lecture, armed: the event again. The haze: only a new press.
            s.timeline.stop();
            s.timeline.play(&clock(6.0));
            step(&mut s, 6.1);
            assert_eq!(all(&s), [0, 0, 220], "{why}");
            assert!(summary(&s)["held"].is_null());
            set_haze(&mut s, true);
            step(&mut s, 6.2);
            assert_eq!(value(&s, "Pompe"), 200, "{why}");
        }
    }

    #[test]
    fn the_other_disarm_paths_hold_too_interlock_watchdog_noir_while_disarmed() {
        // An interlock dropping while armed (the output lost its link).
        let mut s = lit_rig(0.0);
        s.gate.set_output_ready(false);
        assert!(!s.gate.is_armed());
        step(&mut s, 0.1);
        at_rest(&s, "interlock");
        // The engine watchdog.
        let mut s = lit_rig(0.0);
        s.gate.watchdog_trip();
        step(&mut s, 0.1);
        at_rest(&s, "watchdog");
        // « Noir » while already disarmed (the haze running in a rehearsal).
        let mut s = shared();
        s.dmx_out.cfg = rig();
        step(&mut s, 0.0);
        set_haze(&mut s, true);
        set_fan(&mut s, 0.5).unwrap();
        step(&mut s, 0.1);
        assert_eq!(all(&s), [200, 128, 20]);
        // Frames while disarmed are not disarms: the haze keeps running.
        step(&mut s, 0.2);
        assert_eq!(all(&s), [200, 128, 20]);
        s.gate.disarm(DisarmReason::User, ArmSource::Ui);
        step(&mut s, 0.3);
        at_rest(&s, "noir");
        // A new press (and the fan moved again) bring the haze back.
        set_haze(&mut s, true);
        set_fan(&mut s, 0.5).unwrap();
        step(&mut s, 0.4);
        assert_eq!(all(&s), [200, 128, 20]);
    }

    #[test]
    fn a_blackout_zeroes_then_holds_at_rest_after_the_reset() {
        let mut s = lit_rig(0.0);
        crate::controls::apply(&mut s, "transport.blackout", crate::controls::ControlInput::Norm(1.0), true).unwrap();
        step(&mut s, 0.1);
        assert_eq!(all(&s), [0, 0, 0], "latched: zeros");
        let reset = s.estop.clone();
        s.gate.reset_estop(&reset);
        step(&mut s, 0.2);
        at_rest(&s, "after the reset");
        s.gate.request_arm(ArmSource::Ui).unwrap();
        step(&mut s, 0.3);
        at_rest(&s, "re-armed, not started");
        s.timeline.play(&clock(0.4));
        step(&mut s, 0.5);
        assert_eq!(value(&s, "Face"), 220);
        // The external inputs' blackout channel latches the same stop.
        s.emergency_stop(ArmSource::Dmx);
        step(&mut s, 0.6);
        assert_eq!(all(&s), [0, 0, 0]);
    }

    /// Property (SAFETY_CASES): random operation of the rig. After every
    /// frame: e-stop latched → zeros; otherwise each channel is what the
    /// model allows: Face at its event only once started armed after the
    /// last disarm, the pump only from a press since then, the fan only
    /// from a move since then.
    #[test]
    fn property_nothing_comes_back_after_a_disarm_without_a_new_action() {
        for seed in crate::safety_invariants::seeds(453, 40) {
            let mut rng = crate::safety_invariants::Rng::new(seed);
            let mut s = shared();
            s.dmx_out.cfg = rig();
            s.dmx_out.cfg.haze.max_burst_s = 1000.0;
            s.timeline.load(dmx_show(vec![dmx_event(1, 0.0, 1.0e6, "Face", 220)]));
            let mut t = 0.0;
            step(&mut s, t);
            // The model.
            let (mut started_ok, mut pump, mut fan) = (true, false, 0.0f32);
            for _ in 0..200 {
                t += 0.05;
                match rng.below(10) {
                    0 => {
                        if s.gate.request_arm(ArmSource::Ui).is_ok() {}
                    }
                    1 => {
                        s.gate.disarm(DisarmReason::User, ArmSource::Ui);
                        (started_ok, pump, fan) = (false, false, 0.0);
                    }
                    2 => {
                        s.emergency_stop(ArmSource::Ui);
                        (started_ok, pump, fan) = (false, false, 0.0);
                    }
                    3 => {
                        let e = s.estop.clone();
                        s.gate.reset_estop(&e);
                    }
                    4 => {
                        set_haze(&mut s, true);
                        pump = !s.estop.is_latched();
                    }
                    5 => {
                        set_haze(&mut s, false);
                        pump = false;
                    }
                    6 => {
                        let v = rng.f();
                        set_fan(&mut s, v).unwrap();
                        // Latched, the next frame puts it back to 0.
                        fan = if s.estop.is_latched() { 0.0 } else { v };
                    }
                    7 => {
                        let was = s.timeline.is_playing();
                        s.timeline.play(&clock(t));
                        if !was && s.gate.is_armed() {
                            started_ok = true;
                        }
                    }
                    8 => s.timeline.stop(),
                    _ => {
                        let reason = rng.pick(&[DisarmReason::OutputLost, DisarmReason::Schedule, DisarmReason::ProfileChange]).clone();
                        s.gate.disarm(reason, ArmSource::System);
                        (started_ok, pump, fan) = (false, false, 0.0);
                    }
                }
                step(&mut s, t);
                let v = all(&s);
                if s.estop.is_latched() {
                    assert_eq!(v, [0, 0, 0], "seed {seed}: e-stop");
                    continue;
                }
                let face = if started_ok && s.timeline.is_playing() { 220 } else { 20 };
                let want = [if pump { 200 } else { 0 }, level(fan * 255.0), face];
                assert_eq!(v, want, "seed {seed}");
            }
        }
    }

    #[test]
    fn the_output_universe_must_differ_from_the_input_ones_both_ways() {
        let mut s = shared();
        // Art-Net input on universe 1, output Art-Net 1: refused.
        crate::netin::update(&mut s, &json!({ "artnet": { "enabled": true, "universe": 1 } })).unwrap();
        let on = json!({ "enabled": true, "interface": "127.0.0.1", "target": "127.0.0.1:7000", "universe": 1 });
        let err = update(&mut s, &on).unwrap_err();
        assert_eq!((err.status, err.message.as_str()), (409, SELF_DRIVE));
        assert!(!s.dmx_out.config().enabled);
        // sACN on the same number: another protocol, allowed.
        assert_eq!(update(&mut s, &json!({ "enabled": true, "protocol": "sacn", "interface": "127.0.0.1", "target": "127.0.0.1:7000", "universe": 1 })), Ok(true));
        // The input can't take the output's universe now (sACN list).
        let err = crate::netin::update(&mut s, &json!({ "sacn": { "enabled": true, "universes": [1] } })).unwrap_err();
        assert_eq!(err, SELF_DRIVE);
        crate::netin::update(&mut s, &json!({ "sacn": { "enabled": true, "universes": [2] } })).unwrap();
        // Nor patch a mapping on it.
        let mapping = json!({ "dmx": { "mappings": [{ "universe": 1, "channel": 5, "control": "master.size" }] } });
        assert_eq!(crate::netin::update(&mut s, &mapping).unwrap_err(), SELF_DRIVE);
        // Disabled, there is no trap.
        update(&mut s, &json!({ "enabled": false })).unwrap();
        crate::netin::update(&mut s, &mapping).unwrap();
        assert_eq!(update(&mut s, &json!({ "enabled": true })).unwrap_err().status, 409);
    }

    #[test]
    fn the_api_validates_and_refuses_the_laser_address() {
        let mut s = shared();
        assert_eq!(update(&mut s, &json!({ "nope": 1 })).unwrap_err().status, 400);
        assert_eq!(update(&mut s, &json!({ "haze": { "nope": 1 } })).unwrap_err().status, 400);
        assert_eq!(update(&mut s, &json!({ "interface": "8.8.8.8" })).unwrap_err().status, 400);
        assert_eq!(update(&mut s, &json!({ "enabled": true })).unwrap_err().status, 400, "nowhere to send");
        assert_eq!(update(&mut s, &json!({ "channels": [{ "name": "P", "address": 1, "rest": 300 }] })).unwrap_err().status, 400);
        assert_eq!(update(&mut s, &json!({ "channels": [{ "name": "P", "address": 1 }], "haze": { "pump": "P" } })), Ok(true));
        assert_eq!(s.dmx_out.config().haze.max_burst_s, 10.0, "merged field by field");
        assert_eq!(update(&mut s, &json!({ "haze": { "fan": "" } })), Ok(false), "empty is none");
        // The ShowNET card output's address is the laser's.
        s.shownet_dmx.set(crate::artnet::ShowNetDmxConfig { target: Some("10.9.8.7".into()), ..Default::default() }).unwrap();
        let err = update(&mut s, &json!({ "target": "10.9.8.7:6454" })).unwrap_err();
        assert_eq!((err.status, err.message.as_str()), (409, LASER_TARGET));
        assert_eq!(update(&mut s, &json!({ "enabled": true, "interface": "127.0.0.1", "target": "127.0.0.1:7000" })), Ok(true));
        assert_eq!(summary(&s)["enabled"], true);
        assert_eq!(ipv4s_in("idn:192.168.1.5:7255"), vec![Ipv4Addr::new(192, 168, 1, 5)]);
    }

    #[test]
    fn a_show_with_dmx_events_plays_without_a_dmx_output_and_warns() {
        let mut s = shared();
        s.timeline.load(dmx_show(vec![dmx_event(1, 0.0, 10.0, "Fumée", 255)]));
        let c = clock(0.0);
        s.timeline.play(&c);
        s.timeline.frame(&c);
        frame(&mut s, &c);
        assert!(s.dmx_out.values.is_empty());
        assert!(summary(&s)["warning"].as_str().unwrap().contains("désactivée"));
        s.dmx_out.cfg = loopback(9, DmxProtocol::ArtNet);
        frame(&mut s, &c);
        assert!(summary(&s)["warning"].as_str().unwrap().contains("Fumée"));
    }

    #[test]
    fn old_shows_still_load_and_dmx_tracks_round_trip() {
        let old = r#"{"name":"vieux","tracks":[{"name":"P","kind":"cues","layer":1,"events":[{"id":1,"start":0,"len":2,"source":{"kind":"cue","id":"x"}}]},{"kind":"bus","layer":2}]}"#;
        let show: Show = serde_json::from_str(old).unwrap();
        assert_eq!((show.tracks[0].kind, show.tracks[1].kind), (TrackKind::Cues, TrackKind::Bus));
        let mut show = dmx_show(vec![dmx_event(1, 0.0, 2.0, "Pompe", 255)]);
        show.sanitize();
        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        assert_eq!(back, show);
        assert!(serde_json::to_string(&show).unwrap().contains(r#""kind":"dmx""#));
        // The editor: DMX events only on DMX tracks, cues only on cue tracks.
        assert!(show.validate_edit(|_| true).is_ok());
        let mut wrong = show.clone();
        wrong.tracks[0].kind = TrackKind::Cues;
        assert!(wrong.validate_edit(|_| true).is_err());
        let mut wrong = show.clone();
        wrong.tracks[0].events[0].source = EventSource::Cue { id: "x".into() };
        assert!(wrong.validate_edit(|_| true).is_err());
        let mut wrong = show;
        wrong.tracks[0].events[0].source = EventSource::Dmx { channel: " ".into(), value: 1 };
        assert!(wrong.validate_edit(|_| true).is_err());
    }

    #[test]
    fn the_engine_publishes_without_ever_waiting_and_tells_off_once() {
        let mut s = shared();
        let rx = s.dmx_out.take_receiver().unwrap();
        frame(&mut s, &clock(0.0));
        assert!(rx.try_recv().unwrap().route().is_none());
        frame(&mut s, &clock(0.1));
        assert!(rx.try_recv().is_err(), "off: told once");
        s.dmx_out.cfg = loopback(9, DmxProtocol::ArtNet);
        frame(&mut s, &clock(0.2));
        assert!(rx.try_recv().unwrap().route().is_some());
        // Nobody reads: the queue fills and drops, the engine goes on.
        let t0 = Instant::now();
        for i in 0..QUEUE_LEN + 20 {
            frame(&mut s, &clock(0.3 + i as f64 * 0.01));
        }
        assert!(s.dmx_out.dropped >= 20);
        assert!(t0.elapsed() < Duration::from_millis(500));
    }

    /// A receiver on 127.0.0.1 (a port the system picks) and its frames.
    fn receiver() -> (UdpSocket, u16) {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let port = rx.local_addr().unwrap().port();
        assert!(port != ARTNET_PORT && port != SACN_PORT);
        (rx, port)
    }

    fn next(rx: &UdpSocket) -> Option<(Vec<u8>, bool)> {
        let mut buf = [0u8; 1024];
        let n = rx.recv(&mut buf).ok()?;
        match crate::netin::artnet::decode(&buf[..n]) {
            Ok(crate::netin::artnet::Packet::Dmx { data, .. }) => Some((data, false)),
            _ => match crate::netin::sacn::decode(&buf[..n]) {
                Ok(crate::netin::sacn::Packet::Data(d)) => Some((d.slots, d.terminated)),
                other => panic!("{other:?}"),
            },
        }
    }

    /// Frames until one matches (or none comes for 500 ms).
    fn until(rx: &UdpSocket, f: impl Fn(&[u8]) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match next(rx) {
                Some((d, _)) if f(&d) => return true,
                Some(_) => {}
                None => return false,
            }
        }
        false
    }

    fn snapshot(cfg: &DmxOutSettings, values: &[(String, u8)]) -> Snapshot {
        Snapshot { cfg: cfg.clone(), blocked: None, universe: universe_of(cfg, values), zero: false, holds: 0, at: Instant::now() }
    }

    /// A disarm or blackout (a new `holds`) goes out at once, even at
    /// 1 frame a second, and the thread keeps sending the rest values.
    #[test]
    fn loopback_a_disarm_or_blackout_is_sent_at_once_and_kept() {
        let (rx, port) = receiver();
        let cfg = DmxOutSettings { rate_hz: 1, ..loopback(port, DmxProtocol::ArtNet) };
        let (tx, qrx) = sync_channel(QUEUE_LEN);
        let status = Arc::new(Mutex::new(OutStatus::default()));
        let (estop, running) = (Arc::new(EStop::default()), Arc::new(AtomicBool::new(true)));
        let handle = spawn(qrx, Arc::clone(&status), estop, Arc::clone(&running)).unwrap();
        let lit = vec![("Pompe".to_string(), 200u8), ("Face".to_string(), 220)];
        let rest = vec![("Pompe".to_string(), 0u8), ("Face".to_string(), 20)];
        tx.send(snapshot(&cfg, &lit)).unwrap();
        assert!(until(&rx, |d| d[0] == 200 && d[9] == 220));
        // Just after a frame: the next tick is ~1 s away.
        let mut held = snapshot(&cfg, &rest);
        held.holds = 1;
        let t0 = Instant::now();
        tx.send(held).unwrap();
        assert!(until(&rx, |d| d[0] == 0 && d[9] == 20));
        assert!(t0.elapsed() < Duration::from_millis(300), "at once: {:?}", t0.elapsed());
        // Kept: the following frames (one a second) carry the rest values too.
        let mut frames: Vec<(u8, u8)> = Vec::new();
        let t1 = Instant::now();
        rx.set_nonblocking(true).unwrap();
        while t1.elapsed() < Duration::from_millis(2300) {
            let mut again = snapshot(&cfg, &rest);
            again.holds = 1;
            tx.send(again).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            let mut buf = [0u8; 1024];
            while let Ok(n) = rx.recv(&mut buf) {
                if let Ok(crate::netin::artnet::Packet::Dmx { data, .. }) = crate::netin::artnet::decode(&buf[..n]) {
                    frames.push((data[0], data[9]));
                }
            }
        }
        rx.set_nonblocking(false).unwrap();
        assert!(frames.len() >= 2, "{} frames", frames.len());
        assert!(frames.iter().all(|f| *f == (0, 20)), "{frames:?}");
        running.store(false, Ordering::SeqCst);
        handle.join().unwrap();
    }

    /// The real thread, on loopback: values at the rate, zeros on the
    /// e-stop latch (without the engine), zeros at shutdown.
    #[test]
    fn loopback_the_thread_sends_values_then_zeros_on_estop_and_at_shutdown() {
        for protocol in [DmxProtocol::ArtNet, DmxProtocol::Sacn] {
            let (rx, port) = receiver();
            let cfg = loopback(port, protocol);
            let (tx, qrx) = sync_channel(QUEUE_LEN);
            let status = Arc::new(Mutex::new(OutStatus::default()));
            let (estop, running) = (Arc::new(EStop::default()), Arc::new(AtomicBool::new(true)));
            let handle = spawn(qrx, Arc::clone(&status), Arc::clone(&estop), Arc::clone(&running)).unwrap();
            let values = vec![("Pompe".to_string(), 200u8), ("Face".to_string(), 77)];
            let feed = |n: usize| {
                for _ in 0..n {
                    tx.send(snapshot(&cfg, &values)).unwrap();
                    std::thread::sleep(Duration::from_millis(10));
                }
            };
            feed(10);
            assert!(until(&rx, |d| d.len() == UNIVERSE_LEN && d[0] == 200 && d[9] == 77 && d[1] == 0), "{protocol:?}: values");
            // About 30 frames a second.
            let t0 = Instant::now();
            let mut n = 0;
            while t0.elapsed() < Duration::from_millis(500) {
                tx.send(snapshot(&cfg, &values)).unwrap();
                while next_now(&rx) {
                    n += 1;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!((8..=25).contains(&n), "{protocol:?}: {n} frames in 0.5 s");
            assert!(status.lock().unwrap().sending);
            // E-stop latched: zeros at once, the engine still sending values.
            estop.trip(ArmSource::Ui);
            let t0 = Instant::now();
            feed(1);
            assert!(until(&rx, |d| d.iter().all(|&v| v == 0)), "{protocol:?}: zeros on e-stop");
            assert!(t0.elapsed() < Duration::from_millis(200));
            feed(10);
            for _ in 0..5 {
                assert!(next(&rx).unwrap().0.iter().all(|&v| v == 0), "zeros while latched");
            }
            // Shutdown: frames of zeros, then nothing.
            running.store(false, Ordering::SeqCst);
            handle.join().unwrap();
            let mut tail = Vec::new();
            while let Some(f) = next(&rx) {
                tail.push(f);
            }
            assert!(tail.iter().filter(|(d, t)| !*t && d.iter().all(|&v| v == 0)).count() >= OFF_FRAMES, "{protocol:?}: {} frames at close", tail.len());
            assert!(tail.iter().all(|(d, _)| d.iter().all(|&v| v == 0)));
            assert!(!status.lock().unwrap().sending);
        }
    }

    fn next_now(rx: &UdpSocket) -> bool {
        rx.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 1024];
        let got = rx.recv(&mut buf).is_ok();
        rx.set_nonblocking(false).unwrap();
        got
    }

    #[test]
    fn loopback_disabling_or_a_stale_engine_sends_zeros_and_no_socket_while_off() {
        let (rx, port) = receiver();
        let cfg = loopback(port, DmxProtocol::ArtNet);
        let (tx, qrx) = sync_channel(QUEUE_LEN);
        let status = Arc::new(Mutex::new(OutStatus::default()));
        let (estop, running) = (Arc::new(EStop::default()), Arc::new(AtomicBool::new(true)));
        let handle = spawn(qrx, Arc::clone(&status), estop, Arc::clone(&running)).unwrap();
        // Off: nothing at all.
        tx.send(snapshot(&DmxOutSettings { enabled: false, ..cfg.clone() }, &[])).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(next(&rx).is_none());
        assert!(status.lock().unwrap().to.is_none(), "no socket while off");
        // On, then the engine stops feeding: zeros after STALE.
        tx.send(snapshot(&cfg, &[("Pompe".to_string(), 255)])).unwrap();
        assert!(until(&rx, |d| d[0] == 255));
        assert!(until(&rx, |d| d[0] == 0), "a stale engine: zeros");
        // On again, then switched off: zeros, then silence.
        tx.send(snapshot(&cfg, &[("Pompe".to_string(), 255)])).unwrap();
        assert!(until(&rx, |d| d[0] == 255));
        tx.send(snapshot(&DmxOutSettings { enabled: false, ..cfg.clone() }, &[("Pompe".to_string(), 255)])).unwrap();
        assert!(until(&rx, |d| d[0] == 0));
        std::thread::sleep(Duration::from_millis(50));
        while next_now(&rx) {}
        std::thread::sleep(Duration::from_millis(150));
        assert!(!next_now(&rx), "silent once off");
        assert!(status.lock().unwrap().to.is_none());
        running.store(false, Ordering::SeqCst);
        handle.join().unwrap();
    }

    #[test]
    fn nothing_in_the_test_build_leaves_the_loopback() {
        let r = Route { bind: "127.0.0.1".parse().unwrap(), dest: "10.0.0.2:6454".parse().unwrap(), protocol: DmxProtocol::ArtNet, universe: 1 };
        assert!(Sink::open(r).is_err());
    }
}
