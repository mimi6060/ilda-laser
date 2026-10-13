//! Art-Net output « ShowNET carte SD » (T-301): the studio as a console
//! for the files the ShowNET plays from its own microSD card.
//!
//! Without Laserworld's SDK, the one publicly documented way to drive the
//! box is its DMX / Art-Net mode: it plays `NNN.ild` from its card, the
//! number taken from the pattern channel, at the intensity of channel 1.
//! The card holds our own cues, exported by T-302 (`sdbank.rs`); this
//! output sends, for the cue on show, its bank number and an intensity.
//! It ignores the points: the box draws its file by itself.
//!
//! Wire format: ArtDmx from the public Art-Net 4 specification (our own
//! encoder, no dependency), unicast to the address the operator typed.
//! No ArtPoll, no discovery, no broadcast. Channel layouts: the DJ (19
//! channels) and Professional (34 channels) profiles, as described in
//! Laserworld's public DMX charts and the Open Fixture Library (MIT),
//! written here in our own words and code.
//!
//! Safety, since the box keeps playing whatever it was last told and its
//! behaviour on signal loss is not documented:
//! - disarmed, e-stop, blackout, hold-to-run released, every interlock
//!   (they all reach us as a disarmed stage or a dark frame), a cue with
//!   no bank number, a test pattern, an engine that stopped feeding us:
//!   **intensity 0 and pattern 0**, never "stop sending";
//! - a sender thread repeats the current values at `REFRESH` while the
//!   output is open, whatever the engine does; a change goes out at once;
//! - the kill switch sends the dark packet from the e-stop's thread
//!   without waiting for any lock; closing sends `SHUTDOWN_PACKETS` more;
//! - intensity = master × power cap, rounded down: the cap (global ×
//!   the lowest colour cap, since the card's colours can't be changed
//!   live) always bounds it.
//!
//! The box's own hardware stop (projector interlock, key, e-stop) stays
//! the real safety: the UI says so.

use crate::output::{CueSignal, Output};
use crate::patterns::Point;
use crate::sdbank;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The Art-Net UDP port.
pub const PORT: u16 = 6454;
/// `Output::kind` of this output.
pub const KIND: &str = "artnet";
/// Output manager ids: `artnet:<ip>:<port>`.
pub const ID_PREFIX: &str = "artnet:";
/// Channels of one universe (every packet carries the whole universe).
pub const UNIVERSE_LEN: usize = 512;
/// Highest 15-bit Port-Address.
pub const MAX_UNIVERSE: u16 = 0x7FFF;
/// The sender thread's period (~30 Hz; DMX itself refreshes at ≤ 44 Hz).
pub const REFRESH: Duration = Duration::from_millis(33);
/// No frame from the engine for this long: the values go dark.
pub const STALE: Duration = Duration::from_millis(250);
/// Sending has failed for this long: the output reports a fault (the
/// engine then disarms with « Sortie perdue »).
pub const FAULT_AFTER: Duration = Duration::from_secs(1);
/// Dark packets sent when the output closes.
pub const SHUTDOWN_PACKETS: usize = 5;

const HEADER_ID: &[u8; 8] = b"Art-Net\0";
const OP_DMX: u16 = 0x5000;
const PROT_VER: u16 = 14;
/// Length of an ArtDmx packet carrying a whole universe.
pub const PACKET_LEN: usize = 18 + UNIVERSE_LEN;

/// One ArtDmx packet. The sequence is 0 (« disabled » in the spec): two
/// threads may send, and a receiver must apply them in arrival order,
/// never drop our last (dark) packet as an old one.
pub fn encode_dmx(universe: u16, data: &[u8; UNIVERSE_LEN]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[..8].copy_from_slice(HEADER_ID);
    p[8..10].copy_from_slice(&OP_DMX.to_le_bytes());
    p[10..12].copy_from_slice(&PROT_VER.to_be_bytes());
    // p[12] sequence 0, p[13] physical port 0.
    let universe = universe.min(MAX_UNIVERSE);
    p[14] = (universe & 0xFF) as u8; // SubUni
    p[15] = (universe >> 8) as u8; // Net
    p[16..18].copy_from_slice(&(UNIVERSE_LEN as u16).to_be_bytes());
    p[18..].copy_from_slice(data);
    p
}

/// The box's DMX profile, chosen in Laserworld's Admin Tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// « DJ mode », 19 channels: the box's default.
    #[default]
    Dj,
    /// « Professional mode », 34 channels.
    Pro,
}

impl Profile {
    pub fn channels(self) -> usize {
        match self {
            Profile::Dj => 19,
            Profile::Pro => 34,
        }
    }

    pub fn label_fr(self) -> &'static str {
        match self {
            Profile::Dj => "DJ (19 canaux)",
            Profile::Pro => "Professionnel (34 canaux)",
        }
    }
}

