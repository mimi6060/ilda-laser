//! The network remote for a tablet (T-428): a second, reduced HTTP server
//! the operator turns on from the main screen, so they can play and stop
//! from the room.
//!
//! What keeps it safe:
//! - Off at every start: `enabled` is memory only, never written (only the
//!   port is, in `remote.json`). Turning it on is a local action
//!   (`POST /api/remote` on the main server, which listens on 127.0.0.1
//!   only and is never reachable from here).
//! - Its own listener and its own router (`route`), a whitelist: the
//!   pairing page, `POST /pair`, `GET /api/remote/state` and
//!   `POST /api/remote/control`. Any route of the main UI answers 403
//!   (`web::is_known_route`, from the `ROUTES` table), anything else 404.
//!   Nothing here ever calls `web::route`; no file is read or written.
//! - Pairing: a 6-digit code drawn from the system's random source and
//!   shown on the main screen. 5 wrong codes draw a new one and block
//!   pairing for 30 s. A pairing gives a random session token in an
//!   `HttpOnly`, `SameSite=Strict` cookie (never in a URL, never logged),
//!   compared in constant time. Tokens live in memory: turning the access
//!   off, a restart or « Déconnecter » revokes them. At most 4 remotes.
//! - Every control goes through `external::apply` as the source
//!   `SourceKind::Remote`, after this module's own scope (`in_scope`):
//!   cue grid, pages, stop all cues, masters, layer dimmers, tempo, and
//!   the stops. It can never arm, reset an emergency stop, touch safety,
//!   outputs, calibration or the project. The external gate closed cuts
//!   it too, except the blackout and the emergency stop, which are always
//!   accepted.
//! - A pad held down from a tablet that goes quiet (`HOLD_TIMEOUT`), is
//!   disconnected or whose access is turned off is released.
//! - No heartbeat here: a tablet never counts as an operator in front of
//!   the main screen (presence.rs).
//!
//! Zone alignment from a phone (T-460): a paired remote may *propose* a
//! projection zone's 4 corners, never write them.
//! - A session is opened from the main screen only
//!   (`POST /api/zones/align/open`, refused in show mode and under the
//!   safety lock), for one zone and one paired remote. Without it every
//!   nudge is refused.
//! - The remote's three routes (`GET /api/remote/align`,
//!   `POST /api/remote/align/nudge`, `POST /api/remote/align/reset`) move
//!   a draft held here in memory. Steps of `NUDGE_STEP` (×5 at most);
//!   each corner stays in -1..1 and within `MAX_DRAFT_OFFSET` of the
//!   saved one, the quad convex and not flat, else the step is refused.
//! - While open, the output shows a test pattern through the zone's
//!   geometry with the draft corners (`align_geometry`), followed by the
//!   calibration and the whole safety stage, as any test pattern.
//! - Only « Appliquer » on the main screen (`POST /api/zones/align/apply`,
//!   loopback only, refused in show mode and under the safety lock)
//!   writes the zone, and it is logged. « Rejeter », closing, the remote
//!   lost (kicked, access off, quiet for `ALIGN_TIMEOUT`), the zone edited
//!   or deleted, show mode and the e-stop drop the draft: the next frame
//!   is back on the saved zone.
//! - Safety zones, the horizon, the caps, the calibration and arming are
//!   never reachable from the remote (`WHITELIST`).
//!
//! Plain HTTP on the local network, no TLS: acceptable only because the
//! remote can neither arm nor change anything; never expose this port to
//! the Internet.

use crate::controls::{ControlInput, ControlKind};
use crate::external::{self, Refusal, SourceKind};
use crate::projection::{check_quad, Geometry, Warp, SQUARE};
use crate::web::RouteClass;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response, Server};

pub const DEFAULT_PORT: u16 = 8091;
pub const MAX_REMOTES: usize = 4;
pub const MAX_PAIR_ATTEMPTS: u8 = 5;
/// Pairing is blocked this long after `MAX_PAIR_ATTEMPTS` wrong codes.
pub const PAIR_BLOCK: Duration = Duration::from_secs(30);
/// A held pad is released after this long without a request from its
/// remote (the page polls its state twice a second).
pub const HOLD_TIMEOUT: Duration = Duration::from_millis(1500);
/// Request bodies above this are refused unread.
pub const MAX_BODY: usize = 2048;
/// Remote names, in characters.
pub const MAX_NAME: usize = 32;
/// Control ids, in bytes.
const MAX_ID: usize = 64;
/// Pads one remote may hold down at once.
const MAX_HELD: usize = 16;
/// Requests handled at once (each on its own short thread).
const MAX_CONCURRENT: usize = 8;
/// Connections served at once (T-481): a few per tablet, `MAX_REMOTES`
/// tablets.
const MAX_CONNECTIONS: usize = 16;
/// Answer bytes waiting at once on all the connections (T-489): the page
/// is 12 kB, the other answers smaller.
const MAX_OUTPUT: usize = MAX_CONNECTIONS * 64 * 1024;
const CODE_LEN: usize = 6;
const COOKIE: &str = "ls_remote";
const REMOTE_HTML: &str = include_str!("remote.html");

/// The registry ids the remote may drive (before the external gate and
/// the source's scope, T-408). Entries ending in `.` are prefixes; for
/// the layers, only `layer.<n>.dimmer`.
pub const SCOPE: &[&str] = &["grid.", "page.", "cue.stop_all", "master.", "layer.", "tempo.tap", "tempo.bpm", "tempo.resync", "transport.blackout", "safety.estop"];

/// Every route the remote's server answers (`handle`), with its class.
/// None writes anything but the memory-only alignment draft (`Play`):
/// no zone, safety setting, calibration, output or arming is reachable.
pub const WHITELIST: &[(&str, &str, RouteClass)] = &[
    ("GET", "/", RouteClass::Read),
    ("POST", "/pair", RouteClass::Play),
    ("GET", "/api/remote/state", RouteClass::Read),
    ("POST", "/api/remote/control", RouteClass::Play),
    ("GET", ALIGN_PATH, RouteClass::Read),
    ("POST", NUDGE_PATH, RouteClass::Play),
    ("POST", RESET_PATH, RouteClass::Play),
];
const ALIGN_PATH: &str = "/api/remote/align";
const NUDGE_PATH: &str = "/api/remote/align/nudge";
const RESET_PATH: &str = "/api/remote/align/reset";

pub fn in_scope(id: &str) -> bool {
    if id.len() > MAX_ID {
        return false;
    }
    if let Some(rest) = id.strip_prefix("layer.") {
        return rest.strip_suffix(".dimmer").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    }
    SCOPE.iter().any(|s| if s.ends_with('.') { id.len() > s.len() && id.starts_with(s) } else { id == *s })
}

/// `<data-dir>/remote.json`: the port only, never whether it is on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteSettings {
    pub port: u16,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        RemoteSettings { port: DEFAULT_PORT }
    }
}

/// Memory only.
#[derive(Debug, Default)]
pub struct RemoteState {
    /// Always false at start.
    pub enabled: bool,
    /// 6 digits, drawn again at every activation and after too many
    /// wrong codes. Empty while off.
    pub code: String,
    pub failed: u8,
    pub blocked_until: Option<Instant>,
    pub sessions: Vec<RemoteSession>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemoteSession {
    #[serde(skip)]
    pub token: String,
    pub name: String,
    pub addr: String,
    pub paired_s: f64,
    pub last_seen_s: f64,
    #[serde(skip)]
    seen: Option<Instant>,
    /// Momentary controls this remote holds down.
    #[serde(skip)]
    held: Vec<String>,
}

struct ServerHandle {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    port: u16,
}

impl ServerHandle {
    /// Stops the listener and waits for its thread: the port is closed
    /// when this returns. Never called with the `Shared` lock held.
    fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

/// `Shared.remote`.
pub struct Remote {
    pub settings: RemoteSettings,
    pub state: RemoteState,
    path: Option<PathBuf>,
    /// The address the remote listens on: every interface by default
    /// (`--remote-bind`; the tests and e2e runs use 127.0.0.1).
    bind: IpAddr,
    server: Option<ServerHandle>,
    /// The zone alignment session (T-460), memory only.
    pub align: Option<AlignSession>,
}

impl Remote {
    pub fn load(path: PathBuf, bind: IpAddr) -> Self {
        let settings: RemoteSettings = crate::load_json(&path);
        Remote { settings, state: RemoteState::default(), path: Some(path), bind, server: None, align: None }
    }

