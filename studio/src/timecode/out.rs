//! Timecode output (T-417): the timeline's position sent as MIDI Time
//! Code on a MIDI output port and / or as Art-Net timecode on a network
//! interface, so a console, a video server or a second machine follows
//! the show the studio plays (with its song).
//!
//! What it does, and what it never does:
//! - **Off by default** (`enabled`, saved per machine in `timecode.json`,
//!   key `out`, never in a project). It only goes to the port and the
//!   interface / address the operator chose: no discovery, no scan. The
//!   Art-Net target is one host typed by the operator (never multicast,
//!   0.0.0.0 or a typed broadcast); an empty target is the broadcast of
//!   the chosen interface, an explicit choice in the settings.
//! - It reads the timeline, it never drives it: no arming, no output
//!   change, no cue. An e-stop halts the timeline, so the stream stops.
//! - **No self-drive**: refused (409) while the timecode input listens to
//!   the same MIDI port, or the same network interface, as the output;
//!   the engine also holds the stream back if that happens later (a
//!   show's own source).
//! - While the timeline plays (shows in seconds only): MTC quarter frames
//!   (four per frame, eight per time) and one Art-Net packet per frame. A
//!   start, seek or loop wrap sends a full frame (MTC SysEx and one
//!   Art-Net packet) and starts a new quarter-frame run. Paused or
//!   stopped, nothing is sent; a seek while paused or stopped (a locate)
//!   sends a single full frame.
//! - The engine never waits: once per frame it `try_send`s a snapshot of
//!   the playhead into a bounded queue; the "tc-out" thread extrapolates
//!   the position between snapshots and sends each message when due. A
//!   send error is a warning (status `error`); the timeline plays on.
//!
//! Encoders come from mtc.rs and artnet.rs (the input's decoders are the
//! tests' reference).

use super::{artnet, mtc, Rate, Tc, TcConfig, TcSource, MAX_OFFSET_S};
use crate::interlock::ArmSource;
use crate::midi::backend::OutputPort;
use crate::timeline::{Clock, TimeBase, Transport};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The Art-Net port, the default of a typed target without one.
pub const ARTNET_PORT: u16 = 6454;
/// Snapshots waiting for the "tc-out" thread at most (it keeps the newest).
pub const QUEUE_LEN: usize = 8;
/// Quarter frames the playhead may run ahead of (or fall behind) the
/// stream before a new run (with a full frame) starts.
const MAX_DRIFT_QUARTERS: i64 = 8;
/// Longest sleep of the sending thread, so a shutdown and a new snapshot
/// are seen.
const POLL: Duration = Duration::from_millis(20);
/// A port or socket that failed is tried again after this.
const RETRY: Duration = Duration::from_secs(2);

pub const SELF_DRIVE_MIDI: &str = "Entrée et sortie de timecode sur le même port : le studio se piloterait lui-même.";
pub const SELF_DRIVE_NET: &str = "Entrée et sortie de timecode sur la même interface réseau : le studio se piloterait lui-même.";

/// `timecode.json`, key `out`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimecodeOut {
    pub enabled: bool,
    /// MTC on this MIDI output port; `None`: no MTC.
    pub mtc_port: Option<String>,
    /// Art-Net timecode; `None`: none.
    pub artnet: Option<ArtNetTcOut>,
    pub fps: Rate,
    /// Timecode = show position + offset (s): 3600 starts the show at
    /// 01:00:00:00.
    pub offset_s: f64,
}