/// `<data-dir>/shownet_artnet.json`: machine settings (this rig's box),
/// never in a project. No address: the output is off (not listed).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowNetDmxConfig {
    /// `ip:port` of the box, as the operator typed it (port 6454 unless
    /// given). None: off.
    pub target: Option<String>,
    pub universe: u16,
    /// DMX start address (1-based), the one set on the box's DIP switches.
    pub dmx_address: u16,
    pub profile: Profile,
    /// The box's stored safety zone: side/size (DJ ch. 16, Pro ch. 19) and
    /// intensity (DJ ch. 17, Pro ch. 20), sent as they are. 0 and 0: no
    /// reduction.
    pub zone_side: u8,
    pub zone_intensity: u8,
    /// Map the master size (shrinking only) to the box's size channels.
    pub follow_size: bool,
}

impl Default for ShowNetDmxConfig {
    fn default() -> Self {
        Self { target: None, universe: 0, dmx_address: 1, profile: Profile::Dj, zone_side: 0, zone_intensity: 0, follow_size: false }
    }
}

impl ShowNetDmxConfig {
    pub fn validate(&self) -> Result<()> {
        if self.universe > MAX_UNIVERSE {
            bail!("univers Art-Net 0 à {MAX_UNIVERSE}");
        }
        let last = (UNIVERSE_LEN - self.profile.channels() + 1) as u16;
        if !(1..=last).contains(&self.dmx_address) {
            bail!("adresse DMX 1 à {last} pour le profil {}", self.profile.label_fr());
        }
        if let Some(t) = &self.target {
            parse_target(t).map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }

    /// The output manager id of the configured box, if any.
    pub fn id(&self) -> Option<String> {
        let target = parse_target(self.target.as_deref()?).ok()?;
        Some(format!("{ID_PREFIX}{target}"))
    }
}

/// Parses the box's address: an IPv4 address, optionally `:port` (6454 by
/// default). Never a host name (nothing is looked up), never a broadcast,
/// multicast or unspecified address: Art-Net goes to this one box only.
pub fn parse_target(s: &str) -> Result<SocketAddr, String> {
    let s = s.trim();
    let addr = match s.parse::<SocketAddrV4>() {
        Ok(a) => a,
        Err(_) => SocketAddrV4::new(s.parse::<Ipv4Addr>().map_err(|_| format!("adresse IPv4 invalide : « {s} » (ex. 192.168.1.60)"))?, PORT),
    };
    let ip = *addr.ip();
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || ip.octets()[3] == 255 {
        return Err(format!("« {ip} » n'est pas l'adresse d'un boîtier (pas de diffusion : tapez l'adresse du ShowNET)"));
    }
    if addr.port() == 0 {
        return Err("port 0 invalide".into());
    }
    Ok(SocketAddr::V4(addr))
}

/// The id's target (`artnet:<ip>:<port>`).
pub fn id_target(id: &str) -> Option<SocketAddr> {
    parse_target(id.strip_prefix(ID_PREFIX)?).ok()
}

/// The configuration file, shared by the output backend (listing and
/// opening) and the HTTP handlers.
pub struct ShowNetDmxStore {
    /// None: in memory only (tests).
    path: Option<PathBuf>,
    cfg: Mutex<ShowNetDmxConfig>,
}

impl ShowNetDmxStore {
    /// A missing file: off. A damaged one is set aside (datafile.rs).
    pub fn load(path: PathBuf) -> Self {
        let cfg: ShowNetDmxConfig = crate::load_json(&path);
        let cfg = if cfg.validate().is_ok() { cfg } else { ShowNetDmxConfig::default() };
        Self { cfg: Mutex::new(cfg), path: Some(path) }
    }

    pub fn in_memory() -> Self {
        Self { path: None, cfg: Mutex::new(ShowNetDmxConfig::default()) }
    }

