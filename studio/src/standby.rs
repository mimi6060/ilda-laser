//! The synchronised backup machine (T-456): a second Mac follows the main
//! one (same project, same cues, same timeline position, same tempo) so
//! the operator can take over if the main computer fails.
//!
//! Roles, in `<data-dir>/standby.json` (machine settings, never in a
//! project, never replicated):
//! - **Autonome** (default): nothing changes; no listener, no thread.
//! - **Principale**: a dedicated sync listener (its own `tiny_http`
//!   server, own thread, bounded bodies and connections) serves a
//!   whitelist to a paired backup. It never calls `web::route`; every
//!   route of the main UI answers 403.
//! - **Secours** (standby): polls the main machine every 100 ms and plays
//!   the same state. While in standby it can **never** emit: the `standby`
//!   interlock refuses arming (« Machine de secours en veille ») and the
//!   output manager holds every output (none is connected).
//!
//! What keeps it safe:
//! - Replication never arms, never resets an emergency stop and never
//!   touches safety, calibration, zones, profiles or outputs: no field of
//!   `PlaybackState` carries them, and any key the receiving types do not
//!   know rejects the whole message, nested ones included (`parse_strict`:
//!   the top-level messages are `deny_unknown_fields`; `Settings` and
//!   `LiveModifiers` ignore unknown keys on disk for old saved files, so
//!   the sync checks them against their own re-serialisation). Each
//!   machine's safety stage still runs last with its own settings.
//! - **Lease** (`check_lease`): a paired main machine that gets no
//!   authenticated poll from its backup for `LEASE` disarms itself
//!   (« Machine de secours injoignable »). Its output stays connected and
//!   keeps getting dark frames, a warning stays up until the backup polls
//!   again, and only the operator arms again (a poll never re-arms).
//!   Arming while the backup is still unreachable needs the operator's
//!   confirmation of the risk (`arm_check`, 409 otherwise; MIDI never):
//!   the backup may have taken over, and the lease only runs again once a
//!   poll has come back. The main machine advertises its lease
//!   (`lease_ms`); the backup allows a takeover only after
//!   max(`takeover_grace_s`, lease + `LEASE_MARGIN`) without an answer
//!   (1.5 s: covers a late poll handled by a stalled main machine). So a broken link between the two Macs (cable,
//!   switch port) cannot leave two machines armed on one projector: the
//!   main one is disarmed before the backup may take over. The same holds
//!   for a main machine with no peer address: once the backup takes over
//!   it stops polling, and the old main machine's lease runs out. A main
//!   machine frozen while still emitting cannot disarm itself: that is a
//!   hardware stop's job.
//! - A main machine restarted after a crash has forgotten the token: it
//!   is unpaired and has **no lease** for the backup. Its 401s still count
//!   as an answer for the takeover delay (an unpaired backup keeps asking,
//!   `listen_unpaired`), and the takeover dialog warns that it does not
//!   disarm by itself (`main_fenced` false) until the backup pairs again.
//! - If the backup quits, sleeps or reboots, the main machine darkens 3 s
//!   later: a backup in standby keeps the Mac awake (`keep_awake`), and
//!   setting the main machine to *Autonome* ends the lease (then re-arm).
//! - Taking over is a manual action on the backup (`takeover`): it is
//!   refused while the main machine answers and holds an output, or
//!   answered less than the delay above ago; it leaves the laser
//!   disarmed. Nothing switches over by itself.
//! - A main machine that comes back sees the other one main with a newer
//!   generation (`term`) and becomes *Autonome*, disarmed, its outputs let
//!   go (`demote`).
//! - Pairing: a 6-digit code shown on the main machine, 5 wrong codes draw
//!   a new one and block 30 s; the token (32 random bytes) is memory only
//!   and compared in constant time. The peer is an address the operator
//!   types: local network only, never the ShowNET's, no discovery.
//!
//! Plain HTTP, no TLS: acceptable only because the link can neither arm
//! nor loosen anything; keep it on the control network.

use crate::cues::MirroredCue;
use crate::engine::Settings;
use crate::interlock::{self, ArmSource, DisarmReason};
use crate::live::LiveModifiers;
use crate::timeline::{Show, TimeBase, Transport};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response, Server};

pub const DEFAULT_PORT: u16 = 8093;
pub const DEFAULT_GRACE_S: u32 = 5;
pub const MAX_GRACE_S: u32 = 60;
/// The backup asks for the state this often.
pub const POLL: Duration = Duration::from_millis(100);
/// A main machine with a known peer asks for its role this often.
pub const PROBE: Duration = Duration::from_secs(1);
/// The project fingerprint is computed again this often.
const FINGERPRINT_EVERY: Duration = Duration::from_secs(2);
/// A paired main machine disarms itself after this long without a poll
/// from its backup (`check_lease`). Shorter than any takeover delay.
pub const LEASE: Duration = Duration::from_secs(3);
/// What the backup adds to the main machine's lease before a takeover:
/// the main machine's check runs every frame; this covers the rest,
/// including a late poll on a stalled main machine (the backup's next
/// poll leaves `POLL` after the last answer and may be handled up to
/// three request timeouts later, renewing a lease the backup never saw
/// renewed): at least `POLL + 3 × REQUEST_TIMEOUT`.
pub const LEASE_MARGIN: Duration = Duration::from_millis(1500);
/// One request's limit (connect, write, each read).
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(400);
/// The link counts as up while the last answer is this recent.
pub const LINK_TIMEOUT: Duration = Duration::from_secs(1);
/// A playhead further than this from the main machine's jumps there.
pub const JUMP_S: f64 = 0.1;
/// Request bodies the listener reads (only the pairing has one).
pub const MAX_BODY: usize = 1024;
const MAX_CONNECTIONS: usize = 8;
/// Largest answers the backup accepts.
const MAX_STATE: usize = 1 << 20;
const MAX_SHOW: usize = 8 << 20;
const MAX_PROJECT: usize = 64 << 20;
/// Clock samples kept; the one with the shortest round trip wins.
const SAMPLES: usize = 20;
pub const MAX_PAIR_ATTEMPTS: u8 = 5;
pub const PAIR_BLOCK: Duration = Duration::from_secs(30);
const CODE_LEN: usize = 6;
/// The address the ShowNET was seen at (CLAUDE.md): never a peer.
const SHOWNET_SEEN: Ipv4Addr = Ipv4Addr::new(192, 168, 129, 51);

pub const STANDBY_BANNER_FR: &str = "Machine de secours en veille : lecture seule";
pub const PEER_HOLDS_FR: &str = "La machine principale tient encore la sortie.";
pub const PEER_PRIMARY_FR: &str = "Une autre machine est principale";
/// The main machine's lease ran out (the disarm's reason).
pub const BACKUP_LOST_FR: &str = "Machine de secours injoignable";
/// The warning's notice key: it holds until the backup polls again.
const LEASE_NOTICE: &str = "standby_lease";
pub const BACKUP_LOST_WARN_FR: &str = "Machine de secours injoignable : laser désarmé — elle peut prendre le relais. Arrêt réel : matériel";
/// Arming a main machine whose backup is unreachable: refused (409)
/// without the operator's confirmation (`arm_check`).
pub const BACKUP_LOST_ARM_FR: &str = "Machine de secours injoignable : confirmation requise pour armer";
/// The confirmation's question: the risk, word for word.
pub const BACKUP_LOST_CONFIRM_FR: &str = "La machine de secours est injoignable : elle a peut-être pris le relais et peut émettre sur le même projecteur. Tant qu'elle reste injoignable, cette machine ne se désarmera plus d'elle-même. Vérifiez d'abord que la machine de secours n'émet pas (sortie coupée, arrêt matériel). Armer quand même ?";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Standalone,
    Primary,
    Standby,
}

impl Role {
    pub fn label_fr(self) -> &'static str {
        match self {
            Role::Standalone => "Autonome",
            Role::Primary => "Principale",
            Role::Standby => "Secours",
        }
    }
}

/// `<data-dir>/standby.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StandbySettings {
    pub role: Role,
    /// The other machine's sync address (`ip:port`), typed by the operator.
    pub peer: Option<String>,
    /// The address the main machine's listener binds (one of this Mac's).
    pub interface: Option<String>,
    pub port: u16,
    pub takeover_grace_s: u32,
    /// Generation: a takeover sets it one above the main machine's. The
    /// higher one is the main machine when both say they are.
    pub term: u64,
}

impl Default for StandbySettings {
    fn default() -> Self {
        StandbySettings { role: Role::Standalone, peer: None, interface: None, port: DEFAULT_PORT, takeover_grace_s: DEFAULT_GRACE_S, term: 0 }
    }
}

/// A peer address: `ip:port` on the local network (loopback, private,
/// link-local), never the ShowNET, never public, multicast or broadcast.
pub fn parse_peer(raw: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = raw.trim().parse().map_err(|_| format!("adresse invalide « {} » : IP:port attendu (ex. 192.168.1.20:{DEFAULT_PORT})", raw.trim()))?;
    if addr.port() == 0 {
        return Err("port invalide : 1 à 65535".into());
    }
    check_lan(addr.ip())?;
    Ok(addr)
}

/// The listener's address: one IP on the local network (or loopback).
pub fn parse_interface(raw: &str) -> Result<IpAddr, String> {
    let ip: IpAddr = raw.trim().parse().map_err(|_| format!("interface invalide « {} » : une adresse IP de ce Mac", raw.trim()))?;
    check_lan(ip)?;
    Ok(ip)
}

fn check_lan(ip: IpAddr) -> Result<(), String> {
    let ok = match ip {
        IpAddr::V4(v4) => {
            if v4 == SHOWNET_SEEN {
                return Err("Adresse de la ShowNET : jamais pour la machine de secours".into());
            }
            v4.is_loopback() || v4.is_private() || v4.is_link_local()
        }
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{ip} : réseau local seulement (boucle locale, 10.x, 172.16-31.x, 192.168.x)"))
    }
}

