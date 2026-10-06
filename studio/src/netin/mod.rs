//! External inputs over the network (T-013, T-406): OSC, Art-Net and sACN,
//! for a lighting console or another app. Every write goes through the
//! local gate (external.rs, T-408): `external::apply` checks the gate, the
//! source's access and its scope, then calls `controls::apply_as`, the
//! entry point MIDI and the UI use. There is no network-only control path.
//! The DMX fixture instances of the projection zones (dmx/, T-407) read the
//! same universes (`patch::universe`) and go through the same gate.
//!
//! Safety:
//! - Nothing from the network arms the laser. `transport.arm` is not an
//!   external control and `/ls/arm` is refused by the gate (and logged);
//!   the hold-to-run pad (`safety.hold`) is refused too, since a lost UDP
//!   release would keep the beam on. Blackout and the emergency stop are
//!   accepted at all times, gate closed or not: the listener trips the
//!   e-stop latch itself, without waiting for the engine, the queue or the
//!   `Shared` lock.
//! - Off by default, listening on 127.0.0.1 by default; the operator picks
//!   another address (a console on the LAN) in RÉGLAGES or with `--osc` /
//!   `--artnet`. sACN joins its multicast groups only when the operator
//!   ticks it, on the interface they picked. Art-Net and sACN only listen
//!   (no ArtPollReply, no sACN discovery). OSC sends only the feedback
//!   (feedback.rs, T-416): off by default, to the destinations the
//!   operator typed and the clients that subscribed, never a broadcast.
//! - Signal loss (Art-Net and sACN are streams): no packet for
//!   `loss.timeout_ms` applies `loss.policy`: « neutre » (default) puts the
//!   chart's channels back to neutral — intensity 0, size 100 %, centred,
//!   rotation stopped, normal speed — and leaves cues, page and colour as
//!   they are; « garder » keeps the last values; « fondu au noir » and
//!   « noir immédiat » lower the output gain (`gain`, applied to the
//!   master brightness before the safety stage; never a disarm, never an
//!   arm); « blackout » latches the emergency stop. The next packet takes
//!   over again; arming never changes. OSC sends commands, not a stream, so
//!   it has nothing to lose; pads it holds pressed are released when OSC
//!   is switched off or the gate closes.
//!
//! Threads: one listener per protocol (listener.rs) decodes datagrams and
//! pushes them into a bounded queue (dropped when full, counted); the
//! engine thread drains it once per frame (`frame`), applying at most the
//! newest packet of each universe and `MAX_EVENTS_PER_FRAME` events.
//! Neither the engine nor the HTTP worker ever waits on a socket.

pub mod artnet;
pub mod feedback;
pub mod listener;
pub mod monitor;
pub mod osc;
pub mod patch;
pub mod sacn;