    pub fn get(&self) -> ShowNetDmxConfig {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Checks, normalises the target to `ip:port` and saves.
    pub fn set(&self, mut cfg: ShowNetDmxConfig) -> Result<ShowNetDmxConfig> {
        cfg.target = cfg.target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        cfg.validate()?;
        cfg.target = cfg.target.as_deref().map(|t| parse_target(t).map(|a| a.to_string())).transpose().map_err(anyhow::Error::msg)?;
        if let Some(path) = &self.path {
            let bytes = serde_json::to_vec_pretty(&cfg).context("réglages Art-Net illisibles")?;
            crate::datafile::replace(path, &bytes).with_context(|| format!("impossible d'écrire {}", path.display()))?;
        }
        *self.cfg.lock().unwrap_or_else(|e| e.into_inner()) = cfg.clone();
        Ok(cfg)
    }

    /// The listed entry (none while off).
    pub fn entry(&self) -> Option<crate::devices::DeviceInfo> {
        let id = self.get().id()?;
        let target = id_target(&id)?;
        Some(crate::devices::DeviceInfo::new(KIND.into(), id, output_name(target)))
    }
}

/// « ShowNET carte SD (Art-Net 192.168.1.60) », the port only if not 6454.
pub fn output_name(target: SocketAddr) -> String {
    let addr = if target.port() == PORT { target.ip().to_string() } else { target.to_string() };
    format!("ShowNET carte SD (Art-Net {addr})")
}

/// What the box is told to play: pattern 0 always comes with intensity 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmxLook {
    pub intensity: u8,
    pub pattern: u8,
    /// 0 = full size, 255 = closed (« wide to closed » in the charts).
    pub size: u8,
}

impl DmxLook {
    pub const DARK: Self = Self { intensity: 0, pattern: 0, size: 0 };
}

/// The highest DMX intensity a signal may give: master × cap, rounded
/// down, so the cap always bounds it.
pub fn intensity_for(sig: &CueSignal) -> u8 {
    let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    (unit(sig.master) * unit(sig.cap) * 255.0).floor() as u8
}

/// What to send. Lit only when armed, the last frame (after the whole
/// safety stage and the gate) had light, it is fresh, and the cue on show
/// has a valid bank number; everything else is `DARK`.
pub fn look_for(armed: bool, frame_lit: bool, fresh: bool, sig: &CueSignal, follow_size: bool) -> DmxLook {
    let Some(pattern) = sig.pattern.filter(|n| (sdbank::FIRST..=sdbank::LAST).contains(n)) else {
        return DmxLook::DARK;
    };
    if !(armed && frame_lit && fresh) {
        return DmxLook::DARK;
    }
    let intensity = intensity_for(sig);
    if intensity == 0 {
        return DmxLook::DARK;
    }
    let size = if follow_size && sig.size.is_finite() { ((1.0 - sig.size.clamp(0.0, 1.0)) * 255.0).round() as u8 } else { 0 };
    DmxLook { intensity, pattern, size }
}

/// The profile's channels (from the start address, 0 = first) for `look`.
/// Everything not driven sits at its neutral value: original colours,
/// centred, no rotation, no strobe, DMX mode, default speeds, no
/// automatic effect, and never the Professional profile's Setup values.
pub fn channels(cfg: &ShowNetDmxConfig, look: DmxLook) -> Vec<u8> {
    let mut c = vec![0u8; cfg.profile.channels()];
    c[0] = look.intensity;
    c[1] = look.pattern;
    match cfg.profile {
        Profile::Dj => {
            // 3 speed 0 = 50 fps; 4 size 0..127 X+Y (0 = full).
            c[3] = (look.size as u16 * 127 / 255) as u8;
            // 6 rotation 0..192 manual: 96 = 0°.
            c[5] = 96;
            // 7-10 position X, Y coarse/fine: centre.
            c[6] = 128;
            c[8] = 128;
            // 11-14 colours, strobe, mode 0 (DMX); 15 scan speed default.
            c[15] = cfg.zone_side;
            c[16] = cfg.zone_intensity;
        }
        Profile::Pro => {
            // 3 pattern speed 0..255 = 0..100 Hz: 128 ≈ 50 Hz (never 0, a frozen frame).
            c[2] = 128;
            // 4-7 position, 8-9 rotation: centre, 0°.
            c[3] = 128;
            c[5] = 128;
            c[7] = 128;
            // 10-11 size X, Y (0 = full).
            c[9] = look.size;
            c[10] = look.size;
            // 12 inversion, 13 colour (original), 14-16 RGB, 17 strobe,
            // 18 scan speed: 0.
            c[18] = cfg.zone_side;
            c[19] = cfg.zone_intensity;
            // 21-23 colour balance 0 (no reduction); 24-34 reserved: 0.
        }
    }
    c
}

/// The whole universe for `look`, the profile at its start address.
pub fn universe(cfg: &ShowNetDmxConfig, look: DmxLook) -> [u8; UNIVERSE_LEN] {
    let mut u = [0u8; UNIVERSE_LEN];
    let start = (cfg.dmx_address.max(1) as usize - 1).min(UNIVERSE_LEN - cfg.profile.channels());
    let c = channels(cfg, look);
    u[start..start + c.len()].copy_from_slice(&c);
    u
}

/// This frame's signal for the Art-Net output, read under the engine's
/// lock: `hand_cue`, the cue played by hand that is on show (after
/// transitions and the beat mode, `CueDeck::on_show`; never one still
/// waiting for its quantised start), else the newest timeline cue; none
/// during a test pattern (the box can't show it). `live` is the frame's
/// master copy (after the audio and DMX-loss gains).
pub fn cue_signal(s: &crate::Shared, hand_cue: Option<&str>, live: &crate::live::LiveModifiers, timeline_cue: Option<&str>, test_pattern: bool, caps: &crate::power::OutputLimits) -> CueSignal {
    let l = caps.sanitized();
    let cap = l.max_power * l.max_color.iter().copied().fold(1.0f32, f32::min);
    let cue = if test_pattern { None } else { hand_cue.or(timeline_cue) };
    CueSignal {
        cue: cue.map(|id| s.presets.iter().find(|p| p.id == id).map_or_else(|| id.to_string(), |p| p.name.clone())),
        pattern: cue.and_then(|id| s.sdbank.get().slots.iter().find(|slot| slot.cue_id == id)).map(|slot| slot.number),
        master: live.brightness,
        cap,
        size: live.size,
    }
}

/// What the engine told us, and what went out.
struct Feed {
    armed: bool,
    lit: bool,
    signal: CueSignal,
    fed_at: Option<Instant>,
    last: Option<DmxLook>,
    error: Option<String>,
    failing_since: Option<Instant>,
}

/// Shared by the engine thread, the sender thread and the kill switch.
struct Link {
    socket: UdpSocket,
    target: SocketAddr,
    cfg: ShowNetDmxConfig,
    /// The dark packet, ready to go without any lock (kill switch).
    dark: [u8; PACKET_LEN],
    /// Set by the kill switch: dark until the stage arms again.
    killed: AtomicBool,
    packets: AtomicU64,
    feed: Mutex<Feed>,
}

impl Link {
    fn feed(&self) -> std::sync::MutexGuard<'_, Feed> {
        self.feed.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn look(&self, f: &Feed, now: Instant) -> DmxLook {
        if self.killed.load(Ordering::SeqCst) {
            return DmxLook::DARK;
        }
        let fresh = f.fed_at.is_some_and(|t| now.saturating_duration_since(t) < STALE);
        look_for(f.armed, f.lit, fresh, &f.signal, self.cfg.follow_size)
    }