// ---------------------------------------------------------------------
// What is replicated. No arming, safety, calibration or output field:
// a message carrying any unknown field, at any depth, is rejected whole
// (`parse_strict`).

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TempoSnapshot {
    pub bpm: f64,
    pub beats_per_bar: u8,
    /// The clock's beat at `engine_time_s`.
    pub beat: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineSnapshot {
    pub name: String,
    /// The loaded show's revision: a new one is fetched when it changes.
    pub rev: u64,
    pub transport: Transport,
    /// Show units, at `engine_time_s`.
    pub position: f64,
    pub loop_on: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveCueSnapshot {
    pub cue: String,
    pub layer: u8,
    pub group: Option<u8>,
    pub held: bool,
    pub solo: bool,
    /// The main machine's clock.
    pub started_s: f64,
    pub started_beat: f64,
    pub settings: Settings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybackState {
    pub engine_time_s: f64,
    pub project: String,
    pub fingerprint: u64,
    pub tempo: TempoSnapshot,
    pub timeline: Option<TimelineSnapshot>,
    pub cues: Vec<ActiveCueSnapshot>,
    pub settings: Settings,
    pub look_on: bool,
    pub live: LiveModifiers,
    pub cue_page: usize,
}

/// `GET /sync/state`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateReply {
    pub role: Role,
    pub term: u64,
    pub holds_output: bool,
    /// The main machine's lease (`LEASE`), in ms.
    pub lease_ms: u64,
    pub state: PlaybackState,
}

/// `GET /sync/role` (no token: it changes nothing and tells little).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleReply {
    pub role: Role,
    pub term: u64,
    pub holds_output: bool,
    pub lease_ms: u64,
}

/// Memory only.
#[derive(Debug, Default)]
pub struct StandbyState {
    // The main machine's side.
    /// The pairing code shown; empty while not main.
    pub code: String,
    failed: u8,
    blocked_until: Option<Instant>,
    /// The paired backup's token (one backup: a new pairing revokes it).
    token: Option<String>,
    pub paired_addr: Option<String>,
    /// This machine's project fingerprint, computed by the worker.
    pub fingerprint: Option<u64>,
    /// The paired backup's last authenticated poll (or the pairing).
    last_poll: Option<Instant>,
    /// When the lease ran out; cleared by the next poll (never re-arms).
    pub lease_lost_at: Option<Instant>,
    // The backup's side (and a main machine's probe).
    peer_token: Option<String>,
    pub last_ok: Option<Instant>,
    /// The last 401 from the main machine's sync address (a restarted main
    /// machine that forgot the token). Counts like `last_ok` for the
    /// takeover delay (`release_check`), never for the link.
    pub last_heard: Option<Instant>,
    /// When following started (for « Lien perdu depuis » before any answer).
    pub since: Option<Instant>,
    samples: VecDeque<(f64, f64)>,
    pub offset_s: f64,
    pub lag_ms: f32,
    pub last_error: Option<String>,
    pub peer_role: Option<Role>,
    pub peer_term: u64,
    pub peer_holds_output: bool,
    /// When the other machine last said it held an output.
    pub peer_held_at: Option<Instant>,
    /// The main machine's advertised lease.
    pub peer_lease_ms: Option<u64>,
    pub peer_project: Option<String>,
    pub peer_fingerprint: u64,
    /// After « Recevoir le projet »: (this machine's fingerprint then, the
    /// main machine's), counted as the same project.
    accepted: Option<(u64, u64)>,
    pub project_mismatch: bool,
    /// The show the main machine plays: (name, revision), and the one loaded.
    want_show: Option<(String, u64)>,
    loaded_show: Option<(String, u64)>,
    /// The other machine says it is main too, same generation.
    pub conflict: bool,
    /// Why this machine left the main role by itself.
    pub demoted: Option<String>,
}

struct ServerHandle {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    port: u16,
}

impl ServerHandle {
    /// Stops the listener and waits: the port is closed when it returns.
    /// Never called with the `Shared` lock held.
    fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

/// `Shared.standby`.
pub struct Standby {
    pub settings: StandbySettings,
    pub state: StandbyState,
    path: Option<PathBuf>,
    /// `--standby-bind`: every interface by default (tests: 127.0.0.1,
    /// which then always wins over the settings' interface).
    bind: IpAddr,
    server: Option<ServerHandle>,
    /// The worker's switch: set to false, it ends by itself (never joined:
    /// it may be the one asking).
    worker: Option<Arc<AtomicBool>>,
    /// `LEASE` (tests shorten it).
    pub lease: Duration,
}

impl Standby {
    pub fn load(path: PathBuf, bind: IpAddr) -> Self {
        let mut settings: StandbySettings = crate::load_json(&path);
        settings.takeover_grace_s = settings.takeover_grace_s.clamp(1, MAX_GRACE_S);
        Standby { settings, state: StandbyState::default(), path: Some(path), bind, server: None, worker: None, lease: LEASE }
    }

    /// For the tests: loopback only, nothing saved.
    pub fn in_memory() -> Self {
        Standby { settings: StandbySettings::default(), state: StandbyState::default(), path: None, bind: IpAddr::V4(Ipv4Addr::LOCALHOST), server: None, worker: None, lease: LEASE }
    }

    pub fn is_standby(&self) -> bool {
        self.settings.role == Role::Standby
    }

    pub fn bound_port(&self) -> Option<u16> {
        self.server.as_ref().map(|h| h.port)
    }

    fn save(&self) {
        if let Some(path) = &self.path {
            crate::save_json(path, &self.settings);
        }
    }

    fn listen_ip(&self) -> IpAddr {
        if !self.bind.is_unspecified() {
            return self.bind;
        }
        self.settings.interface.as_deref().and_then(|i| parse_interface(i).ok()).unwrap_or(self.bind)
    }

    fn link_up(&self, now: Instant) -> bool {
        self.state.last_ok.is_some_and(|t| now.saturating_duration_since(t) < LINK_TIMEOUT)
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("source aléatoire indisponible : {e}"))?;
    Ok(b)
}

fn new_code() -> Result<String, String> {
    // 4_294_000_000 is a multiple of 10^6 below 2^32: no modulo bias.
    loop {
        let v = u32::from_le_bytes(random_bytes::<4>()?);
        if v < 4_294_000_000 {
            return Ok(format!("{:06}", v % 1_000_000));
        }
    }
}

fn new_token() -> Result<String, String> {
    Ok(random_bytes::<32>()?.iter().map(|b| format!("{b:02x}")).collect())
}

/// Equal without an early exit (lengths are fixed, not secret).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y));
    std::hint::black_box(diff) == 0
}

// ---------------------------------------------------------------------
// Roles, the gate and the threads.

/// The `standby` interlock and the output hold follow the role. Leaving
/// standby only reopens them: it never arms, nothing reconnects by itself.
pub fn sync_gate(s: &mut Shared) {
    let standby = s.standby.is_standby();
    if standby || s.gate.has_interlock(interlock::STANDBY) {
        s.gate.register(interlock::STANDBY, interlock::STANDBY_LABEL, !standby);
    }
    s.output_mgr.set_hold(standby.then(|| interlock::STANDBY_LABEL.to_string()));
}

/// Starts what the role needs, after stopping what ran: the listener of
/// a main machine (a new code), the worker of a main machine or a backup.
/// Autonome: nothing at all.
pub fn apply_role(shared: &Arc<Mutex<Shared>>) -> Result<(), String> {
    let old = {
        let mut s = shared.lock().unwrap();
        sync_gate(&mut s);
        if let Some(w) = s.standby.worker.take() {
            w.store(false, Ordering::SeqCst);
        }
        s.standby.server.take()
    };
    if let Some(h) = old {
        h.stop();
    }
    let mut s = shared.lock().unwrap();
    let role = s.standby.settings.role;
    let st = &mut s.standby.state;
    st.code.clear();
    st.token = None;
    st.paired_addr = None;
    st.failed = 0;
    st.blocked_until = None;
    st.samples.clear();
    st.last_ok = None;
    st.last_heard = None;
    st.peer_role = None;
    st.conflict = false;
    st.since = (role == Role::Standby).then(Instant::now);
    st.last_poll = None;
    if st.lease_lost_at.take().is_some() {
        s.notices.condition(LEASE_NOTICE, "standby", None);
    }
    let st = &mut s.standby.state;
    st.peer_held_at = None;
    st.peer_lease_ms = None;
    st.want_show = None;
    st.loaded_show = None;
    match role {
        Role::Standalone => return Ok(()),
        Role::Primary => start_server(&mut s, Arc::clone(shared))?,
        Role::Standby => {}
    }
    let running = Arc::new(AtomicBool::new(true));
    std::thread::Builder::new()
        .name("standby".into())
        .spawn({
            let (shared, running) = (Arc::clone(shared), Arc::clone(&running));
            move || worker(shared, running)
        })
        .map_err(|e| format!("machine de secours : {e}"))?;
    s.standby.worker = Some(running);
    Ok(())
}

/// Stops the listener and the worker (shutdown).
pub fn stop(shared: &Arc<Mutex<Shared>>) {
    let old = {
        let mut s = shared.lock().unwrap();
        if let Some(w) = s.standby.worker.take() {
            w.store(false, Ordering::SeqCst);
        }
        s.standby.server.take()
    };
    if let Some(h) = old {
        h.stop();
    }
}

fn start_server(s: &mut Shared, shared: Arc<Mutex<Shared>>) -> Result<(), String> {
    let code = new_code()?;
    let addr = SocketAddr::new(s.standby.listen_ip(), s.standby.settings.port);
    let server = crate::web::bind_with(addr, MAX_BODY, MAX_BODY * MAX_CONNECTIONS, MAX_CONNECTIONS).map_err(|e| format!("port {} indisponible : {e}", s.standby.settings.port))?;
    let port = server.server_addr().to_ip().map_or(s.standby.settings.port, |a| a.port());
    let running = Arc::new(AtomicBool::new(true));
    let thread = std::thread::Builder::new()
        .name("standby-sync".into())
        .spawn({
            let running = Arc::clone(&running);
            move || serve(server, shared, running)
        })
        .map_err(|e| format!("machine de secours : {e}"))?;
    s.standby.server = Some(ServerHandle { running, thread: Some(thread), port });
    s.standby.state.code = code;
    log::info!("Machine principale : synchro sur {addr} (port {port})");
    s.gate.log().record("standby_listen", Some(ArmSource::Ui.id()), json!({ "port": port }));
    Ok(())
}

fn worker(shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) {
    let mut next_fp = Instant::now();
    let mut next_probe = Instant::now();
    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= next_fp {
            next_fp = now + FINGERPRINT_EVERY;
            let fp = crate::project::current_project(&shared).fingerprint();
            shared.lock().unwrap().standby.state.fingerprint = Some(fp);
        }
        let (role, peer, token) = {
            let mut s = shared.lock().unwrap();
            // The engine checks every frame too; this one keeps it in unit
            // tests and while the engine is between frames.
            check_lease(&mut s, Instant::now());
            let peer = s.standby.settings.peer.as_deref().and_then(|p| parse_peer(p).ok());
            (s.standby.settings.role, peer, s.standby.state.peer_token.clone())
        };
        match (role, peer) {
            (Role::Standby, Some(peer)) => {
                match token {
                    Some(token) => poll_once(&shared, peer, &token),
                    // Unpaired: a 401 still says the main machine is up.
                    None => listen_unpaired(&shared, peer),
                }
            }
            (Role::Primary, Some(peer)) if now >= next_probe => {
                next_probe = now + PROBE;
                let probe = get_role(peer);
                let demoted = {
                    let mut s = shared.lock().unwrap();
                    note_probe(&mut s, probe.as_ref().ok().copied(), Instant::now())
                };
                if demoted {
                    if let Err(e) = apply_role(&shared) {
                        log::warn!("machine de secours : {e}");
                    }
                    return;
                }
            }
            _ => {}
        }
        let end = now + POLL;
        while running.load(Ordering::SeqCst) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A main machine's probe of its peer. Returns whether it stepped down.
fn note_probe(s: &mut Shared, probe: Option<RoleReply>, now: Instant) -> bool {
    let st = &mut s.standby.state;
    st.peer_role = probe.map(|r| r.role);
    st.peer_holds_output = probe.is_some_and(|r| r.holds_output);
    if let Some(r) = probe {
        st.last_ok = Some(now);
        st.peer_term = r.term;
        if r.holds_output {
            st.peer_held_at = Some(now);
        }
    }
    let term = s.standby.settings.term;
    match probe {
        Some(r) if r.role == Role::Primary && r.term > term && s.standby.settings.role == Role::Primary => {
            demote(s);
            true
        }
        Some(r) => {
            s.standby.state.conflict = r.role == Role::Primary && r.term == term;
            false
        }
        None => {
            s.standby.state.conflict = false;
            false
        }
    }
}

/// At start-up, before any output opens: a main machine whose peer says
/// it is main too asks no output by itself. A newer generation took over
/// while this one was away: it steps down (`demote`). The same one (both
/// set to Principale): a conflict the operator settles. Returns whether
/// outputs must stay closed. Autonome, Secours or no peer: nothing asked.
pub fn startup_check(s: &mut Shared) -> bool {
    if s.standby.settings.role != Role::Primary {
        return false;
    }
    let Some(peer) = s.standby.settings.peer.as_deref().and_then(|p| parse_peer(p).ok()) else {
        return false;
    };
    let probe = get_role(peer).ok();
    startup_verdict(s, probe, Instant::now())
}

/// `startup_check` once the peer answered (or not).
fn startup_verdict(s: &mut Shared, probe: Option<RoleReply>, now: Instant) -> bool {
    let demoted = note_probe(s, probe, now);
    let conflict = s.standby.state.conflict;
    if conflict {
        let msg = format!("{PEER_PRIMARY_FR} (même génération) : aucune sortie connectée au démarrage. Choisissez quelle machine est la principale.");
        log::warn!("{msg}");
        s.notices.push(crate::notices::Level::Warning, "standby", msg, None);
    }
    let term = s.standby.settings.term;
    demoted || conflict || probe.is_some_and(|r| r.role == Role::Primary && r.term >= term)
}

/// The lease (every frame, on a main machine): a paired backup that has
/// not polled for `lease` may be about to take over, or the link between
/// the two is broken and it cannot tell this machine is alive. This
/// machine disarms itself so two machines are never armed on one output.
/// The output stays connected (dark frames), a warning stays until the
/// backup polls again, and only the operator arms again. Unpaired (no
/// token) or not main: nothing.
pub fn check_lease(s: &mut Shared, now: Instant) {
    let sb = &s.standby;
    if sb.settings.role != Role::Primary || sb.state.token.is_none() || sb.state.lease_lost_at.is_some() {
        return;
    }
    let Some(last) = sb.state.last_poll else { return };
    if now.saturating_duration_since(last) < sb.lease {
        return;
    }
    s.standby.state.lease_lost_at = Some(now);
    let armed = s.gate.is_armed();
    if armed {
        s.gate.disarm(DisarmReason::Interlock(BACKUP_LOST_FR.into()), ArmSource::System);
    }
    log::warn!("{BACKUP_LOST_WARN_FR}");
    s.gate.log().record("standby_lease_lost", Some(ArmSource::System.id()), json!({ "armed": armed, "lease_ms": s.standby.lease.as_millis() as u64 }));
    s.notices.condition(LEASE_NOTICE, "standby", Some((crate::notices::Level::Warning, BACKUP_LOST_WARN_FR.to_string())));
}

/// The paired backup polled: the lease starts again. Never arms.
fn note_poll(s: &mut Shared, now: Instant) {
    s.standby.state.last_poll = Some(now);
    if s.standby.state.lease_lost_at.take().is_some() {
        log::info!("Machine de secours de nouveau joignable (le laser reste désarmé jusqu'à l'action de l'opérateur)");
        s.gate.log().record("standby_lease_back", Some(ArmSource::System.id()), json!({}));
        s.notices.condition(LEASE_NOTICE, "standby", None);
    }
}

/// Arming a main machine whose backup is unreachable (`lease_lost_at`):
/// the backup may have taken over, and while it stays unreachable this
/// machine's lease does not disarm it again (only a poll restarts it). So
/// arming then needs the operator's explicit confirmation of that risk
/// (`confirmed`, sent only by the page's confirmation): `Err` otherwise.
/// Logged when confirmed. Anything else: nothing to confirm.
pub fn arm_check(s: &mut Shared, confirmed: bool, src: ArmSource) -> Result<(), String> {
    if s.standby.settings.role != Role::Primary || s.standby.state.lease_lost_at.is_none() || s.gate.is_armed() {
        return Ok(());
    }
    if !confirmed {
        return Err(BACKUP_LOST_ARM_FR.into());
    }
    s.gate.log().record("standby_arm_backup_lost", Some(src.id()), json!({ "confirmed": true }));
    Ok(())
}

/// Parses a message refusing every key its types do not know, nested ones
/// included: `Settings` and `LiveModifiers` ignore unknown keys (old saved
/// files must load), so the message is checked against its own
/// re-serialisation, which carries every known field.
pub fn parse_strict<T: serde::de::DeserializeOwned + Serialize>(body: &[u8]) -> Result<T, String> {
    let raw: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let parsed: T = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
    let known = serde_json::to_value(&parsed).map_err(|e| e.to_string())?;
    match unknown_key(&raw, &known, "") {
        Some(path) => Err(format!("champ inconnu « {path} »")),
        None => Ok(parsed),
    }
}

/// The first key of `raw` that `known` lacks, as a path.
fn unknown_key(raw: &Value, known: &Value, path: &str) -> Option<String> {
    match (raw, known) {
        (Value::Object(r), Value::Object(k)) => r.iter().find_map(|(key, v)| {
            let p = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
            match k.get(key) {
                None => Some(p),
                Some(kv) => unknown_key(v, kv, &p),
            }
        }),
        (Value::Array(r), Value::Array(k)) => r.iter().zip(k).enumerate().find_map(|(i, (a, b))| unknown_key(a, b, &format!("{path}[{i}]"))),
        _ => None,
    }
}

/// The other machine took over: this one becomes *Autonome*, disarmed,
/// its outputs let go (the remembered choice stays). It never takes
/// anything back by itself.
pub fn demote(s: &mut Shared) {
    s.standby.settings.role = Role::Standalone;
    s.standby.save();
    if s.gate.is_armed() {
        s.gate.disarm(DisarmReason::Interlock(PEER_PRIMARY_FR.into()), ArmSource::System);
    }
    s.output_mgr.set_hold(Some(PEER_PRIMARY_FR.into()));
    s.output_mgr.set_hold(None);
    let msg = "Une autre machine a pris le relais : celle-ci est passée en Autonome, désarmée, sans sortie".to_string();
    log::warn!("{msg}");
    s.gate.log().record("standby_demoted", Some(ArmSource::System.id()), json!({ "peer_term": s.standby.state.peer_term }));
    s.notices.push(crate::notices::Level::Warning, "standby", msg.clone(), None);
    s.standby.state.demoted = Some(msg);
}

// ---------------------------------------------------------------------
// The backup's side: following.

fn poll_once(shared: &Arc<Mutex<Shared>>, peer: SocketAddr, token: &str) {
    let t0 = shared.lock().unwrap().now_s();
    let got = http(peer, "GET", "/sync/state", Some(token), None, REQUEST_TIMEOUT, MAX_STATE);
    let fetch = {
        let mut s = shared.lock().unwrap();
        match got {
            Ok((200, body)) => match parse_strict::<StateReply>(&body) {
                Ok(reply) => {
                    let t1 = s.now_s();
                    apply(&mut s, reply, t0, t1, Instant::now());
                }
                // Ignored whole: the link is not counted as up.
                Err(e) => s.standby.state.last_error = Some(format!("message ignoré : {e}")),
            },
            Ok((401, _)) => {
                // A restarted main machine forgot the token: it is up, and
                // it has no lease for this backup (it never disarms by
                // itself). Heard, not followed.
                heard_unpaired(&mut s, Instant::now());
                s.standby.state.peer_token = None;
                s.standby.state.last_error = Some("Appairage refusé : saisissez le code affiché sur la machine principale".into());
            }
            Ok((code, body)) => s.standby.state.last_error = Some(format!("réponse {code} : {}", String::from_utf8_lossy(&body).chars().take(120).collect::<String>())),
            Err(e) => s.standby.state.last_error = Some(e),
        }
        let st = &s.standby.state;
        st.want_show.clone().filter(|w| st.loaded_show.as_ref() != Some(w))
    };
    if let Some(want) = fetch {
        match http(peer, "GET", "/sync/show", Some(token), None, Duration::from_secs(3), MAX_SHOW).and_then(|(code, body)| {
            if code != 200 {
                return Err(format!("show : réponse {code}"));
            }
            serde_json::from_slice::<Show>(&body).map_err(|e| format!("show illisible : {e}"))
        }) {
            Ok(show) => {
                let mut s = shared.lock().unwrap();
                s.timeline.load(show);
                s.standby.state.loaded_show = Some(want);
            }
            Err(e) => shared.lock().unwrap().standby.state.last_error = Some(e),
        }
    }
}

/// An unpaired backup still asks for the state, without a token: a 401
/// means a main machine answers at that address (restarted after a crash,
/// say), so the takeover waits as if it had been followed. Nothing else is
/// read.
fn listen_unpaired(shared: &Arc<Mutex<Shared>>, peer: SocketAddr) {
    if let Ok((401, _)) = http(peer, "GET", "/sync/state", None, None, REQUEST_TIMEOUT, 1024) {
        heard_unpaired(&mut shared.lock().unwrap(), Instant::now());
    }
}

/// A 401 from the main machine: it is up, without a lease for this
/// backup (it never disarms by itself).
fn heard_unpaired(s: &mut Shared, now: Instant) {
    let st = &mut s.standby.state;
    st.last_heard = Some(now);
    st.peer_lease_ms = None;
}

/// Applies one answer of the main machine (`t0`, `t1`: this machine's
/// clock when asked and answered). Only playback state changes: never
/// the gate, the e-stop, safety, calibration or the outputs.
pub fn apply(s: &mut Shared, reply: StateReply, t0: f64, t1: f64, now: Instant) {
    let p = reply.state.engine_time_s;
    if !(p.is_finite() && t0.is_finite() && t1.is_finite()) {
        s.standby.state.last_error = Some("message ignoré : horloge invalide".into());
        return;
    }
    let st = &mut s.standby.state;
    let rtt = (t1 - t0).max(0.0);
    st.samples.push_back((rtt, p + rtt / 2.0 - t1));
    while st.samples.len() > SAMPLES {
        st.samples.pop_front();
    }
    let best = st.samples.iter().copied().fold((f64::INFINITY, 0.0), |a, b| if b.0 < a.0 { b } else { a });
    st.offset_s = best.1;
    st.lag_ms = (rtt * 500.0) as f32;
    st.last_ok = Some(now);
    st.last_error = None;
    st.peer_role = Some(reply.role);
    st.peer_term = reply.term;
    st.peer_holds_output = reply.holds_output;
    if reply.holds_output {
        st.peer_held_at = Some(now);
    }
    st.peer_lease_ms = Some(reply.lease_ms);
    st.peer_project = Some(reply.state.project.clone());
    st.peer_fingerprint = reply.state.fingerprint;
    st.project_mismatch = match st.fingerprint {
        Some(local) if reply.state.fingerprint != 0 => reply.state.fingerprint != local && st.accepted != Some((local, reply.state.fingerprint)),
        _ => false,
    };
    let offset = st.offset_s;
    if !s.standby.is_standby() {
        return;
    }
    let state = reply.state;
    let local_p = p - offset;
    s.tempo.mirror(state.tempo.bpm, state.tempo.beats_per_bar, state.tempo.beat, local_p);
    let cues: Vec<MirroredCue> = state
        .cues
        .into_iter()
        .filter(|c| c.started_s.is_finite() && c.started_beat.is_finite())
        .map(|c| MirroredCue { cue: c.cue, layer: c.layer, group: c.group, held: c.held, solo: c.solo, started_s: c.started_s - offset, started_beat: c.started_beat, settings: c.settings })
        .collect();
    s.deck.mirror(&cues);
    if s.settings != state.settings {
        s.settings = state.settings;
        s.settings_rev += 1;
    }
    s.look_on = state.look_on;
    s.active_cue = s.deck.primary().map(|a| a.cue.clone());
    let mut live = state.live;
    live.brightness = if live.brightness.is_finite() { live.brightness.clamp(0.0, 1.0) } else { s.live.brightness };
    live.sanitize_effects();
    if s.live != live {
        s.live = live;
    }
    if state.cue_page < s.grid.view().len() {
        s.cue_page = state.cue_page;
    }
    match state.timeline {
        Some(tl) => {
            let key = (tl.name.clone(), tl.rev);
            s.standby.state.want_show = Some(key.clone());
            if s.standby.state.loaded_show.as_ref() == Some(&key) {
                let c = s.timeline_clock();
                let dt = c.t - local_p;
                let beats = s.timeline.show.as_ref().is_some_and(|sh| sh.time_base == TimeBase::Beats);
                let pos = match tl.transport {
                    Transport::Playing if beats => tl.position + dt * c.bpm / 60.0,
                    Transport::Playing => tl.position + dt,
                    _ => tl.position,
                };
                s.timeline.mirror(tl.transport, pos, tl.loop_on, &c, JUMP_S);
            }
        }
        None => {
            s.standby.state.want_show = None;
            if s.timeline.is_playing() {
                s.timeline.stop();
            }
        }
    }
}

/// The main machine's state now (`GET /sync/state`).
pub fn snapshot(s: &Shared) -> PlaybackState {
    let t = s.now_s();
    let c = s.timeline_clock();
    PlaybackState {
        engine_time_s: t,
        project: s.project.current.clone().unwrap_or_else(|| crate::project::UNTITLED.into()),
        fingerprint: s.standby.state.fingerprint.unwrap_or(0),
        tempo: TempoSnapshot { bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar, beat: s.tempo.beat_at(t) },
        timeline: s.timeline.show.as_ref().map(|sh| TimelineSnapshot { name: sh.name.clone(), rev: s.timeline.rev(), transport: s.timeline.transport, position: s.timeline.position_at(&c), loop_on: s.timeline.loop_on }),
        cues: s
            .deck
            .active
            .iter()
            .map(|a| ActiveCueSnapshot { cue: a.cue.clone(), layer: a.layer, group: a.group, held: a.held, solo: a.solo, started_s: a.started_s, started_beat: a.started_beat, settings: a.settings.clone() })
            .collect(),
        settings: s.settings.clone(),
        look_on: s.look_on,
        live: s.live.clone(),
        cue_page: s.cue_page,
    }
}

fn role_reply(s: &Shared) -> RoleReply {
    RoleReply { role: s.standby.settings.role, term: s.standby.settings.term, holds_output: s.output_mgr.holds_any(), lease_ms: s.standby.lease.as_millis() as u64 }
}

// ---------------------------------------------------------------------
// The main machine's listener.

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;

fn serve(server: Server, shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) {
    while running.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(mut request)) => {
                let response = route(&mut request, &shared);
                if let Err(e) = request.respond(response) {
                    log::debug!("synchro : réponse non envoyée : {e}");
                }
            }
            Ok(None) => {}
            Err(e) => log::debug!("synchro : {e}"),
        }
    }
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: &'static str,
}