impl Default for TimecodeOut {
    fn default() -> Self {
        TimecodeOut { enabled: false, mtc_port: None, artnet: None, fps: Rate::F25, offset_s: 0.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtNetTcOut {
    /// The address of this Mac to send from.
    pub interface: String,
    /// `ip` or `ip:port` of one host; "" = the broadcast of the interface.
    pub target: String,
}

/// A refused request: the HTTP status and the French message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub message: String,
}

impl Refused {
    pub fn bad(message: impl Into<String>) -> Self {
        Refused { status: 400, message: message.into() }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Refused { status: 409, message: message.into() }
    }
}

impl From<String> for Refused {
    fn from(message: String) -> Self {
        Refused::bad(message)
    }
}

impl From<&str> for Refused {
    fn from(message: &str) -> Self {
        Refused::bad(message)
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.message)
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

/// A typed target: `ip` or `ip:port`, one host (no port 0, no 0.0.0.0, no
/// multicast, no broadcast: the empty target is the broadcast).
pub fn parse_target(s: &str) -> Result<SocketAddr, String> {
    let s = s.trim();
    let addr = match s.parse::<SocketAddr>() {
        Ok(a) => a,
        Err(_) => SocketAddr::new(s.parse::<IpAddr>().map_err(|_| format!("adresse cible invalide : « {s} » (adresse IP ou IP:port, ex. 192.168.1.20)"))?, ARTNET_PORT),
    };
    let ip = addr.ip();
    let broadcast = matches!(ip, IpAddr::V4(v4) if v4.is_broadcast() || v4.octets()[3] == 255);
    if addr.port() == 0 || ip.is_unspecified() || ip.is_multicast() || broadcast {
        return Err(format!("adresse cible invalide : {addr} (une seule machine ; laissez vide pour la diffusion sur l'interface)"));
    }
    Ok(addr)
}

/// The broadcast address of the interface whose address is `ip`.
fn broadcast_of(ip: IpAddr) -> Option<Ipv4Addr> {
    if_addrs::get_if_addrs().ok()?.into_iter().find_map(|i| match i.addr {
        if_addrs::IfAddr::V4(v4) if IpAddr::V4(v4.ip) == ip => v4.broadcast,
        _ => None,
    })
}

/// Where the packets go from and to; `broadcast`: the interface's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub bind: IpAddr,
    pub dest: SocketAddr,
    pub broadcast: bool,
}

impl ArtNetTcOut {
    pub fn validate(&self) -> Result<(), String> {
        parse_interface(&self.interface)?;
        if !self.target.trim().is_empty() {
            parse_target(&self.target)?;
        }
        Ok(())
    }

    /// The route now (the broadcast address is looked up on the interface).
    pub fn route(&self) -> Result<Route, String> {
        let bind = parse_interface(&self.interface)?;
        if !self.target.trim().is_empty() {
            return Ok(Route { bind, dest: parse_target(&self.target)?, broadcast: false });
        }
        let b = broadcast_of(bind).ok_or_else(|| format!("pas de diffusion sur {bind} : saisissez une adresse cible"))?;
        Ok(Route { bind, dest: SocketAddr::new(IpAddr::V4(b), ARTNET_PORT), broadcast: true })
    }
}

impl TimecodeOut {
    pub fn validate(&self) -> Result<(), String> {
        if !self.offset_s.is_finite() || self.offset_s.abs() > MAX_OFFSET_S {
            return Err("décalage hors limites (± 24 h)".into());
        }
        if self.mtc_port.as_ref().is_some_and(|p| p.trim().is_empty() || p.len() > 128) {
            return Err("port MIDI invalide".into());
        }
        if let Some(a) = &self.artnet {
            a.validate()?;
        }
        Ok(())
    }

    /// Something to send to.
    pub fn has_destination(&self) -> bool {
        self.mtc_port.is_some() || self.artnet.is_some()
    }
}

fn same_ip(a: &str, b: &str) -> bool {
    match (a.trim().parse::<IpAddr>(), b.trim().parse::<IpAddr>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.trim() == b.trim(),
    }
}

/// The self-drive trap: the input (`input`, whose active source is
/// `source`: the loaded show's own, else the global one) would receive
/// what `out` sends. MTC: the input listens to the output's port (or to
/// every port). Art-Net: the input listens on the output's interface (or
/// on all of them), or the target is the input's address.
pub fn conflicts(out: &TimecodeOut, input: &TcConfig, source: TcSource) -> Option<String> {
    match source {
        TcSource::Mtc => {
            let port = out.mtc_port.as_ref()?;
            input.midi_port.as_ref().is_none_or(|p| p == port).then(|| SELF_DRIVE_MIDI.to_string())
        }
        TcSource::ArtNet => {
            let a = out.artnet.as_ref()?;
            let listen = input.interface.trim();
            if listen.is_empty() {
                return None;
            }
            let all = listen.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified());
            let target = parse_target(&a.target).ok().is_some_and(|t| same_ip(&t.ip().to_string(), listen));
            (all || same_ip(&a.interface, listen) || target).then(|| SELF_DRIVE_NET.to_string())
        }
        TcSource::Off | TcSource::Ltc => None,
    }
}

/// The eight quarter-frame messages (`F1 0nnn dddd`) of `tc`: they are
/// sent over two frames, piece 0 at the start of `tc`.
pub fn quarter_frames(tc: Tc, rate: Rate) -> [[u8; 2]; 8] {
    mtc::quarter_frames(tc, rate).map(|d| [0xF1, d])
}

/// The full-frame SysEx (a locate).
pub fn full_frame(tc: Tc, rate: Rate) -> Vec<u8> {
    mtc::full_frame_bytes(tc, rate)
}

/// The ArtTimeCode packet.
pub fn artnet_packet(tc: Tc, rate: Rate) -> Vec<u8> {
    artnet::encode(tc, rate)
}

/// What the engine hands the "tc-out" thread, once per frame.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub cfg: TimecodeOut,
    /// Why nothing may be sent now (self-drive, no show in seconds).
    pub blocked: Option<String>,
    pub transport: Transport,
    /// Show seconds at `at`.
    pub position: f64,
    /// `Player::jumps`: every seek, start, pause, stop, loop wrap.
    pub jumps: u64,
    pub at: Instant,
}

impl Snapshot {
    fn active(&self) -> bool {
        self.cfg.enabled && self.blocked.is_none() && self.cfg.has_destination()
    }

    /// Timecode seconds at `now` (run on from `at` while playing).
    fn secs(&self, now: Instant) -> f64 {
        let run = if self.transport == Transport::Playing { now.saturating_duration_since(self.at).as_secs_f64() } else { 0.0 };
        self.cfg.offset_s + self.position + run
    }