    fn send_raw(&self, packet: &[u8]) -> std::io::Result<()> {
        self.socket.send_to(packet, self.target)?;
        self.packets.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Sends the current values, under the feed lock (so two threads never
    /// overtake each other with stale values). A kill that lands while we
    /// send is followed by a dark packet.
    fn send_current(&self, f: &mut Feed, now: Instant) {
        let look = self.look(f, now);
        let result = if look == DmxLook::DARK { self.send_raw(&self.dark) } else { self.send_raw(&encode_dmx(self.cfg.universe, &universe(&self.cfg, look))) };
        if look != DmxLook::DARK && self.killed.load(Ordering::SeqCst) {
            let _ = self.send_raw(&self.dark);
            f.last = Some(DmxLook::DARK);
        } else {
            f.last = Some(look);
        }
        match result {
            Ok(()) => {
                f.error = None;
                f.failing_since = None;
            }
            // A full buffer: the next packet follows within `REFRESH`.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                let msg = format!("envoi Art-Net vers {} impossible : {e}", self.target);
                if f.error.as_deref() != Some(&msg) {
                    log::warn!("{msg}");
                }
                f.error = Some(msg);
                f.failing_since.get_or_insert(now);
            }
        }
    }

    /// Sends now if the values changed (arm, disarm, cue, intensity).
    fn send_if_changed(&self, f: &mut Feed, now: Instant) {
        if f.last != Some(self.look(f, now)) {
            self.send_current(f, now);
        }
    }

    /// The e-stop: dark from now on, sent at once. The lock is only tried,
    /// never waited for (the panic hook may run on a thread that holds it).
    fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        if let Ok(mut f) = self.feed.try_lock() {
            f.last = Some(DmxLook::DARK);
        }
        if let Err(e) = self.send_raw(&self.dark) {
            log::warn!("emergency blank of the Art-Net output failed: {e}");
        }
    }
}

/// Testing only: where `ShowNetDmxOutput` may send. A test build never
/// sends outside the loopback, whatever it is asked.
fn check_target(target: SocketAddr) -> Result<()> {
    if cfg!(test) && !target.ip().is_loopback() {
        bail!("tests: Art-Net vers {target} refusé (127.0.0.1 seulement)");
    }
    Ok(())
}

pub struct ShowNetDmxOutput {
    name: String,
    link: Arc<Link>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ShowNetDmxOutput {
    /// Opens a UDP socket towards the box and starts the sender thread.
    /// Starts disarmed: the first packet is the dark one.
    pub fn open(target: SocketAddr, cfg: &ShowNetDmxConfig) -> Result<Self> {
        check_target(target)?;
        cfg.validate()?;
        let bind: SocketAddr = if target.ip().is_loopback() { (Ipv4Addr::LOCALHOST, 0).into() } else { (Ipv4Addr::UNSPECIFIED, 0).into() };
        let socket = UdpSocket::bind(bind).context("failed to open the Art-Net socket")?;
        // Never wait on the network from the engine thread.
        socket.set_nonblocking(true).context("failed to configure the Art-Net socket")?;
        let cfg = ShowNetDmxConfig { target: Some(target.to_string()), ..cfg.clone() };
        let link = Arc::new(Link {
            socket,
            target,
            dark: encode_dmx(cfg.universe, &universe(&cfg, DmxLook::DARK)),
            cfg,
            killed: AtomicBool::new(false),
            packets: AtomicU64::new(0),
            feed: Mutex::new(Feed { armed: false, lit: false, signal: CueSignal::default(), fed_at: None, last: None, error: None, failing_since: None }),
        });
        {
            let mut f = link.feed();
            link.send_current(&mut f, Instant::now());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("artnet-shownet".into())
            .spawn({
                let (link, stop) = (Arc::clone(&link), Arc::clone(&stop));
                move || {
                    while !stop.load(Ordering::SeqCst) {
                        std::thread::park_timeout(REFRESH);
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let mut f = link.feed();
                        link.send_current(&mut f, Instant::now());
                    }
                }
            })
            .context("failed to start the Art-Net sender")?;
        Ok(Self { name: output_name(target), link, stop, thread: Some(thread) })
    }
}

impl Output for ShowNetDmxOutput {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> &'static str {
        KIND
    }
    fn set_armed(&mut self, armed: bool) -> Result<()> {
        let mut f = self.link.feed();
        f.armed = armed;
        if armed {
            self.link.killed.store(false, Ordering::SeqCst);
        } else {
            // Disarmed: dark at once, even if nothing seems to change.
            f.last = None;
        }
        self.link.send_if_changed(&mut f, Instant::now());
        Ok(())
    }
    fn send(&mut self, points: &[Point]) {
        let now = Instant::now();
        let mut f = self.link.feed();
        f.lit = points.iter().any(|p| p.is_lit());
        f.fed_at = Some(now);
        self.link.send_if_changed(&mut f, now);
    }
    fn cue_signal(&mut self, signal: &CueSignal) {
        let mut f = self.link.feed();
        if f.signal != *signal {
            f.signal = signal.clone();
        }
    }
    fn blank_now(&mut self) -> Result<()> {
        let mut f = self.link.feed();
        f.lit = false;
        f.last = Some(DmxLook::DARK);
        self.link.send_raw(&self.link.dark).with_context(|| format!("envoi Art-Net vers {} impossible", self.link.target))
    }
    fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        let link = Arc::clone(&self.link);
        Some(Box::new(move || link.kill()))
    }
    fn fault(&self) -> Option<String> {
        let f = self.link.feed.try_lock().ok()?;
        let since = f.failing_since?;
        (since.elapsed() >= FAULT_AFTER).then(|| f.error.clone().unwrap_or_else(|| format!("{} injoignable", self.name)))
    }
    fn status(&self) -> Option<serde_json::Value> {
        let f = self.link.feed.try_lock().ok()?;
        let look = f.last.unwrap_or(DmxLook::DARK);
        let cue = f.signal.cue.as_deref();
        let warning = match (cue, f.signal.pattern) {
            (Some(name), None) => Some(format!("Le cue « {name} » n'a pas de numéro dans la banque SD : noir (figure 0)")),
            (None, _) if f.armed => Some("Aucun cue de la banque SD à l'écran : noir (figure 0)".to_string()),
            _ => None,
        };
        let c = &self.link.cfg;
        Some(json!({
            "kind": KIND,
            "target": self.link.target.to_string(),
            "universe": c.universe,
            "dmx_address": c.dmx_address,
            "profile": c.profile,
            "intensity": look.intensity,
            "pattern": look.pattern,
            "cue": cue,
            "bank_number": f.signal.pattern,
            "warning": warning,
            "packets": self.link.packets.load(Ordering::Relaxed),
            "error": f.error,
        }))
    }
}

impl Drop for ShowNetDmxOutput {
    /// Stops the sender, then a few dark packets: the box is left on
    /// intensity 0 and pattern 0.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.link.killed.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
        for _ in 0..SHUTDOWN_PACKETS {
            if let Err(e) = self.link.send_raw(&self.link.dark) {
                log::warn!("dark Art-Net packet at close failed: {e}");
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::interlock::{ArmSource, EStop};
    use crate::output::OutputStage;

    /// A local receiver on 127.0.0.1 and a port the OS picks: never 6454,
    /// never a real box.
    pub fn receiver() -> (UdpSocket, SocketAddr) {
        let rx = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let addr = rx.local_addr().unwrap();
        assert_ne!(addr.port(), PORT);
        (rx, addr)
    }

    /// Our own decoder for the received packets (`netin::artnet::decode`):
    /// universe and channel data.
    pub fn next_dmx(rx: &UdpSocket) -> (u16, Vec<u8>) {
        let mut buf = [0u8; 1024];
        let n = rx.recv(&mut buf).expect("an Art-Net packet");
        match crate::netin::artnet::decode(&buf[..n]).unwrap() {
            crate::netin::artnet::Packet::Dmx { universe, data } => (universe, data),
            other => panic!("not ArtDmx: {other:?}"),
        }
    }

    /// Drops what is queued, then the next packet's (ch. 1, ch. 2) at the
    /// profile's start address.
    fn fresh(rx: &UdpSocket, cfg: &ShowNetDmxConfig) -> (u8, u8) {
        rx.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 1024];
        while rx.recv(&mut buf).is_ok() {}
        rx.set_nonblocking(false).unwrap();
        let (_, d) = next_dmx(rx);
        let a = cfg.dmx_address as usize - 1;
        (d[a], d[a + 1])
    }

    /// Every packet received within `ms` (0: what is queued): (ch. 1, ch. 2).
    fn collect(rx: &UdpSocket, cfg: &ShowNetDmxConfig, ms: u64) -> Vec<(u8, u8)> {
        let until = Instant::now() + Duration::from_millis(ms);
        let a = cfg.dmx_address as usize - 1;
        let mut out = Vec::new();
        rx.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
        let mut buf = [0u8; 1024];
        loop {
            let got = rx.recv(&mut buf);
            if got.is_err() && Instant::now() >= until {
                break;
            }
            if let Ok(n) = got {
                if let Ok(crate::netin::artnet::Packet::Dmx { data, .. }) = crate::netin::artnet::decode(&buf[..n]) {
                    out.push((data[a], data[a + 1]));
                }
            }
        }
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        out
    }

    fn sig(pattern: Option<u8>) -> CueSignal {
        CueSignal { cue: Some("Tunnel".into()), pattern, master: 1.0, cap: 0.5, size: 1.0 }
    }

    fn lit() -> Vec<Point> {
        vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0)]
    }