impl Reply {
    fn text(status: u16, body: &str) -> Self {
        Reply { status, body: body.as_bytes().to_vec(), content_type: "text/plain; charset=utf-8" }
    }

    fn json(status: u16, v: &impl Serialize) -> Self {
        Reply { status, body: serde_json::to_vec(v).unwrap_or_default(), content_type: "application/json" }
    }

    fn into_response(self) -> HttpResponse {
        let h = |k: &str, v: &str| Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header");
        // A known length, never chunked: the backup's client reads plain bodies.
        Response::from_data(self.body)
            .with_status_code(self.status)
            .with_chunked_threshold(usize::MAX)
            .with_header(h("Content-Type", self.content_type))
            .with_header(h("Cache-Control", "no-store"))
            .with_header(h("X-Content-Type-Options", "nosniff"))
    }
}

fn route(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let method = request.method().as_str().to_string();
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let auth = request.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str().to_string());
    let peer = request.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    if request.body_length().is_some_and(|n| n > MAX_BODY) {
        return Reply::text(413, "requête trop grande").into_response();
    }
    let mut body = Vec::new();
    if let Err(e) = request.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body) {
        return Reply::text(400, &format!("requête illisible : {e}")).into_response();
    }
    if body.len() > MAX_BODY {
        return Reply::text(413, "requête trop grande").into_response();
    }
    if (method.as_str(), path.as_str()) == ("GET", "/sync/project") {
        // Checked under the lock, built without it.
        let refused = {
            let s = shared.lock().unwrap();
            gate_request(&s, auth.as_deref())
        };
        if let Some(r) = refused {
            return r.into_response();
        }
        let p = crate::project::current_project(shared);
        return match crate::project::project_bytes(&p) {
            Ok(bytes) => Reply { status: 200, body: bytes, content_type: "application/json" }.into_response(),
            Err(e) => Reply::text(500, &format!("{e:#}")).into_response(),
        };
    }
    let reply = handle(&mut shared.lock().unwrap(), &method, &path, auth.as_deref(), &peer, &body, Instant::now());
    reply.into_response()
}