use crate::controls::{self, ControlInput, ControlKind};
use crate::external::{self, SourceKind};
use crate::interlock::{ArmSource, EStop};
use crate::Shared;
use artnet::{Function, CHART};
use osc::{OscArg, OscCommand, OscMessage};
use patch::{PatchConfig, PatchState, UniverseEvent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
pub const LOSS_FADE_MAX_MS: u64 = 30_000;
/// sACN universes listed at most.
pub const MAX_SACN_UNIVERSES: usize = 32;
/// Sources kept for the monitor at most, and how long a silent one stays.
const MAX_SOURCES: usize = 64;
const SOURCE_SHOWN: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    Osc,
    ArtNet,
    Sacn,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Osc => "OSC",
            Proto::ArtNet => "Art-Net",
            Proto::Sacn => "sACN",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Proto::Osc => "osc",
            Proto::ArtNet => "artnet",
            Proto::Sacn => "sacn",
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SacnConfig {
    pub enabled: bool,
    /// The interface's address: 127.0.0.1 by default (unicast from this
    /// Mac only). Multicast needs the address of the console's interface.
    pub bind: String,
    pub port: u16,
    /// Join the universes' multicast groups on that interface. Off: the
    /// console must send unicast to this Mac.
    pub multicast: bool,
    /// Universes listened to (the patch's universes are always accepted).
    pub universes: Vec<u16>,
}

impl Default for SacnConfig {
    fn default() -> Self {
        SacnConfig { enabled: false, bind: DEFAULT_BIND.into(), port: sacn::PORT, multicast: false, universes: vec![1] }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossPolicy {
    Hold,
    #[default]
    Neutral,
    /// Fade the output to black over `fade_ms` (gain, not a disarm).
    FadeOut,
    /// Output gain to 0 at once (not a disarm).
    Dark,
    Blackout,
}

pub const LOSS_POLICIES: [LossPolicy; 5] = [LossPolicy::Neutral, LossPolicy::Hold, LossPolicy::FadeOut, LossPolicy::Dark, LossPolicy::Blackout];

impl LossPolicy {
    pub fn label_fr(self) -> &'static str {
        match self {
            LossPolicy::Hold => "garder les dernières valeurs",
            LossPolicy::Neutral => "revenir au neutre (intensité 0)",
            LossPolicy::FadeOut => "fondu au noir",
            LossPolicy::Dark => "noir immédiat",
            LossPolicy::Blackout => "blackout (arrêt d'urgence)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LossConfig {
    pub policy: LossPolicy,
    pub timeout_ms: u64,
    /// « Fondu au noir »: its length.
    pub fade_ms: u64,
}

impl Default for LossConfig {
    fn default() -> Self {
        LossConfig { policy: LossPolicy::Neutral, timeout_ms: 2500, fade_ms: 2000 }
    }
}

/// `<data-dir>/netin.json`: a setting of this machine, never in a project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetInConfig {
    pub osc: OscConfig,
    pub artnet: ArtNetConfig,
    pub sacn: SacnConfig,
    /// The free DMX patch (patch.rs).
    pub dmx: PatchConfig,
    pub loss: LossConfig,
    /// OSC feedback (T-416).
    pub feedback: feedback::OscFeedback,
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
        let sacn_addr = parse_bind(&self.sacn.bind, self.sacn.port)?;
        if self.artnet.universe > artnet::MAX_UNIVERSE {
            return Err(format!("univers Art-Net invalide : {} (0 à {})", self.artnet.universe, artnet::MAX_UNIVERSE));
        }
        if !(1..=artnet::MAX_ADDRESS).contains(&self.artnet.address) {
            return Err(format!("adresse DMX invalide : {} (1 à {}, {} canaux)", self.artnet.address, artnet::MAX_ADDRESS, CHART.len()));
        }
        if self.sacn.universes.len() > MAX_SACN_UNIVERSES {
            return Err(format!("trop d'univers sACN ({MAX_SACN_UNIVERSES} au plus)"));
        }
        if let Some(u) = self.sacn.universes.iter().find(|u| !(sacn::MIN_UNIVERSE..=sacn::MAX_UNIVERSE).contains(*u)) {
            return Err(format!("univers sACN invalide : {u} (1 à {})", sacn::MAX_UNIVERSE));
        }
        let unicast_v4 = matches!(sacn_addr.ip(), IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified());
        if self.sacn.multicast && !unicast_v4 {
            return Err("multidiffusion sACN : choisissez l'adresse IPv4 de l'interface reliée à la console (ni 127.0.0.1, ni 0.0.0.0)".into());
        }
        self.dmx.validate()?;
        self.feedback.validate()?;
        if !(LOSS_TIMEOUT_MIN_MS..=LOSS_TIMEOUT_MAX_MS).contains(&self.loss.timeout_ms) {
            return Err(format!("délai de perte du signal invalide : {} ms ({} à {} ms)", self.loss.timeout_ms, LOSS_TIMEOUT_MIN_MS, LOSS_TIMEOUT_MAX_MS));
        }
        if self.loss.fade_ms > LOSS_FADE_MAX_MS {
            return Err(format!("durée du fondu invalide : {} ms (0 à {LOSS_FADE_MAX_MS} ms)", self.loss.fade_ms));
        }
        Ok(())
    }

    /// Where a protocol listens, if it is on.
    pub fn listen(&self, proto: Proto) -> Option<SocketAddr> {
        let (on, bind, port) = match proto {
            Proto::Osc => (self.osc.enabled, &self.osc.bind, self.osc.port),
            Proto::ArtNet => (self.artnet.enabled, &self.artnet.bind, self.artnet.port),
            Proto::Sacn => (self.sacn.enabled, &self.sacn.bind, self.sacn.port),
        };
        on.then(|| parse_bind(bind, port).ok()).flatten()
    }

    pub fn enabled(&self, proto: Proto) -> bool {
        match proto {
            Proto::Osc => self.osc.enabled,
            Proto::ArtNet => self.artnet.enabled,
            Proto::Sacn => self.sacn.enabled,
        }
    }

    /// sACN multicast: the interface and the universes to join, only when
    /// sACN is on and the operator asked for it.
    pub fn multicast(&self) -> Option<(Ipv4Addr, Vec<u16>)> {
        if !(self.sacn.enabled && self.sacn.multicast) {
            return None;
        }
        match parse_bind(&self.sacn.bind, self.sacn.port).ok()?.ip() {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some((v4, self.sacn.universes.clone())),
            _ => None,
        }
    }

    /// Whether a universe is listened to by sACN.
    pub fn sacn_wants(&self, universe: u16) -> bool {
        self.sacn.universes.contains(&universe) || self.dmx.universes().contains(&universe)
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
    /// Not OSC / not Art-Net / not sACN, or malformed.
    pub rejected: u64,
    /// Lost because the engine's queue was full.
    pub dropped: u64,
    /// Art-Net and sACN: packets for a universe nobody listens to.
    pub ignored: u64,
    pub last_from: Option<String>,
    /// sACN: multicast groups joined.
    pub groups: Vec<String>,
    #[serde(skip)]
    pub last_at: Option<Instant>,
}

/// A DMX source seen by a listener (the monitor).
#[derive(Clone, Debug, Serialize)]
pub struct DmxSource {
    pub name: String,
    pub protocol: &'static str,
    pub addr: String,
    pub priority: u8,
    pub universe: u16,
    /// Its data is the universe's (sACN: highest priority, newest).
    pub active: bool,
    #[serde(skip)]
    pub at: Instant,
}

struct HubInner {
    file: NetInConfig,
    cli: CliOverride,
    path: Option<PathBuf>,
    osc: ListenStatus,
    artnet: ListenStatus,
    sacn: ListenStatus,
    sources: Vec<DmxSource>,
    timecode: TimecodeIn,
    feedback: feedback::FeedbackStatus,
}

/// ArtTimeCode readings received (`TIMECODE_QUEUE` newest at most).
pub type TimecodeReading = (crate::timecode::Tc, crate::timecode::Rate, Instant);
const TIMECODE_QUEUE: usize = 64;

/// Art-Net timecode (T-409), set by the engine each frame: wanted only
/// while the timecode source is Art-Net and TC-IN is armed. `bind`: where
/// to listen when the Art-Net chart is off (with it on, its socket
/// carries the timecode too).
#[derive(Default)]
struct TimecodeIn {
    wanted: bool,
    bind: Option<SocketAddr>,
    queue: VecDeque<TimecodeReading>,
}

/// Shared with the listener threads (its own short lock, never the
/// `Shared` one): the configuration, their status, the sources seen, and
/// the e-stop latch for the blackout fast path.
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
        let inner = HubInner {
            file,
            cli,
            path,
            osc: ListenStatus::default(),
            artnet: ListenStatus::default(),
            sacn: ListenStatus::default(),
            sources: Vec::new(),
            timecode: TimecodeIn::default(),
            feedback: feedback::FeedbackStatus::default(),
        };
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

    /// Where a listener binds: its own settings, and for Art-Net the
    /// timecode's address when only the timecode wants it (T-409).
    pub fn listen_addr(&self, cfg: &NetInConfig, proto: Proto) -> Option<SocketAddr> {
        let own = cfg.listen(proto);
        if proto != Proto::ArtNet || own.is_some() {
            return own;
        }
        let tc = &self.lock().timecode;
        tc.bind.filter(|_| tc.wanted)
    }

    /// The engine says whether Art-Net timecode is wanted, and where to
    /// listen for it without the chart. Not wanted: what waits is dropped.
    pub fn set_timecode(&self, wanted: bool, bind: Option<SocketAddr>) {
        let mut inner = self.lock();
        let tc = &mut inner.timecode;
        (tc.wanted, tc.bind) = (wanted, bind);
        if !wanted {
            tc.queue.clear();
        }
    }

    /// A listener received an ArtTimeCode: kept if wanted (false: ignored).
    pub fn push_timecode(&self, reading: TimecodeReading) -> bool {
        let mut inner = self.lock();
        let tc = &mut inner.timecode;
        if !tc.wanted {
            return false;
        }
        if tc.queue.len() >= TIMECODE_QUEUE {
            tc.queue.pop_front();
        }
        tc.queue.push_back(reading);
        true
    }

    /// The timecode readings received since the last call, oldest first.
    pub fn take_timecode(&self) -> Vec<TimecodeReading> {
        self.lock().timecode.queue.drain(..).collect()
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
            Proto::Sacn => inner.sacn.clone(),
        }
    }

    pub fn update_status(&self, proto: Proto, f: impl FnOnce(&mut ListenStatus)) {
        let mut inner = self.lock();
        f(match proto {
            Proto::Osc => &mut inner.osc,
            Proto::ArtNet => &mut inner.artnet,
            Proto::Sacn => &mut inner.sacn,
        });
    }

    /// The "osc-out" thread's counters (T-416).
    pub fn feedback_status(&self) -> feedback::FeedbackStatus {
        self.lock().feedback.clone()
    }

    pub fn update_feedback(&self, f: impl FnOnce(&mut feedback::FeedbackStatus)) {
        f(&mut self.lock().feedback);
    }

    /// Replaces what a listener knows of one universe's sources.
    pub fn set_sources(&self, proto: Proto, universe: u16, list: Vec<DmxSource>) {
        let mut inner = self.lock();
        let now = Instant::now();
        inner.sources.retain(|s| !(s.protocol == proto.id() && s.universe == universe) && now.saturating_duration_since(s.at) <= SOURCE_SHOWN);
        inner.sources.extend(list);
        let n = inner.sources.len();
        if n > MAX_SOURCES {
            inner.sources.sort_by_key(|s| std::cmp::Reverse(s.at));
            inner.sources.truncate(MAX_SOURCES);
        }
    }

    /// The sources seen in the last few seconds.
    pub fn sources(&self, now: Instant) -> Vec<DmxSource> {
        self.lock().sources.iter().filter(|s| now.saturating_duration_since(s.at) <= SOURCE_SHOWN).cloned().collect()
    }
}

/// What a listener hands to the engine.
#[derive(Debug)]
pub enum NetEvent {
    Osc { from: IpAddr, msg: OscMessage, cmd: Result<OscCommand, String> },
    /// The chart's channels from one ArtDmx of our universe.
    Dmx { from: IpAddr, values: [Option<u8>; CHART.len()] },
    /// A whole universe for the patch (Art-Net, or sACN after the merge).
    Universe(UniverseEvent),
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
    /// The free patch's universes, pickup and learn.
    pub patch: PatchState,
    /// Output gain of the loss policies (1: nothing lost), set each frame.
    pub gain: f32,
    /// The DMX fixture instances, one per projection zone (T-407).
    pub fixtures: crate::dmx::fixture::FixtureState,
    /// OSC feedback: subscribers and the sending queue (T-416).
    pub feedback: feedback::FeedbackState,
    /// The OSC monitor: the last 200 messages in and out (T-416).
    pub monitor: monitor::Monitor,
}

impl NetInState {
    pub fn new(hub: NetHub) -> Self {
        let (tx, rx) = sync_channel(QUEUE_LEN);
        NetInState {
            hub: Arc::new(hub),
            tx,
            rx,
            recent: VecDeque::new(),
            dmx: DmxState::default(),
            held: HashSet::new(),
            patch: PatchState::default(),
            gain: 1.0,
            fixtures: Default::default(),
            feedback: Default::default(),
            monitor: Default::default(),
        }
    }

    /// For the listener threads.
    pub fn sender(&self) -> SyncSender<NetEvent> {
        self.tx.clone()
    }
}

/// Once per engine frame: apply what arrived, then watch for signal loss
/// and work out the output gain.
pub fn frame(s: &mut Shared, now: Instant) {
    external::sync(s);
    let cfg = s.netin.hub.effective();
    let mut dmx = None;
    let mut universes: Vec<UniverseEvent> = Vec::new();
    for _ in 0..MAX_EVENTS_PER_FRAME {
        match s.netin.rx.try_recv() {
            Ok(NetEvent::Osc { from, msg, cmd }) if cfg.osc.enabled => osc_message(s, from, msg, cmd, now),
            // Only the newest packet counts: each one carries every channel.
            Ok(NetEvent::Dmx { from, values }) if cfg.artnet.enabled => dmx = Some((from, values)),
            Ok(NetEvent::Universe(u)) if cfg.enabled(u.proto) => match universes.iter_mut().find(|o| o.universe == u.universe) {
                Some(o) => *o = u,
                None => universes.push(u),
            },
            Ok(_) => {} // arrived just before it was switched off
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    // The patch first: its access channel counts for this frame's chart.
    for u in universes {
        patch::universe(s, &cfg, u, now);
    }
    if let Some((from, values)) = dmx {
        dmx_packet(s, from, values, now);
    }
    if !cfg.artnet.enabled {
        // Switched off by hand: nothing is lost, values stay as they are.
        s.netin.dmx = DmxState::default();
    } else if let Some(at) = s.netin.dmx.at {
        if !s.netin.dmx.lost && now.saturating_duration_since(at) > Duration::from_millis(cfg.loss.timeout_ms) {
            signal_lost(s, cfg.loss.policy, now);
        }
    }
    patch::drop_disabled(s, &cfg);
    patch::check_loss(s, &cfg, now);
    // The zones' remotes at this instant (after the losses: a lost
    // universe's instances go neutral under « neutre »).
    crate::dmx::fixture::tick(s, &cfg, now);
    if (!cfg.osc.enabled || !s.external.gate.open) && !s.netin.held.is_empty() {
        release_held(s);
    }
    if !s.external.gate.open && !s.netin.patch.held.is_empty() {
        patch::release_held(s);
    }
    s.netin.gain = gain(s, &cfg, now);
    // Last: what changed this frame, whoever changed it, goes back out.
    feedback::tick(s, &cfg, now);
}

/// The output gain of the « fondu au noir » and « noir immédiat » policies:
/// the lowest over every lost DMX input. Only while the gate is open (a
/// source that has no hand can't take the light away either); the next
/// packet brings it back to 1.
fn gain(s: &Shared, cfg: &NetInConfig, now: Instant) -> f32 {
    if !s.external.gate.open || !matches!(cfg.loss.policy, LossPolicy::FadeOut | LossPolicy::Dark) {
        return 1.0;
    }
    let timeout = Duration::from_millis(cfg.loss.timeout_ms);
    let chart = s.netin.dmx.lost.then_some(s.netin.dmx.at).flatten();
    let feeds = patch::lost(s).map(|(_, f)| f.at);
    chart
        .into_iter()
        .chain(feeds)
        .map(|at| match cfg.loss.policy {
            LossPolicy::FadeOut if cfg.loss.fade_ms > 0 => {
                let faded = now.saturating_duration_since(at + timeout).as_secs_f32() / (cfg.loss.fade_ms as f32 / 1000.0);
                (1.0 - faded).clamp(0.0, 1.0)
            }
            _ => 0.0,
        })
        .fold(1.0, f32::min)
}

fn osc_message(s: &mut Shared, from: IpAddr, msg: OscMessage, cmd: Result<OscCommand, String>, now: Instant) {
    let client = from.to_string();
    external::touch(s, SourceKind::Osc, &client, now);
    s.netin.feedback.seen(from, now);
    let error = cmd.and_then(|c| osc_command(s, from, c, now)).err();
    if let Some(e) = &error {
        log::debug!("OSC {} : {e}", msg.address);
    }
    let result = match &error {
        None => monitor::APPLIED,
        Some(e) if is_unknown(e) => monitor::UNKNOWN,
        Some(_) => monitor::REFUSED,
    };
    s.netin.monitor.push(true, client, &msg.address, &monitor::args_text(&msg.args), result, error.clone());
    let recent = &mut s.netin.recent;
    if recent.len() >= RECENT_LEN {
        recent.pop_front();
    }
    recent.push_back(OscLog { at: now, from, address: msg.address, args: msg.args, error });
}

/// An address or a control we don't know (the monitor's « inconnu »).
fn is_unknown(error: &str) -> bool {
    ["contrôle inconnu", "cue inconnu", "adresse inconnue", "adresse hors de /ls"].iter().any(|p| error.starts_with(p))
}

fn refused(id: &str, why: external::Refusal) -> String {
    match why {
        external::Refusal::Unknown => format!("contrôle inconnu : {id}"),
        external::Refusal::Forbidden => format!("« {id} » ne se pilote pas depuis le réseau"),
        other => other.label_fr().to_string(),
    }
}

fn osc_command(s: &mut Shared, from: IpAddr, cmd: OscCommand, now: Instant) -> Result<(), String> {
    let client = &from.to_string();
    match cmd {
        OscCommand::Control { id, input } => {
            let desc = s.controls.get(&id).cloned();
            // T-416: a value equal to the current one (a client echoing the
            // feedback) changes nothing and sends nothing back; the gate
            // still answers.
            if let Some(d) = &desc {
                let want = feedback::native(&d.kind, input);
                if let (Some(want), Some(cur)) = (want, feedback::value_of(s, d)) {
                    if feedback::same(&d.kind, want, cur) {
                        return external::check(s, SourceKind::Osc, client, d, now).map_err(|why| refused(&id, why));
                    }
                }
            }
            external::apply(s, SourceKind::Osc, client, &id, input, now).map_err(|why| refused(&id, why))?;
            // The sender has this value: it is not sent back to it.
            if let Some(v) = desc.as_ref().and_then(|d| feedback::value_of(s, d)) {
                s.netin.feedback.echoed(from, &desc.as_ref().unwrap().id, v);
            }
            if let Some(desc) = desc.filter(|d| d.kind == ControlKind::Momentary) {
                let pressed = matches!(input, ControlInput::Value(v) | ControlInput::Norm(v) if v >= 0.5);
                if pressed {
                    s.netin.held.insert(desc.id);
                } else {
                    s.netin.held.remove(&desc.id);
                }
            }
            Ok(())
        }
        OscCommand::PlayCue(id) => match external::play_cue(s, SourceKind::Osc, client, &id, now) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("cue inconnu : {id}")),
            Err(why) => Err(why.label_fr().to_string()),
        },
        OscCommand::StopCues => external::apply(s, SourceKind::Osc, client, "cue.stop_all", ControlInput::Value(1.0), now).map_err(|why| refused("cue.stop_all", why)),
        OscCommand::Blackout => {
            s.emergency_stop(ArmSource::Osc);
            Ok(())
        }
        OscCommand::Arm => {
            // Through the gate, which refuses the network and logs it.
            let _ = s.gate.request_arm(ArmSource::Osc);
            Err("refusé : l'armement ne se fait jamais depuis le réseau".into())
        }
        OscCommand::Access(on) => {
            external::set_access(s, SourceKind::Osc, client, on, now);
            Ok(())
        }
        OscCommand::Ping => Ok(()),
        OscCommand::Subscribe(port) => {
            let cfg = s.netin.hub.effective().feedback;
            s.netin.feedback.subscribe(&cfg, SocketAddr::new(from, port), now)
        }
        OscCommand::Unsubscribe => {
            s.netin.feedback.unsubscribe(from);
            Ok(())
        }
    }
}

/// OSC was switched off or the gate closed: the pads it held come up.
fn release_held(s: &mut Shared) {
    for id in std::mem::take(&mut s.netin.held) {
        external::release(s, SourceKind::Osc, &id);
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
        s.gate.log().record("input_back", Some(ArmSource::Dmx.id()), json!({ "protocol": "artnet", "protocol_fr": "Art-Net" }));
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
                if let Err(why) = external::apply(s, SourceKind::Dmx, "", id, artnet::input(v, centred), now) {
                    log::debug!("Art-Net {id} : {}", why.label_fr());
                }
            }
            Function::Page => {
                if external::set_page(s, SourceKind::Dmx, "", artnet::page(v, s.grid.page_count()), now).is_ok() {
                    cue_changed = true;
                }
            }
            Function::Cue => cue_changed = true,
            // The listener has already tripped it; same rule here.
            Function::Blackout => {
                let was_on = prev[ch.offset].is_some_and(artnet::blackout_on);
                if artnet::blackout_on(v) && !was_on {
                    s.emergency_stop(ArmSource::Dmx);
                }
            }
        }
    }
    if cue_changed {
        dmx_cue(s, values[2], fresh, now);
    }
}