    fn cfg(addr: u16) -> ShowNetDmxConfig {
        ShowNetDmxConfig { dmx_address: addr, universe: 3, ..Default::default() }
    }

    #[test]
    fn an_artdmx_packet_is_encoded_byte_for_byte() {
        let mut data = [0u8; UNIVERSE_LEN];
        data[0] = 200;
        data[1] = 7;
        data[511] = 9;
        let p = encode_dmx(0x1234, &data);
        assert_eq!(p.len(), 530);
        assert_eq!(&p[..8], b"Art-Net\0");
        assert_eq!(&p[8..10], &[0x00, 0x50], "OpDmx, little-endian");
        assert_eq!(&p[10..12], &[0, 14], "protocol 14, big-endian");
        assert_eq!((p[12], p[13]), (0, 0), "sequence off, physical 0");
        assert_eq!((p[14], p[15]), (0x34, 0x12), "SubUni then Net");
        assert_eq!(&p[16..18], &[0x02, 0x00], "512 channels, big-endian");
        assert_eq!((p[18], p[19], p[529]), (200, 7, 9));
        // Our own receiver reads it back.
        assert_eq!(crate::netin::artnet::decode(&p), Ok(crate::netin::artnet::Packet::Dmx { universe: 0x1234, data: data.to_vec() }));
        assert_eq!((encode_dmx(0xFFFF, &data)[14], encode_dmx(0xFFFF, &data)[15]), (0xFF, 0x7F), "15-bit universe");
    }