/// None when the request may read: this machine is main and the token
/// is the paired backup's.
fn gate_request(s: &Shared, auth: Option<&str>) -> Option<Reply> {
    if s.standby.settings.role != Role::Primary {
        return Some(Reply::text(503, "cette machine n'est pas la machine principale"));
    }
    let token = auth.and_then(|a| a.strip_prefix("Bearer ")).map(str::trim);
    let ok = match (token, &s.standby.state.token) {
        (Some(t), Some(mine)) => ct_eq(t.as_bytes(), mine.as_bytes()),
        _ => false,
    };
    (!ok).then(|| Reply::text(401, "non appairé"))
}

/// The whitelist. `auth`: the `Authorization` header; `peer`: the IP.
pub fn handle(s: &mut Shared, method: &str, path: &str, auth: Option<&str>, peer: &str, body: &[u8], now: Instant) -> Reply {
    match (method, path) {
        ("GET", "/sync/role") => Reply::json(200, &role_reply(s)),
        ("POST", "/sync/pair") => {
            if s.standby.settings.role != Role::Primary {
                return Reply::text(503, "cette machine n'est pas la machine principale");
            }
            pair_request(s, body, peer, now)
        }
        ("GET", "/sync/state") | ("GET", "/sync/show") => {
            if let Some(r) = gate_request(s, auth) {
                return r;
            }
            if path == "/sync/state" {
                note_poll(s, now);
                let r = role_reply(s);
                Reply::json(200, &StateReply { role: r.role, term: r.term, holds_output: r.holds_output, lease_ms: r.lease_ms, state: snapshot(s) })
            } else {
                match &s.timeline.show {
                    Some(show) => Reply::json(200, show),
                    None => Reply::text(404, "aucun show chargé"),
                }
            }
        }
        // Whitelist: any route of the main UI is refused.
        _ if crate::web::is_known_route(method, path) => Reply::text(403, "interdit depuis la synchro"),
        _ => Reply::text(404, "introuvable"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairBody {
    code: String,
}

fn pair_request(s: &mut Shared, body: &[u8], peer: &str, now: Instant) -> Reply {
    let st = &mut s.standby.state;
    if let Some(until) = st.blocked_until {
        if now < until {
            let left = until.saturating_duration_since(now).as_secs() + 1;
            return Reply::text(429, &format!("Trop de codes faux : réessayez dans {left} s avec le nouveau code"));
        }
        st.blocked_until = None;
    }
    let Ok(req) = serde_json::from_slice::<PairBody>(body) else {
        return Reply::text(400, "requête invalide");
    };
    if st.code.len() != CODE_LEN || !ct_eq(req.code.trim().as_bytes(), st.code.as_bytes()) {
        st.failed += 1;
        if st.failed >= MAX_PAIR_ATTEMPTS {
            st.failed = 0;
            st.blocked_until = Some(now + PAIR_BLOCK);
            st.code = new_code().unwrap_or_default();
            log::warn!("Machine de secours : {MAX_PAIR_ATTEMPTS} codes faux, nouveau code, essais bloqués 30 s");
            s.gate.log().record("standby_pair_blocked", Some(ArmSource::System.id()), json!({ "addr": peer }));
        }
        return Reply::text(403, "Code incorrect");
    }
    let token = match new_token() {
        Ok(t) => t,
        Err(e) => return Reply::text(500, &e),
    };
    st.failed = 0;
    st.token = Some(token.clone());
    st.paired_addr = Some(peer.to_string());
    // The lease starts with the pairing.
    note_poll(s, now);
    log::info!("Machine de secours appairée ({peer})");
    s.gate.log().record("standby_paired", Some(ArmSource::System.id()), json!({ "addr": peer }));
    Reply::json(200, &json!({ "token": token }))
}

// ---------------------------------------------------------------------
// A small HTTP/1.1 client: one request per connection, bounded answer.

fn http(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: Option<&[u8]>, timeout: Duration, max: usize) -> Result<(u16, Vec<u8>), String> {
    let lost = |e: std::io::Error| format!("{addr} injoignable : {e}");
    let mut c = TcpStream::connect_timeout(&addr, timeout).map_err(lost)?;
    c.set_read_timeout(Some(timeout)).map_err(lost)?;
    c.set_write_timeout(Some(timeout)).map_err(lost)?;
    let _ = c.set_nodelay(true);
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(t) = token {
        head.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    let body = body.unwrap_or(&[]);
    if method == "POST" {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    c.write_all(head.as_bytes()).and_then(|()| c.write_all(body)).map_err(lost)?;
    let mut raw = Vec::new();
    (&mut c).take((max + 16 * 1024) as u64).read_to_end(&mut raw).map_err(lost)?;
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| format!("{addr} : réponse incomplète"))?;
    let head = String::from_utf8_lossy(&raw[..end]).to_string();
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|c| c.parse().ok()).ok_or_else(|| format!("{addr} : réponse invalide"))?;
    let mut length = None;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        if k.trim().eq_ignore_ascii_case("transfer-encoding") {
            return Err(format!("{addr} : réponse non prise en charge"));
        }
        if k.trim().eq_ignore_ascii_case("content-length") {
            length = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = raw.split_off(end + 4);
    if let Some(n) = length {
        if n > max {
            return Err(format!("{addr} : réponse trop grande"));
        }
        if body.len() < n {
            return Err(format!("{addr} : réponse tronquée"));
        }
        body.truncate(n);
    } else if body.len() > max {
        return Err(format!("{addr} : réponse trop grande"));
    }
    Ok((status, body))
}

fn get_role(peer: SocketAddr) -> Result<RoleReply, String> {
    match http(peer, "GET", "/sync/role", None, None, REQUEST_TIMEOUT, 1024)? {
        (200, body) => serde_json::from_slice(&body).map_err(|e| format!("réponse illisible : {e}")),
        (code, _) => Err(format!("réponse {code}")),
    }
}

// ---------------------------------------------------------------------
// Local actions (the main server, 127.0.0.1 only).

/// Body of `POST /api/standby`. Empty strings clear `peer` and `interface`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsRequest {
    pub role: Option<Role>,
    pub peer: Option<String>,
    pub interface: Option<String>,
    pub port: Option<u16>,
    pub takeover_grace_s: Option<u32>,
}

/// How long the main machine must have been silent before a takeover:
/// the grace, and never less than its lease plus `LEASE_MARGIN`, so it has
/// disarmed itself first (`check_lease`). An unknown lease counts as
/// `LEASE`; an advertised one is capped at `MAX_GRACE_S`.
pub fn takeover_delay(grace_s: u32, peer_lease_ms: Option<u64>) -> Duration {
    let lease = peer_lease_ms.map_or(LEASE, |ms| Duration::from_millis(ms.min(MAX_GRACE_S as u64 * 1000)));
    Duration::from_secs(grace_s as u64).max(lease + LEASE_MARGIN)
}

/// Whether this backup may let go of standby now: refused while the main
/// machine answers and holds an output, or answered (a state, or a 401
/// from a restarted main machine) less than `takeover_delay` ago. `probe`: its answer to `GET /sync/role` just now,
/// if any.
pub fn release_check(st: &StandbyState, grace_s: u32, probe: Option<RoleReply>, now: Instant) -> Result<(), String> {
    match probe {
        Some(r) if r.holds_output => Err(PEER_HOLDS_FR.into()),
        Some(_) => Ok(()),
        None => {
            let wait = takeover_delay(grace_s, st.peer_lease_ms);
            match st.last_ok.max(st.last_heard) {
                Some(t) if now.saturating_duration_since(t) < wait => {
                    let ago = now.saturating_duration_since(t).as_secs();
                    let wait_s = wait.as_secs_f64().ceil() as u64;
                    Err(format!("La machine principale répondait encore il y a {ago} s : reprise possible après {wait_s} s sans réponse (elle se désarme d'elle-même avant)."))
                }
                _ => Ok(()),
            }
        }
    }
}

/// « Prendre le relais » (backup only): see `release_check`. The machine
/// becomes main one generation above the other, keeps playing what it
/// plays, stays **disarmed** (nothing here arms) and may then connect an
/// output; its listener opens with a new code.
pub fn takeover(shared: &Arc<Mutex<Shared>>) -> Result<(), (u16, String)> {
    let peer = {
        let s = shared.lock().unwrap();
        if !s.standby.is_standby() {
            return Err((409, "Cette machine n'est pas une machine de secours en veille".into()));
        }
        s.standby.settings.peer.as_deref().and_then(|p| parse_peer(p).ok())
    };
    let probe = peer.and_then(|p| get_role(p).ok());
    {
        let mut s = shared.lock().unwrap();
        if !s.standby.is_standby() {
            return Err((409, "Cette machine n'est pas une machine de secours en veille".into()));
        }
        release_check(&s.standby.state, s.standby.settings.takeover_grace_s, probe, Instant::now()).map_err(|e| (409, e))?;
        let term = s.standby.settings.term.max(s.standby.state.peer_term).max(probe.map_or(0, |r| r.term)) + 1;
        s.standby.settings.role = Role::Primary;
        s.standby.settings.term = term;
        s.standby.save();
        log::warn!("Reprise : cette machine devient principale (génération {term}), laser désarmé");
        let armed = s.gate.is_armed();
        s.gate.log().record("standby_takeover", Some(ArmSource::Ui.id()), json!({ "term": term, "armed": armed, "peer_answered": probe.is_some() }));
    }
    apply_role(shared).map_err(|e| (500, e))
}

/// `POST /api/standby`.
pub fn set_settings(shared: &Arc<Mutex<Shared>>, req: SettingsRequest) -> Result<(), (u16, String)> {
    let peer = match req.peer.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(p) => Some(Some(parse_peer(p).map_err(|e| (400, e))?.to_string())),
        None => None,
    };
    let interface = match req.interface.as_deref().map(str::trim) {
        Some("") => Some(None),
        Some(i) => Some(Some(parse_interface(i).map_err(|e| (400, e))?.to_string())),
        None => None,
    };
    if let Some(p) = req.port {
        if p < 1024 {
            return Err((400, "port invalide : 1024 à 65535".into()));
        }
    }
    if let Some(g) = req.takeover_grace_s {
        if !(1..=MAX_GRACE_S).contains(&g) {
            return Err((400, format!("délai de reprise : 1 à {MAX_GRACE_S} s")));
        }
    }
    let (was, wants) = {
        let s = shared.lock().unwrap();
        (s.standby.settings.role, req.role.unwrap_or(s.standby.settings.role))
    };
    if was == Role::Standby && wants != Role::Standby {
        // Leaving standby is a takeover: the same checks, and as main one
        // generation above.
        if wants == Role::Primary {
            {
                let mut s = shared.lock().unwrap();
                apply_fields(&mut s, &req, peer, interface);
            }
            return takeover(shared);
        }
        let peer_addr = shared.lock().unwrap().standby.settings.peer.as_deref().and_then(|p| parse_peer(p).ok());
        let probe = peer_addr.and_then(|p| get_role(p).ok());
        let s = shared.lock().unwrap();
        release_check(&s.standby.state, s.standby.settings.takeover_grace_s, probe, Instant::now()).map_err(|e| (409, e))?;
    }
    let restart = {
        let mut s = shared.lock().unwrap();
        let before = s.standby.settings.clone();
        apply_fields(&mut s, &req, peer, interface);
        s.standby.settings.role = wants;
        if wants == Role::Standby && before.role != Role::Standby {
            s.standby.state.demoted = None;
        }
        if wants == Role::Primary {
            s.standby.state.demoted = None;
        }
        s.standby.save();
        let after = &s.standby.settings;
        s.gate.log().record("standby_settings", Some(ArmSource::Ui.id()), json!({ "role": after.role, "peer": after.peer, "port": after.port }));
        before.role != after.role || before.port != after.port || before.interface != after.interface || before.peer != after.peer
    };
    if restart {
        apply_role(shared).map_err(|e| (409, e))?;
    }
    Ok(())
}

fn apply_fields(s: &mut Shared, req: &SettingsRequest, peer: Option<Option<String>>, interface: Option<Option<String>>) {
    let set = &mut s.standby.settings;
    if let Some(p) = peer {
        if set.peer != p {
            s.standby.state.peer_token = None;
        }
        set.peer = p;
    }
    if let Some(i) = interface {
        set.interface = i;
    }
    if let Some(p) = req.port {
        set.port = p;
    }
    if let Some(g) = req.takeover_grace_s {
        set.takeover_grace_s = g;
    }
}

/// `POST /api/standby/pair {code}` on the backup: pairs with the main
/// machine at `peer` with the code shown there.
pub fn pair(shared: &Arc<Mutex<Shared>>, code: &str) -> Result<(), (u16, String)> {
    let peer = {
        let s = shared.lock().unwrap();
        if !s.standby.is_standby() {
            return Err((409, "Choisissez d'abord le rôle Secours".into()));
        }
        s.standby.settings.peer.as_deref().map(parse_peer).transpose().map_err(|e| (400, e))?.ok_or((400, "Adresse de l'autre machine manquante".to_string()))?
    };
    let code = code.trim();
    if code.len() != CODE_LEN || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err((400, "Code à 6 chiffres attendu".into()));
    }
    let body = json!({ "code": code }).to_string();
    let (status, reply) = http(peer, "POST", "/sync/pair", None, Some(body.as_bytes()), Duration::from_secs(2), 4096).map_err(|e| (502, e))?;
    let text = String::from_utf8_lossy(&reply).to_string();
    if status != 200 {
        return Err((if status == 429 { 429 } else { 403 }, text));
    }
    let token = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["token"].as_str().map(str::to_string)).filter(|t| t.len() == 64).ok_or((502, "réponse d'appairage invalide".to_string()))?;
    let mut s = shared.lock().unwrap();
    s.standby.state.peer_token = Some(token);
    s.standby.state.peer_lease_ms = None;
    s.standby.state.samples.clear();
    s.standby.state.last_error = None;
    s.standby.state.since = Some(Instant::now());
    log::info!("Appairée avec la machine principale {peer}");
    s.gate.log().record("standby_pair", Some(ArmSource::Ui.id()), json!({ "peer": peer.to_string() }));
    Ok(())
}