    /// For the tests: loopback only, nothing saved.
    pub fn in_memory() -> Self {
        Remote { settings: RemoteSettings::default(), state: RemoteState::default(), path: None, bind: IpAddr::V4(Ipv4Addr::LOCALHOST), server: None, align: None }
    }

    /// The port the remote really listens on, if it does.
    pub fn bound_port(&self) -> Option<u16> {
        self.server.as_ref().map(|h| h.port)
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("source aléatoire indisponible : {e}"))?;
    Ok(b)
}

/// 6 uniformly drawn digits.
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

/// Equal without an early exit (the length is not secret: fixed).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y));
    std::hint::black_box(diff) == 0
}

fn token_from_cookie(header: &str) -> Option<&str> {
    header.split(';').find_map(|kv| kv.trim().strip_prefix(COOKIE).and_then(|v| v.strip_prefix('=')))
}

/// The session a token belongs to: every session is compared, in
/// constant time each.
fn find_session(sessions: &[RemoteSession], token: &str) -> Option<usize> {
    let mut found = None;
    for (i, sess) in sessions.iter().enumerate() {
        if ct_eq(sess.token.as_bytes(), token.as_bytes()) {
            found = Some(i);
        }
    }
    found
}

// ---------------------------------------------------------------------
// Turning it on and off (main server, local only).

/// `POST /api/remote { enabled, port? }`. Changing the port while on
/// restarts the listener (and revokes the sessions: their cookies belong
/// to the old port). Returns the error to show.
pub fn set_enabled(shared: &Arc<Mutex<Shared>>, on: bool, port: Option<u16>) -> Result<(), String> {
    if let Some(p) = port {
        if p != 0 && p < 1024 {
            return Err("port invalide : 1024 à 65535".into());
        }
    }
    let (old, job) = {
        let mut s = shared.lock().unwrap();
        let mut job = None;
        if let Some(p) = port.filter(|&p| p != s.remote.settings.port) {
            s.remote.settings.port = p;
            job = s.remote.path.clone().map(|path| (path, s.remote.settings.clone()));
        }
        let restart = on && port.is_some_and(|p| p != 0 && s.remote.server.as_ref().is_some_and(|h| h.port != p));
        let old = if !on || restart {
            if s.remote.state.enabled {
                disable(&mut s, "operator");
            }
            s.remote.server.take()
        } else {
            None
        };
        (old, job)
    };
    if let Some(h) = old {
        h.stop();
    }
    if let Some((path, settings)) = job {
        crate::save_json(&path, &settings);
    }
    if on {
        let mut s = shared.lock().unwrap();
        if s.remote.server.is_none() {
            start(&mut s, Arc::clone(shared))?;
        }
    }
    Ok(())
}

fn start(s: &mut Shared, shared: Arc<Mutex<Shared>>) -> Result<(), String> {
    let code = new_code()?;
    let addr = SocketAddr::new(s.remote.bind, s.remote.settings.port);
    // Timeouts (T-449): a stalled tablet frees its request thread. Bodies
    // (at most `MAX_BODY`) read on the connection's own thread, and at most
    // `MAX_CONNECTIONS` at once, the stalest pushed out (T-481): a flood
    // from the LAN holds neither threads nor descriptors. Answers written
    // by their connection's thread, at most `MAX_OUTPUT` waiting (T-489).
    let server = crate::web::bind_with(addr, MAX_BODY, MAX_BODY * MAX_CONNECTIONS, MAX_CONNECTIONS, MAX_OUTPUT).map_err(|e| format!("port {} indisponible : {e}", s.remote.settings.port))?;
    let port = server.server_addr().to_ip().map_or(s.remote.settings.port, |a| a.port());
    let running = Arc::new(AtomicBool::new(true));
    let thread = std::thread::Builder::new()
        .name("remote".into())
        .spawn({
            let running = Arc::clone(&running);
            move || serve(server, shared, running)
        })
        .map_err(|e| format!("télécommande : {e}"))?;
    s.remote.server = Some(ServerHandle { running, thread: Some(thread), port });
    s.remote.state = RemoteState { enabled: true, code, ..Default::default() };
    log::info!("Télécommande autorisée sur le port {port}");
    s.gate.log().record("remote_access", Some(crate::interlock::ArmSource::Ui.id()), json!({ "enabled": true, "port": port }));
    Ok(())
}

/// Off: every session revoked (and its pads released), the code dropped.
/// The caller stops the listener out of the lock.
fn disable(s: &mut Shared, reason: &str) {
    align_end(s, "remote_off");
    kick(s, None);
    s.remote.state = RemoteState::default();
    log::info!("Télécommande désactivée");
    s.gate.log().record("remote_access", Some(crate::interlock::ArmSource::Ui.id()), json!({ "enabled": false, "reason": reason }));
}

/// « Déconnecter » (one name) or « Tout déconnecter » (`None`). Returns
/// how many remotes were dropped.
pub fn kick(s: &mut Shared, name: Option<&str>) -> usize {
    let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut s.remote.state.sessions).into_iter().partition(|sess| name.is_none_or(|n| n == sess.name));
    s.remote.state.sessions = kept;
    let now = Instant::now();
    if s.remote.align.as_ref().is_some_and(|a| gone.iter().any(|g| g.name == a.remote)) {
        align_end(s, "remote_lost");
    }
    for sess in &gone {
        for id in &sess.held {
            external::release(s, SourceKind::Remote, id);
        }
        external::set_access(s, SourceKind::Remote, &sess.name, false, now);
        log::info!("Télécommande déconnectée : {}", sess.name);
    }
    if !gone.is_empty() {
        let names: Vec<&str> = gone.iter().map(|g| g.name.as_str()).collect();
        s.gate.log().record("remote_kicked", Some(crate::interlock::ArmSource::Ui.id()), json!({ "names": names }));
    }
    gone.len()
}

/// Releases the pads of the remotes quiet for `HOLD_TIMEOUT`.
pub fn sweep(s: &mut Shared, now: Instant) {
    let mut release = Vec::new();
    for sess in &mut s.remote.state.sessions {
        let quiet = sess.seen.is_none_or(|t| now.saturating_duration_since(t) > HOLD_TIMEOUT);
        if quiet && !sess.held.is_empty() {
            release.append(&mut sess.held);
        }
    }
    for id in release {
        external::release(s, SourceKind::Remote, &id);
    }
}

/// `GET /api/remote` (main server only): the code is shown there.
pub fn view(s: &Shared, now: Instant) -> Value {
    let r = &s.remote;
    let port = r.bound_port();
    let urls: Vec<String> = match port {
        None => Vec::new(),
        Some(p) => local_addresses(r.bind).into_iter().map(|ip| format!("http://{ip}:{p}")).collect(),
    };
    let t = s.now_s();
    let sessions: Vec<Value> = r
        .state
        .sessions
        .iter()
        .map(|sess| json!({ "name": sess.name, "addr": sess.addr, "paired_s": sess.paired_s, "last_seen_s": sess.last_seen_s, "idle_s": (t - sess.last_seen_s).max(0.0) }))
        .collect();
    json!({
        "enabled": r.state.enabled,
        "port": r.settings.port,
        "bound_port": port,
        "code": if r.state.enabled { r.state.code.as_str() } else { "" },
        "urls": urls,
        "sessions": sessions,
        "max": MAX_REMOTES,
        "blocked_s": r.state.blocked_until.map(|u| u.saturating_duration_since(now).as_secs_f64()).filter(|&b| b > 0.0),
        "gate_open": s.external.gate.open,
    })
}

/// The addresses a tablet can open: this Mac's IPv4 addresses on the
/// local network when listening on every interface, else the one bound.
fn local_addresses(bind: IpAddr) -> Vec<IpAddr> {
    if !bind.is_unspecified() {
        return vec![bind];
    }
    let mut list: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.ip())
        .filter(|ip| ip.is_ipv4() && !ip.is_loopback() && !ip.is_unspecified())
        .collect();
    list.sort();
    list.dedup();
    list
}

// ---------------------------------------------------------------------
// The remote's own server.

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;

fn serve(server: Server, shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) {
    let busy = Arc::new(AtomicUsize::new(0));
    while running.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(request)) => aside(request, &shared, &busy),
            Ok(None) => {}
            Err(e) => log::debug!("télécommande : {e}"),
        }
        sweep(&mut shared.lock().unwrap(), Instant::now());
    }
    // Dropping the server closes the port.
}