    #[test]
    fn the_dj_profile_maps_the_look_and_keeps_the_rest_neutral() {
        let c = ShowNetDmxConfig { zone_side: 70, zone_intensity: 129, ..Default::default() };
        let ch = channels(&c, DmxLook { intensity: 128, pattern: 7, size: 0 });
        assert_eq!(ch.len(), 19);
        assert_eq!(ch, [128, 7, 0, 0, 0, 96, 128, 0, 128, 0, 0, 0, 0, 0, 0, 70, 129, 0, 0]);
        let dark = channels(&c, DmxLook::DARK);
        assert_eq!((dark[0], dark[1]), (0, 0));
        assert_eq!(dark[13], 0, "DMX mode, never auto / demo / sound");
        assert_eq!(channels(&c, DmxLook { intensity: 1, pattern: 1, size: 255 })[3], 127, "closed");
    }

    #[test]
    fn the_pro_profile_never_sends_setup_values() {
        let c = ShowNetDmxConfig { profile: Profile::Pro, ..Default::default() };
        let ch = channels(&c, DmxLook { intensity: 50, pattern: 12, size: 10 });
        assert_eq!(ch.len(), 34);
        assert_eq!(&ch[..12], &[50, 12, 128, 128, 0, 128, 0, 128, 0, 10, 10, 0]);
        assert!(ch[20..].iter().all(|&v| v == 0), "balance and reserved (Setup) channels stay 0");
    }

    #[test]
    fn the_profile_sits_at_its_start_address() {
        let c = cfg(100);
        let u = universe(&c, DmxLook { intensity: 9, pattern: 4, size: 0 });
        assert_eq!((u[99], u[100]), (9, 4));
        assert!(u[..99].iter().all(|&v| v == 0));
        assert!(u[99 + 19..].iter().all(|&v| v == 0));
    }

    #[test]
    fn dark_unless_armed_lit_fresh_and_banked() {
        let s = sig(Some(7));
        assert_eq!(look_for(true, true, true, &s, false), DmxLook { intensity: 127, pattern: 7, size: 0 });
        for (armed, lit, fresh) in [(false, true, true), (true, false, true), (true, true, false), (false, false, false)] {
            assert_eq!(look_for(armed, lit, fresh, &s, false), DmxLook::DARK, "{armed} {lit} {fresh}");
        }
        assert_eq!(look_for(true, true, true, &sig(None), false), DmxLook::DARK, "no bank number");
        for bad in [0, 230, 255] {
            assert_eq!(look_for(true, true, true, &sig(Some(bad)), false), DmxLook::DARK, "reserved {bad}");
        }
        let zero = CueSignal { master: 0.0, ..s.clone() };
        assert_eq!(look_for(true, true, true, &zero, false), DmxLook::DARK, "intensity 0 means pattern 0 too");
        let half = CueSignal { size: 0.5, ..s };
        assert_eq!(look_for(true, true, true, &half, true).size, 128);
        assert_eq!(look_for(true, true, true, &half, false).size, 0, "size not followed by default");
    }