    /// Instant at which the timecode reaches `secs`.
    fn instant_of(&self, secs: f64) -> Instant {
        let d = secs - self.cfg.offset_s - self.position;
        if d > 0.0 {
            self.at + Duration::from_secs_f64(d.min(3600.0))
        } else {
            self.at
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    Mtc(Vec<u8>),
    ArtNet(Vec<u8>),
}

/// One quarter-frame run, from a full frame on.
#[derive(Clone, Copy, Debug)]
struct Run {
    /// Quarter index (frames × 4) of piece 0 of the first time.
    origin: i64,
    next: i64,
    last_artnet: Option<i64>,
    rate: Rate,
    offset_s: f64,
}

/// Decides what to send and when; pure (the clock is passed in), so the
/// tests run it on simulated time.
#[derive(Debug, Default)]
pub struct Scheduler {
    run: Option<Run>,
    jumps: Option<u64>,
    transport: Option<Transport>,
    pub last: Option<(Tc, Rate)>,
}

fn quarter_at(secs: f64, rate: Rate) -> i64 {
    (secs * rate.fps() * 4.0 + 1e-6).floor() as i64
}

impl Scheduler {
    /// A run is going (playing, allowed, something to send to).
    pub fn sending(&self) -> bool {
        self.run.is_some()
    }

    fn full(&mut self, out: &mut Vec<Packet>, cfg: &TimecodeOut, frame: i64) {
        let tc = Tc::from_frames(frame, cfg.fps);
        if cfg.mtc_port.is_some() {
            out.push(Packet::Mtc(full_frame(tc, cfg.fps)));
        }
        if cfg.artnet.is_some() {
            out.push(Packet::ArtNet(artnet_packet(tc, cfg.fps)));
        }
        self.last = Some((tc, cfg.fps));
    }

    /// What is due at `now` for the latest snapshot.
    pub fn tick(&mut self, snap: Option<&Snapshot>, now: Instant) -> Vec<Packet> {
        let mut out = Vec::new();
        let Some(s) = snap.filter(|s| s.active()) else {
            (self.run, self.jumps, self.transport) = (None, None, None);
            return out;
        };
        let jumped = self.jumps.is_some_and(|j| j != s.jumps);
        let same_transport = self.transport == Some(s.transport);
        (self.jumps, self.transport) = (Some(s.jumps), Some(s.transport));
        let rate = s.cfg.fps;
        let q = quarter_at(s.secs(now), rate);
        if s.transport != Transport::Playing {
            self.run = None;
            // A seek while paused or stopped: one full frame. Pausing and
            // stopping send nothing.
            if jumped && same_transport {
                self.full(&mut out, &s.cfg, q.div_euclid(4));
            }
            return out;
        }
        let resync = match self.run {
            None => true,
            Some(r) => jumped || r.rate != rate || r.offset_s != s.cfg.offset_s || (q - r.next).abs() > MAX_DRIFT_QUARTERS,
        };
        if resync {
            self.full(&mut out, &s.cfg, q.div_euclid(4));
            // Piece 0 on the next frame boundary (now, if on one).
            let origin = (q + 3).div_euclid(4) * 4;
            self.run = Some(Run { origin, next: origin, last_artnet: Some(q.div_euclid(4)), rate, offset_s: s.cfg.offset_s });
        }
        let Some(run) = self.run.as_mut() else { return out };
        while run.next <= q {
            let rel = run.next - run.origin;
            let piece = rel.rem_euclid(8) as usize;
            let tc = Tc::from_frames(run.origin / 4 + 2 * rel.div_euclid(8), rate);
            if s.cfg.mtc_port.is_some() {
                out.push(Packet::Mtc(quarter_frames(tc, rate)[piece].to_vec()));
            }
            run.next += 1;
        }
        let frame = q.div_euclid(4);
        if s.cfg.artnet.is_some() && run.last_artnet != Some(frame) {
            run.last_artnet = Some(frame);
            out.push(Packet::ArtNet(artnet_packet(Tc::from_frames(frame, rate), rate)));
        }
        self.last = Some((Tc::from_frames(frame, rate), rate));
        out
    }

    /// When the next message is due (None: nothing runs).
    pub fn next_due(&self, snap: Option<&Snapshot>) -> Option<Instant> {
        let (run, s) = (self.run.as_ref()?, snap?);
        let fps = run.rate.fps();
        let quarter = if s.cfg.mtc_port.is_some() { run.next } else { (run.last_artnet.unwrap_or(0) + 1) * 4 };
        Some(s.instant_of(quarter as f64 / 4.0 / fps))
    }
}

/// What the "tc-out" thread reports.
#[derive(Clone, Debug, Default, Serialize)]
pub struct OutStatus {
    pub sending: bool,
    /// The last time sent, hh:mm:ss:ff.
    pub last: Option<String>,
    pub mtc_messages: u64,
    pub artnet_packets: u64,
    pub errors: u64,
    pub error: Option<String>,
    /// The MIDI port open for MTC.
    pub midi_open: Option<String>,
    /// Where Art-Net timecode goes (`ip:port`).
    pub artnet_to: Option<String>,
}

/// Opens a MIDI output port by name on the thread that calls it (the
/// MIDI backend in use: CoreMIDI, or the simulator of `--midi-test`).
pub type MidiOpener = Arc<dyn Fn(&str) -> Result<Box<dyn OutputPort>, String> + Send + Sync>;

/// The engine's side (`TimecodeState::out`): never saved.
pub struct OutRuntime {
    tx: SyncSender<Snapshot>,
    rx: Option<Receiver<Snapshot>>,
    pub status: Arc<Mutex<OutStatus>>,
    /// The reason the stream is held back now (for the panel).
    pub blocked: Option<String>,
    /// Snapshots the full queue refused.
    pub dropped: u64,
    /// The last one queued said « disabled »: no need to repeat it.
    idle_sent: bool,
}

impl Default for OutRuntime {
    fn default() -> Self {
        let (tx, rx) = sync_channel(QUEUE_LEN);
        OutRuntime { tx, rx: Some(rx), status: Arc::default(), blocked: None, dropped: 0, idle_sent: false }
    }
}

impl OutRuntime {
    pub fn take_receiver(&mut self) -> Option<Receiver<Snapshot>> {
        self.rx.take()
    }

    pub fn status(&self) -> OutStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Queues `snap`. A disabled output is told once, not every frame.
    fn publish(&mut self, snap: Snapshot) {
        let off = !snap.cfg.enabled;
        if off && self.idle_sent {
            return;
        }
        match self.tx.try_send(snap) {
            Ok(()) => self.idle_sent = off,
            Err(_) => self.dropped += 1,
        }
    }
}

/// Why the stream is held back (None: it may run).
fn blocked(s: &Shared) -> Option<String> {
    let tc = &s.timecode;
    let cfg = tc.config();
    let mut input = cfg.clone();
    // The Art-Net input's socket carries the timecode as well when its
    // chart is on.
    let net = s.netin.hub.effective();
    if input.interface.trim().is_empty() && net.artnet.enabled {
        input.interface = net.artnet.bind.clone();
    }
    if let Some(why) = conflicts(&cfg.out, &input, tc.source()) {
        return Some(why);
    }
    match s.timeline.show.as_ref().map(|sh| sh.time_base) {
        None => Some("aucun show chargé".into()),
        Some(TimeBase::Beats) => Some("show en temps : le timecode n'est émis que pour les shows en secondes".into()),
        Some(TimeBase::Seconds) => None,
    }
}

/// Once per engine frame, after the playhead moved (main.rs).
pub fn frame(s: &mut Shared, c: &Clock) {
    let cfg = s.timecode.config().out.clone();
    let why = if cfg.enabled { blocked(s) } else { None };
    // The instant of `c.t` on the studio clock (`now_s`).
    let at = s.epoch + Duration::from_secs_f64(c.t.max(0.0));
    let snap = Snapshot { cfg, blocked: why.clone(), transport: s.timeline.transport, position: s.timeline.position_at(c), jumps: s.timeline.jumps(), at };
    let out = &mut s.timecode.out;
    out.blocked = why;
    out.publish(snap);
}

/// The two senders the thread owns.
struct Sinks {
    opener: Option<MidiOpener>,
    midi: Option<(String, Box<dyn OutputPort>)>,
    midi_retry: Option<(String, Instant)>,
    udp: Option<(Route, UdpSocket)>,
    udp_retry: Option<(ArtNetTcOut, Instant)>,
}

fn set_error(status: &Mutex<OutStatus>, error: String) {
    let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
    st.errors += 1;
    if st.error.as_ref() != Some(&error) {
        log::warn!("Timecode sortant : {error}");
    }
    st.error = Some(error);
}

impl Sinks {
    /// Opens or closes the port and the socket the settings ask for.
    fn reconcile(&mut self, snap: Option<&Snapshot>, status: &Mutex<OutStatus>, now: Instant) {
        let cfg = snap.map(|s| &s.cfg).filter(|c| c.enabled);
        let port = cfg.and_then(|c| c.mtc_port.clone());
        if self.midi.as_ref().map(|(n, _)| n) != port.as_ref() {
            self.midi = None;
            if let Some(name) = port.as_ref() {
                let retry = self.midi_retry.as_ref().is_some_and(|(n, at)| n == name && now < *at);
                match (&self.opener, retry) {
                    (_, true) => {}
                    (None, false) => {
                        set_error(status, "MIDI désactivé : pas de MTC".into());
                        self.midi_retry = Some((name.clone(), now + RETRY));
                    }
                    (Some(open), false) => match open(name) {
                        Ok(p) => {
                            log::info!("Timecode sortant : MTC sur {name}");
                            self.midi = Some((name.clone(), p));
                            self.midi_retry = None;
                        }
                        Err(e) => {
                            set_error(status, format!("port MIDI {name} indisponible : {e}"));
                            self.midi_retry = Some((name.clone(), now + RETRY));
                        }
                    },
                }
            }
        }
        let art = cfg.and_then(|c| c.artnet.clone());
        let route = art.as_ref().map(|a| a.route());
        let wanted = match &route {
            Some(Ok(r)) => Some(*r),
            _ => None,
        };
        if self.udp.as_ref().map(|(r, _)| *r) != wanted {
            self.udp = None;
        }
        if let (Some(a), None) = (art, &self.udp) {
            let retry = self.udp_retry.as_ref().is_some_and(|(r, at)| *r == a && now < *at);
            if !retry {
                let opened = route.unwrap_or_else(|| Err("Art-Net".into())).and_then(|r| {
                    let sock = UdpSocket::bind(SocketAddr::new(r.bind, 0)).map_err(|e| format!("impossible d'envoyer depuis {} : {e}", r.bind))?;
                    // Broadcast only when the operator left the target empty.
                    sock.set_broadcast(r.broadcast).map_err(|e| format!("diffusion impossible sur {} : {e}", r.bind))?;
                    Ok((r, sock))
                });
                match opened {
                    Ok(u) => {
                        log::info!("Timecode sortant : Art-Net vers {}", u.0.dest);
                        self.udp = Some(u);
                        self.udp_retry = None;
                    }
                    Err(e) => {
                        set_error(status, e);
                        self.udp_retry = Some((a, now + RETRY));
                    }
                }
            }
        }
        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
        st.midi_open = self.midi.as_ref().map(|(n, _)| n.clone());
        st.artnet_to = self.udp.as_ref().map(|(r, _)| r.dest.to_string());
    }

    fn deliver(&mut self, packets: Vec<Packet>, status: &Mutex<OutStatus>, now: Instant) {
        for p in packets {
            match p {
                Packet::Mtc(bytes) => {
                    let Some((name, port)) = self.midi.as_mut() else { continue };
                    match port.send(&bytes) {
                        Ok(()) => status.lock().unwrap_or_else(|e| e.into_inner()).mtc_messages += 1,
                        Err(e) => {
                            let name = name.clone();
                            set_error(status, format!("envoi MTC vers {name} : {e}"));
                            // Reopened after a pause (the port may be gone).
                            self.midi = None;
                            self.midi_retry = Some((name, now + RETRY));
                        }
                    }
                }
                Packet::ArtNet(bytes) => {
                    let Some((r, sock)) = self.udp.as_ref() else { continue };
                    match sock.send_to(&bytes, r.dest) {
                        Ok(_) => status.lock().unwrap_or_else(|e| e.into_inner()).artnet_packets += 1,
                        Err(e) => set_error(status, format!("envoi Art-Net vers {} : {e}", r.dest)),
                    }
                }
            }
        }
    }
}

/// Starts the "tc-out" thread. None: it could not be created (logged);
/// the queue then fills and drops, the engine is not affected.
pub fn spawn(rx: Receiver<Snapshot>, status: Arc<Mutex<OutStatus>>, opener: Option<MidiOpener>, running: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("tc-out".into())
        .spawn(move || run(rx, &status, opener, &running))
        .map_err(|e| log::warn!("Timecode sortant : impossible de démarrer le thread : {e}"))
        .ok()
}

fn run(rx: Receiver<Snapshot>, status: &Mutex<OutStatus>, opener: Option<MidiOpener>, running: &AtomicBool) {
    let mut sched = Scheduler::default();
    let mut sinks = Sinks { opener, midi: None, midi_retry: None, udp: None, udp_retry: None };
    let mut snap: Option<Snapshot> = None;
    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        let wait = sched.next_due(snap.as_ref()).map_or(POLL, |at| at.saturating_duration_since(now).clamp(Duration::from_micros(500), POLL));
        match rx.recv_timeout(wait) {
            Ok(s) => snap = Some(rx.try_iter().last().unwrap_or(s)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        sinks.reconcile(snap.as_ref(), status, now);
        let packets = sched.tick(snap.as_ref(), now);
        sinks.deliver(packets, status, now);
        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
        st.sending = sched.sending();
        st.last = sched.last.map(|(tc, _)| tc.to_string());
        if !snap.as_ref().is_some_and(|s| s.cfg.enabled) {
            st.error = None;
        }
    }
}

/// The MIDI output ports listed now.
fn midi_outputs(s: &Shared) -> Vec<String> {
    s.midi.devices.iter().filter(|d| d.output).map(|d| d.name.clone()).collect()
}

/// The addresses of this Mac's interfaces (IPv4).
fn local_ips() -> Vec<String> {
    crate::netin::interfaces().iter().filter_map(|i| i["ip"].as_str().map(str::to_string)).collect()
}

/// The panel's summary, under `timeline.timecode_out` of `/api/frame`.
pub fn summary(s: &Shared) -> Value {
    let cfg = &s.timecode.config().out;
    let st = s.timecode.out.status();
    json!({
        "enabled": cfg.enabled,
        "sending": cfg.enabled && st.sending,
        "last": st.last,
        "blocked": s.timecode.out.blocked,
        "error": st.error,
    })
}

/// `GET /api/timecode/out`.
pub fn view(s: &Shared) -> Value {
    let cfg = &s.timecode.config().out;
    json!({
        "config": cfg,
        "status": s.timecode.out.status(),
        "summary": summary(s),
        "midi_enabled": s.midi.enabled,
        "midi_ports": midi_outputs(s),
        "interfaces": crate::netin::interfaces(),
        "rates": Rate::ALL.iter().map(|r| json!({ "id": r, "label": r.label_fr() })).collect::<Vec<_>>(),
    })
}

/// `POST /api/timecode/out` `{ "enabled": true, "mtc_port": "…", … }`:
/// the fields given replace the current ones. The MIDI port must be a
/// listed output, the interface an address of this Mac; the self-drive
/// trap is refused (409). Returns whether anything changed (the caller
/// saves `timecode.json`).
pub fn update(s: &mut Shared, patch: &Value) -> Result<bool, Refused> {
    let obj = patch.as_object().ok_or("objet JSON attendu")?;
    let before = s.timecode.config().out.clone();
    let mut v = json!(before);
    for (k, val) in obj {
        if v.get(k).is_none() {
            return Err(Refused::bad(format!("réglage inconnu : {k}")));
        }
        v[k] = val.clone();
    }
    let cfg: TimecodeOut = serde_json::from_value(v).map_err(|e| format!("réglage invalide : {e}"))?;
    cfg.validate()?;
    if let Some(port) = cfg.mtc_port.as_ref().filter(|p| before.mtc_port.as_ref() != Some(*p)) {
        if !s.midi.enabled {
            return Err("MIDI désactivé : pas de MTC".into());
        }
        if !midi_outputs(s).contains(port) {
            return Err(Refused::bad(format!("port MIDI introuvable : {port}")));
        }
    }
    if let Some(a) = cfg.artnet.as_ref().filter(|a| before.artnet.as_ref() != Some(*a)) {
        if !local_ips().iter().any(|ip| same_ip(ip, &a.interface)) {
            return Err(Refused::bad(format!("{} n'est pas une adresse de ce Mac", a.interface.trim())));
        }
        a.route()?;
    }
    if cfg.enabled {
        let mut input = s.timecode.config().clone();
        let net = s.netin.hub.effective();
        if input.interface.trim().is_empty() && net.artnet.enabled {
            input.interface = net.artnet.bind.clone();
        }
        if let Some(why) = conflicts(&cfg, &input, s.timecode.source()) {
            return Err(Refused::conflict(why));
        }
    }
    if cfg == before {
        return Ok(false);
    }
    if let Some(change) = crate::safety_log::settings_change(&before, &cfg) {
        s.gate.log().record("timecode_out_settings", Some(ArmSource::Ui.id()), change);
    }
    if cfg.enabled != before.enabled {
        log::info!("Timecode sortant {}", if cfg.enabled { "activé" } else { "désactivé" });
    }
    s.timecode.set_out(cfg);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::testing::{FakeApc, SimMidi};
    use crate::test_support::shared;
    use crate::timecode::mtc::{MtcDecoder, PIECE7_LAG_FRAMES};

    fn tc(h: u8, m: u8, s: u8, f: u8) -> Tc {
        Tc { h, m, s, f }
    }

    fn mtc_cfg(rate: Rate) -> TimecodeOut {
        TimecodeOut { enabled: true, mtc_port: Some("Sortie".into()), artnet: None, fps: rate, offset_s: 0.0 }
    }

    fn loop_cfg(rate: Rate, port: u16) -> TimecodeOut {
        TimecodeOut { enabled: true, mtc_port: None, artnet: Some(ArtNetTcOut { interface: "127.0.0.1".into(), target: format!("127.0.0.1:{port}") }), fps: rate, offset_s: 0.0 }
    }

    fn snap(cfg: &TimecodeOut, transport: Transport, position: f64, jumps: u64, at: Instant) -> Snapshot {
        Snapshot { cfg: cfg.clone(), blocked: None, transport, position, jumps, at }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Every quarter frame of an hour, through the encoder then the
    /// input's decoder: each completed time is the frame it was sent for.
    #[test]
    fn an_hour_of_quarter_frames_round_trips_at_every_rate() {
        for rate in Rate::ALL {
            let mut d = MtcDecoder::default();
            let frames = (3600.0 * rate.fps()) as i64;
            let start = Tc { h: 1, m: 0, s: 0, f: 0 }.frames(rate);
            let mut decoded = 0;
            for k in (0..frames).step_by(2) {
                let want = Tc::from_frames(start + k, rate);
                assert!(want.valid(rate), "{rate:?} {want}");
                let msgs = quarter_frames(want, rate);
                for (i, m) in msgs.iter().enumerate() {
                    assert_eq!(m[0], 0xF1);
                    let got = d.quarter(m[1]);
                    assert_eq!(got, (i == 7).then_some((want, rate)), "{rate:?} {want} piece {i}");
                }
                decoded += 1;
            }
            assert_eq!(decoded, (frames + 1) / 2);
        }
    }

    #[test]
    fn full_frames_and_artnet_packets_decode_to_the_same_time() {
        for rate in Rate::ALL {
            for n in [0i64, 1, 1_799, 17_982, 107_892, 2_000_000] {
                let t = Tc::from_frames(n, rate);
                assert_eq!(mtc::full_frame(&full_frame(t, rate)), Some((t, rate)));
                assert_eq!(artnet::decode(&artnet_packet(t, rate)), Ok((t, rate)));
                assert_eq!(crate::netin::artnet::decode(&artnet_packet(t, rate)).map_err(|_| ()), Ok(crate::netin::artnet::Packet::TimeCode { tc: t, rate }));
            }
        }
    }

    /// Plays `secs` of timeline from `from` with a tick every `step`;
    /// returns (time of the tick, packet) for every packet.
    fn play(sched: &mut Scheduler, s: &Snapshot, secs: f64, step: Duration) -> Vec<(Instant, Packet)> {
        let mut out = Vec::new();
        let mut now = s.at;
        while now <= s.at + Duration::from_secs_f64(secs) {
            out.extend(sched.tick(Some(s), now).into_iter().map(|p| (now, p)));
            now += step;
        }
        out
    }

    #[test]
    fn playing_sends_a_full_frame_then_a_steady_stream_that_decodes_to_the_playhead() {
        let t0 = Instant::now();
        for rate in Rate::ALL {
            let mut cfg = mtc_cfg(rate);
            cfg.offset_s = 3600.0;
            let s = snap(&cfg, Transport::Playing, 12.3, 1, t0);
            let mut sched = Scheduler::default();
            let sent = play(&mut sched, &s, 20.0, ms(1));
            // First a full frame of the playhead.
            let Packet::Mtc(first) = &sent[0].1 else { panic!() };
            let (full, _) = mtc::full_frame(first).expect("full frame");
            assert_eq!(full, Tc::from_seconds(3600.0 + 12.3, rate), "{rate:?}");
            // Then four quarter frames per frame, each decoded time within
            // one frame of the playhead when its piece 7 arrives.
            let qf: Vec<_> = sent[1..].iter().map(|(at, p)| (*at, if let Packet::Mtc(b) = p { b.clone() } else { panic!() })).collect();
            assert!(qf.iter().all(|(_, b)| b.len() == 2 && b[0] == 0xF1));
            let expected = 20.0 * rate.fps() * 4.0;
            assert!((qf.len() as f64 - expected).abs() <= 8.0, "{rate:?}: {} quarter frames", qf.len());
            let mut d = MtcDecoder::default();
            let mut times = 0;
            for (at, b) in &qf {
                if let Some((t, r)) = d.quarter(b[1]) {
                    assert_eq!(r, rate);
                    let playhead = s.secs(*at) * rate.fps();
                    let decoded = t.frames(rate) as f64 + PIECE7_LAG_FRAMES;
                    assert!((decoded - playhead).abs() <= 1.0, "{rate:?} {t}: {decoded} vs {playhead}");
                    times += 1;
                }
            }
            assert!(times as f64 >= 20.0 * rate.fps() / 2.0 - 2.0, "{rate:?}: {times}");
            // Even if the thread wakes late, nothing is skipped or doubled.
            let mut late = Scheduler::default();
            let coarse = play(&mut late, &s, 20.0, ms(7));
            assert!((coarse.len() as i64 - sent.len() as i64).abs() <= 4);
        }
    }

    #[test]
    fn drop_frame_never_sends_a_dropped_label() {
        let t0 = Instant::now();
        let mut cfg = mtc_cfg(Rate::F2997Df);
        cfg.artnet = Some(ArtNetTcOut { interface: "127.0.0.1".into(), target: "127.0.0.1:6454".into() });
        // Across 00:00:59 → 00:02:01 and 00:09:59 → 00:10:01.
        for start in [59.0, 599.0] {
            let s = snap(&cfg, Transport::Playing, start, 1, t0);
            let mut sched = Scheduler::default();
            let mut d = MtcDecoder::default();
            let mut labels = Vec::new();
            for (_, p) in play(&mut sched, &s, 62.0, ms(3)) {
                match p {
                    Packet::Mtc(b) if b[0] == 0xF1 => labels.extend(d.quarter(b[1]).map(|(t, _)| t)),
                    Packet::Mtc(b) => labels.extend(mtc::full_frame(&b).map(|(t, _)| t)),
                    Packet::ArtNet(b) => labels.push(artnet::decode(&b).expect("valid").0),
                }
            }
            assert!(!labels.is_empty());
            for t in &labels {
                assert!(t.valid(Rate::F2997Df), "{t}");
                assert!(!(t.s == 0 && t.f < 2 && t.m % 10 != 0), "dropped label {t}");
            }
            if start == 599.0 {
                assert!(labels.iter().any(|t| (t.m, t.s, t.f) == (10, 0, 0)), "00:10:00;00 exists");
            }
        }
    }

    #[test]
    fn a_seek_sends_a_full_frame_and_a_new_run_and_a_loop_wrap_too() {
        let t0 = Instant::now();
        let cfg = mtc_cfg(Rate::F25);
        let mut sched = Scheduler::default();
        let s1 = snap(&cfg, Transport::Playing, 0.0, 1, t0);
        let fulls = |p: &[(Instant, Packet)]| p.iter().filter(|(_, p)| matches!(p, Packet::Mtc(b) if b[0] == 0xF0)).count();
        assert_eq!(fulls(&play(&mut sched, &s1, 1.0, ms(2))), 1);
        // The operator seeks to 30 s (or the loop wraps): jumps changes.
        let s2 = snap(&cfg, Transport::Playing, 30.0, 2, t0 + ms(1002));
        let sent = play(&mut sched, &s2, 1.0, ms(2));
        assert_eq!(fulls(&sent), 1);
        let Packet::Mtc(b) = &sent[0].1 else { panic!() };
        assert_eq!(mtc::full_frame(b).map(|(t, _)| t), Some(tc(0, 0, 30, 0)));
        // The new run starts with piece 0 of 00:00:30:00.
        let mut d = MtcDecoder::default();
        let first = sent.iter().filter_map(|(_, p)| if let Packet::Mtc(b) = p { (b[0] == 0xF1).then(|| d.quarter(b[1])).flatten() } else { None }).next();
        assert_eq!(first, Some((tc(0, 0, 30, 0), Rate::F25)));
        // The same position moved by the song's pull (no jump): no full frame.
        let s3 = snap(&cfg, Transport::Playing, 31.0 + 0.01, 2, t0 + ms(2004));
        assert_eq!(fulls(&play(&mut sched, &s3, 0.5, ms(2))), 0);
    }

    #[test]
    fn pause_and_stop_stop_the_stream_and_a_locate_sends_one_full_frame() {
        let t0 = Instant::now();
        let mut cfg = mtc_cfg(Rate::F25);
        cfg.artnet = Some(ArtNetTcOut { interface: "127.0.0.1".into(), target: "127.0.0.1:6454".into() });
        let mut sched = Scheduler::default();
        let playing = snap(&cfg, Transport::Playing, 5.0, 1, t0);
        assert!(!play(&mut sched, &playing, 1.0, ms(2)).is_empty());
        assert!(sched.sending());
        // Paused: nothing, however long.
        let paused = snap(&cfg, Transport::Paused, 6.0, 2, t0 + ms(1002));
        assert!(play(&mut sched, &paused, 3.0, ms(5)).is_empty());
        assert!(!sched.sending());
        assert!(sched.next_due(Some(&paused)).is_none());
        // A seek while paused: one full frame (MTC and Art-Net), once.
        let located = snap(&cfg, Transport::Paused, 42.0, 3, t0 + ms(5000));
        let sent = play(&mut sched, &located, 2.0, ms(5));
        assert_eq!(sent.len(), 2);
        assert_eq!(sent.iter().find_map(|(_, p)| if let Packet::Mtc(b) = p { mtc::full_frame(b) } else { None }), Some((tc(0, 0, 42, 0), Rate::F25)));
        assert_eq!(sent.iter().find_map(|(_, p)| if let Packet::ArtNet(b) = p { artnet::decode(b).ok() } else { None }), Some((tc(0, 0, 42, 0), Rate::F25)));
        // Stop (back to 0): nothing.
        let stopped = snap(&cfg, Transport::Stopped, 0.0, 4, t0 + ms(8000));
        assert!(play(&mut sched, &stopped, 2.0, ms(5)).is_empty());
    }

    #[test]
    fn disabled_blocked_or_without_destination_never_sends() {
        let t0 = Instant::now();
        let mut cfg = mtc_cfg(Rate::F30);
        cfg.enabled = false;
        let mut sched = Scheduler::default();
        assert!(play(&mut sched, &snap(&cfg, Transport::Playing, 0.0, 1, t0), 2.0, ms(2)).is_empty());
        cfg.enabled = true;
        let mut blocked = snap(&cfg, Transport::Playing, 0.0, 1, t0);
        blocked.blocked = Some(SELF_DRIVE_MIDI.into());
        assert!(play(&mut sched, &blocked, 2.0, ms(2)).is_empty());
        cfg.mtc_port = None;
        assert!(play(&mut sched, &snap(&cfg, Transport::Playing, 0.0, 1, t0), 2.0, ms(2)).is_empty());
        assert!(sched.tick(None, t0).is_empty());
    }

    #[test]
    fn artnet_sends_one_packet_per_frame_on_loopback_that_decodes_to_the_playhead() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(ms(2000))).unwrap();
        let port = rx.local_addr().unwrap().port();
        for rate in Rate::ALL {
            let cfg = loop_cfg(rate, port);
            let route = cfg.artnet.as_ref().unwrap().route().unwrap();
            assert_eq!((route.dest.port(), route.broadcast), (port, false));
            let t0 = Instant::now();
            let s = snap(&cfg, Transport::Playing, 100.0, 1, t0);
            let mut sched = Scheduler::default();
            let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
            let sent = play(&mut sched, &s, 2.0, ms(1));
            let expected = 2.0 * rate.fps();
            assert!((sent.len() as f64 - expected).abs() <= 2.0, "{rate:?}: {}", sent.len());
            for (at, p) in &sent {
                let Packet::ArtNet(b) = p else { panic!("Art-Net only") };
                tx.send_to(b, route.dest).unwrap();
                let mut buf = [0u8; 64];
                let n = rx.recv(&mut buf).unwrap();
                let (t, r) = crate::timecode::artnet::decode(&buf[..n]).expect("decodes");
                assert_eq!(r, rate);
                let playhead = s.secs(*at) * rate.fps();
                assert!((t.frames(rate) as f64 - playhead).abs() <= 1.0, "{rate:?} {t} vs {playhead}");
            }
        }
    }

    #[test]
    fn targets_and_interfaces_are_checked() {
        assert_eq!(parse_target("192.168.1.20").unwrap(), "192.168.1.20:6454".parse().unwrap());
        assert_eq!(parse_target("10.0.0.2:7000").unwrap(), "10.0.0.2:7000".parse().unwrap());
        for bad in ["", "nom.local", "0.0.0.0", "239.1.2.3", "255.255.255.255", "192.168.1.255", "10.0.0.2:0"] {
            assert!(parse_target(bad).is_err(), "{bad}");
        }
        for bad in ["", "0.0.0.0", "224.0.0.1", "255.255.255.255", "en0"] {
            assert!(ArtNetTcOut { interface: bad.into(), target: "10.0.0.2".into() }.validate().is_err(), "{bad}");
        }
        // Loopback has no broadcast: a target must be typed.
        assert!(ArtNetTcOut { interface: "127.0.0.1".into(), target: String::new() }.route().is_err());
        let mut out = TimecodeOut::default();
        assert!(!out.enabled && out.validate().is_ok() && !out.has_destination());
        out.offset_s = f64::NAN;
        assert!(out.validate().is_err());
    }

    #[test]
    fn the_self_drive_trap_is_refused() {
        let out = mtc_cfg(Rate::F25);
        let mut input = TcConfig { source: TcSource::Mtc, midi_port: Some("Sortie".into()), ..Default::default() };
        assert_eq!(conflicts(&out, &input, TcSource::Mtc).as_deref(), Some(SELF_DRIVE_MIDI));
        // Any port includes ours.
        input.midi_port = None;
        assert!(conflicts(&out, &input, TcSource::Mtc).is_some());
        input.midi_port = Some("Autre".into());
        assert_eq!(conflicts(&out, &input, TcSource::Mtc), None);
        // Another source in use (the show's own): no conflict.
        input.midi_port = Some("Sortie".into());
        assert_eq!(conflicts(&out, &input, TcSource::Ltc), None);
        assert_eq!(conflicts(&out, &input, TcSource::Off), None);
        // Art-Net: same interface, all interfaces, or our target is the input.
        let net = loop_cfg(Rate::F25, 7000);
        let mut input = TcConfig { source: TcSource::ArtNet, interface: "127.0.0.1".into(), ..Default::default() };
        assert_eq!(conflicts(&net, &input, TcSource::ArtNet).as_deref(), Some(SELF_DRIVE_NET));
        input.interface = "0.0.0.0".into();
        assert!(conflicts(&net, &input, TcSource::ArtNet).is_some());
        input.interface = String::new();
        assert_eq!(conflicts(&net, &input, TcSource::ArtNet), None);
        input.interface = "10.1.2.3".into();
        assert_eq!(conflicts(&net, &input, TcSource::ArtNet), None);
        let mut to_input = net.clone();
        to_input.artnet = Some(ArtNetTcOut { interface: "127.0.0.1".into(), target: "10.1.2.3".into() });
        assert!(conflicts(&to_input, &input, TcSource::ArtNet).is_some());
        // MTC out, Art-Net in: nothing in common.
        assert_eq!(conflicts(&out, &input, TcSource::ArtNet), None);
    }

    fn studio_with_port(port: &str) -> Shared {
        let mut s = shared();
        s.midi.enabled = true;
        let d = s.midi.device_mut(port);
        d.output = true;
        s
    }

    #[test]
    fn the_api_is_off_by_default_validates_and_refuses_self_drive() {
        let mut s = studio_with_port("Sortie MTC");
        assert!(!s.timecode.config().out.enabled);
        assert_eq!(summary(&s)["enabled"], false);
        assert_eq!(update(&mut s, &json!({ "mtc_port": "Inconnu" })).unwrap_err().status, 400);
        assert_eq!(update(&mut s, &json!({ "nope": 1 })).unwrap_err().status, 400);
        assert_eq!(update(&mut s, &json!({ "artnet": { "interface": "8.8.8.8", "target": "10.0.0.2" } })).unwrap_err().status, 400);
        assert_eq!(update(&mut s, &json!({ "enabled": true, "mtc_port": "Sortie MTC", "fps": "30", "offset_s": 3600 })), Ok(true));
        assert_eq!(update(&mut s, &json!({ "enabled": true })), Ok(false));
        assert_eq!(s.timecode.config().out.fps, Rate::F30);
        // The input on the same port: refused both ways (409).
        let err = crate::timecode::update(&mut s, &json!({ "source": "mtc", "midi_port": "Sortie MTC" })).unwrap_err();
        assert_eq!((err.status, err.message.as_str()), (409, SELF_DRIVE_MIDI));
        assert_eq!(s.timecode.config().source, TcSource::Off);
        assert_eq!(crate::timecode::update(&mut s, &json!({ "source": "mtc", "midi_port": "Autre" })), Ok(true));
        assert!(crate::timecode::update(&mut s, &json!({ "out": {} })).is_err());
        update(&mut s, &json!({ "enabled": false })).unwrap();
        crate::timecode::update(&mut s, &json!({ "midi_port": null })).unwrap();
        let err = update(&mut s, &json!({ "enabled": true })).unwrap_err();
        assert_eq!(err.status, 409);
        // Disabled, the trap is not one (nothing is sent).
        assert_eq!(update(&mut s, &json!({ "fps": "24" })), Ok(true));
        // Art-Net on loopback, typed target.
        crate::timecode::update(&mut s, &json!({ "source": "off" })).unwrap();
        assert_eq!(update(&mut s, &json!({ "enabled": true, "mtc_port": null, "artnet": { "interface": "127.0.0.1", "target": "127.0.0.1:7001" } })), Ok(true));
        assert!(update(&mut s, &json!({ "artnet": { "interface": "127.0.0.1", "target": "" } })).is_err(), "no broadcast on loopback");
    }

    #[test]
    fn the_engine_publishes_and_holds_back_without_a_show_or_on_self_drive() {
        let mut s = studio_with_port("Sortie MTC");
        let rx = s.timecode.out.take_receiver().unwrap();
        let c = Clock { t: 1.0, beat: 2.0, bpm: 120.0, beats_per_bar: 4 };
        frame(&mut s, &c);
        let first = rx.try_recv().unwrap();
        assert!(!first.cfg.enabled);
        // Disabled and idle: not repeated.
        frame(&mut s, &c);
        assert!(rx.try_recv().is_err());
        update(&mut s, &json!({ "enabled": true, "mtc_port": "Sortie MTC" })).unwrap();
        frame(&mut s, &c);
        assert_eq!(rx.try_recv().unwrap().blocked.as_deref(), Some("aucun show chargé"));
        assert_eq!(summary(&s)["blocked"], "aucun show chargé");
        let ev = crate::timeline::Event { id: 1, start: 0.0, len: 10.0, source: crate::timeline::EventSource::Cue { id: "x".into() }, ..Default::default() };
        s.timeline.load(crate::timeline::Show { name: "o".into(), tracks: vec![crate::timeline::Track { events: vec![ev], ..Default::default() }], ..Default::default() });
        s.timeline.play(&c);
        frame(&mut s, &c);
        let p = rx.try_recv().unwrap();
        assert_eq!((p.blocked, p.transport), (None, Transport::Playing));
        // A show's own source on the output port: held back at run time.
        s.timeline.show.as_mut().unwrap().timecode = Some(crate::timecode::ShowTimecode { source: TcSource::Mtc, ..Default::default() });
        crate::timecode::sync(&mut s);
        frame(&mut s, &c);
        assert_eq!(rx.try_recv().unwrap().blocked.as_deref(), Some(SELF_DRIVE_MIDI));
        // A full queue drops without blocking the engine.
        for _ in 0..QUEUE_LEN + 5 {
            frame(&mut s, &c);
        }
        assert!(s.timecode.out.dropped >= 5);
    }

    /// The real thread on the MIDI simulator: MTC reaches the port while
    /// the snapshots say « playing », and stops with them.
    #[test]
    fn the_thread_sends_mtc_to_the_simulated_port_and_stops_with_the_timeline() {
        let sim = SimMidi::default();
        sim.plug("Sortie MTC", FakeApc::generic());
        let opener: MidiOpener = {
            let sim = sim.clone();
            Arc::new(move |name: &str| {
                let mut b = sim.clone();
                crate::midi::backend::Backend::open_output(&mut b, name)
            })
        };
        let (tx, rx) = sync_channel(QUEUE_LEN);
        let status = Arc::new(Mutex::new(OutStatus::default()));
        let running = Arc::new(AtomicBool::new(true));
        let handle = spawn(rx, Arc::clone(&status), Some(opener), Arc::clone(&running)).unwrap();
        let cfg = TimecodeOut { mtc_port: Some("Sortie MTC".into()), ..mtc_cfg(Rate::F25) };
        let count = || sim.with_device("Sortie MTC", |d| d.received.iter().filter(|m| m[0] == 0xF1).count()).unwrap();
        let t0 = Instant::now();
        for k in 0..20u64 {
            tx.send(snap(&cfg, Transport::Playing, 0.0, 1, t0)).unwrap();
            std::thread::sleep(ms(10 + k % 2));
        }
        // 100 quarter frames a second at 25 im/s.
        let (n, want) = (count() as f64, t0.elapsed().as_secs_f64() * 100.0);
        assert!(n >= want - 8.0 && n <= want + 8.0, "quarter frames received: {n}, expected {want}");
        assert!(status.lock().unwrap().sending);
        tx.send(snap(&cfg, Transport::Stopped, 0.0, 2, Instant::now())).unwrap();
        std::thread::sleep(ms(80));
        let stopped = count();
        std::thread::sleep(ms(150));
        assert_eq!(count(), stopped, "nothing after Stop");
        assert!(!status.lock().unwrap().sending);
        // An unplugged port: an error, the thread carries on.
        sim.unplug("Sortie MTC");
        tx.send(snap(&cfg, Transport::Playing, 0.0, 3, Instant::now())).unwrap();
        std::thread::sleep(ms(80));
        assert!(status.lock().unwrap().error.is_some());
        running.store(false, Ordering::SeqCst);
        handle.join().unwrap();
    }
}