/// Channel 3: 0 stops the cues (not on the first packet: a console that
/// connects with it at 0 leaves the operator's cues alone), 1–40 plays
/// that cell of the current page from the start.
fn dmx_cue(s: &mut Shared, value: Option<u8>, fresh: bool, now: Instant) {
    let cells = controls::GRID_ROWS * controls::GRID_COLS;
    match value {
        Some(0) if !fresh => {
            let _ = external::apply(s, SourceKind::Dmx, "", "cue.stop_all", ControlInput::Value(1.0), now);
        }
        Some(v) if (1..=cells).contains(&(v as usize)) => {
            let i = v as usize - 1;
            let cell = format!("{}.{}.{}", s.cue_page + 1, i / controls::GRID_COLS + 1, i % controls::GRID_COLS + 1);
            if let Some(id) = controls::grid_cell_preset(&s.grid, &cell) {
                let _ = external::play_cue(s, SourceKind::Dmx, "", &id, now);
            }
        }
        _ => {}
    }
}

fn signal_lost(s: &mut Shared, policy: LossPolicy, now: Instant) {
    s.netin.dmx.lost = true;
    log::warn!("Art-Net : signal perdu, {}", policy.label_fr());
    s.gate.log().record(
        "input_lost",
        Some(ArmSource::Dmx.id()),
        json!({ "protocol": "artnet", "protocol_fr": "Art-Net", "policy": policy, "policy_fr": policy.label_fr() }),
    );
    match policy {
        LossPolicy::Hold | LossPolicy::FadeOut | LossPolicy::Dark => {}
        LossPolicy::Neutral => {
            for ch in CHART {
                if let Function::Control { id, neutral: Some(v), .. } = ch.function {
                    let _ = external::apply(s, SourceKind::Dmx, "", id, ControlInput::Value(v), now);
                }
            }
        }
        LossPolicy::Blackout => s.emergency_stop(ArmSource::Dmx),
    }
}