    #[test]
    fn the_cap_always_bounds_the_intensity() {
        for (master, cap) in [(1.0, 1.0), (1.0, 0.5), (0.7, 0.3), (2.0, 0.2), (1.0, 0.0), (f32::NAN, 1.0), (1.0, f32::INFINITY), (-1.0, 1.0)] {
            let i = intensity_for(&CueSignal { master, cap, ..Default::default() });
            let bound = if cap.is_finite() { cap.clamp(0.0, 1.0) } else { 0.0 };
            assert!(i as f32 <= bound * 255.0, "{master} × {cap} gave {i}");
        }
        assert_eq!(intensity_for(&CueSignal { master: 1.0, cap: 1.0, ..Default::default() }), 255);
    }

    #[test]
    fn targets_are_one_box_never_a_broadcast_or_a_name() {
        assert_eq!(parse_target("192.168.1.60").unwrap().to_string(), "192.168.1.60:6454");
        assert_eq!(parse_target(" 127.0.0.1:7000 ").unwrap().to_string(), "127.0.0.1:7000");
        for bad in ["", "shownet.local", "255.255.255.255", "192.168.1.255", "0.0.0.0", "239.1.2.3", "2.0.0.1:0", "::1", "10.0.0.1:99999"] {
            assert!(parse_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_config_is_checked_and_off_by_default() {
        let c = ShowNetDmxConfig::default();
        assert_eq!((c.target.as_deref(), c.id()), (None, None));
        let store = ShowNetDmxStore::in_memory();
        assert!(store.entry().is_none(), "off: not listed");
        assert!(store.set(ShowNetDmxConfig { dmx_address: 0, ..Default::default() }).is_err());
        assert!(store.set(ShowNetDmxConfig { dmx_address: 495, ..Default::default() }).is_err(), "19 channels from 495 overflow");
        assert!(store.set(ShowNetDmxConfig { dmx_address: 494, ..Default::default() }).is_ok());
        assert!(store.set(ShowNetDmxConfig { dmx_address: 494, profile: Profile::Pro, ..Default::default() }).is_err());
        assert!(store.set(ShowNetDmxConfig { universe: 0x8000, ..Default::default() }).is_err());
        assert!(store.set(ShowNetDmxConfig { target: Some("192.168.1.255".into()), ..Default::default() }).is_err());
        let saved = store.set(ShowNetDmxConfig { target: Some(" 127.0.0.1 ".into()), ..Default::default() }).unwrap();
        assert_eq!(saved.target.as_deref(), Some("127.0.0.1:6454"));
        let e = store.entry().unwrap();
        assert_eq!((e.kind.as_str(), e.id.as_str(), e.name.as_str()), ("artnet", "artnet:127.0.0.1:6454", "ShowNET carte SD (Art-Net 127.0.0.1)"));
        let old: ShowNetDmxConfig = serde_json::from_str(r#"{"target":"10.0.0.5"}"#).unwrap();
        assert_eq!((old.dmx_address, old.profile, old.follow_size), (1, Profile::Dj, false));
    }

    #[test]
    fn tests_never_send_outside_the_loopback() {
        let c = ShowNetDmxConfig::default();
        assert!(ShowNetDmxOutput::open("192.168.129.51:6454".parse().unwrap(), &c).is_err());
    }

    /// Every arm / disarm / e-stop / hold / close path, through the real
    /// output stage, decoded from a loopback receiver.
    #[test]
    fn every_safety_path_sends_intensity_0_and_pattern_0() {
        let (rx, addr) = receiver();
        let c = cfg(10);
        let out = ShowNetDmxOutput::open(addr, &c).unwrap();
        let (u, d) = next_dmx(&rx);
        assert_eq!((u, d[9], d[10]), (3, 0, 0), "opens dark, on its universe");
        let mut stage = OutputStage::new(Some(Box::new(out)));
        let estop = EStop::default();
        estop.set_kill_switch(stage.kill_switch().unwrap());

        // Disarmed: dark, repeated at ~30 Hz while nothing else happens.
        stage.cue_signal(&sig(Some(7)));
        stage.emit(&lit(), false, true, &estop);
        let idle = collect(&rx, &c, 300);
        assert!(idle.len() >= 5, "keeps sending: {}", idle.len());
        assert!(idle.iter().all(|&v| v == (0, 0)));

        // Armed on bank number 7: pattern 7, intensity master × cap.
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (127, 7));

        // A cue without a bank number: dark.
        stage.cue_signal(&sig(None));
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (0, 0));
        stage.cue_signal(&sig(Some(7)));
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (127, 7));

        // Hold-to-run released: dark, still armed; pressed again: back.
        stage.emit(&lit(), true, false, &estop);
        assert_eq!(fresh(&rx, &c), (0, 0));
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (127, 7));

        // A frame the safety stage left dark (blackout, zones, caps at 0).
        stage.emit(&[Point::blanked(0.0, 0.0)], true, true, &estop);
        assert_eq!(fresh(&rx, &c), (0, 0));
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (127, 7));