/// « Recevoir le projet » on the backup: the main machine's project is
/// written to `projects/` under its name and opened the usual way (every
/// check, atomic, never safety or calibration). Returns its name.
pub fn receive_project(shared: &Arc<Mutex<Shared>>) -> Result<String, (u16, String)> {
    let (peer, token, dir) = {
        let s = shared.lock().unwrap();
        if !s.standby.is_standby() {
            return Err((409, "Cette machine n'est pas une machine de secours en veille".into()));
        }
        let peer = s.standby.settings.peer.as_deref().and_then(|p| parse_peer(p).ok()).ok_or((400, "Adresse de l'autre machine manquante".to_string()))?;
        let token = s.standby.state.peer_token.clone().ok_or((409, "Non appairée : saisissez le code de la machine principale".to_string()))?;
        (peer, token, s.project.dir.clone())
    };
    let (status, bytes) = http(peer, "GET", "/sync/project", Some(&token), None, Duration::from_secs(10), MAX_PROJECT).map_err(|e| (502, e))?;
    if status != 200 {
        return Err((502, format!("projet : réponse {status}")));
    }
    let text = String::from_utf8(bytes).map_err(|_| (502, "projet illisible".to_string()))?;
    let p = crate::project::parse(&text).map_err(|e| (502, format!("{e:#}")))?;
    let path = crate::project::resolve(&dir, &p.name).map_err(|e| (400, format!("{e:#}")))?;
    crate::datafile::write_atomic(&path, text.as_bytes()).map_err(|e| (500, format!("{e:#}")))?;
    match crate::project::route(shared, true, "open", &json!({ "path": p.name }).to_string()) {
        Some(crate::project::Reply::Json(_)) => {}
        Some(crate::project::Reply::Text(code, t)) => return Err((code, t)),
        None => return Err((500, "ouverture impossible".into())),
    }
    let gone = shared.lock().unwrap().history.take();
    drop(gone);
    let local = crate::project::current_project(shared).fingerprint();
    let mut s = shared.lock().unwrap();
    let remote = s.standby.state.peer_fingerprint;
    s.standby.state.fingerprint = Some(local);
    s.standby.state.accepted = Some((local, remote));
    s.standby.state.project_mismatch = false;
    log::info!("Projet reçu de la machine principale : « {} »", p.name);
    Ok(p.name)
}

/// What the page needs at a glance, every frame (top bar).
pub fn summary(s: &Shared, now: Instant) -> Value {
    let sb = &s.standby;
    let link = sb.link_up(now);
    let lost_s = (sb.is_standby() && !link).then(|| sb.state.last_ok.or(sb.state.since).map(|t| now.saturating_duration_since(t).as_secs())).flatten();
    json!({
        "role": sb.settings.role,
        "link": link,
        "lag_ms": sb.state.lag_ms,
        "lost_s": lost_s,
        "paired": sb.state.peer_token.is_some(),
        // The main machine followed now has a lease for this backup, so it
        // disarms itself before a takeover. False: unpaired, or no state
        // received since pairing (the takeover dialog warns).
        "main_fenced": sb.state.peer_token.is_some() && sb.state.peer_lease_ms.is_some(),
        "peer_holds_output": sb.state.peer_holds_output,
        "peer_held_s": sb.state.peer_held_at.map(|t| now.saturating_duration_since(t).as_secs()),
        "backup_lost": sb.state.lease_lost_at.is_some(),
        "takeover_after_s": takeover_delay(sb.settings.takeover_grace_s, sb.state.peer_lease_ms).as_secs_f64().ceil() as u64,
        "project_mismatch": sb.state.project_mismatch,
        "conflict": sb.state.conflict,
        "demoted": sb.state.demoted,
    })
}