/// A lost or recovered patch universe in the safety log.
pub(crate) fn signal_log(s: &Shared, kind: &'static str, universe: u16, name: &str, policy: Option<LossPolicy>) {
    let protocol_fr = format!("DMX univers {universe} ({name})");
    let mut detail = json!({ "protocol": "dmx", "protocol_fr": protocol_fr, "universe": universe, "source": name });
    if let Some(p) = policy {
        (detail["policy"], detail["policy_fr"]) = (json!(p), json!(p.label_fr()));
    }
    s.gate.log().record(kind, Some(ArmSource::Dmx.id()), detail);
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

fn sources_json(list: &[DmxSource], now: Instant) -> Vec<Value> {
    list.iter()
        .map(|src| {
            let mut v = json!(src);
            v["age_ms"] = json!(now.saturating_duration_since(src.at).as_millis() as u64);
            v
        })
        .collect()
}

/// The DMX inputs in short, for `/api/state` and the top bar: on, sources,
/// and the first lost input with its source's name.
pub fn summary(s: &Shared, now: Instant) -> Value {
    let cfg = s.netin.hub.effective();
    let enabled = cfg.artnet.enabled || cfg.sacn.enabled;
    let chart_lost = (cfg.artnet.enabled && s.netin.dmx.lost).then(|| s.netin.dmx.from.map_or_else(|| "Art-Net".to_string(), |ip| format!("Art-Net {ip}")));
    let lost = chart_lost.or_else(|| patch::lost(s).next().map(|(u, f)| format!("{} (univers {u})", f.name)));
    let receiving = (cfg.artnet.enabled && s.netin.dmx.at.is_some() && !s.netin.dmx.lost) || s.netin.patch.feeds.values().any(|f| !f.lost);
    json!({
        "enabled": enabled,
        "receiving": receiving,
        "sources": s.netin.hub.sources(now).len(),
        "lost": lost.is_some(),
        "lost_source": lost,
        "gain": s.netin.gain,
    })
}

/// `GET /api/dmx/monitor?universe=N`: the 512 values of a patch universe,
/// its sources and rate.
pub fn monitor(s: &Shared, universe: u16, now: Instant) -> Value {
    let feed = s.netin.patch.feeds.get(&universe);
    let values: Option<Vec<u8>> = feed.map(|f| {
        let mut v = f.data.clone();
        v.resize(patch::UNIVERSE_LEN, 0);
        v
    });
    let sources: Vec<DmxSource> = s.netin.hub.sources(now).into_iter().filter(|src| src.universe == universe).collect();
    json!({
        "universe": universe,
        "values": values,
        "lost": feed.is_some_and(|f| f.lost),
        "rate": feed.map_or(0.0, |f| f.rate),
        "age_ms": feed.map(|f| now.saturating_duration_since(f.at).as_millis() as u64),
        "protocol": feed.map(|f| f.proto.id()),
        "source": feed.map(|f| f.name.clone()),
        "sources": sources_json(&sources, now),
        "fixtures": crate::dmx::fixture::monitor(&s.netin.hub.effective(), universe),
    })
}

/// `GET /api/dmx/interfaces`: this Mac's IPv4 addresses (to pick where to
/// listen). Reading them opens nothing.
pub fn interfaces() -> Vec<Value> {
    let mut list: Vec<Value> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) => Some(json!({ "name": i.name, "ip": v4.to_string(), "loopback": v4.is_loopback() })),
            IpAddr::V6(_) => None,
        })
        .collect();
    if !list.iter().any(|v| v["ip"] == DEFAULT_BIND) {
        list.insert(0, json!({ "name": "lo0", "ip": DEFAULT_BIND, "loopback": true }));
    }
    list
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
    let universes: Vec<Value> = s
        .netin
        .patch
        .feeds
        .iter()
        .map(|(u, f)| json!({ "universe": u, "protocol": f.proto.id(), "source": f.name, "lost": f.lost, "rate": f.rate, "age_ms": now.saturating_duration_since(f.at).as_millis() as u64 }))
        .collect();
    let mappable: Vec<Value> = s
        .controls
        .list()
        .iter()
        .filter(|d| patch::mappable(d))
        .map(|d| json!({ "id": d.id, "label": d.label_fr, "group": d.group }))
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
        "sacn": { "status": status_json(&s.netin.hub.status(Proto::Sacn), now) },
        "dmx": {
            "summary": summary(s, now),
            "universes": universes,
            "sources": sources_json(&s.netin.hub.sources(now), now),
            "learn": s.netin.patch.learn.as_ref().map(|l| l.control.clone()),
            "learned": s.netin.patch.learned,
            "controls": mappable,
            "fixtures": crate::dmx::fixture::view(s, &eff),
        },
        "policies": LOSS_POLICIES.iter().map(|p| json!({ "id": p, "label": p.label_fr() })).collect::<Vec<_>>(),
    })
}