        // The engine stops feeding (stall): dark within `STALE`.
        std::thread::sleep(STALE + REFRESH * 3);
        assert_eq!(fresh(&rx, &c), (0, 0), "stale values go dark");
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (127, 7));

        // E-stop: the kill switch sends dark at once, before any frame.
        collect(&rx, &c, 0);
        estop.trip(ArmSource::Keyboard);
        let after = collect(&rx, &c, 120);
        // At most one packet the sender had already sent before the kill.
        let first_dark = after.iter().position(|&v| v == (0, 0)).expect("a dark packet at once");
        assert!(first_dark <= 1 && after[first_dark..].iter().all(|&v| v == (0, 0)), "{after:?}");
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (0, 0), "latched: stays dark");
        assert!(collect(&rx, &c, 150).iter().all(|&v| v == (0, 0)));
        // Reset (a new latch here): the stage disarms, then arms again.
        let estop = EStop::default();
        stage.emit(&lit(), false, true, &estop);
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (127, 7), "re-armed after the reset");

        // Disarm: dark at once.
        stage.emit(&lit(), false, true, &estop);
        assert_eq!(fresh(&rx, &c), (0, 0));

        // Shutdown, armed: disarm, dark frames, then the close packets.
        stage.emit(&lit(), true, true, &estop);
        assert_eq!(fresh(&rx, &c), (127, 7));
        stage.shutdown();
        let last = collect(&rx, &c, 200);
        assert!(last.len() >= 3 + SHUTDOWN_PACKETS, "{}", last.len());
        assert!(last.iter().all(|&v| v == (0, 0)), "{last:?}");
        assert!(collect(&rx, &c, 100).is_empty(), "closed: nothing more");
    }

    /// The engine's signal: the cue on show and its bank number, the
    /// master copy, the cap as one factor; nothing during a test pattern.
    #[test]
    fn the_signal_names_the_cue_on_show_and_its_bank_number() {
        let mut s = crate::test_support::shared();
        let (a, b) = (s.presets[0].clone(), s.presets[1].clone());
        s.sdbank.set(sdbank::SdBank { slots: vec![sdbank::SdSlot { number: 7, cue_id: a.id.clone(), seconds: 1.0 }] }).unwrap();
        let live = crate::live::LiveModifiers { brightness: 0.8, size: 0.5, ..Default::default() };
        let caps = crate::power::OutputLimits { max_power: 0.5, max_color: [1.0, 0.4, 0.9] };
        assert_eq!(cue_signal(&s, s.active_cue.as_deref(), &live, None, false, &caps), CueSignal { cue: None, pattern: None, master: 0.8, cap: 0.2, size: 0.5 });
        s.active_cue = Some(a.id.clone());
        let sig = cue_signal(&s, s.active_cue.as_deref(), &live, Some(&b.id), false, &caps);
        assert_eq!((sig.cue.as_deref(), sig.pattern), (Some(a.name.as_str()), Some(7)));
        assert_eq!(intensity_for(&sig), 40, "0.8 × 0.2 × 255, rounded down");
        assert_eq!(cue_signal(&s, s.active_cue.as_deref(), &live, Some(&b.id), true, &caps).pattern, None, "a test pattern plays nothing from the card");
        s.active_cue = None;
        let sig = cue_signal(&s, s.active_cue.as_deref(), &live, Some(&b.id), false, &caps);
        assert_eq!((sig.cue.as_deref(), sig.pattern), (Some(b.name.as_str()), None), "the timeline's cue, not in the bank");
    }

    #[test]
    fn the_status_warns_about_a_cue_without_bank_number() {
        let (rx, addr) = receiver();
        let mut out = ShowNetDmxOutput::open(addr, &cfg(1)).unwrap();
        out.cue_signal(&sig(None));
        out.set_armed(true).unwrap();
        out.send(&lit());
        let st = out.status().unwrap();
        assert_eq!((st["pattern"].as_u64(), st["intensity"].as_u64()), (Some(0), Some(0)));
        assert!(st["warning"].as_str().unwrap().contains("« Tunnel » n'a pas de numéro"));
        out.cue_signal(&sig(Some(12)));
        out.send(&lit());
        let st = out.status().unwrap();
        assert_eq!((st["pattern"].as_u64(), st["warning"].as_str()), (Some(12), None));
        out.cue_signal(&CueSignal { cue: None, pattern: None, ..sig(None) });
        out.send(&lit());
        assert!(out.status().unwrap()["warning"].as_str().unwrap().contains("Aucun cue"));
        assert!(out.fault().is_none());
        drop(out);
        drop(rx);
    }
}