/// Each request on its own short thread, a bounded number: a slow client
/// never holds up another tablet's stop.
fn aside(request: Request, shared: &Arc<Mutex<Shared>>, busy: &Arc<AtomicUsize>) {
    if busy.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
        busy.fetch_sub(1, Ordering::SeqCst);
        let _ = request.respond(Reply::text(503, "occupé, réessayez").into_response());
        return;
    }
    let (shared, busy2) = (Arc::clone(shared), Arc::clone(busy));
    let spawned = std::thread::Builder::new().name("remote-request".into()).stack_size(256 * 1024).spawn(move || {
        let mut request = request;
        let response = route(&mut request, &shared);
        if let Err(e) = request.respond(response) {
            log::debug!("télécommande : réponse non envoyée : {e}");
        }
        busy2.fetch_sub(1, Ordering::SeqCst);
    });
    if let Err(e) = spawned {
        busy.fetch_sub(1, Ordering::SeqCst);
        log::warn!("télécommande : pas de thread pour une requête : {e}");
    }
}

/// The remote's router: reads the request (bounded), then `handle`.
pub fn route(request: &mut Request, shared: &Arc<Mutex<Shared>>) -> HttpResponse {
    let method = request.method().as_str().to_string();
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let cookie = request.headers().iter().find(|h| h.field.equiv("Cookie")).map(|h| h.value.as_str().to_string());
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
    let reply = handle(&mut shared.lock().unwrap(), &method, &path, cookie.as_deref(), &peer, &body, Instant::now());
    reply.into_response()
}

/// What `handle` answers, before it becomes an HTTP response.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub content_type: &'static str,
    pub set_cookie: Option<String>,
}

impl Reply {
    fn text(status: u16, body: &str) -> Self {
        Reply { status, body: body.to_string(), content_type: "text/plain; charset=utf-8", set_cookie: None }
    }

    fn json(status: u16, v: Value) -> Self {
        Reply { status, body: v.to_string(), content_type: "application/json", set_cookie: None }
    }

    fn into_response(self) -> HttpResponse {
        let h = |k: &str, v: &str| Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header");
        let mut r = Response::from_string(self.body)
            .with_status_code(self.status)
            .with_header(h("Content-Type", self.content_type))
            .with_header(h("Cache-Control", "no-store"))
            .with_header(h("X-Content-Type-Options", "nosniff"))
            .with_header(h("X-Frame-Options", "DENY"))
            .with_header(h("Referrer-Policy", "no-referrer"))
            .with_header(h("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; form-action 'none'"));
        if let Some(c) = self.set_cookie {
            r = r.with_header(h("Set-Cookie", &c));
        }
        r
    }
}