/// `GET /api/standby` (main server only: the code is shown there).
pub fn view(s: &Shared, now: Instant) -> Value {
    let sb = &s.standby;
    let mut v = summary(s, now);
    let primary = sb.settings.role == Role::Primary;
    let extra = json!({
        "peer": sb.settings.peer,
        "interface": sb.settings.interface,
        "port": sb.settings.port,
        "bound_port": sb.bound_port(),
        "takeover_grace_s": sb.settings.takeover_grace_s,
        "term": sb.settings.term,
        "code": if primary { sb.state.code.as_str() } else { "" },
        "standby_paired": primary && sb.state.token.is_some(),
        "paired_addr": if primary { sb.state.paired_addr.clone() } else { None },
        "last_error": sb.state.last_error,
        "peer_role": sb.state.peer_role,
        "peer_project": sb.state.peer_project,
        "interfaces": local_addresses(),
        "armed": s.gate.is_armed(),
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    v
}

/// This Mac's IPv4 addresses on the local network, for the choice of
/// interface.
fn local_addresses() -> Vec<String> {
    let mut list: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.ip())
        .filter(|ip| ip.is_ipv4() && check_lan(*ip).is_ok())
        .map(|ip| ip.to_string())
        .collect();
    list.sort();
    list.dedup();
    list
}

/// The routes a backup in standby refuses (423): playing and editing.
/// Reading, the stops, arming (refused by the `standby` interlock with
/// its reason), the machine's own settings and the operator's views stay.
pub fn refused_in_standby(method: &str, path: &str) -> bool {
    use crate::web::RouteClass;
    if method != "POST" || path.starts_with("/api/operators") || path == "/api/about/seen" {
        return false;
    }
    matches!(crate::web::route_class(method, path), Some(RouteClass::Play | RouteClass::Edit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;

    fn primary_on(s: &mut Shared) {
        s.standby.settings.role = Role::Primary;
        s.standby.state.code = "123456".into();
    }

    fn arc(s: Shared) -> Arc<Mutex<Shared>> {
        Arc::new(Mutex::new(s))
    }

    #[test]
    fn standalone_by_default_and_nothing_listens() {
        let s = arc(shared());
        apply_role(&s).unwrap();
        let g = s.lock().unwrap();
        assert_eq!(g.standby.settings, StandbySettings::default());
        assert!(g.standby.bound_port().is_none() && g.standby.worker.is_none());
        assert!(!g.gate.has_interlock(interlock::STANDBY), "the default leaves the gate as it was");
        assert!(g.output_mgr.hold().is_none());
    }

    #[test]
    fn peers_are_local_typed_addresses_never_the_shownet() {
        assert!(parse_peer("127.0.0.1:8093").is_ok());
        assert!(parse_peer("192.168.1.20:8093").is_ok());
        assert!(parse_peer("10.0.0.3:9000").is_ok());
        assert!(parse_peer("192.168.129.51:8093").unwrap_err().contains("ShowNET"));
        for bad in ["8.8.8.8:8093", "224.0.0.1:8093", "255.255.255.255:8093", "0.0.0.0:8093", "127.0.0.1:0", "machine:8093", "192.168.1.20"] {
            assert!(parse_peer(bad).is_err(), "{bad}");
        }
        assert!(parse_interface("192.168.129.51").is_err());
        assert!(parse_interface("127.0.0.1").is_ok());
    }

    #[test]
    fn standby_refuses_arming_with_its_reason_and_holds_the_outputs() {
        let s = arc(shared());
        s.lock().unwrap().gate.set_output_ready(true);
        assert!(s.lock().unwrap().request_arm(ArmSource::Ui).is_ok());
        s.lock().unwrap().standby.settings.role = Role::Standby;
        apply_role(&s).unwrap();
        let mut g = s.lock().unwrap();
        assert!(!g.gate.is_armed(), "entering standby disarms");
        let refused = g.request_arm(ArmSource::Ui).unwrap_err();
        assert!(refused.iter().any(|r| r == interlock::STANDBY_LABEL), "{refused:?}");
        assert_eq!(g.output_mgr.hold().as_deref(), Some(interlock::STANDBY_LABEL));
        drop(g);
        stop(&s);
    }

    fn sample_state(s: &Shared) -> StateReply {
        StateReply { role: Role::Primary, term: 0, holds_output: true, lease_ms: 3000, state: snapshot(s) }
    }

    #[test]
    fn applying_a_state_mirrors_cues_tempo_and_masters_on_the_local_clock() {
        let mut main = shared();
        main.tempo.set_bpm_manual(128.0, main.now_s());
        let id = main.presets[0].id.clone();
        assert!(crate::controls::press_cue(&mut main, &id, None, true));
        main.live.brightness = 0.4;
        let reply = sample_state(&main);
        let mut backup = shared();
        backup.standby.settings.role = Role::Standby;
        let t = backup.now_s();
        apply(&mut backup, reply.clone(), t, t + 0.02, Instant::now());
        assert_eq!(backup.deck.active.len(), 1);
        assert_eq!(backup.deck.active[0].cue, main.deck.active[0].cue);
        assert_eq!(backup.active_cue, main.active_cue);
        assert_eq!(backup.live.brightness, 0.4);
        assert!((backup.tempo.bpm - 128.0).abs() < 1e-9);
        // The beat on both machines at the same instant: within 100 ms.
        let (bm, bb) = (main.tempo.beat_at(main.now_s()), backup.tempo.beat_at(backup.now_s()));
        assert!(((bm - bb) * 60.0 / 128.0).abs() < 0.1, "{bm} vs {bb}");
        assert!(backup.standby.state.lag_ms <= 10.0 + 1e-3);
        // Applied again: the cue keeps its instance.
        let id = backup.deck.active[0].id;
        apply(&mut backup, reply, t, t + 0.01, Instant::now());
        assert_eq!(backup.deck.active[0].id, id);
        // Stopped on the main machine: stopped here.
        crate::controls::stop_running(&mut main);
        apply(&mut backup, sample_state(&main), t, t + 0.01, Instant::now());
        assert!(backup.deck.active.is_empty());
    }

    #[test]
    fn a_timeline_follows_the_main_playhead() {
        let mut main = shared();
        let mut show = Show::new_empty("Concert", TimeBase::Seconds);
        show.tracks.push(Default::default());
        main.timeline.load(show.clone());
        let c = main.timeline_clock();
        main.timeline.play(&c);
        main.timeline.seek(12.0, &c);
        let mut backup = shared();
        backup.standby.settings.role = Role::Standby;
        let t = backup.now_s();
        apply(&mut backup, sample_state(&main), t, t, Instant::now());
        assert_eq!(backup.standby.state.want_show, Some(("Concert".into(), main.timeline.rev())));
        // The worker fetches the show; then the playhead follows.
        backup.timeline.load(show);
        backup.standby.state.loaded_show = backup.standby.state.want_show.clone();
        apply(&mut backup, sample_state(&main), t, t, Instant::now());
        assert!(backup.timeline.is_playing());
        let (pm, pb) = (main.timeline.position_at(&main.timeline_clock()), backup.timeline.position_at(&backup.timeline_clock()));
        assert!((pm - pb).abs() < 0.1, "{pm} vs {pb}");
    }

    #[test]
    fn replication_never_arms_resets_a_stop_or_touches_safety() {
        // Property over generated messages: whatever they carry, the gate,
        // the e-stop, safety, calibration and outputs stay as they were.
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let main = shared();
        let base = serde_json::to_value(sample_state(&main)).unwrap();
        let mut backup = shared();
        backup.standby.settings.role = Role::Standby;
        backup.estop.trip(ArmSource::Ui);
        backup.gate.sync_estop(&backup.estop);
        let safety = serde_json::to_value(backup.safety.get()).unwrap();
        let calibration = serde_json::to_value(backup.calibration).unwrap();
        let hostile = ["armed", "arm", "estop", "reset", "safety", "calibration", "output", "interlocks", "role", "term"];
        for case in 0..400 {
            let mut v = base.clone();
            let r = next();
            let key = hostile[(r % hostile.len() as u64) as usize];
            match case % 4 {
                0 => v[key] = json!(true),
                1 => v["state"][key] = json!({ "on": true }),
                2 => v["state"]["live"]["brightness"] = json!((r % 1000) as f64 / 100.0),
                _ => v["state"]["engine_time_s"] = json!((r % 10_000) as f64 - 5000.0),
            }
            let parsed = serde_json::from_value::<StateReply>(v.clone());
            if case % 4 < 2 {
                assert!(parsed.is_err(), "unknown field `{key}` must reject the whole message");
            }
            if let Ok(reply) = parsed {
                let t = backup.now_s();
                apply(&mut backup, reply, t, t, Instant::now());
            }
            assert!(!backup.gate.is_armed());
            assert!(backup.estop.is_latched(), "only the operator resets a stop");
            assert_eq!(serde_json::to_value(backup.safety.get()).unwrap(), safety);
            assert_eq!(serde_json::to_value(backup.calibration).unwrap(), calibration);
            assert!(backup.output_mgr.selected().is_none());
        }
    }

    #[test]
    fn pairing_by_code_with_blocking_after_five_wrong_ones() {
        let mut s = shared();
        primary_on(&mut s);
        let now = Instant::now();
        let r = handle(&mut s, "GET", "/sync/state", None, "10.0.0.2", b"", now);
        assert_eq!(r.status, 401);
        for _ in 0..MAX_PAIR_ATTEMPTS {
            assert_eq!(handle(&mut s, "POST", "/sync/pair", None, "10.0.0.2", br#"{"code":"000000"}"#, now).status, 403);
        }
        assert_ne!(s.standby.state.code, "123456", "a new code is drawn");
        let code = s.standby.state.code.clone();
        let body = json!({ "code": code }).to_string();
        assert_eq!(handle(&mut s, "POST", "/sync/pair", None, "10.0.0.2", body.as_bytes(), now).status, 429);
        let later = now + PAIR_BLOCK + Duration::from_secs(1);
        let r = handle(&mut s, "POST", "/sync/pair", None, "10.0.0.2", body.as_bytes(), later);
        assert_eq!(r.status, 200);
        let token = serde_json::from_slice::<Value>(&r.body).unwrap()["token"].as_str().unwrap().to_string();
        let auth = format!("Bearer {token}");
        assert_eq!(handle(&mut s, "GET", "/sync/state", Some(&auth), "10.0.0.2", b"", later).status, 200);
        assert_eq!(handle(&mut s, "GET", "/sync/state", Some("Bearer nope"), "10.0.0.2", b"", later).status, 401);
        assert_eq!(handle(&mut s, "POST", "/sync/pair", None, "10.0.0.2", b"{bad", later).status, 400);
        // No route of the main UI, arming above all.
        for (m, p) in [("POST", "/api/arm"), ("POST", "/api/estop/reset"), ("POST", "/api/safety"), ("POST", "/api/standby/takeover"), ("GET", "/api/state")] {
            assert_eq!(handle(&mut s, m, p, Some(&auth), "10.0.0.2", b"{}", later).status, 403, "{p}");
        }
        assert_eq!(handle(&mut s, "GET", "/nope", Some(&auth), "10.0.0.2", b"", later).status, 404);
    }

    #[test]
    fn takeover_checks_the_main_machine_and_never_arms() {
        let st = StandbyState::default();
        let now = Instant::now();
        let holding = RoleReply { role: Role::Primary, term: 0, holds_output: true, lease_ms: 3000 };
        assert_eq!(release_check(&st, 5, Some(holding), now).unwrap_err(), PEER_HOLDS_FR);
        assert!(release_check(&st, 5, Some(RoleReply { holds_output: false, ..holding }), now).is_ok());
        let recent = StandbyState { last_ok: Some(now - Duration::from_secs(2)), ..Default::default() };
        assert!(release_check(&recent, 5, None, now).unwrap_err().contains("il y a 2 s"));
        let old = StandbyState { last_ok: Some(now - Duration::from_secs(6)), ..Default::default() };
        assert!(release_check(&old, 5, None, now).is_ok(), "unreachable for longer than the grace");
        assert!(release_check(&st, 5, None, now).is_ok(), "never linked");
    }

    #[test]
    fn a_returning_main_machine_steps_down_disarmed() {
        let mut s = shared();
        s.standby.settings.role = Role::Primary;
        s.gate.set_output_ready(true);
        s.request_arm(ArmSource::Ui).unwrap();
        // Same generation: a warning only, the show goes on.
        assert!(!note_probe(&mut s, Some(RoleReply { role: Role::Primary, term: 0, holds_output: false, lease_ms: 3000 }), Instant::now()));
        assert!(s.standby.state.conflict && s.gate.is_armed());
        // A newer generation took over.
        assert!(note_probe(&mut s, Some(RoleReply { role: Role::Primary, term: 1, holds_output: true, lease_ms: 3000 }), Instant::now()));
        assert_eq!(s.standby.settings.role, Role::Standalone);
        assert!(!s.gate.is_armed());
        assert!(s.standby.state.demoted.is_some());
        assert!(!s.output_mgr.holds_any());
    }

    #[test]
    fn a_main_machine_restarting_after_a_takeover_opens_nothing() {
        let mut s = shared();
        s.standby.settings.role = Role::Primary;
        // The peer unreachable: outputs as usual.
        assert!(!startup_verdict(&mut s, None, Instant::now()));
        // The peer is the backup in standby: as usual.
        assert!(!startup_verdict(&mut s, Some(RoleReply { role: Role::Standby, term: 0, holds_output: false, lease_ms: 3000 }), Instant::now()));
        assert_eq!(s.standby.settings.role, Role::Primary);
        // An older main machine (it will step down): as usual.
        s.standby.settings.term = 2;
        assert!(!startup_verdict(&mut s, Some(RoleReply { role: Role::Primary, term: 1, holds_output: true, lease_ms: 3000 }), Instant::now()));
        s.standby.settings.term = 0;
        // Both set to Principale, same generation: nothing opens, still main.
        assert!(startup_verdict(&mut s, Some(RoleReply { role: Role::Primary, term: 0, holds_output: false, lease_ms: 3000 }), Instant::now()));
        assert_eq!(s.standby.settings.role, Role::Primary);
        // The backup took over meanwhile: Autonome, nothing opens.
        assert!(startup_verdict(&mut s, Some(RoleReply { role: Role::Primary, term: 1, holds_output: true, lease_ms: 3000 }), Instant::now()));
        assert_eq!(s.standby.settings.role, Role::Standalone);
        assert!(!s.gate.is_armed());
        // Not main: never asks.
        assert!(!startup_check(&mut s));
    }

    #[test]
    fn mirrored_masters_stay_in_range() {
        let main = shared();
        let mut reply = sample_state(&main);
        reply.state.live.brightness = 7.0;
        reply.state.live.trace = f32::NAN;
        let mut backup = shared();
        backup.standby.settings.role = Role::Standby;
        let t = backup.now_s();
        apply(&mut backup, reply, t, t, Instant::now());
        assert_eq!(backup.live.brightness, 1.0);
        assert!(backup.live.trace.is_finite());
    }

    #[test]
    fn standby_refuses_playing_and_editing_routes_only() {
        assert!(refused_in_standby("POST", "/api/settings"));
        assert!(!refused_in_standby("POST", "/api/arm"));
        assert!(!refused_in_standby("POST", "/api/estop"));
        assert!(!refused_in_standby("POST", "/api/standby/takeover"));
        assert!(!refused_in_standby("POST", "/api/standby"));
        assert!(!refused_in_standby("GET", "/api/state"));
    }

    /// Two studios on loopback: a main one with its listener, a backup that
    /// pairs, follows, then takes over (refused while the main one holds
    /// its output, allowed once it is gone), never armed.
    #[test]
    fn two_studios_on_loopback_follow_and_take_over_disarmed() {
        let _guard = crate::test_support::time_guard("two studios on loopback", Duration::from_secs(60));
        let main = arc(shared());
        {
            let mut g = main.lock().unwrap();
            g.standby.settings.role = Role::Primary;
            g.standby.settings.port = 0;
            // A short lease, so the takeover delay is the 1 s grace + margin.
            g.standby.lease = Duration::from_millis(300);
        }
        apply_role(&main).unwrap();
        let port = main.lock().unwrap().standby.bound_port().unwrap();
        let code = main.lock().unwrap().standby.state.code.clone();
        let backup = arc(shared());
        set_settings(&backup, SettingsRequest { role: Some(Role::Standby), peer: Some(format!("127.0.0.1:{port}")), ..Default::default() }).unwrap();
        assert!(pair(&backup, "000000").is_err());
        pair(&backup, &code).unwrap();
        // A cue on the main machine shows on the backup.
        {
            let mut g = main.lock().unwrap();
            let id = g.presets[1].id.clone();
            assert!(crate::controls::press_cue(&mut g, &id, None, true));
        }
        let want = main.lock().unwrap().deck.active[0].cue.clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        while backup.lock().unwrap().deck.active.first().map(|a| a.cue.clone()) != Some(want.clone()) {
            assert!(Instant::now() < deadline, "the backup never followed");
            std::thread::sleep(Duration::from_millis(20));
        }
        {
            let g = backup.lock().unwrap();
            assert!(g.standby.link_up(Instant::now()));
            assert!(g.standby.state.lag_ms < 100.0);
            assert!(!g.gate.is_armed());
        }
        // The main machine holds an output: takeover refused.
        main.lock().unwrap().output_mgr.select(Some("nothing".into()), ArmSource::Ui);
        assert_eq!(takeover(&backup).unwrap_err().1, PEER_HOLDS_FR);
        // The main machine is gone; within the grace: refused, after: taken.
        stop(&main);
        backup.lock().unwrap().standby.settings.takeover_grace_s = 1;
        let err = takeover(&backup);
        assert!(err.is_err(), "within the grace");
        // max(1 s grace, 0.3 s lease + 1.5 s margin) = 1.8 s.
        assert_eq!(backup.lock().unwrap().standby.state.peer_lease_ms, Some(300));
        std::thread::sleep(Duration::from_millis(1900));
        takeover(&backup).unwrap();
        let g = backup.lock().unwrap();
        assert_eq!(g.standby.settings.role, Role::Primary);
        assert_eq!(g.standby.settings.term, 1);
        assert!(!g.gate.is_armed(), "a takeover never arms");
        assert!(g.output_mgr.hold().is_none(), "it may now connect an output");
        assert_eq!(g.deck.active.first().map(|a| a.cue.clone()), Some(want), "playback carries on");
        drop(g);
        stop(&backup);
    }

    /// A main machine paired with a backup at `now` (token, first poll).
    fn paired_main(now: Instant) -> (Shared, String) {
        let mut s = shared();
        primary_on(&mut s);
        let r = handle(&mut s, "POST", "/sync/pair", None, "10.0.0.2", br#"{"code":"123456"}"#, now);
        assert_eq!(r.status, 200);
        let token = serde_json::from_slice::<Value>(&r.body).unwrap()["token"].as_str().unwrap().to_string();
        (s, format!("Bearer {token}"))
    }

    fn arm_on_output(s: &mut Shared) {
        s.output_mgr.select(Some("nothing".into()), ArmSource::Ui);
        s.gate.set_output_ready(true);
        s.request_arm(ArmSource::Ui).unwrap();
    }

    #[test]
    fn a_main_machine_without_polls_disarms_itself_and_keeps_its_output() {
        let t0 = Instant::now();
        let (mut s, auth) = paired_main(t0);
        arm_on_output(&mut s);
        let lease = s.standby.lease;
        // Polled in time: armed.
        check_lease(&mut s, t0 + lease - Duration::from_millis(1));
        assert!(s.gate.is_armed());
        assert_eq!(handle(&mut s, "GET", "/sync/state", Some(&auth), "10.0.0.2", b"", t0 + Duration::from_secs(1)).status, 200);
        check_lease(&mut s, t0 + lease + Duration::from_millis(500));
        assert!(s.gate.is_armed(), "the poll at 1 s renewed the lease");
        // No poll for the lease: disarmed, the output kept (dark frames).
        let t1 = t0 + Duration::from_secs(1) + lease;
        check_lease(&mut s, t1);
        assert!(!s.gate.is_armed());
        assert!(s.standby.state.lease_lost_at.is_some());
        assert_eq!(s.output_mgr.selected().as_deref(), Some("nothing"), "the output stays connected");
        assert!(s.notices.since(0).iter().any(|n| n.text == BACKUP_LOST_WARN_FR && !n.resolved));
        assert_eq!(summary(&s, t1)["backup_lost"], json!(true));
        // A fresh poll ends the warning but never re-arms.
        let t2 = t1 + Duration::from_secs(1);
        assert_eq!(handle(&mut s, "GET", "/sync/state", Some(&auth), "10.0.0.2", b"", t2).status, 200);
        check_lease(&mut s, t2);
        assert!(!s.gate.is_armed(), "only the operator arms again");
        assert_eq!(summary(&s, t2)["backup_lost"], json!(false));
        // The operator arms; a later silence disarms again.
        s.request_arm(ArmSource::Ui).unwrap();
        check_lease(&mut s, t2 + lease);
        assert!(!s.gate.is_armed());
        // The other routes (role, show) do not renew the lease.
        let (mut s, auth) = paired_main(t0);
        arm_on_output(&mut s);
        handle(&mut s, "GET", "/sync/role", None, "10.0.0.2", b"", t0 + lease);
        handle(&mut s, "GET", "/sync/show", Some(&auth), "10.0.0.2", b"", t0 + lease);
        check_lease(&mut s, t0 + lease);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn an_unpaired_or_non_main_machine_has_no_lease() {
        let t0 = Instant::now();
        let mut s = shared();
        primary_on(&mut s);
        arm_on_output(&mut s);
        check_lease(&mut s, t0 + Duration::from_secs(3600));
        assert!(s.gate.is_armed(), "no backup paired: nothing to fence");
        let (mut s, _) = paired_main(t0);
        arm_on_output(&mut s);
        s.standby.settings.role = Role::Standalone;
        check_lease(&mut s, t0 + Duration::from_secs(3600));
        assert!(s.gate.is_armed());
    }

    #[test]
    fn the_backup_waits_for_the_main_lease_plus_a_margin() {
        let now = Instant::now();
        assert_eq!(takeover_delay(1, None), LEASE + LEASE_MARGIN, "an unknown lease counts as the default");
        assert_eq!(takeover_delay(1, Some(3000)), Duration::from_millis(4500));
        assert_eq!(takeover_delay(10, Some(3000)), Duration::from_secs(10));
        assert!(LEASE_MARGIN >= POLL + 3 * REQUEST_TIMEOUT, "a late poll on a stalled main machine stays inside the margin");
        assert_eq!(takeover_delay(1, Some(u64::MAX)), Duration::from_secs(MAX_GRACE_S as u64) + LEASE_MARGIN, "capped");
        let heard = |ago_ms: u64| StandbyState { last_ok: Some(now - Duration::from_millis(ago_ms)), peer_lease_ms: Some(3000), ..Default::default() };
        // Grace 1 s, but the main machine's lease is 3 s: refused until 4.5 s.
        assert!(release_check(&heard(1500), 1, None, now).unwrap_err().contains("après 5 s"));
        assert!(release_check(&heard(3900), 1, None, now).is_err());
        assert!(release_check(&heard(4400), 1, None, now).is_err());
        assert!(release_check(&heard(4500), 1, None, now).is_ok());
    }

    /// Review 2, window 1: the backup's next poll leaves `POLL` after the
    /// last answer it saw, the link breaks, and the stalled main machine
    /// only handles that poll three request timeouts later: it renews its
    /// lease from a poll the backup never saw answered. Still disarmed
    /// before a takeover is allowed.
    #[test]
    fn a_late_poll_on_a_stalled_main_machine_still_disarms_it_before_a_takeover() {
        for (lease_ms, grace_s) in [(3000, 1), (500, 1), (3000, 4)] {
            let t0 = Instant::now();
            let (mut main, auth) = paired_main(t0);
            main.standby.lease = Duration::from_millis(lease_ms);
            arm_on_output(&mut main);
            let r = handle(&mut main, "GET", "/sync/state", Some(&auth), "10.0.0.2", b"", t0);
            let reply = parse_strict::<StateReply>(&r.body).unwrap();
            let mut backup = shared();
            backup.standby.settings.role = Role::Standby;
            let t = backup.now_s();
            apply(&mut backup, reply, t, t, t0);
            // The late one: handled, its answer never arrives.
            let late = t0 + POLL + 3 * REQUEST_TIMEOUT;
            let mut renewed = false;
            let mut taken = false;
            for step in 0..=(MAX_GRACE_S as u64 * 100 + 200) {
                let now = t0 + Duration::from_millis(step * 10);
                if !renewed && now >= late {
                    assert_eq!(handle(&mut main, "GET", "/sync/state", Some(&auth), "10.0.0.2", b"", late).status, 200);
                    renewed = true;
                }
                check_lease(&mut main, now);
                if release_check(&backup.standby.state, grace_s, None, now).is_ok() {
                    assert!(!main.gate.is_armed(), "lease {lease_ms} ms, grace {grace_s} s: the backup may take over at {} ms while the main one is armed", step * 10);
                    taken = true;
                    break;
                }
            }
            assert!(renewed && taken);
        }
    }

    #[test]
    fn arming_a_main_machine_whose_backup_is_unreachable_needs_a_confirmation() {
        let t0 = Instant::now();
        let (mut s, auth) = paired_main(t0);
        arm_on_output(&mut s);
        assert_eq!(arm_check(&mut s, false, ArmSource::Ui), Ok(()), "armed: nothing to confirm");
        let lost = t0 + s.standby.lease;
        check_lease(&mut s, lost);
        assert!(!s.gate.is_armed());
        assert_eq!(arm_check(&mut s, false, ArmSource::Ui).unwrap_err(), BACKUP_LOST_ARM_FR);
        assert_eq!(arm_check(&mut s, false, ArmSource::Keyboard).unwrap_err(), BACKUP_LOST_ARM_FR);
        // MIDI cannot confirm: refused even with its opt-in.
        s.midi.store.devices.safety.allow_arm = true;
        assert!(s.request_arm_midi_opt_in().unwrap_err().contains(&BACKUP_LOST_ARM_FR.to_string()));
        assert!(!s.gate.is_armed());
        // Confirmed: allowed (and logged); the lease stays lost, so this
        // machine no longer disarms by itself until a poll comes back.
        assert_eq!(arm_check(&mut s, true, ArmSource::Ui), Ok(()));
        s.request_arm(ArmSource::Ui).unwrap();
        check_lease(&mut s, lost + Duration::from_secs(60));
        assert!(s.gate.is_armed(), "the risk the confirmation states");
        // Once the backup polls again: nothing to confirm, and a new
        // silence disarms again.
        s.gate.disarm(DisarmReason::User, ArmSource::Ui);
        let back = lost + Duration::from_secs(61);
        assert_eq!(handle(&mut s, "GET", "/sync/state", Some(&auth), "10.0.0.2", b"", back).status, 200);
        assert_eq!(arm_check(&mut s, false, ArmSource::Ui), Ok(()));
        s.request_arm(ArmSource::Ui).unwrap();
        let lease = s.standby.lease;
        check_lease(&mut s, back + lease);
        assert!(!s.gate.is_armed());
        // Unpaired, Autonome or Secours: nothing to confirm.
        let mut other = shared();
        assert_eq!(arm_check(&mut other, false, ArmSource::Ui), Ok(()));
    }

    #[test]
    fn a_401_from_a_restarted_main_machine_counts_as_heard() {
        let now = Instant::now();
        let mut s = shared();
        s.standby.settings.role = Role::Standby;
        s.standby.state.last_ok = Some(now - Duration::from_secs(30));
        s.standby.state.peer_lease_ms = Some(3000);
        s.standby.state.peer_token = Some("x".repeat(64));
        assert!(release_check(&s.standby.state, 1, None, now).is_ok(), "followed long ago, nothing since");
        assert_eq!(summary(&s, now)["main_fenced"], json!(true));
        heard_unpaired(&mut s, now - Duration::from_secs(1));
        s.standby.state.peer_token = None;
        let err = release_check(&s.standby.state, 1, None, now).unwrap_err();
        assert!(err.contains("il y a 1 s"), "{err}");
        assert!(release_check(&s.standby.state, 1, None, now + LEASE + LEASE_MARGIN).is_ok());
        let v = summary(&s, now);
        assert_eq!(v["main_fenced"], json!(false), "unpaired: the main machine has no lease for this backup");
        assert_eq!(v["paired"], json!(false));
        assert_eq!(v["link"], json!(false), "heard is not followed");
    }

    /// Two studios on loopback: the main machine restarts (a new listener
    /// forgets the token). The backup keeps hearing its 401s, so a takeover
    /// still waits; once the link also fails it is refused within the delay,
    /// and the backup says it is not fenced.
    #[test]
    fn two_studios_on_loopback_a_restarted_main_machine_is_still_heard() {
        let _guard = crate::test_support::time_guard("restarted main on loopback", Duration::from_secs(60));
        let main = arc(shared());
        {
            let mut g = main.lock().unwrap();
            g.standby.settings.role = Role::Primary;
            g.standby.settings.port = 0;
        }
        apply_role(&main).unwrap();
        let port = main.lock().unwrap().standby.bound_port().unwrap();
        main.lock().unwrap().standby.settings.port = port;
        let code = main.lock().unwrap().standby.state.code.clone();
        let backup = arc(shared());
        set_settings(&backup, SettingsRequest { role: Some(Role::Standby), peer: Some(format!("127.0.0.1:{port}")), ..Default::default() }).unwrap();
        backup.lock().unwrap().standby.settings.takeover_grace_s = 1;
        pair(&backup, &code).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while summary(&backup.lock().unwrap(), Instant::now())["main_fenced"] != json!(true) {
            assert!(Instant::now() < deadline, "the backup never followed");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The main machine restarts: same port, a new code, no token.
        apply_role(&main).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while backup.lock().unwrap().standby.state.peer_token.is_some() {
            assert!(Instant::now() < deadline, "the backup never got a 401");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Heard again and again while unpaired.
        let first = backup.lock().unwrap().standby.state.last_heard.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while backup.lock().unwrap().standby.state.last_heard.unwrap() == first {
            assert!(Instant::now() < deadline, "an unpaired backup stopped listening");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(summary(&backup.lock().unwrap(), Instant::now())["main_fenced"], json!(false));
        // The link fails too (the main one stops answering): within the
        // delay from the last 401, refused.
        let heard = backup.lock().unwrap().standby.state.last_heard.unwrap();
        stop(&main);
        let err = takeover(&backup).unwrap_err();
        assert_eq!(err.0, 409);
        assert!(err.1.contains("reprise possible après"), "{}", err.1);
        assert!(heard.elapsed() < takeover_delay(1, None));
        assert_eq!(backup.lock().unwrap().standby.settings.role, Role::Standby);
        stop(&backup);
    }

    #[test]
    fn a_broken_link_disarms_the_main_machine_before_the_backup_may_take_over() {
        // Test clock: the link breaks right after the last exchange at t0.
        for (lease_ms, grace_s) in [(3000, 1), (3000, 5), (500, 1), (20_000, 2)] {
            let t0 = Instant::now();
            let (mut main, auth) = paired_main(t0);
            main.standby.lease = Duration::from_millis(lease_ms);
            arm_on_output(&mut main);
            let r = handle(&mut main, "GET", "/sync/state", Some(&auth), "10.0.0.2", b"", t0);
            let reply = parse_strict::<StateReply>(&r.body).unwrap();
            let mut backup = shared();
            backup.standby.settings.role = Role::Standby;
            let t = backup.now_s();
            apply(&mut backup, reply, t, t, t0);
            let mut taken = false;
            for step in 0..=(MAX_GRACE_S as u64 * 20 + 40) {
                let now = t0 + Duration::from_millis(step * 50);
                check_lease(&mut main, now);
                if release_check(&backup.standby.state, grace_s, None, now).is_ok() {
                    assert!(!main.gate.is_armed(), "lease {lease_ms} ms, grace {grace_s} s: the backup may take over at {} ms while the main one is armed", step * 50);
                    // A whole margin after the main one disarmed.
                    let lost = main.standby.state.lease_lost_at.unwrap();
                    assert!(now.saturating_duration_since(lost) >= LEASE_MARGIN);
                    taken = true;
                    break;
                }
            }
            assert!(taken);
        }
    }

    #[test]
    fn nested_unknown_fields_reject_the_whole_message() {
        let mut main = shared();
        let id = main.presets[0].id.clone();
        assert!(crate::controls::press_cue(&mut main, &id, None, true));
        let reply = sample_state(&main);
        let bytes = serde_json::to_vec(&reply).unwrap();
        assert_eq!(parse_strict::<StateReply>(&bytes).unwrap(), reply, "a real state goes through");
        let base = serde_json::to_value(&reply).unwrap();
        for path in [&["state", "settings"][..], &["state", "live"], &["state", "tempo"], &["state", "cues", "0", "settings"]] {
            for key in ["armed", "safety", "calibration", "estop"] {
                let mut v = base.clone();
                let mut at = &mut v;
                for p in path {
                    at = match p.parse::<usize>() {
                        Ok(i) => &mut at[i],
                        Err(_) => &mut at[*p],
                    };
                }
                at[key] = json!(true);
                let err = parse_strict::<StateReply>(&serde_json::to_vec(&v).unwrap()).unwrap_err();
                assert!(err.contains(key), "{path:?}.{key}: {err}");
            }
        }
    }

    /// Two studios on loopback: the backup stops polling (its link broken);
    /// the main machine disarms by itself, keeps its output and says so.
    #[test]
    fn two_studios_on_loopback_the_main_disarms_when_the_backup_goes_silent() {
        let _guard = crate::test_support::time_guard("lease on loopback", Duration::from_secs(60));
        let main = arc(shared());
        {
            let mut g = main.lock().unwrap();
            g.standby.settings.role = Role::Primary;
            g.standby.settings.port = 0;
            g.standby.lease = Duration::from_millis(500);
        }
        apply_role(&main).unwrap();
        let port = main.lock().unwrap().standby.bound_port().unwrap();
        let code = main.lock().unwrap().standby.state.code.clone();
        let backup = arc(shared());
        set_settings(&backup, SettingsRequest { role: Some(Role::Standby), peer: Some(format!("127.0.0.1:{port}")), ..Default::default() }).unwrap();
        pair(&backup, &code).unwrap();
        arm_on_output(&mut main.lock().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backup.lock().unwrap().standby.link_up(Instant::now()) {
            assert!(Instant::now() < deadline, "the backup never followed");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Following: well within the lease, the main machine stays armed.
        std::thread::sleep(Duration::from_millis(800));
        assert!(main.lock().unwrap().gate.is_armed(), "polls renew the lease");
        // The backup's link is cut: no more polls.
        stop(&backup);
        let deadline = Instant::now() + Duration::from_secs(5);
        while main.lock().unwrap().gate.is_armed() {
            assert!(Instant::now() < deadline, "the main machine never disarmed");
            std::thread::sleep(Duration::from_millis(20));
        }
        {
            let g = main.lock().unwrap();
            assert!(g.standby.state.lease_lost_at.is_some());
            assert_eq!(g.output_mgr.selected().as_deref(), Some("nothing"));
            assert_eq!(g.standby.settings.role, Role::Primary);
            assert_eq!(view(&g, Instant::now())["backup_lost"], json!(true));
        }
        stop(&main);
    }
}