/// `POST /api/netin` (and `/api/dmx/config`): a partial update
/// (`{"osc": {"enabled": true}}`); missing fields keep their value. Returns
/// the names of what changed; the caller saves (`NetHub::save`) once the
/// `Shared` lock is released.
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
    let mut cfg: NetInConfig = serde_json::from_value(merged).map_err(|e| format!("réglage invalide : {e}"))?;
    // Each mapping names a control DMX may drive (its canonical id).
    for m in &mut cfg.dmx.mappings {
        let desc = s.controls.get(&m.control).ok_or_else(|| format!("contrôle inconnu : « {} »", m.control))?;
        if !patch::mappable(desc) {
            return Err(format!("« {} » ne se pilote pas depuis l'extérieur", desc.id));
        }
        m.control = desc.id.clone();
    }
    // The fixture instances, with the zones' names in the messages.
    let projection = s.projection.get();
    crate::dmx::fixture::check(&cfg.dmx.fixtures, |z| projection.zone(z).map_or_else(|| format!("Zone {z}"), |z| z.name.clone()))?;
    let changed = changed_keys(&json!(before), &json!(cfg));
    if changed.is_empty() {
        cfg.validate()?;
        return Ok(changed);
    }
    s.netin.hub.store(cfg.clone())?;
    s.gate.log().record("netin_settings", Some(ArmSource::Ui.id()), json!({ "changed": changed, "before": before, "after": cfg }));
    Ok(changed)
}

/// `GET /api/osc/config` (T-416): the feedback settings, where it is sent
/// from, the subscribers and the sending counters.
pub fn osc_view(s: &Shared, now: Instant) -> Value {
    let (file, eff) = (s.netin.hub.file(), s.netin.hub.effective());
    let fb = &s.netin.feedback;
    let subscribers: Vec<Value> = fb
        .subscribers
        .iter()
        .map(|sub| json!({ "addr": sub.addr.to_string(), "fixed": sub.fixed, "age_ms": now.saturating_duration_since(sub.last_seen).as_millis() as u64 }))
        .collect();
    json!({
        "config": file.feedback,
        "osc": { "enabled": eff.osc.enabled, "bind": eff.osc.bind, "port": eff.osc.port },
        "subscribers": subscribers,
        "status": s.netin.hub.feedback_status(),
        "queued": fb.queued,
        "dropped": fb.dropped,
        "limits": {
            "max_targets": feedback::MAX_TARGETS,
            "max_subscribers": feedback::MAX_SUBSCRIBERS,
            "min_rate_hz": feedback::MIN_RATE_HZ,
            "max_rate_hz": feedback::MAX_RATE_HZ,
            "subscriber_timeout_s": feedback::SUBSCRIBER_TIMEOUT.as_secs(),
        },
    })
}