/// The whitelist. `cookie`: the `Cookie` header; `peer`: the client's IP.
pub fn handle(s: &mut Shared, method: &str, path: &str, cookie: Option<&str>, peer: &str, body: &[u8], now: Instant) -> Reply {
    if !s.remote.state.enabled {
        return Reply::text(503, "télécommande désactivée sur l'écran principal");
    }
    // Whitelist, never a blacklist: any route of the main UI is refused,
    // a route added there tomorrow included.
    if !WHITELIST.iter().any(|&(m, p, _)| m == method && p == path) {
        return if crate::web::is_known_route(method, path) { Reply::text(403, "interdit depuis la télécommande") } else { Reply::text(404, "introuvable") };
    }
    let session = cookie.and_then(token_from_cookie).and_then(|t| find_session(&s.remote.state.sessions, t));
    match (method, path) {
        // The page holds no data: unpaired, it only shows the pairing form.
        ("GET", "/") => Reply { status: 200, body: REMOTE_HTML.to_string(), content_type: "text/html; charset=utf-8", set_cookie: None },
        ("POST", "/pair") => pair(s, body, peer, now),
        ("GET", "/api/remote/state") | ("POST", "/api/remote/control") | ("GET", ALIGN_PATH) | ("POST", NUDGE_PATH) | ("POST", RESET_PATH) => {
            let Some(i) = session else {
                return Reply::text(401, "non appairé");
            };
            let t = s.now_s();
            let sess = &mut s.remote.state.sessions[i];
            sess.seen = Some(now);
            sess.last_seen_s = t;
            sess.addr = peer.to_string();
            match (method, path) {
                ("GET", "/api/remote/state") => Reply::json(200, state_view(s, i)),
                ("POST", "/api/remote/control") => control(s, i, body, now),
                ("GET", _) => {
                    align_sync(s, now);
                    Reply::json(200, align_remote_view(s, i))
                }
                (_, NUDGE_PATH) => nudge(s, i, body, now),
                _ => reset_draft(s, i, now),
            }
        }
        // A `WHITELIST` line without its arm here.
        _ => Reply::text(404, "introuvable"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRequest {
    code: String,
    #[serde(default)]
    name: String,
}

fn pair(s: &mut Shared, body: &[u8], peer: &str, now: Instant) -> Reply {
    let st = &mut s.remote.state;
    if let Some(until) = st.blocked_until {
        if now < until {
            let left = until.saturating_duration_since(now).as_secs() + 1;
            return Reply::text(429, &format!("Trop de codes faux : réessayez dans {left} s avec le nouveau code"));
        }
        st.blocked_until = None;
    }
    let req: PairRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => return Reply::text(400, "requête invalide"),
    };
    if st.sessions.len() >= MAX_REMOTES {
        return Reply::text(409, &format!("{MAX_REMOTES} télécommandes au plus : déconnectez-en une sur l'écran principal"));
    }
    let code = req.code.trim();
    if st.code.len() != CODE_LEN || !ct_eq(code.as_bytes(), st.code.as_bytes()) {
        st.failed += 1;
        if st.failed >= MAX_PAIR_ATTEMPTS {
            st.failed = 0;
            st.blocked_until = Some(now + PAIR_BLOCK);
            // An empty code never matches (length check above).
            st.code = new_code().unwrap_or_default();
            log::warn!("Télécommande : {MAX_PAIR_ATTEMPTS} codes faux, nouveau code d'appairage, essais bloqués 30 s");
            s.gate.log().record("remote_pair_blocked", Some(crate::interlock::ArmSource::Remote.id()), json!({ "addr": peer }));
        }
        return Reply::text(403, "Code incorrect");
    }
    let token = match new_token() {
        Ok(t) => t,
        Err(e) => return Reply::text(500, &e),
    };
    st.failed = 0;
    let name = clean_name(&req.name, &st.sessions);
    let t = s.now_s();
    let st = &mut s.remote.state;
    st.sessions.push(RemoteSession { token: token.clone(), name: name.clone(), addr: peer.to_string(), paired_s: t, last_seen_s: t, seen: Some(now), held: Vec::new() });
    log::info!("Télécommande appairée : {name} ({peer})");
    s.gate.log().record("remote_paired", Some(crate::interlock::ArmSource::Remote.id()), json!({ "name": name, "addr": peer }));
    let mut r = Reply::json(200, json!({ "name": name }));
    // A session cookie: gone when the tablet's browser closes.
    r.set_cookie = Some(format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/"));
    r
}

/// Printable, at most `MAX_NAME` characters, unique among the remotes.
fn clean_name(raw: &str, sessions: &[RemoteSession]) -> String {
    let base: String = raw.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(MAX_NAME).collect();
    let base = if base.is_empty() { format!("Tablette {}", sessions.len() + 1) } else { base };
    let mut name = base.clone();
    let mut n = 2;
    while sessions.iter().any(|s| s.name == name) {
        name = format!("{} {n}", base.chars().take(MAX_NAME - 3).collect::<String>());
        n += 1;
    }
    name
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlBody {
    id: String,
    value: Option<f64>,
    norm: Option<f64>,
    down: Option<bool>,
}

fn control(s: &mut Shared, i: usize, body: &[u8], now: Instant) -> Reply {
    let req: ControlBody = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => return Reply::text(400, "requête invalide"),
    };
    if !in_scope(&req.id) {
        return Reply::text(403, "hors du périmètre de la télécommande");
    }
    let input = match (req.value, req.norm, req.down) {
        (Some(v), None, None) if v.is_finite() && v.abs() <= 1e6 => ControlInput::Value(v as f32),
        (None, Some(n), None) if n.is_finite() && (0.0..=1.0).contains(&n) => ControlInput::Norm(n as f32),
        (None, None, Some(d)) => ControlInput::Value(if d { 1.0 } else { 0.0 }),
        (None, None, None) => ControlInput::Value(1.0),
        _ => return Reply::text(400, "valeur invalide"),
    };
    let momentary = s.controls.get(&req.id).is_some_and(|d| d.kind == ControlKind::Momentary);
    let down = matches!(input, ControlInput::Value(v) | ControlInput::Norm(v) if v >= 0.5);
    let held = &s.remote.state.sessions[i].held;
    if momentary && down && !held.contains(&req.id) && held.len() >= MAX_HELD {
        return Reply::text(429, "trop de pads maintenus");
    }
    let name = s.remote.state.sessions[i].name.clone();
    // Pairing is this source's access, renewed by every request.
    external::set_access(s, SourceKind::Remote, &name, true, now);
    match external::apply(s, SourceKind::Remote, &name, &req.id, input, now) {
        Ok(()) => {
            if momentary {
                let held = &mut s.remote.state.sessions[i].held;
                held.retain(|h| *h != req.id);
                if down {
                    held.push(req.id.clone());
                }
            }
            Reply::json(200, json!({ "ok": true }))
        }
        Err(Refusal::Unknown) => Reply::text(404, Refusal::Unknown.label_fr()),
        Err(r @ Refusal::GateClosed) => Reply::text(409, r.label_fr()),
        Err(r) => Reply::text(403, r.label_fr()),
    }
}

/// The masters shown as sliders: (id, label on the tablet).
const MASTERS: [(&str, &str); 3] = [("master.brightness", "Luminosité"), ("master.size", "Taille"), ("master.speed", "Vitesse")];

/// `GET /api/remote/state`: read only.
fn state_view(s: &Shared, i: usize) -> Value {
    let grid = &s.grid;
    let pages = grid.view();
    let page = s.cue_page.min(pages.len().saturating_sub(1));
    let cols = (grid.cols as usize).max(1);
    let name_of = |id: &str| s.presets.iter().find(|p| p.id == id).map(|p| p.name.clone());
    let cells: Vec<Value> = pages
        .get(page)
        .map(|p| {
            p.cells
                .iter()
                .enumerate()
                .map(|(n, cue)| {
                    let Some(cue) = cue.as_deref() else { return Value::Null };
                    let id = format!("grid.{}.{}.{}", page + 1, n / cols + 1, n % cols + 1);
                    if s.controls.get(&id).is_none() {
                        return Value::Null;
                    }
                    json!({ "id": id, "name": name_of(cue).unwrap_or_default(), "active": s.deck.active.iter().any(|a| a.cue == cue) })
                })
                .collect()
        })
        .unwrap_or_default();
    let masters: Vec<Value> = MASTERS
        .iter()
        .filter_map(|&(id, label)| {
            let d = s.controls.get(id)?;
            let ControlKind::Continuous { min, max, default, .. } = d.kind else { return None };
            Some(json!({ "id": id, "label": label, "min": min, "max": max, "default": default, "value": crate::controls::current(s, d) }))
        })
        .collect();
    let bpm = s.controls.get("tempo.bpm").and_then(|d| crate::controls::current(s, d));
    let active: Vec<String> = s.deck.active.iter().filter_map(|a| name_of(&a.cue)).collect();
    json!({
        "name": s.remote.state.sessions[i].name,
        "armed": s.gate.is_armed() && !s.estop.is_latched(),
        "estop": s.estop.is_latched(),
        "gate_open": s.external.gate.open,
        "page": page,
        "pages": pages.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "cols": cols,
        "cells": cells,
        "masters": masters,
        "bpm": bpm,
        "active": active,
        // An alignment session open for this remote (T-460).
        "align": s.remote.align.as_ref().is_some_and(|a| a.remote == s.remote.state.sessions[i].name),
    })
}

// ---------------------------------------------------------------------
// Zone alignment from a phone (T-460).

/// One step, in output units.
pub const NUDGE_STEP: f32 = 0.002;
/// How far a draft corner may go from the saved one (beyond: the main
/// screen).
pub const MAX_DRAFT_OFFSET: f32 = 0.2;
/// The session ends after this long without a request from its remote
/// (the page polls twice a second).
pub const ALIGN_TIMEOUT: Duration = Duration::from_secs(10);
/// « Pas ×5 »: the largest step multiple a nudge may carry.
pub const MAX_NUDGE: u8 = 5;
/// Rounding slack on the bounds.
const ALIGN_EPS: f32 = 1e-4;
/// The corners in warp order (`projection::SQUARE`), as the phone names
/// them.
pub const CORNERS_FR: [&str; 4] = ["Bas gauche", "Bas droit", "Haut droit", "Haut gauche"];
/// What a remote reads while no session is open for it.
pub const ASK_FR: &str = "Demandez l'ouverture de l'alignement sur l'écran principal";

/// A zone alignment session opened from the main screen (T-460). Never
/// saved.
#[derive(Clone, Debug)]
pub struct AlignSession {
    pub zone: u8,
    /// The remote's name (unique among the paired remotes).
    pub remote: String,
    /// The proposed corners, in warp order.
    pub draft: [[f32; 2]; 4],
    /// The saved zone's effective corners when the session opened.
    pub base: [[f32; 2]; 4],
    pub opened_at: Instant,
    pub opened_s: f64,
    pub nudges: u32,
    /// The zone's whole geometry when the session opened: an edit of the
    /// zone on the main screen ends the session.
    geometry: Geometry,
    /// The session showed the Grid pattern itself (none was on): it
    /// leaves it at the end.
    own_pattern: bool,
}

/// The corners a geometry's warp really draws: its own when on, else the
/// field's.
pub fn effective_corners(g: &Geometry) -> [[f32; 2]; 4] {
    if g.warp.enabled {
        g.warp.corners
    } else {
        SQUARE
    }
}

/// One nudge of `corner` by (dx, dy) steps, or why it is refused
/// (French): out of -1..1, farther than `MAX_DRAFT_OFFSET` from `base`, or
/// a quad that isn't convex.
pub fn step_draft(draft: &[[f32; 2]; 4], base: &[[f32; 2]; 4], corner: usize, dx: i8, dy: i8) -> Result<[[f32; 2]; 4], String> {
    if corner > 3 || dx.unsigned_abs() > MAX_NUDGE || dy.unsigned_abs() > MAX_NUDGE {
        return Err("pas invalide".into());
    }
    let p = [draft[corner][0] + NUDGE_STEP * dx as f32, draft[corner][1] + NUDGE_STEP * dy as f32];
    if !p.iter().all(|v| v.is_finite() && v.abs() <= 1.0 + ALIGN_EPS) {
        return Err("le coin sortirait du champ (-1..1)".into());
    }
    let p = p.map(|v| v.clamp(-1.0, 1.0));
    let off = (p[0] - base[corner][0]).hypot(p[1] - base[corner][1]);
    if off > MAX_DRAFT_OFFSET + ALIGN_EPS {
        return Err("0,2 au plus depuis la zone enregistrée : continuez sur l'écran principal".into());
    }
    let mut d = *draft;
    d[corner] = p;
    check_quad(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

/// `POST /api/zones/align/open` (main screen only): a session for `zone`
/// and the paired remote `remote`. Replaces a session already open. The
/// Grid pattern is shown if no pattern is. Never arms. Returns the HTTP
/// status and the message to show when refused.
pub fn align_open(s: &mut Shared, zone: u8, remote: &str, now: Instant) -> Result<(), (u16, String)> {
    if !s.remote.state.enabled {
        return Err((409, "Autorisez d'abord la télécommande (RÉGLAGES › Télécommande)".into()));
    }
    if !s.remote.state.sessions.iter().any(|x| x.name == remote) {
        return Err((404, format!("télécommande inconnue : {remote}")));
    }
    if s.estop.is_latched() {
        return Err((409, "Arrêt d'urgence en cours : réinitialisez-le d'abord".into()));
    }
    let Some(z) = s.projection.get().zone(zone) else {
        return Err((404, "zone inconnue".into()));
    };
    let (geometry, name) = (z.geometry, z.name.clone());
    let base = effective_corners(&geometry);
    align_end(s, "replaced");
    let own_pattern = s.test_pattern.active.is_none();
    if own_pattern {
        crate::testpattern::select(s, Some(crate::testpattern::TestPattern::Grid));
    }
    let opened_s = s.now_s();
    s.remote.align = Some(AlignSession { zone, remote: remote.to_string(), draft: base, base, opened_at: now, opened_s, nudges: 0, geometry, own_pattern });
    log::info!("Alignement de la zone {zone} ({name}) ouvert pour la télécommande {remote}");
    s.gate.log().record("remote_align", Some(crate::interlock::ArmSource::Ui.id()), json!({ "event": "open", "zone": zone, "name": name, "remote": remote }));
    Ok(())
}

/// Ends the session, if any: the draft is dropped, the output goes back
/// to the saved zone (and leaves the pattern the session showed).
/// Returns whether one was open.
pub fn align_end(s: &mut Shared, reason: &str) -> bool {
    let Some(a) = s.remote.align.take() else { return false };
    if a.own_pattern && s.test_pattern.active == Some(crate::testpattern::TestPattern::Grid) {
        crate::testpattern::select(s, None);
    }
    log::info!("Alignement de la zone {} terminé ({reason})", a.zone);
    s.gate.log().record("remote_align", Some(crate::interlock::ArmSource::Ui.id()), json!({ "event": "end", "reason": reason, "zone": a.zone, "remote": a.remote, "nudges": a.nudges }));
    true
}

/// `POST /api/zones/align/apply` (main screen, loopback only): writes the
/// draft as the zone's 4-corner warp, logs it, ends the session.
pub fn align_apply(s: &mut Shared) -> Result<(), (u16, String)> {
    let Some(a) = s.remote.align.clone() else {
        return Err((409, "aucun alignement en cours".into()));
    };
    let mut cfg = s.projection.get().clone();
    let Some(z) = cfg.zones.iter_mut().find(|z| z.id == a.zone) else {
        align_end(s, "zone_gone");
        return Err((404, "zone inconnue".into()));
    };
    if z.geometry != a.geometry {
        return Err((409, "la zone a changé entre-temps : rejetez et recommencez".into()));
    }
    if a.draft != a.base {
        z.geometry.warp = Warp { enabled: true, corners: a.draft };
        s.projection.set(cfg).map_err(|e| (400, e.to_string()))?;
    }
    log::warn!("Zone {} alignée depuis la télécommande {} : {:?} → {:?}", a.zone, a.remote, a.base, a.draft);
    s.gate.log().record("remote_align", Some(crate::interlock::ArmSource::Ui.id()), json!({ "event": "applied", "zone": a.zone, "remote": a.remote, "before": a.base, "after": a.draft }));
    align_end(s, "applied");
    Ok(())
}

/// Ends the session when its conditions go: the e-stop, the remote
/// access off, show mode, the remote gone or quiet for `ALIGN_TIMEOUT`,
/// the zone deleted or edited. The engine calls it every frame.
pub fn align_sync(s: &mut Shared, now: Instant) {
    let Some(a) = &s.remote.align else { return };
    let zone = s.projection.get().zone(a.zone);
    let reason = if s.estop.is_latched() {
        "estop"
    } else if !s.remote.state.enabled {
        "remote_off"
    } else if s.show_mode.on {
        "show_mode"
    } else if zone.is_none() {
        "zone_gone"
    } else if zone.is_some_and(|z| z.geometry != a.geometry) {
        "zone_changed"
    } else {
        match s.remote.state.sessions.iter().find(|x| x.name == a.remote) {
            None => "remote_lost",
            Some(x) if x.seen.is_none_or(|t| now.saturating_duration_since(t) > ALIGN_TIMEOUT) => "heartbeat",
            Some(_) => return,
        }
    };
    align_end(s, reason);
}

/// The geometry the test pattern goes through while a session is open:
/// the zone's, with the draft as its 4-corner warp.
pub fn align_geometry(s: &Shared) -> Option<Geometry> {
    let a = s.remote.align.as_ref()?;
    let mut g = s.projection.get().zone(a.zone)?.geometry;
    g.warp = Warp { enabled: true, corners: a.draft };
    Some(g)
}

/// `/api/state.align` (main screen): the session, or null.
pub fn align_view(s: &Shared) -> Value {
    let Some(a) = &s.remote.align else { return Value::Null };
    let name = s.projection.get().zone(a.zone).map(|z| z.name.clone()).unwrap_or_default();
    json!({
        "zone": a.zone,
        "zone_name": name,
        "remote": a.remote,
        "draft": a.draft,
        "base": a.base,
        "changed": a.draft != a.base,
        "nudges": a.nudges,
        "opened_s": a.opened_s,
        "corners": CORNERS_FR,
    })
}

/// `GET /api/remote/align`: this remote's session, read only.
fn align_remote_view(s: &Shared, i: usize) -> Value {
    let me = &s.remote.state.sessions[i].name;
    let mine = s.remote.align.as_ref().filter(|a| a.remote == *me);
    let zone = mine.and_then(|a| s.projection.get().zone(a.zone)).map(|z| json!({ "id": z.id, "name": z.name }));
    json!({
        "open": mine.is_some(),
        "other": s.remote.align.is_some() && mine.is_none(),
        "message": if mine.is_some() { "En attente de confirmation sur l'écran principal" } else { ASK_FR },
        "estop": s.estop.is_latched(),
        "zone": zone,
        "draft": mine.map(|a| a.draft),
        "base": mine.map(|a| a.base),
        "changed": mine.is_some_and(|a| a.draft != a.base),
        "corners": CORNERS_FR,
        "step": NUDGE_STEP,
        "max_nudge": MAX_NUDGE,
        "max_offset": MAX_DRAFT_OFFSET,
    })
}

/// The session open for remote `i`, or the refusal to send it.
fn my_session(s: &mut Shared, i: usize, now: Instant) -> Result<(), Reply> {
    align_sync(s, now);
    let me = &s.remote.state.sessions[i].name;
    match &s.remote.align {
        Some(a) if a.remote == *me => {}
        Some(_) => return Err(Reply::text(409, "L'alignement est ouvert pour une autre télécommande")),
        None => return Err(Reply::text(409, ASK_FR)),
    }
    // Moving the output's geometry is a safety edit: never in show mode
    // nor under the safety lock (T-261), whatever the session.
    if s.show_mode.on {
        return Err(Reply::text(423, crate::showmode::LOCKED_FR));
    }
    if crate::safetylock::check(s, now) {
        return Err(Reply::text(423, crate::safetylock::LOCKED_FR));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NudgeBody {
    corner: u8,
    dx: i8,
    dy: i8,
}

/// `POST /api/remote/align/nudge {corner, dx, dy}`: moves the draft only.
fn nudge(s: &mut Shared, i: usize, body: &[u8], now: Instant) -> Reply {
    let req: NudgeBody = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => return Reply::text(400, "requête invalide"),
    };
    if req.corner > 3 || req.dx.unsigned_abs() > MAX_NUDGE || req.dy.unsigned_abs() > MAX_NUDGE || (req.dx == 0 && req.dy == 0) {
        return Reply::text(400, "pas invalide");
    }
    if let Err(r) = my_session(s, i, now) {
        return r;
    }
    let Some(a) = s.remote.align.as_mut() else { return Reply::text(409, ASK_FR) };
    match step_draft(&a.draft, &a.base, req.corner as usize, req.dx, req.dy) {
        Ok(d) => {
            a.draft = d;
            a.nudges = a.nudges.saturating_add(1);
            Reply::json(200, align_remote_view(s, i))
        }
        Err(e) => Reply::text(409, &format!("Limite atteinte : {e}")),
    }
}

/// `POST /api/remote/align/reset`: « Annuler mes déplacements ». Never
/// locked: it only goes back to the saved zone.
fn reset_draft(s: &mut Shared, i: usize, now: Instant) -> Reply {
    if let Err(r) = my_session(s, i, now) {
        if r.status != 423 {
            return r;
        }
    }
    let Some(a) = s.remote.align.as_mut() else { return Reply::text(409, ASK_FR) };
    a.draft = a.base;
    Reply::json(200, align_remote_view(s, i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::ArmSource;
    use crate::test_support::shared;

    fn on(s: &mut Shared) {
        s.remote.state = RemoteState { enabled: true, code: "123456".into(), ..Default::default() };
    }

    fn post(s: &mut Shared, path: &str, cookie: Option<&str>, body: Value, now: Instant) -> Reply {
        handle(s, "POST", path, cookie, "10.0.0.7", body.to_string().as_bytes(), now)
    }

    /// Pairs and returns the `Cookie` header to send back.
    fn paired(s: &mut Shared, name: &str) -> String {
        let r = post(s, "/pair", None, json!({ "code": s.remote.state.code.clone(), "name": name }), Instant::now());
        assert_eq!(r.status, 200, "{}", r.body);
        let c = r.set_cookie.unwrap();
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict"), "{c}");
        c.split(';').next().unwrap().to_string()
    }

    fn open_gate(s: &mut Shared) {
        external::set_open(s, true, ArmSource::Ui).unwrap();
    }

    #[test]
    fn scope_is_a_whitelist() {
        for id in ["grid.1.1.1", "page.2", "page.next", "cue.stop_all", "master.brightness", "layer.3.dimmer", "tempo.tap", "tempo.bpm", "tempo.resync", "transport.blackout", "safety.estop"] {
            assert!(in_scope(id), "{id}");
        }
        for id in ["transport.arm", "safety.hold", "timeline.play", "layer.1.mute", "layer.1.clear", "layer..dimmer", "layer.x.dimmer", "grid.", "master.", "cue.mode", "tempo.auto", "running.stop_all", "fx.1.1", "list.go", ""] {
            assert!(!in_scope(id), "{id}");
        }
        // Every registry id: nothing in scope that is not an external
        // control, and never arming or the hold-to-run pad.
        let s = shared();
        for d in s.controls.list() {
            if in_scope(&d.id) {
                assert!(d.external, "{} in scope but not external", d.id);
                assert!(!external::forbidden(d) || external::ALWAYS.contains(&d.id.as_str()), "{}", d.id);
            }
        }
        assert!(s.controls.list().iter().any(|d| d.id.starts_with("grid.") && in_scope(&d.id)));
    }

    #[test]
    fn off_by_default_and_the_port_alone_is_saved() {
        let s = shared();
        assert!(!s.remote.state.enabled && s.remote.state.sessions.is_empty() && s.remote.bound_port().is_none());
        let saved = serde_json::to_string(&s.remote.settings).unwrap();
        assert_eq!(saved, r#"{"port":8091}"#);
        let back: RemoteSettings = serde_json::from_str(r#"{"port":9000,"enabled":true}"#).unwrap();
        assert_eq!(back.port, 9000, "an `enabled` key in the file is ignored");
        // A fresh load is off whatever the file says.
        let dir = crate::datafile::tests::temp_dir("remote");
        let path = dir.join("remote.json");
        std::fs::write(&path, r#"{"port":9001,"enabled":true}"#).unwrap();
        let r = Remote::load(path, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(!r.state.enabled && r.settings.port == 9001);
        let mut s = shared();
        assert_eq!(handle(&mut s, "GET", "/", None, "x", b"", Instant::now()).status, 503);
    }

    #[test]
    fn codes_are_six_digits_and_tokens_long() {
        for _ in 0..50 {
            let c = new_code().unwrap();
            assert!(c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit()), "{c}");
        }
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(ct_eq(b"abc", b"abc") && !ct_eq(b"abc", b"abd") && !ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn unpaired_clients_get_the_page_and_nothing_else() {
        let mut s = shared();
        on(&mut s);
        let now = Instant::now();
        let page = handle(&mut s, "GET", "/", None, "x", b"", now);
        assert_eq!(page.status, 200);
        assert!(page.body.contains("Code d'appairage"));
        assert_eq!(handle(&mut s, "GET", "/api/remote/state", None, "x", b"", now).status, 401);
        assert_eq!(post(&mut s, "/api/remote/control", None, json!({ "id": "safety.estop" }), now).status, 401);
        assert_eq!(post(&mut s, "/api/remote/control", Some("ls_remote=forged"), json!({ "id": "master.size", "value": 0.5 }), now).status, 401);
        assert_eq!(handle(&mut s, "GET", "/api/state", None, "x", b"", now).status, 403);
        assert_eq!(handle(&mut s, "GET", "/../../etc/passwd", None, "x", b"", now).status, 404);
        assert!(!s.estop.is_latched());
    }

    #[test]
    fn wrong_codes_block_and_draw_a_new_code() {
        let mut s = shared();
        on(&mut s);
        let t0 = Instant::now();
        for n in 1..=MAX_PAIR_ATTEMPTS {
            let r = post(&mut s, "/pair", None, json!({ "code": "000000", "name": "x" }), t0);
            assert_eq!(r.status, 403, "{n}");
            assert!(r.set_cookie.is_none());
        }
        assert_ne!(s.remote.state.code, "123456", "a new code after 5 wrong ones");
        assert_eq!(s.remote.state.code.len(), 6);
        let code = s.remote.state.code.clone();
        // Blocked for 30 s, even with the right code.
        assert_eq!(post(&mut s, "/pair", None, json!({ "code": code }), t0 + Duration::from_secs(29)).status, 429);
        assert!(s.remote.state.sessions.is_empty());
        // Then the new code pairs; the old one never does.
        let later = t0 + Duration::from_secs(31);
        assert_eq!(post(&mut s, "/pair", None, json!({ "code": "123456" }), later).status, 403);
        let r = post(&mut s, "/pair", None, json!({ "code": code, "name": "  Régie\u{7}  " }), later);
        assert_eq!(r.status, 200);
        assert_eq!(s.remote.state.sessions[0].name, "Régie");
        assert_eq!(s.remote.state.failed, 0);
        // Bad bodies.
        for bad in ["", "{", r#"{"code":1}"#, r#"{"code":"123456","extra":1}"#] {
            assert_eq!(handle(&mut s, "POST", "/pair", None, "x", bad.as_bytes(), later).status, 400, "{bad}");
        }
    }

    #[test]
    fn at_most_four_remotes_with_unique_names() {
        let mut s = shared();
        on(&mut s);
        for _ in 0..MAX_REMOTES {
            paired(&mut s, "Tab");
        }
        let names: Vec<_> = s.remote.state.sessions.iter().map(|x| x.name.clone()).collect();
        assert_eq!(names, ["Tab", "Tab 2", "Tab 3", "Tab 4"]);
        let code = s.remote.state.code.clone();
        assert_eq!(post(&mut s, "/pair", None, json!({ "code": code }), Instant::now()).status, 409);
    }

    #[test]
    fn a_paired_remote_plays_a_cue_and_sets_a_master() {
        let mut s = shared();
        on(&mut s);
        open_gate(&mut s);
        let c = paired(&mut s, "Tablette");
        let now = Instant::now();
        let st: Value = serde_json::from_str(&handle(&mut s, "GET", "/api/remote/state", Some(&c), "x", b"", now).body).unwrap();
        let cell = st["cells"].as_array().unwrap().iter().find(|c| !c.is_null()).unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(st["armed"], false);
        assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": cell, "down": true }), now).status, 200);
        assert!(!s.deck.active.is_empty(), "the cue plays");
        assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": cell, "down": false }), now).status, 200);
        assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": "master.brightness", "norm": 0.25 }), now).status, 200);
        assert!((s.live.brightness - 0.25).abs() < 1e-6);
        // Out of scope, unknown, bad values.
        for (body, code) in [
            (json!({ "id": "transport.arm", "value": 1 }), 403),
            (json!({ "id": "timeline.play" }), 403),
            (json!({ "id": "layer.1.mute", "value": 1 }), 403),
            (json!({ "id": "grid.99.1.1", "down": true }), 404),
            (json!({ "id": "master.size", "norm": 2.0 }), 400),
            (json!({ "id": "master.size", "value": 1, "norm": 0.5 }), 400),
            (json!({ "id": "master.size", "value": 1e9 }), 400),
            (json!({ "id": "x".repeat(200) }), 403),
            (json!({ "id": "master.size", "evil": 1 }), 400),
        ] {
            assert_eq!(post(&mut s, "/api/remote/control", Some(&c), body.clone(), now).status, code, "{body}");
        }
        assert!(!s.gate.is_armed());
        assert_eq!(s.external.gate.accepted.get(&SourceKind::Remote).copied(), Some(3));
    }

    #[test]
    fn the_closed_gate_cuts_the_remote_except_the_stop() {
        let mut s = shared();
        on(&mut s);
        let c = paired(&mut s, "T");
        let now = Instant::now();
        assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": "master.size", "value": 0.5 }), now).status, 409);
        assert_eq!(s.live.size, 1.0);
        assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": "safety.estop" }), now).status, 200);
        assert!(s.estop.is_latched());
        assert_eq!(s.gate.status(&s.estop).estop.unwrap().source, ArmSource::Remote);
    }

    /// The remote can stop, never arm or reset: the stop latches, and
    /// arming, resetting, safety, heartbeat, outputs, calibration and the
    /// project are all 403 here.
    #[test]
    fn the_remote_stops_but_never_arms_or_resets() {
        let mut s = shared();
        on(&mut s);
        open_gate(&mut s);
        let c = paired(&mut s, "T");
        let now = Instant::now();
        let (safety, calibration) = (s.safety.get(), s.calibration);
        assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": "transport.blackout" }), now).status, 200);
        assert!(s.estop.is_latched());
        for path in ["/api/arm", "/api/estop/reset", "/api/estop", "/api/safety", "/api/safety/profiles/load", "/api/heartbeat", "/api/presence", "/api/calibration", "/api/outputs/select", "/api/project/open", "/api/external/gate", "/api/remote", "/api/remote/kick", "/api/control", "/api/test/output"] {
            let r = post(&mut s, path, Some(&c), json!({ "on": true, "source": "ui" }), now);
            assert_eq!(r.status, 403, "{path}");
        }
        for id in ["transport.arm", "safety.hold"] {
            assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": id, "value": 1 }), now).status, 403);
        }
        assert!(s.estop.is_latched() && !s.gate.is_armed());
        assert_eq!(s.safety.get(), safety);
        assert_eq!(s.calibration, calibration);
    }

    #[test]
    fn held_pads_are_released_on_silence_kick_and_disable() {
        for how in ["silence", "kick", "disable"] {
            let mut s = shared();
            on(&mut s);
            open_gate(&mut s);
            s.deck.click_mode = crate::cues::ClickMode::Flash;
            let c = paired(&mut s, "T");
            let cell = crate::controls::pad_cell(&s.grid, 0, 0).unwrap();
            let t0 = Instant::now();
            assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": cell, "down": true }), t0).status, 200);
            assert!(s.deck.active.iter().any(|a| a.held), "{how}: held");
            match how {
                "silence" => {
                    sweep(&mut s, t0 + Duration::from_millis(500));
                    assert!(s.deck.active.iter().any(|a| a.held), "still polled");
                    sweep(&mut s, t0 + HOLD_TIMEOUT + Duration::from_millis(100));
                }
                "kick" => assert_eq!(kick(&mut s, Some("T")), 1),
                _ => disable(&mut s, "test"),
            }
            assert!(!s.deck.active.iter().any(|a| a.held), "{how}: released");
            if how != "silence" {
                assert!(s.remote.state.sessions.is_empty());
                let r = handle(&mut s, "GET", "/api/remote/state", Some(&c), "x", b"", t0);
                assert!(r.status == 401 || r.status == 503, "{how}: the token is revoked");
            }
            assert!(!s.gate.is_armed());
        }
    }

    /// Every route of the main UI's table, from a paired remote: 403
    /// (only `GET /` is served, the remote's own page).
    #[test]
    fn every_main_route_is_forbidden_to_a_paired_remote() {
        let mut s = shared();
        on(&mut s);
        let c = paired(&mut s, "T");
        let now = Instant::now();
        for &(method, path, _) in crate::web::ROUTES {
            let path = path.strip_suffix('*').map_or(path.to_string(), |p| format!("{p}x"));
            for m in if method == "*" { vec!["GET", "POST"] } else { vec![method] } {
                let r = handle(&mut s, m, &path, Some(&c), "x", b"{}", now);
                if (m, path.as_str()) == ("GET", "/") {
                    assert!(r.status == 200 && r.body.contains("Télécommande"), "the remote's own page");
                } else {
                    assert_eq!(r.status, 403, "{m} {path}");
                }
            }
        }
        assert!(!s.gate.is_armed() && !s.estop.is_latched());
    }

    /// Over HTTP on 127.0.0.1: turning it on opens a port, pairing sets a
    /// cookie, turning it off closes the port and revokes the token.
    #[test]
    fn the_listener_opens_and_closes_on_loopback() {
        use std::io::Write;
        let shared = Arc::new(Mutex::new(shared()));
        set_enabled(&shared, true, Some(0)).unwrap();
        let (port, code) = {
            let s = shared.lock().unwrap();
            assert!(s.remote.state.enabled);
            (s.remote.bound_port().unwrap(), s.remote.state.code.clone())
        };
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let http = |method: &str, path: &str, cookie: &str, body: &str| -> (u16, String) {
            let mut c = std::net::TcpStream::connect(addr).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let cookie = if cookie.is_empty() { String::new() } else { format!("Cookie: {cookie}\r\n") };
            write!(c, "{method} {path} HTTP/1.1\r\nHost: x\r\n{cookie}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            let mut raw = String::new();
            let _ = c.read_to_string(&mut raw);
            (raw.get(9..12).and_then(|c| c.parse().ok()).unwrap_or(0), raw)
        };
        let (st, raw) = http("POST", "/pair", "", &json!({ "code": code, "name": "iPad" }).to_string());
        assert_eq!(st, 200, "{raw}");
        let cookie = raw.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
        assert!(!raw.contains(&format!("\"{}\"", &cookie[COOKIE.len() + 1..])), "the token is only in the cookie");
        assert_eq!(http("GET", "/api/remote/state", &cookie, "").0, 200);
        assert_eq!(http("POST", "/api/arm", &cookie, r#"{"on":true}"#).0, 403);
        assert_eq!(http("POST", "/api/remote/control", &cookie, &"x".repeat(MAX_BODY + 1)).0, 413);
        set_enabled(&shared, false, None).unwrap();
        assert!(std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err(), "the port is closed");
        assert!(shared.lock().unwrap().remote.state.sessions.is_empty());
        // On again: a new code, the old token refused.
        set_enabled(&shared, true, Some(0)).unwrap();
        let port = shared.lock().unwrap().remote.bound_port().unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        write!(c, "GET /api/remote/state HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = String::new();
        let _ = c.read_to_string(&mut raw);
        assert!(raw.starts_with("HTTP/1.1 401"), "{raw}");
        set_enabled(&shared, false, None).unwrap();
        assert!(!shared.lock().unwrap().gate.is_armed());
    }

    // ----- Zone alignment from a phone (T-460) -----

    fn nudge_req(s: &mut Shared, c: &str, corner: u8, dx: i8, dy: i8, now: Instant) -> Reply {
        post(s, NUDGE_PATH, Some(c), json!({ "corner": corner, "dx": dx, "dy": dy }), now)
    }

    /// Paired, a session opened from the main screen on zone 1.
    fn aligning(s: &mut Shared, name: &str) -> (String, Instant) {
        on(s);
        let c = paired(s, name);
        let now = Instant::now();
        align_open(s, 1, name, now).unwrap();
        (c, now)
    }

    #[test]
    fn steps_stay_in_the_field_close_to_the_saved_zone_and_convex() {
        let base = SQUARE;
        // A step moves by NUDGE_STEP; ×5 by five.
        let d = step_draft(&base, &base, 2, -1, -5, ).unwrap();
        assert!((d[2][0] - (1.0 - NUDGE_STEP)).abs() < 1e-6 && (d[2][1] - (1.0 - 5.0 * NUDGE_STEP)).abs() < 1e-6);
        // Out of -1..1: refused (never clamped into place).
        assert!(step_draft(&base, &base, 2, 1, 0).is_err());
        assert!(step_draft(&base, &base, 0, -1, 0).is_err());
        // At most MAX_DRAFT_OFFSET from the saved corner.
        let mut d = base;
        let mut n = 0;
        while let Ok(next) = step_draft(&d, &base, 3, 5, 0) {
            d = next;
            n += 1;
            assert!(n < 100);
        }
        assert!((d[3][0] - base[3][0]).abs() <= MAX_DRAFT_OFFSET + 1e-4);
        assert!(step_draft(&d, &base, 3, 5, 0).unwrap_err().contains("0,2"));
        // A quad that would cross itself: refused.
        let near = [[-1.0, -1.0], [1.0, -1.0], [-0.95, -0.85], [-1.0, 1.0]];
        assert!(step_draft(&near, &near, 2, -5, 0).is_err(), "concave");
        // Bad steps.
        assert!(step_draft(&base, &base, 4, 1, 0).is_err());
        assert!(step_draft(&base, &base, 1, -128, 0).is_err());
        assert!(step_draft(&base, &base, 1, 6, 0).is_err());
    }

    /// The phone only moves a draft: the saved zone changes only with
    /// « Appliquer » on the main screen, which the phone can't reach.
    #[test]
    fn the_phone_cannot_write_a_zone_without_the_main_screen() {
        let mut s = shared();
        let (c, now) = aligning(&mut s, "Téléphone");
        let saved = s.projection.get().clone();
        assert_eq!(s.test_pattern.active, Some(crate::testpattern::TestPattern::Grid), "the pattern shows the draft");
        for _ in 0..3 {
            assert_eq!(nudge_req(&mut s, &c, 3, 1, -1, now).status, 200);
        }
        let draft = s.remote.align.as_ref().unwrap().draft;
        assert!((draft[3][0] - (-1.0 + 3.0 * NUDGE_STEP)).abs() < 1e-5);
        assert_eq!(s.projection.get(), &saved, "the saved zone is untouched");
        // The preview goes through the draft.
        let g = align_geometry(&s).unwrap();
        assert!(g.warp.enabled && g.warp.corners == draft);
        let st: Value = serde_json::from_str(&handle(&mut s, "GET", ALIGN_PATH, Some(&c), "x", b"", now).body).unwrap();
        assert_eq!(st["open"], true);
        assert_eq!(st["changed"], true);
        assert_eq!(st["zone"]["name"], "Principale");
        // Every writing route of the main screen is 403 from the phone.
        for path in ["/api/zones/align/apply", "/api/zones/align/open", "/api/projection", "/api/projection/align", "/api/safety", "/api/calibration", "/api/arm", "/api/estop/reset", "/api/outputs/limits", "/api/safety/trace"] {
            assert_eq!(post(&mut s, path, Some(&c), json!({ "zone": 1, "remote": "Téléphone", "on": true }), now).status, 403, "{path}");
        }
        assert_eq!(s.projection.get(), &saved);
        assert!(s.remote.align.is_some());
        // Reset: back to the saved corners.
        assert_eq!(post(&mut s, RESET_PATH, Some(&c), json!({}), now).status, 200);
        assert_eq!(s.remote.align.as_ref().unwrap().draft, SQUARE);
        assert_eq!(nudge_req(&mut s, &c, 1, -2, 2, now).status, 200);
        let draft = s.remote.align.as_ref().unwrap().draft;
        // The main screen applies.
        align_apply(&mut s).unwrap();
        let z = s.projection.get().zone(1).unwrap();
        assert!(z.geometry.warp.enabled && z.geometry.warp.corners == draft);
        assert!(s.remote.align.is_none() && s.test_pattern.active.is_none());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_nudge_without_a_session_is_refused() {
        let mut s = shared();
        on(&mut s);
        let c = paired(&mut s, "A");
        let other = paired(&mut s, "B");
        let now = Instant::now();
        let r = nudge_req(&mut s, &c, 0, 1, 1, now);
        assert_eq!((r.status, r.body.as_str()), (409, ASK_FR));
        assert_eq!(post(&mut s, RESET_PATH, Some(&c), json!({}), now).status, 409);
        // Open for B: A still can't.
        align_open(&mut s, 1, "B", now).unwrap();
        assert_eq!(nudge_req(&mut s, &c, 0, 1, 1, now).status, 409);
        assert_eq!(nudge_req(&mut s, &other, 0, 1, 1, now).status, 200);
        // Unpaired, bad bodies.
        assert_eq!(nudge_req(&mut s, "ls_remote=forged", 0, 1, 1, now).status, 401);
        for bad in [json!({ "corner": 4, "dx": 1, "dy": 0 }), json!({ "corner": 0, "dx": 0, "dy": 0 }), json!({ "corner": 0, "dx": 6, "dy": 0 }), json!({ "corner": 0, "dx": 0.5, "dy": 0 }), json!({ "corner": 0, "dx": 300, "dy": 0 }), json!({ "corner": 0, "dx": 1, "dy": 0, "zone": 2 }), json!({ "corner": 0, "dx": "NaN", "dy": 0 })] {
            assert_eq!(post(&mut s, NUDGE_PATH, Some(&other), bad.clone(), now).status, 400, "{bad}");
        }
        // Opening: unknown zone or remote, the access off.
        assert_eq!(align_open(&mut s, 9, "B", now).unwrap_err().0, 404);
        assert_eq!(align_open(&mut s, 1, "Z", now).unwrap_err().0, 404);
    }

    #[test]
    fn locked_or_in_show_mode_the_phone_cannot_move_the_zone() {
        let mut s = shared();
        let (c, now) = aligning(&mut s, "T");
        s.safety_lock.set_code(crate::safetylock::new_file("5555", 10).unwrap(), now).unwrap();
        assert!(crate::safetylock::check(&mut s, now));
        assert_eq!(nudge_req(&mut s, &c, 3, 1, 0, now).status, 423);
        assert_eq!(s.remote.align.as_ref().unwrap().draft, SQUARE);
        // Undoing the draft is never locked.
        assert_eq!(post(&mut s, RESET_PATH, Some(&c), json!({}), now).status, 200);
        // Show mode ends the session.
        let mut s = shared();
        let (c, now) = aligning(&mut s, "T");
        s.show_mode.on = true;
        assert_ne!(nudge_req(&mut s, &c, 3, 1, 0, now).status, 200);
        assert!(s.remote.align.is_none());
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn the_draft_is_dropped_on_estop_silence_kick_reject_and_zone_edit() {
        for how in ["estop", "silence", "kick", "off", "reject", "edit", "remote_estop"] {
            let mut s = shared();
            let (c, t0) = aligning(&mut s, "T");
            assert_eq!(nudge_req(&mut s, &c, 2, -5, -5, t0).status, 200);
            let saved = s.projection.get().clone();
            match how {
                "estop" => s.emergency_stop(crate::interlock::ArmSource::Ui),
                "silence" => {
                    align_sync(&mut s, t0 + Duration::from_secs(9));
                    assert!(s.remote.align.is_some(), "still within the timeout");
                    align_sync(&mut s, t0 + ALIGN_TIMEOUT + Duration::from_secs(1));
                }
                "kick" => {
                    kick(&mut s, Some("T"));
                }
                "off" => disable(&mut s, "test"),
                "reject" => {
                    align_end(&mut s, "rejected");
                }
                "edit" => {
                    let mut cfg = saved.clone();
                    cfg.zones[0].geometry.size_x = 0.5;
                    s.projection.set(cfg).unwrap();
                    align_sync(&mut s, t0);
                }
                _ => {
                    open_gate(&mut s);
                    assert_eq!(post(&mut s, "/api/remote/control", Some(&c), json!({ "id": "safety.estop" }), t0).status, 200);
                    align_sync(&mut s, t0);
                }
            }
            assert!(s.remote.align.is_none(), "{how}");
            assert!(align_geometry(&s).is_none(), "{how}: the next frame uses the saved zone");
            if how != "edit" {
                assert_eq!(s.projection.get(), &saved, "{how}: nothing written");
            }
            assert!(s.test_pattern.active.is_none(), "{how}: the session's pattern is left");
            assert!(!s.gate.is_armed());
        }
    }

    /// The remote's own table: nothing but reads, plays and the draft;
    /// none of its paths belongs to the main UI.
    #[test]
    fn the_whitelist_has_no_writing_route() {
        for &(m, p, class) in WHITELIST {
            assert!(matches!(class, RouteClass::Read | RouteClass::Play), "{m} {p}");
            assert!(p == "/" || !crate::web::is_known_route(m, p), "{m} {p} is a main UI route");
            assert!(!p.contains("zones/align/") && !p.contains("safety") && !p.contains("calibration") && !p.contains("arm"), "{p}");
        }
        let mut s = shared();
        on(&mut s);
        let c = paired(&mut s, "T");
        // A method not listed: refused.
        assert_eq!(handle(&mut s, "POST", ALIGN_PATH, Some(&c), "x", b"{}", Instant::now()).status, 404);
        assert_eq!(handle(&mut s, "GET", NUDGE_PATH, Some(&c), "x", b"", Instant::now()).status, 404);
    }
}