/// `GET /api/osc/monitor?dir=in|out` (T-416): newest first.
pub fn osc_monitor(s: &Shared, dir: Option<&str>) -> Value {
    let incoming = match dir {
        Some("in") => Some(true),
        Some("out") => Some(false),
        _ => None,
    };
    json!({ "entries": s.netin.monitor.list(incoming), "capacity": monitor::MONITOR_LEN })
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

    /// Every group for every source, no access needed.
    fn full_scope() -> crate::external::ExternalConfig {
        let groups: Vec<String> = external::SCOPE_SETS.iter().flat_map(|set| set.groups.iter().map(|g| g.to_string())).collect();
        crate::external::ExternalConfig {
            scopes: SourceKind::ALL.iter().map(|&source| crate::external::SourceScope { source, groups: groups.clone(), require_access: false }).collect(),
        }
    }

    /// A studio whose inputs are all on (in memory, nothing bound), with
    /// the gate open on every group and no access needed.
    fn studio() -> Shared {
        let mut s = shared();
        s.netin = NetInState::new(NetHub::in_memory(Arc::clone(&s.estop)));
        let mut cfg = NetInConfig::default();
        (cfg.osc.enabled, cfg.artnet.enabled, cfg.sacn.enabled) = (true, true, true);
        s.netin.hub.set_file(cfg).unwrap();
        s.external.set_config(full_scope());
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
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

    /// T-416: the monitor's results, the listed addresses as input, and
    /// an echo of the current value changes nothing.
    #[test]
    fn the_monitor_tells_applied_refused_and_unknown() {
        let mut s = studio();
        osc(&mut s, "/ls/master/size", &[OscArg::Float(1.5)]);
        osc(&mut s, "/ls/no/such", &[OscArg::Float(1.0)]);
        osc(&mut s, "/elsewhere", &[]);
        osc(&mut s, "/ls/transport/arm", &[OscArg::Int(1)]);
        let results: Vec<&str> = s.netin.monitor.list(Some(true)).iter().rev().map(|e| e.result).collect();
        assert_eq!(results, ["appliqué", "inconnu", "inconnu", "refusé"]);
        assert_eq!(s.live.size, 1.5);
        assert!(!s.gate.is_armed());
        // The gate closed: refused, even an echo of the current value.
        external::set_open(&mut s, false, ArmSource::Ui).unwrap();
        osc(&mut s, "/ls/master/size", &[OscArg::Float(1.5)]);
        assert_eq!(s.netin.monitor.list(Some(true))[0].result, "refusé");
        // Open: an echo changes nothing.
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        s.live_dirty = false;
        osc(&mut s, "/ls/master/size", &[OscArg::Float(1.5)]);
        assert_eq!(s.netin.monitor.list(Some(true))[0].result, "appliqué");
        assert!(!s.live_dirty, "nothing changed");
        // Subscribing needs the feedback on.
        osc(&mut s, "/ls/subscribe", &[OscArg::Int(9000)]);
        assert_eq!(s.netin.monitor.list(Some(true))[0].result, "refusé");
        assert!(s.netin.feedback.subscribers.is_empty());
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
        let want = controls::grid_cell_preset(&s.grid, "1.1.3").unwrap();
        assert_eq!(s.active_cue.as_deref(), Some(want.as_str()));
        v[1] = 255; // last page
        dmx_at(&mut s, &v, t0);
        assert_eq!(s.cue_page, s.grid.page_count() - 1);
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
        assert_eq!(s.gate.status(&s.estop).estop.unwrap().source, ArmSource::Osc);
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

    // ---------- the gate (T-408) ----------

    #[test]
    fn a_closed_gate_ignores_everything_but_the_stops() {
        let mut s = studio();
        external::set_open(&mut s, false, ArmSource::Ui).unwrap();
        osc(&mut s, "/ls/norm/master.size", &[OscArg::Float(0.25)]);
        assert_eq!(s.live.size, 1.0);
        assert!(last_error(&s).unwrap().contains("fermées"));
        let cue = s.presets[0].id.clone();
        osc(&mut s, "/ls/cue/play", &[OscArg::Str(cue)]);
        assert!(s.deck.active.is_empty());
        let mut v = NEUTRAL;
        v[0] = 51;
        dmx_at(&mut s, &v, Instant::now());
        assert_eq!(s.live.brightness, 1.0);
        patched(&mut s, "master.perspective", false);
        universe(&mut s, Proto::Sacn, 1, &[255], Instant::now());
        assert_eq!(s.live.perspective, LiveModifiers::default().perspective);
        assert!(s.external.gate.refused[&SourceKind::Osc] >= 2 && s.external.gate.refused[&SourceKind::Dmx] >= 1);
        // The stops always pass, and close the gate.
        osc(&mut s, "/ls/blackout", &[]);
        assert!(s.estop.is_latched());
        let mut s = studio();
        osc(&mut s, "/ls/control/safety.estop", &[OscArg::Int(1)]);
        assert!(s.estop.is_latched() && !s.external.gate.open);
    }

    #[test]
    fn osc_access_is_per_client_and_required_by_default() {
        let mut s = studio();
        s.external.set_config(crate::external::ExternalConfig::default());
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        osc(&mut s, "/ls/control/master.size", &[OscArg::Float(1.5)]);
        assert!(last_error(&s).unwrap().contains("accès"));
        osc(&mut s, "/ls/access", &[OscArg::Int(1)]);
        osc(&mut s, "/ls/control/master.size", &[OscArg::Float(1.5)]);
        assert_eq!(s.live.size, 1.5);
        // Default scope: the masters only, no cue.
        let cue = s.presets[0].id.clone();
        osc(&mut s, "/ls/cue/play", &[OscArg::Str(cue)]);
        assert!(last_error(&s).unwrap().contains("périmètre") && s.deck.active.is_empty());
        osc(&mut s, "/ls/access", &[OscArg::Int(0)]);
        osc(&mut s, "/ls/control/master.size", &[OscArg::Float(1.0)]);
        assert_eq!(s.live.size, 1.5);
    }

    #[test]
    fn closing_the_gate_releases_held_pads() {
        let mut s = studio();
        osc(&mut s, "/ls/control/master.rot.reverse", &[OscArg::Int(1)]);
        assert!(s.live.rot_reverse);
        external::set_open(&mut s, false, ArmSource::Ui).unwrap();
        frame(&mut s, Instant::now());
        assert!(!s.live.rot_reverse && s.netin.held.is_empty());
    }

    // ---------- sACN and the patch (T-406) ----------

    use crate::live::LiveModifiers;
    use patch::{DmxAddr, DmxMapping};

    fn universe(s: &mut Shared, proto: Proto, universe: u16, data: &[u8], at: Instant) {
        let ev = UniverseEvent { proto, universe, from: ip(), name: "Console".into(), data: data.to_vec() };
        s.netin.sender().try_send(NetEvent::Universe(ev)).unwrap();
        frame(s, at);
    }

    /// Universe 1 channel 1 → `control`.
    fn patched(s: &mut Shared, control: &str, pickup: bool) {
        let mut cfg = s.netin.hub.file();
        cfg.dmx.mappings = vec![DmxMapping { control: control.into(), pickup, ..DmxMapping::default() }];
        s.netin.hub.store(cfg).unwrap();
    }

    #[test]
    fn artnet_and_sacn_drive_the_same_control() {
        let mut s = studio();
        patched(&mut s, "master.perspective", false);
        let t0 = Instant::now();
        universe(&mut s, Proto::ArtNet, 1, &[255], t0);
        assert_eq!(s.live.perspective, 1.0);
        universe(&mut s, Proto::Sacn, 1, &[0], t0);
        assert_eq!(s.live.perspective, 0.0);
        // Another universe: nothing.
        universe(&mut s, Proto::Sacn, 2, &[255], t0);
        assert_eq!(s.live.perspective, 0.0);
        // A protocol switched off is ignored.
        update(&mut s, &json!({ "sacn": { "enabled": false } })).unwrap();
        universe(&mut s, Proto::Sacn, 1, &[128], t0);
        assert_eq!(s.live.perspective, 0.0);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn eight_and_sixteen_bits_inverted_triggers_and_momentary() {
        let m = |fine, invert| DmxMapping { channel: 2, fine, invert, ..DmxMapping::default() };
        let data = [0u8, 0x80, 0x40];
        assert_eq!(patch::read(&m(None, false), &data).unwrap().0, 0x8080);
        assert_eq!(patch::read(&m(Some(3), false), &data).unwrap().0, 0x8040);
        assert!((patch::read(&m(Some(3), false), &data).unwrap().1 - 0x8040 as f32 / 65535.0).abs() < 1e-6);
        assert!((patch::read(&m(None, true), &[0, 255]).unwrap().1).abs() < 1e-6);
        assert_eq!(patch::read(&m(Some(3), false), &[0, 1]), None, "short packet");

        let mut s = studio();
        let t0 = Instant::now();
        // A trigger fires on the way up only.
        patched(&mut s, "page.next", false);
        universe(&mut s, Proto::Sacn, 1, &[200], t0);
        assert_eq!(s.cue_page, 1);
        universe(&mut s, Proto::Sacn, 1, &[250], t0);
        assert_eq!(s.cue_page, 1, "still high: no second press");
        universe(&mut s, Proto::Sacn, 1, &[0], t0);
        universe(&mut s, Proto::Sacn, 1, &[128], t0);
        assert_eq!(s.cue_page, 2);
        // A momentary control follows the channel, and is released on loss.
        patched(&mut s, "master.rot.reverse", false);
        universe(&mut s, Proto::Sacn, 1, &[127], t0);
        assert!(!s.live.rot_reverse);
        universe(&mut s, Proto::Sacn, 1, &[128], t0);
        assert!(s.live.rot_reverse);
        universe(&mut s, Proto::Sacn, 1, &[10], t0);
        assert!(!s.live.rot_reverse);
        universe(&mut s, Proto::Sacn, 1, &[255], t0);
        frame(&mut s, t0 + Duration::from_secs(3));
        assert!(!s.live.rot_reverse, "released on signal loss");
        // Sixteen bits, inverted.
        let mut cfg = s.netin.hub.file();
        cfg.dmx.mappings = vec![DmxMapping { control: "master.perspective".into(), fine: Some(2), invert: true, pickup: false, ..DmxMapping::default() }];
        s.netin.hub.store(cfg).unwrap();
        universe(&mut s, Proto::Sacn, 1, &[0x40, 0x00], t0 + Duration::from_secs(4));
        assert!((s.live.perspective - (1.0 - 0x4000 as f32 / 65535.0)).abs() < 1e-4);
    }

    #[test]
    fn pickup_waits_for_the_channel_to_cross_the_screen_value() {
        let mut s = studio();
        patched(&mut s, "master.perspective", true);
        let t0 = Instant::now();
        s.live.perspective = 0.5;
        // Far from 0.5: no jump.
        universe(&mut s, Proto::Sacn, 1, &[20], t0);
        assert_eq!(s.live.perspective, 0.5);
        // Crosses it: takes over.
        universe(&mut s, Proto::Sacn, 1, &[200], t0);
        assert!((s.live.perspective - 200.0 / 255.0).abs() < 1e-3);
        // The screen moves it: a still channel doesn't take it back...
        s.live.perspective = 0.1;
        universe(&mut s, Proto::Sacn, 1, &[200], t0);
        universe(&mut s, Proto::Sacn, 1, &[210], t0);
        assert_eq!(s.live.perspective, 0.1, "not crossed yet");
        // ...until it crosses the new value.
        universe(&mut s, Proto::Sacn, 1, &[10], t0);
        assert!((s.live.perspective - 10.0 / 255.0).abs() < 1e-3);
    }

    #[test]
    fn the_access_channel_raises_dmx_access() {
        let mut s = studio();
        let scopes = vec![crate::external::SourceScope { source: SourceKind::Dmx, groups: vec!["master".into()], require_access: true }];
        s.external.set_config(crate::external::ExternalConfig { scopes });
        let mut cfg = s.netin.hub.file();
        cfg.dmx.mappings = vec![DmxMapping { control: "master.perspective".into(), pickup: false, ..DmxMapping::default() }];
        cfg.dmx.access = Some(DmxAddr { universe: 1, channel: 10 });
        s.netin.hub.store(cfg).unwrap();
        let t0 = Instant::now();
        let mut data = [0u8; 10];
        data[0] = 255;
        universe(&mut s, Proto::ArtNet, 1, &data, t0);
        assert_eq!(s.live.perspective, LiveModifiers::default().perspective, "access low");
        (data[0], data[9]) = (128, 128);
        universe(&mut s, Proto::ArtNet, 1, &data, t0);
        assert!((s.live.perspective - 128.0 / 255.0).abs() < 1e-3);
        // The Art-Net chart obeys the same access.
        let mut v = NEUTRAL;
        v[0] = 51;
        dmx_at(&mut s, &v, t0);
        assert!((s.live.brightness - 0.2).abs() < 1e-6);
        data[9] = 0;
        universe(&mut s, Proto::ArtNet, 1, &data, t0);
        v[0] = 255;
        dmx_at(&mut s, &v, t0);
        assert!((s.live.brightness - 0.2).abs() < 1e-6, "access dropped");
    }

    #[test]
    fn two_sacn_sources_highest_priority_wins_end_to_end() {
        // The merge runs in the listener; here, its verdicts feed the patch.
        let mut m = sacn::Merger::default();
        let t0 = Instant::now();
        let mut s = studio();
        patched(&mut s, "master.perspective", false);
        let p = |cid: u8, pri, seq, v: u8| match sacn::decode(&sacn::tests::packet_from([cid; 16], 1, "x", pri, seq, 0, &[v])).unwrap() {
            sacn::Packet::Data(d) => d,
            _ => unreachable!(),
        };
        for (d, want) in [(p(1, 100, 0, 255), 1.0), (p(2, 150, 0, 0), 0.0), (p(1, 100, 1, 255), 0.0)] {
            if m.packet(&d, "a".into(), t0) == sacn::Verdict::Take {
                universe(&mut s, Proto::Sacn, 1, &d.slots, t0);
            }
            assert_eq!(s.live.perspective, want);
        }
    }

    #[test]
    fn fade_and_dark_policies_lower_the_gain_then_recover_without_arming() {
        for (policy, mid) in [(LossPolicy::FadeOut, 0.5), (LossPolicy::Dark, 0.0), (LossPolicy::Hold, 1.0)] {
            let mut s = studio();
            update(&mut s, &json!({ "loss": { "policy": policy, "timeout_ms": 1000, "fade_ms": 2000 } })).unwrap();
            patched(&mut s, "master.perspective", false);
            assert!(s.request_arm(ArmSource::Ui).is_ok());
            let t0 = Instant::now();
            universe(&mut s, Proto::Sacn, 1, &[255], t0);
            frame(&mut s, t0 + Duration::from_millis(900));
            assert_eq!(s.netin.gain, 1.0);
            frame(&mut s, t0 + Duration::from_millis(2000));
            assert!((s.netin.gain - mid).abs() < 0.02, "{policy:?}: {}", s.netin.gain);
            assert_eq!(summary(&s, t0)["lost_source"], "Console (univers 1)");
            assert!(s.gate.is_armed(), "{policy:?}: no disarm");
            frame(&mut s, t0 + Duration::from_millis(4000));
            assert!(s.netin.gain <= mid);
            universe(&mut s, Proto::Sacn, 1, &[255], t0 + Duration::from_secs(5));
            assert_eq!(s.netin.gain, 1.0, "back");
            assert_eq!(summary(&s, t0)["lost"], false);
            // A disarmed laser stays disarmed through a loss and a return.
            s.gate.disarm(crate::interlock::DisarmReason::User, ArmSource::Ui);
            frame(&mut s, t0 + Duration::from_secs(8));
            universe(&mut s, Proto::Sacn, 1, &[0], t0 + Duration::from_secs(9));
            assert!(!s.gate.is_armed());
        }
        // The chart's loss counts too; a closed gate never dims.
        let mut s = studio();
        update(&mut s, &json!({ "loss": { "policy": "dark" } })).unwrap();
        let t0 = Instant::now();
        dmx_at(&mut s, &NEUTRAL, t0);
        frame(&mut s, t0 + Duration::from_secs(3));
        assert_eq!(s.netin.gain, 0.0);
        external::set_open(&mut s, false, ArmSource::Ui).unwrap();
        frame(&mut s, t0 + Duration::from_secs(4));
        assert_eq!(s.netin.gain, 1.0);
    }

    #[test]
    fn learn_maps_the_first_channel_that_moves() {
        let mut s = studio();
        assert!(patch::start_learn(&mut s, Some("transport.arm")).is_err());
        assert!(patch::start_learn(&mut s, Some("safety.hold")).is_err());
        patch::start_learn(&mut s, Some("live.perspective")).unwrap();
        let t0 = Instant::now();
        let mut data = [100u8; 16];
        universe(&mut s, Proto::Sacn, 3, &data, t0);
        data[4] = 105; // 5 steps: noise
        universe(&mut s, Proto::Sacn, 3, &data, t0);
        assert!(s.netin.patch.learn.is_some());
        data[6] = 120;
        universe(&mut s, Proto::Sacn, 3, &data, t0);
        assert!(s.netin.patch.learn.is_none());
        let m = s.netin.hub.file().dmx.mappings;
        assert_eq!((m.len(), m[0].universe, m[0].channel, m[0].control.as_str()), (1, 3, 7, "master.perspective"));
        assert_eq!(s.netin.patch.learned.as_ref(), Some(&m[0]));
        patch::start_learn(&mut s, Some("master.size")).unwrap();
        patch::start_learn(&mut s, None).unwrap();
        assert!(s.netin.patch.learn.is_none());
    }

    #[test]
    fn patch_settings_validate() {
        let mut s = studio();
        let ok = json!({ "dmx": { "mappings": [{ "universe": 2, "channel": 5, "control": "live.size" }], "access": { "universe": 2, "channel": 1 } } });
        update(&mut s, &ok).unwrap();
        assert_eq!(s.netin.hub.file().dmx.mappings[0].control, "master.size", "canonical id");
        assert!(s.netin.hub.file().dmx.mappings[0].pickup, "pickup on by default");
        for bad in [
            json!({ "dmx": { "mappings": [{ "channel": 0, "control": "master.size" }] } }),
            json!({ "dmx": { "mappings": [{ "channel": 513, "control": "master.size" }] } }),
            json!({ "dmx": { "mappings": [{ "channel": 5, "fine": 5, "control": "master.size" }] } }),
            json!({ "dmx": { "mappings": [{ "control": "transport.arm" }] } }),
            json!({ "dmx": { "mappings": [{ "control": "safety.hold" }] } }),
            json!({ "dmx": { "mappings": [{ "control": "nope" }] } }),
            json!({ "dmx": { "access": { "universe": 1, "channel": 0 } } }),
            json!({ "sacn": { "universes": [0] } }),
            json!({ "sacn": { "universes": [64000] } }),
            json!({ "sacn": { "multicast": true } }),
            json!({ "sacn": { "bind": "0.0.0.0", "multicast": true } }),
            json!({ "loss": { "fade_ms": 60000 } }),
        ] {
            assert!(update(&mut s, &bad).is_err(), "{bad}");
        }
        // The stops may be patched.
        update(&mut s, &json!({ "dmx": { "mappings": [{ "control": "transport.blackout" }] } })).unwrap();
        // Multicast only on a real interface address.
        let mut c = NetInConfig::default();
        (c.sacn.enabled, c.sacn.bind, c.sacn.multicast, c.sacn.universes) = (true, "192.168.1.20".into(), true, vec![1, 300]);
        c.validate().unwrap();
        assert_eq!(c.multicast(), Some(("192.168.1.20".parse().unwrap(), vec![1, 300])));
        c.sacn.multicast = false;
        assert_eq!(c.multicast(), None, "never joined unless asked");
        assert_eq!(NetInConfig::default().multicast(), None);
        assert!(!NetInConfig::default().sacn.enabled && NetInConfig::default().sacn.bind == "127.0.0.1");
    }

    /// Every control of the registry patched (even those the UI refuses),
    /// arbitrary frames on both protocols, gate open, full scope: never an
    /// arm, safety settings and calibration untouched.
    #[test]
    fn no_dmx_frame_ever_arms() {
        let mut s = studio();
        let (safety, calibration) = (s.safety.get(), s.calibration);
        let ids: Vec<String> = s.controls.list().iter().map(|d| d.id.clone()).filter(|id| !external::ALWAYS.contains(&id.as_str())).collect();
        let t0 = Instant::now();
        let mut rng = crate::safety_invariants::Rng::new(408);
        // A patch holds at most `MAX_MAPPINGS`: the registry (the executor
        // bank, T-418, made it longer) is patched a share at a time.
        for share in ids.chunks(crate::netin::patch::MAX_MAPPINGS) {
            let mut cfg = s.netin.hub.file();
            cfg.dmx.mappings = share
                .iter()
                .enumerate()
                .map(|(i, id)| DmxMapping { universe: 1 + (i / 256) as u16, channel: 1 + (i % 256) as u16 * 2, control: id.clone(), pickup: false, ..DmxMapping::default() })
                .collect();
            s.netin.hub.store(cfg).unwrap();
            for step in 0..60 {
                let data: Vec<u8> = (0..512).map(|_| (rng.next() & 0xFF) as u8).collect();
                let proto = if step % 2 == 0 { Proto::ArtNet } else { Proto::Sacn };
                for u in [1, 2] {
                    universe(&mut s, proto, u, &data, t0);
                }
                assert!(!s.gate.is_armed(), "step {step}");
                assert!(!s.presence.status().midi_hold, "no hold-to-run from DMX");
            }
            for v in [0u8, 127, 128, 255] {
                universe(&mut s, Proto::Sacn, 1, &[v; 512], t0);
                universe(&mut s, Proto::Sacn, 2, &[v; 512], t0);
                assert!(!s.gate.is_armed());
            }
        }
        assert_eq!(s.safety.get(), safety);
        assert_eq!(s.calibration, calibration);
        assert!(!s.estop.is_latched(), "no stop was patched");
    }
}
