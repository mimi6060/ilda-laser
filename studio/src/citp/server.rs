//! The "citp" thread (T-454): the TCP listener a console connects to, the
//! PINF/PLoc announce, one session per console, the media index and the
//! picture cache. Everything here is non-blocking and owned by the thread;
//! the `Shared` lock is taken only to read the grid's revision and copy
//! the index. Nothing received changes the studio.

use super::thumb;
use super::wire::{self, LibraryId, LibraryInfo, MediaInfo, Message, Msex, ThumbRequest, Version, MEDIA};
use super::{CitpHub, MediaIndex};
use crate::Shared;
use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Consoles at once; more are closed as soon as accepted (§1.4).
pub const MAX_PEERS: usize = 8;
/// Largest message read; past it the connection is closed.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Bytes read and not yet parsed, per console.
pub const MAX_INPUT: usize = 256 * 1024;
/// Bytes waiting to go out, per console: past it its requests wait, no
/// LSta / ELUp is queued and a picture list stops (the console asks
/// again). One message may cross it, so the queue stays under
/// `MAX_OUTPUT + MAX_MESSAGE`; a console past `HARD_OUTPUT` is closed.
pub const MAX_OUTPUT: usize = 1024 * 1024;
pub const HARD_OUTPUT: usize = 2 * MAX_OUTPUT;
/// A console whose pending output has not moved for this long (hung,
/// paused, half-open TCP) is closed, which frees its slot.
pub const STALL: Duration = Duration::from_secs(5);
/// Pictures kept.
pub const CACHE_LEN: usize = 512;
/// No CInf this long after the connection: a MSEX 1.0/1.1 console.
const CINF_WAIT: Duration = Duration::from_millis(500);
/// LSta (mandatory), « suggestion: 4 times / second ».
const LSTA_EVERY: Duration = Duration::from_millis(250);
const PLOC_EVERY: Duration = Duration::from_secs(1);
/// The grid's revision is looked at this often, the index rebuilt at
/// least this often while a console is connected.
const REV_EVERY: Duration = Duration::from_millis(250);
const REINDEX_EVERY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(5);
const RETRY: Duration = Duration::from_secs(2);
const PRODUCT_VERSION: (u8, u8, u8) = (0, 1, 0);
/// The versions we speak.
const VERSIONS: [Version; 3] = Version::ALL;

pub fn spawn(shared: Arc<Mutex<Shared>>, hub: Arc<CitpHub>, running: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("citp".into())
        .spawn(move || Server::new(shared, hub).run(&running))
        .map_err(|e| log::warn!("CITP : impossible de démarrer le thread : {e}"))
        .ok()
}

struct Conn {
    stream: TcpStream,
    peer: SocketAddr,
    name: Option<String>,
    input: Vec<u8>,
    output: Vec<u8>,
    opened: Instant,
    /// The version negotiated through CInf (1.2), or None: a console that
    /// sent no CInf, answered in each request's own version.
    session: Option<Version>,
    /// The version of its last request (for unsolicited messages).
    last: Version,
    sinf_sent: bool,
    last_lsta: Option<Instant>,
    /// Last time its output was empty or a write took bytes.
    drained: Instant,
    /// An ELUp was dropped while over the cap: re-announce everything
    /// once it drains.
    missed_update: bool,
    closed: bool,
}

impl Conn {
    /// The version of unsolicited messages (LSta, ELUp).
    fn version(&self) -> Version {
        self.session.unwrap_or(self.last)
    }

    fn send(&mut self, msg: Message, index: u16) {
        self.output.extend_from_slice(&wire::encode(&msg, index));
    }

    /// Room for more answers (or unsolicited messages).
    fn has_room(&self) -> bool {
        self.output.len() < MAX_OUTPUT
    }

    fn msex(&mut self, version: Version, body: Msex, index: u16) {
        self.send(Message::Msex { version, body }, index);
    }

    fn label(&self) -> String {
        match &self.name {
            Some(n) => format!("{n} ({})", self.peer),
            None => self.peer.to_string(),
        }
    }
}

type ThumbKey = (String, u64, u16, u16, [u8; 4], bool);

/// Pictures by cue, look, size and format; the oldest goes first.
#[derive(Default)]
struct Cache {
    map: HashMap<ThumbKey, Arc<(u16, u16, Vec<u8>)>>,
    order: VecDeque<ThumbKey>,
}

impl Cache {
    fn get_or(&mut self, key: ThumbKey, draw: impl FnOnce() -> Option<(u16, u16, Vec<u8>)>) -> Option<Arc<(u16, u16, Vec<u8>)>> {
        if let Some(t) = self.map.get(&key) {
            return Some(Arc::clone(t));
        }
        let t = Arc::new(draw()?);
        if self.order.len() >= CACHE_LEN {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, Arc::clone(&t));
        Some(t)
    }
}

pub(crate) struct Server {
    shared: Arc<Mutex<Shared>>,
    hub: Arc<CitpHub>,
    listener: Option<(Ipv4Addr, TcpListener)>,
    failed: Option<(Ipv4Addr, Instant)>,
    announce: Option<(SocketAddr, UdpSocket)>,
    last_ploc: Option<Instant>,
    conns: Vec<Conn>,
    index: Option<Arc<MediaIndex>>,
    grid_rev: Option<u64>,
    rev_seen: Option<Instant>,
    indexed: Option<Instant>,
    cache: Cache,
    uuid: String,
    served: u64,
    ignored: u64,
}

impl Server {
    fn new(shared: Arc<Mutex<Shared>>, hub: Arc<CitpHub>) -> Self {
        // A stable id for this studio's data (the console caches by it).
        let seed = hub.seed();
        let h = super::fnv(format!("laser-studio-citp\0{seed}").as_bytes());
        let h2 = super::fnv(&h.to_le_bytes());
        let uuid = format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", h >> 32, (h >> 16) & 0xffff, h & 0xffff, h2 >> 48, h2 & 0xffff_ffff_ffff);
        Server {
            shared,
            hub,
            listener: None,
            failed: None,
            announce: None,
            last_ploc: None,
            conns: Vec::new(),
            index: None,
            grid_rev: None,
            rev_seen: None,
            indexed: None,
            cache: Cache::default(),
            uuid,
            served: 0,
            ignored: 0,
        }
    }

    fn run(mut self, running: &AtomicBool) {
        while running.load(Ordering::SeqCst) {
            let busy = self.step(Instant::now());
            if !busy {
                std::thread::sleep(if self.listener.is_some() { POLL } else { Duration::from_millis(50) });
            }
        }
        self.close_all();
    }

    /// One pass; true when something was read or written.
    pub(crate) fn step(&mut self, now: Instant) -> bool {
        self.reconcile(now);
        if self.listener.is_none() {
            return false;
        }
        let mut busy = self.accept(now);
        self.refresh_index(now);
        self.announce(now);
        for i in 0..self.conns.len() {
            busy |= self.serve(i, now);
        }
        let before = self.conns.len();
        self.conns.retain(|c| !c.closed);
        if self.conns.len() != before {
            self.publish_peers();
        }
        self.hub.update_status(|st| (st.served_thumbs, st.ignored_messages) = (self.served, self.ignored));
        busy
    }

    /// Opens, moves or closes the listener to match the settings.
    fn reconcile(&mut self, now: Instant) {
        let cfg = self.hub.config();
        let want = cfg.listen_ip();
        if self.listener.as_ref().map(|(ip, _)| *ip) == want {
            return;
        }
        if self.listener.take().is_some() {
            log::info!("CITP : écoute arrêtée");
        }
        self.close_all();
        self.announce = None;
        self.index = None;
        let Some(ip) = want else {
            self.failed = None;
            self.hub.update_status(|st| (st.listening, st.local, st.announcing, st.error) = (false, None, false, None));
            return;
        };
        if self.failed.is_some_and(|(a, at)| a == ip && now.duration_since(at) < RETRY) {
            return;
        }
        match TcpListener::bind(SocketAddrV4::new(ip, 0)).and_then(|l| l.set_nonblocking(true).map(|()| l)) {
            Ok(l) => {
                let local = l.local_addr().map(|a| a.to_string()).ok();
                log::info!("CITP : écoute sur {}", local.as_deref().unwrap_or("?"));
                let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
                self.announce = announce_socket(ip, self.hub.announce_override()).map(|(to, sock)| {
                    log::info!("CITP : annonce PINF/PLoc vers {to} (port {port})");
                    (to, sock)
                });
                let announcing = self.announce.is_some();
                self.hub.update_status(|st| (st.listening, st.local, st.announcing, st.error) = (true, local, announcing, None));
                self.listener = Some((ip, l));
                self.failed = None;
                self.last_ploc = None;
            }
            Err(e) => {
                log::warn!("CITP : impossible d'écouter sur {ip} : {e}");
                let error = format!("impossible d'écouter sur {ip} : {e}");
                self.hub.update_status(|st| (st.listening, st.local, st.announcing, st.error) = (false, None, false, Some(error)));
                self.failed = Some((ip, now));
            }
        }
    }

    fn close_all(&mut self) {
        if !self.conns.is_empty() {
            self.conns.clear();
            self.publish_peers();
        }
    }

    fn publish_peers(&self) {
        let peers = self.conns.iter().map(Conn::label).collect();
        self.hub.update_status(|st| st.peers = peers);
    }

    fn accept(&mut self, now: Instant) -> bool {
        let Some((_, l)) = &self.listener else { return false };
        let mut got = false;
        while let Ok((stream, peer)) = l.accept() {
            got = true;
            if self.conns.len() >= MAX_PEERS {
                log::info!("CITP : {peer} refusé ({MAX_PEERS} consoles au plus)");
                continue; // dropped: closed
            }
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let _ = stream.set_nodelay(true);
            log::info!("CITP : console connectée depuis {peer}");
            let mut c = Conn {
                stream,
                peer,
                name: None,
                input: Vec::new(),
                output: Vec::new(),
                opened: now,
                session: None,
                last: Version::V1_0,
                sinf_sent: false,
                last_lsta: None,
                drained: now,
                missed_update: false,
                closed: false,
            };
            // §1.4: PNam right after the connection.
            c.send(Message::PNam { name: self.hub.config().server_name }, 0);
            self.conns.push(c);
        }
        if got {
            self.publish_peers();
        }
        got
    }

    /// Rebuilds the index when the grid changed (or every few seconds)
    /// while a console is connected, and tells the consoles what changed.
    fn refresh_index(&mut self, now: Instant) {
        if self.conns.is_empty() {
            return;
        }
        if self.index.is_some() && self.rev_seen.is_some_and(|t| now.duration_since(t) < REV_EVERY) {
            return;
        }
        self.rev_seen = Some(now);
        let stale = self.indexed.is_none_or(|t| now.duration_since(t) >= REINDEX_EVERY);
        let raw = {
            let s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            let held = Instant::now();
            let rev = s.grid.rev;
            let raw = if self.index.is_some() && self.grid_rev == Some(rev) && !stale {
                None
            } else {
                self.grid_rev = Some(rev);
                Some(MediaIndex::collect(&s))
            };
            drop(s);
            self.hub.held_lock(held.elapsed());
            match raw {
                Some(r) => r,
                None => return,
            }
        };
        self.indexed = Some(now);
        let new = Arc::new(MediaIndex::from_raw(raw));
        if let Some(old) = self.index.replace(Arc::clone(&new)) {
            let ups = updates(&old, &new);
            for c in self.conns.iter_mut().filter(|c| c.sinf_sent && !ups.is_empty()) {
                if !c.has_room() {
                    // Not drained: what changed is told once it is.
                    c.missed_update = true;
                    continue;
                }
                let v = c.version();
                for u in ups.iter().filter(|u| u.for_version(v)) {
                    c.msex(v, u.clone().into_msex(), 0);
                }
            }
        }
    }

    /// For a console that missed ELUps: everything may have changed.
    fn resend_all(&mut self, i: usize) {
        let Some(idx) = self.index.clone() else { return };
        let c = &mut self.conns[i];
        c.missed_update = false;
        let v = c.version();
        for u in everything(&idx).into_iter().filter(|u| u.for_version(v)) {
            c.msex(v, u.into_msex(), 0);
        }
    }

    fn announce(&mut self, now: Instant) {
        let Some((to, sock)) = &self.announce else { return };
        if self.last_ploc.is_some_and(|t| now.duration_since(t) < PLOC_EVERY) {
            return;
        }
        self.last_ploc = Some(now);
        let port = self.listener.as_ref().and_then(|(_, l)| l.local_addr().ok()).map_or(0, |a| a.port());
        let msg = Message::PLoc { port, kind: "MediaServer".into(), name: self.hub.config().server_name, state: "Idle".into() };
        if let Err(e) = sock.send_to(&wire::encode(&msg, 0), to) {
            log::debug!("CITP : annonce : {e}");
        }
    }

    /// Reads, answers and writes for console `i`.
    fn serve(&mut self, i: usize, now: Instant) -> bool {
        let mut busy = false;
        // Read only while its answers drain (back-pressure).
        if self.conns[i].output.len() < MAX_OUTPUT && self.conns[i].input.len() < MAX_INPUT {
            let mut buf = [0u8; 16 * 1024];
            let c = &mut self.conns[i];
            loop {
                match c.stream.read(&mut buf) {
                    Ok(0) => {
                        c.closed = true;
                        break;
                    }
                    Ok(n) => {
                        busy = true;
                        c.input.extend_from_slice(&buf[..n]);
                        if c.input.len() >= MAX_INPUT {
                            break;
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => {
                        c.closed = true;
                        break;
                    }
                }
            }
        }
        // Whole messages, while the output has room.
        while !self.conns[i].closed && self.conns[i].output.len() < MAX_OUTPUT {
            let c = &mut self.conns[i];
            if c.input.len() < wire::HEADER_LEN {
                break;
            }
            let h = match wire::Header::parse(&c.input) {
                Ok(h) => h,
                Err(_) => {
                    // Lost sync: nothing after this can be trusted.
                    self.ignored += 1;
                    log::debug!("CITP : {} : flux illisible, connexion fermée", c.peer);
                    c.closed = true;
                    break;
                }
            };
            let size = h.size as usize;
            if !(wire::HEADER_LEN..=MAX_MESSAGE).contains(&size) {
                self.ignored += 1;
                log::debug!("CITP : {} : message de {size} octets, connexion fermée", c.peer);
                c.closed = true;
                break;
            }
            if c.input.len() < size {
                break;
            }
            let bytes: Vec<u8> = c.input.drain(..size).collect();
            busy = true;
            match wire::decode(&bytes) {
                Ok((h, msg)) => self.handle(i, h.index, msg),
                Err(e) => {
                    log::debug!("CITP : {} : {e}", self.conns[i].peer);
                    self.ignored += 1;
                }
            }
        }
        // Timers: SInf 1.0 for a console without CInf, then LSta.
        let c = &mut self.conns[i];
        if !c.sinf_sent && now.duration_since(c.opened) >= CINF_WAIT {
            let info = self.server_info(Version::V1_0);
            let c = &mut self.conns[i];
            c.msex(Version::V1_0, Msex::SInf(info), 0);
            c.sinf_sent = true;
        }
        let c = &self.conns[i];
        if c.missed_update && c.has_room() && !c.closed {
            self.resend_all(i);
        }
        let c = &mut self.conns[i];
        // LSta only once the previous answers are out: a console that
        // does not read gets nothing more queued.
        if c.sinf_sent && c.output.is_empty() && c.last_lsta.is_none_or(|t| now.duration_since(t) >= LSTA_EVERY) {
            c.last_lsta = Some(now);
            let v = c.version();
            c.msex(v, Msex::LSta, 0);
        }
        // Write what the socket takes.
        while !c.output.is_empty() && !c.closed {
            match c.stream.write(&c.output) {
                Ok(0) => c.closed = true,
                Ok(n) => {
                    busy = true;
                    c.drained = now;
                    c.output.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => c.closed = true,
            }
        }
        if c.output.is_empty() {
            c.drained = now;
        } else if !c.closed && (c.output.len() > HARD_OUTPUT || now.duration_since(c.drained) >= STALL) {
            log::info!("CITP : {} ne lit plus ({} octets en attente), connexion fermée", c.label(), c.output.len());
            c.closed = true;
        }
        if c.closed {
            log::info!("CITP : console déconnectée : {}", c.label());
        }
        busy
    }

    fn server_info(&self, v: Version) -> wire::ServerInfo {
        wire::ServerInfo {
            uuid: self.uuid.clone(),
            product: self.hub.config().server_name,
            version: PRODUCT_VERSION,
            versions: VERSIONS.iter().map(|v| v.word()).collect(),
            library_types: if v >= Version::V1_2 { 1 << (MEDIA - 1) } else { 0 },
            thumb_formats: vec![wire::FMT_PNG, wire::FMT_JPEG, wire::FMT_RGB8],
            // Streams (GVSr / RqSt) are not served: none advertised.
            stream_formats: Vec::new(),
            layers: Vec::new(),
        }
    }

    fn index(&mut self) -> Arc<MediaIndex> {
        if self.index.is_none() {
            self.rev_seen = None;
            self.refresh_index(Instant::now());
        }
        self.index.clone().unwrap_or_default()
    }

    /// One received message. Read-only: answers or counts, never more.
    fn handle(&mut self, i: usize, index: u16, msg: Message) {
        let (version, body) = match msg {
            Message::PNam { name } | Message::PLoc { name, .. } => {
                self.conns[i].name = Some(wire::short_name(&name));
                self.publish_peers();
                return;
            }
            Message::Other { .. } => {
                self.ignored += 1;
                return;
            }
            Message::Msex { version, body } => (version, body),
        };
        if let Msex::CInf { versions } = &body {
            let common = versions.iter().filter_map(|w| Version::from_word(*w)).filter(|v| VERSIONS.contains(v)).max();
            match common {
                Some(v) if v >= Version::V1_2 => {
                    let info = self.server_info(v);
                    let c = &mut self.conns[i];
                    c.session = Some(v);
                    c.msex(v, Msex::SInf(info), index);
                    c.sinf_sent = true;
                }
                Some(v) => {
                    // A pre-1.2 console that sent CInf anyway: SInf 1.0.
                    let info = self.server_info(Version::V1_0);
                    let c = &mut self.conns[i];
                    c.last = v;
                    if !c.sinf_sent {
                        c.msex(Version::V1_0, Msex::SInf(info), index);
                        c.sinf_sent = true;
                    }
                }
                None => self.ignored += 1,
            }
            return;
        }
        // A 1.2 session speaks 1.2 only (§10, « MSEX Versions »).
        let c = &mut self.conns[i];
        if c.session.is_some_and(|s| s != version) {
            self.ignored += 1;
            return;
        }
        if c.session.is_none() {
            c.last = version;
        }
        let nack = |s: &mut Self, content: [u8; 4]| {
            s.ignored += 1;
            if version >= Version::V1_2 {
                s.conns[i].msex(version, Msex::Nack { received: content }, index);
            }
        };
        match body {
            Msex::Geli { library_type, parent, numbers } => {
                let idx = self.index();
                let libraries = if library_type == MEDIA && parent == wire::ROOT { libraries(&idx, numbers.as_deref()) } else { Vec::new() };
                self.conns[i].msex(version, Msex::Elin { library_type, libraries }, index);
            }
            Msex::Gein { library_type, library, elements } => {
                if library_type != MEDIA {
                    return nack(self, *b"GEIn");
                }
                let idx = self.index();
                let elements = page_of(&idx, library).map_or_else(Vec::new, |p| media(&idx, p, elements.as_deref(), self.hub.config().thumb_max_px));
                self.conns[i].msex(version, Msex::Mein { library, elements }, index);
            }
            Msex::Geth { req, library, elements } => {
                if req.library_type != MEDIA || !thumb::supported(req.format) {
                    return nack(self, *b"GETh");
                }
                let idx = self.index();
                let Some(p) = page_of(&idx, library) else { return };
                let cues: Vec<(u8, super::IndexCue)> = idx.pages[p]
                    .cues
                    .iter()
                    .enumerate()
                    .filter(|(n, _)| elements.as_ref().is_none_or(|l| l.contains(&(*n as u8))))
                    .map(|(n, c)| (n as u8, c.clone()))
                    .collect();
                for (n, cue) in cues {
                    if !self.conns[i].has_room() {
                        break; // the console asks again for the rest
                    }
                    if let Some(t) = self.picture(&cue, &req, version) {
                        let (w, h, data) = (t.0, t.1, t.2.clone());
                        self.conns[i].msex(version, Msex::Ethn { library_type: MEDIA, library, element: n, format: req.format, width: w, height: h, data }, index);
                        self.served += 1;
                    }
                }
            }
            Msex::Gelt { req, libraries } => {
                if req.library_type != MEDIA || !thumb::supported(req.format) {
                    return nack(self, *b"GELT");
                }
                let idx = self.index();
                let mut ids: Vec<LibraryId> = libraries.unwrap_or_else(|| (0..idx.pages.len()).map(|n| wire::library_id(n as u8)).collect());
                let mut seen = std::collections::HashSet::new();
                ids.retain(|id| seen.insert(*id));
                for library in ids {
                    if !self.conns[i].has_room() {
                        break; // the console asks again for the rest
                    }
                    let Some(p) = page_of(&idx, library) else { continue };
                    let t = match idx.pages[p].cues.first() {
                        Some(cue) => self.picture(&cue.clone(), &req, version),
                        None => {
                            let (w, h) = thumb::fit(req.width, req.height, req.flags, self.hub.config().thumb_max_px);
                            thumb::render_points(&[], req.format, w, h, version == Version::V1_0).map(Arc::new)
                        }
                    };
                    if let Some(t) = t {
                        let (w, h, data) = (t.0, t.1, t.2.clone());
                        self.conns[i].msex(version, Msex::Elth { library_type: MEDIA, library, format: req.format, width: w, height: h, data }, index);
                        self.served += 1;
                    }
                }
            }
            other => {
                let content = other.cookie();
                nack(self, content)
            }
        }
    }

    fn picture(&mut self, cue: &super::IndexCue, req: &ThumbRequest, version: Version) -> Option<Arc<(u16, u16, Vec<u8>)>> {
        let (w, h) = thumb::fit(req.width, req.height, req.flags, self.hub.config().thumb_max_px);
        let bgr = version == Version::V1_0 && req.format == wire::FMT_RGB8;
        let settings = Arc::clone(&cue.settings);
        self.cache.get_or((cue.id.clone(), cue.look, w, h, req.format, bgr), || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| thumb::render(&settings, req.format, w, h, bgr))).ok().flatten()
        })
    }
}

/// The UDP socket for PLoc and where to send it: the multicast group on a
/// real interface (never on the loopback), or the tests' loopback port.
fn announce_socket(ip: Ipv4Addr, test_to: Option<SocketAddr>) -> Option<(SocketAddr, UdpSocket)> {
    let to = match test_to {
        Some(t) => t,
        None if ip.is_loopback() => return None,
        None => SocketAddr::V4(SocketAddrV4::new(wire::MULTICAST, wire::MULTICAST_PORT)),
    };
    let sock = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP)).ok()?;
    let bind = SocketAddr::V4(SocketAddrV4::new(ip, 0));
    sock.bind(&bind.into()).ok()?;
    if to.ip().is_multicast() {
        sock.set_multicast_if_v4(&ip).ok()?;
        sock.set_multicast_ttl_v4(1).ok()?;
    }
    sock.set_nonblocking(true).ok()?;
    Some((to, sock.into()))
}

/// The page of library `id` (`{1, n, 0, 0}`).
fn page_of(idx: &MediaIndex, id: LibraryId) -> Option<usize> {
    (id[0] == 1 && id[2] == 0 && id[3] == 0 && (id[1] as usize) < idx.pages.len()).then_some(id[1] as usize)
}

fn libraries(idx: &MediaIndex, numbers: Option<&[u8]>) -> Vec<LibraryInfo> {
    idx.pages
        .iter()
        .enumerate()
        .filter(|(n, _)| numbers.is_none_or(|l| l.contains(&(*n as u8))))
        .map(|(n, p)| LibraryInfo { id: wire::library_id(n as u8), serial: p.serial, dmx: (n as u8 + 1, n as u8 + 1), name: p.name.clone(), sub_libraries: 0, elements: p.cues.len() as u16 })
        .collect()
}

fn media(idx: &MediaIndex, page: usize, numbers: Option<&[u8]>, side: u16) -> Vec<MediaInfo> {
    idx.pages[page]
        .cues
        .iter()
        .enumerate()
        .filter(|(n, _)| numbers.is_none_or(|l| l.contains(&(*n as u8))))
        .map(|(n, c)| MediaInfo { number: n as u8, serial: c.serial, dmx: (c.dmx, c.dmx), name: c.name.clone(), stamp: 0, width: side, height: side, length: 0, fps: 60 })
        .collect()
}

/// An ELUp to send.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Update {
    pub library: LibraryId,
    pub flags: u8,
    pub elements: [u8; 32],
    pub libraries: [u8; 32],
}

impl Update {
    /// The per-page "all elements" ELUp of an added or removed page is for
    /// MSEX 1.0 only (no root there); 1.1+ get the root ELUp instead,
    /// never a message naming a page that is gone.
    fn for_version(&self, v: Version) -> bool {
        if v == Version::V1_0 {
            self.library != wire::ROOT
        } else {
            self.library == wire::ROOT || self.flags & wire::UP_ALL_ELEMENTS == 0
        }
    }

    fn into_msex(self) -> Msex {
        Msex::Elup { library_type: MEDIA, library: self.library, flags: self.flags, elements: self.elements, libraries: self.libraries }
    }
}

/// What changed between two indexes, as ELUp messages: per page, cues
/// updated (0x01) and / or added or removed (0x02); on the root, pages
/// renamed (0x04) or added or removed (0x08).
pub(crate) fn updates(old: &MediaIndex, new: &MediaIndex) -> Vec<Update> {
    let mut out = Vec::new();
    let mut root = Update { library: wire::ROOT, flags: 0, elements: [0; 32], libraries: [0; 32] };
    let n = old.pages.len().max(new.pages.len());
    for p in 0..n {
        match (old.pages.get(p), new.pages.get(p)) {
            (Some(a), Some(b)) if a.serial == b.serial => {}
            (Some(a), Some(b)) => {
                if a.name != b.name {
                    root.flags |= 0x04;
                    wire::set_bit(&mut root.libraries, p as u8);
                }
                let mut u = Update { library: wire::library_id(p as u8), flags: 0, elements: [0; 32], libraries: [0; 32] };
                if a.cues.len() != b.cues.len() || a.cues.iter().zip(&b.cues).any(|(x, y)| x.id != y.id || x.dmx != y.dmx) {
                    u.flags |= wire::UP_ELEMENTS_ADDED_REMOVED;
                }
                for e in 0..a.cues.len().max(b.cues.len()) {
                    if a.cues.get(e).map(|c| c.serial) != b.cues.get(e).map(|c| c.serial) {
                        if e < a.cues.len().min(b.cues.len()) {
                            u.flags |= wire::UP_ELEMENTS_UPDATED;
                        }
                        wire::set_bit(&mut u.elements, e as u8);
                    }
                }
                if u.flags != 0 {
                    out.push(u);
                }
            }
            _ => {
                root.flags |= wire::UP_LIBRARIES_ADDED_REMOVED;
                wire::set_bit(&mut root.libraries, p as u8);
                // MSEX 1.0 has no root: the page itself, all elements.
                out.push(Update { library: wire::library_id(p as u8), flags: wire::UP_ELEMENTS_ADDED_REMOVED | wire::UP_ALL_ELEMENTS, elements: [0; 32], libraries: [0; 32] });
            }
        }
    }
    if root.flags != 0 {
        out.insert(0, root);
    }
    out
}

/// "Everything may have changed": every page added/removed on the root
/// (1.1+), every page's elements (1.0).
fn everything(idx: &MediaIndex) -> Vec<Update> {
    let mut root = Update { library: wire::ROOT, flags: wire::UP_LIBRARIES_ADDED_REMOVED, elements: [0; 32], libraries: [0; 32] };
    let mut out = Vec::new();
    for p in 0..idx.pages.len() {
        wire::set_bit(&mut root.libraries, p as u8);
        out.push(Update { library: wire::library_id(p as u8), flags: wire::UP_ELEMENTS_ADDED_REMOVED | wire::UP_ALL_ELEMENTS, elements: [0; 32], libraries: [0; 32] });
    }
    out.insert(0, root);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::citp::wire::{FMT_PNG, KEEP_ASPECT};
    use crate::test_support::shared;
    use serde_json::json;

    /// Our console: a blocking client on the loopback.
    pub struct Client {
        pub stream: TcpStream,
        next: u16,
    }

    impl Client {
        pub fn connect(addr: &str) -> Client {
            let stream = TcpStream::connect(addr).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            Client { stream, next: 1 }
        }

        pub fn raw(&mut self, bytes: &[u8]) {
            self.stream.write_all(bytes).unwrap();
        }

        pub fn send(&mut self, msg: Message) -> u16 {
            let i = self.next;
            self.next += 1;
            self.raw(&wire::encode(&msg, i));
            i
        }

        pub fn msex(&mut self, body: Msex) -> u16 {
            self.send(Message::Msex { version: Version::V1_2, body })
        }

        pub fn read(&mut self) -> (wire::Header, Message) {
            let mut head = [0u8; wire::HEADER_LEN];
            self.stream.read_exact(&mut head).unwrap();
            let h = wire::Header::parse(&head).unwrap();
            let mut rest = vec![0u8; h.size as usize - wire::HEADER_LEN];
            self.stream.read_exact(&mut rest).unwrap();
            let mut all = head.to_vec();
            all.extend(rest);
            wire::decode(&all).unwrap()
        }

        /// The next message that `pick` accepts (LSta and others skipped).
        pub fn wait<T>(&mut self, mut pick: impl FnMut(&wire::Header, Message) -> Option<T>) -> T {
            for _ in 0..10_000 {
                let (h, m) = self.read();
                if let Some(t) = pick(&h, m) {
                    return t;
                }
            }
            panic!("no such message");
        }

        /// PNam + CInf 1.2, then the SInf.
        pub fn hello(&mut self, name: &str) -> wire::ServerInfo {
            self.send(Message::PNam { name: name.into() });
            let i = self.msex(Msex::CInf { versions: vec![Version::V1_2.word()] });
            self.wait(|h, m| match m {
                Message::Msex { version: Version::V1_2, body: Msex::SInf(s) } if h.index == i => Some(s),
                _ => None,
            })
        }
    }

    /// Waits (20 s at most) until `cond` holds.
    pub fn eventually(cond: impl Fn() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !cond() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A running "citp" thread on 127.0.0.1, and its address.
    pub fn start(shared: &Arc<Mutex<Shared>>) -> (Arc<AtomicBool>, JoinHandle<()>, String) {
        let hub = Arc::clone(&shared.lock().unwrap().citp.hub);
        super::super::update(&mut shared.lock().unwrap(), &json!({ "enabled": true, "interface": "127.0.0.1" })).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let t = spawn(Arc::clone(shared), Arc::clone(&hub), Arc::clone(&running)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(l) = hub.status().local {
                return (running, t, l);
            }
            assert!(Instant::now() < deadline, "not listening");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn elin(c: &mut Client) -> Vec<LibraryInfo> {
        let i = c.msex(Msex::Geli { library_type: MEDIA, parent: wire::ROOT, numbers: None });
        c.wait(|h, m| match m {
            Message::Msex { body: Msex::Elin { libraries, .. }, .. } if h.index == i => Some(libraries),
            _ => None,
        })
    }

    fn mein(c: &mut Client, page: u8) -> Vec<MediaInfo> {
        let i = c.msex(Msex::Gein { library_type: MEDIA, library: wire::library_id(page), elements: None });
        c.wait(|h, m| match m {
            Message::Msex { body: Msex::Mein { elements, .. }, .. } if h.index == i => Some(elements),
            _ => None,
        })
    }

    #[test]
    fn a_console_reads_pages_cues_and_pictures_on_the_loopback() {
        let shared = Arc::new(Mutex::new(shared()));
        let (running, t, addr) = start(&shared);
        assert!(addr.starts_with("127.0.0.1:"));
        let hub = Arc::clone(&shared.lock().unwrap().citp.hub);
        assert!(hub.status().listening && !hub.status().announcing, "no announce on the loopback");
        let mut c = Client::connect(&addr);
        // The server's PNam first.
        let (_, first) = c.read();
        assert_eq!(first, Message::PNam { name: "Laser Studio".into() });
        let info = c.hello("Console test");
        assert_eq!((info.product.as_str(), info.library_types, info.uuid.len()), ("Laser Studio", 1, 36));
        assert!(info.thumb_formats.contains(&wire::FMT_RGB8) && info.versions.contains(&Version::V1_2.word()));
        assert!(info.stream_formats.is_empty(), "no stream is served");
        // Listed under « Consoles connectées » with its PINF name.
        let deadline = Instant::now() + Duration::from_secs(20);
        while !hub.status().peers.iter().any(|p| p.starts_with("Console test (127.0.0.1:")) {
            assert!(Instant::now() < deadline, "{:?}", hub.status().peers);
            std::thread::sleep(Duration::from_millis(5));
        }
        // Pages = the grid's pages, numbered as the DMX *Page* channel.
        let idx = MediaIndex::build(&shared.lock().unwrap());
        let libs = elin(&mut c);
        assert_eq!(libs.len(), idx.pages.len());
        for (n, l) in libs.iter().enumerate() {
            assert_eq!((l.id, l.dmx, l.name.as_str(), l.elements as usize), (wire::library_id(n as u8), (n as u8 + 1, n as u8 + 1), idx.pages[n].name.as_str(), idx.pages[n].cues.len()));
        }
        // Cues of page 2, numbered as the DMX *Cue* channel.
        let cues = mein(&mut c, 1);
        assert_eq!(cues.len(), idx.pages[1].cues.len());
        for (n, m) in cues.iter().enumerate() {
            assert_eq!(m.number as usize, n);
            assert_eq!(idx.cue_id(2, m.dmx.0), Some(idx.pages[1].cues[n].id.as_str()));
            assert_eq!(m.name, idx.pages[1].cues[n].name);
        }
        // A picture of the first cue: our own drawing, 128 px.
        let req = ThumbRequest { format: FMT_PNG, width: 200, height: 160, flags: KEEP_ASPECT, library_type: MEDIA };
        let i = c.msex(Msex::Geth { req, library: wire::library_id(1), elements: Some(vec![0]) });
        let (w, h, data) = c.wait(|hd, m| match m {
            Message::Msex { body: Msex::Ethn { element: 0, width, height, data, .. }, .. } if hd.index == i => Some((width, height, data)),
            _ => None,
        });
        assert_eq!((w, h), (128, 128));
        let img = image::load_from_memory(&data).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (128, 128));
        assert!(img.pixels().any(|p| p.0.iter().any(|&v| v > 40)), "lit");
        // Counters are published at the end of the server's pass, maybe
        // after the answer reached us.
        eventually(|| hub.status().served_thumbs >= 1, "served_thumbs");
        // Something we don't do: a Nack, counted.
        let i = c.msex(Msex::Other { content: *b"GVSr" });
        c.wait(|h, m| matches!(m, Message::Msex { body: Msex::Nack { received }, .. } if h.index == i && received == *b"GVSr").then_some(()));
        eventually(|| hub.status().ignored_messages >= 1, "ignored_messages");
        // Switched off: the connection closes, nothing listens.
        super::super::update(&mut shared.lock().unwrap(), &json!({ "enabled": false })).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while hub.status().listening || !hub.status().peers.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(TcpStream::connect(&addr).is_err(), "closed");
        running.store(false, Ordering::SeqCst);
        t.join().unwrap();
    }

    #[test]
    fn renaming_a_cue_reaches_the_console() {
        let shared = Arc::new(Mutex::new(shared()));
        // A user cue in the first empty cell of a user page.
        let (page, id) = {
            let mut s = shared.lock().unwrap();
            let mut g = s.grid.clone();
            let page = g.new_page("Ma page", &s.presets).unwrap();
            s.grid = g;
            crate::grid::refresh(&mut s);
            let id = crate::usercues::store(&mut s, page, 0, Some("Avant"), Default::default(), false).unwrap();
            (page, id)
        };
        let (running, t, addr) = start(&shared);
        let mut c = Client::connect(&addr);
        c.hello("Console");
        let cues = mein(&mut c, page as u8);
        assert_eq!(cues[0].name, "Avant");
        let serial = cues[0].serial;
        {
            let mut s = shared.lock().unwrap();
            crate::usercues::edit(&mut s, crate::usercues::GridEdit::Rename { from: (page, 0), name: "Après".into() }).unwrap();
            assert!(s.presets.iter().any(|p| p.id == id && p.name == "Après"));
        }
        // ELUp for that page, element 0 updated.
        c.wait(|_, m| match m {
            Message::Msex { body: Msex::Elup { library, flags, elements, .. }, .. } if library == wire::library_id(page as u8) => {
                assert!(flags & wire::UP_ELEMENTS_UPDATED != 0 && elements[0] & 1 == 1);
                Some(())
            }
            _ => None,
        });
        let cues = mein(&mut c, page as u8);
        assert_eq!(cues[0].name, "Après");
        assert_ne!(cues[0].serial, serial);
        running.store(false, Ordering::SeqCst);
        t.join().unwrap();
    }

    /// A snapshot of what a console could want to change.
    fn fingerprint(s: &Shared) -> String {
        json!({
            "settings": s.settings,
            "armed": s.gate.is_armed(),
            "estop": s.estop.is_latched(),
            "active": s.active_cue,
            "deck": s.deck.active.len(),
            "page": s.cue_page,
            "presets": s.presets.iter().map(|p| (&p.id, &p.name)).collect::<Vec<_>>(),
            "grid": s.grid.view().iter().map(|p| (&p.name, &p.cells)).collect::<Vec<_>>(),
            "live_speed": s.live.speed,
            "external_open": s.external.gate.open,
        })
        .to_string()
    }

    #[test]
    fn no_message_sequence_changes_the_studio() {
        let shared = Arc::new(Mutex::new(shared()));
        let before = fingerprint(&shared.lock().unwrap());
        let (running, t, addr) = start(&shared);
        let hub = Arc::clone(&shared.lock().unwrap().citp.hub);
        let mut seed = 7u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        // Every message we know, in every version, with random fields,
        // plus other layers (SDMX channel levels, FSEL…) and garbage.
        for round in 0..6 {
            let mut c = Client::connect(&addr);
            let mut cinf = false;
            for _ in 0..60 {
                let v = Version::ALL[rnd() as usize % 3];
                let lib = [rnd() % 3, rnd() % 4, rnd() % 2, 0];
                let req = ThumbRequest { format: [wire::FMT_PNG, wire::FMT_RGB8, *b"XXXX"][rnd() as usize % 3], width: rnd() as u16, height: rnd() as u16, flags: rnd(), library_type: rnd() % 3 };
                let body = match rnd() % 9 {
                    0 => {
                        cinf = true;
                        Msex::CInf { versions: vec![(rnd() as u16) << 8 | rnd() as u16, 0x0102] }
                    }
                    1 => Msex::Geli { library_type: rnd() % 3, parent: lib, numbers: Some(vec![rnd(), rnd()]) },
                    2 => Msex::Gein { library_type: 1, library: lib, elements: None },
                    3 => Msex::Geth { req, library: lib, elements: Some(vec![rnd() % 4]) },
                    4 => Msex::Gelt { req, libraries: Some(vec![lib]) },
                    5 => Msex::Elup { library_type: 1, library: lib, flags: rnd(), elements: [rnd(); 32], libraries: [0; 32] },
                    6 => Msex::Nack { received: *b"SInf" },
                    7 => Msex::Other { content: *b"RqSt" },
                    _ => Msex::LSta,
                };
                c.send(Message::Msex { version: v, body });
                c.send(Message::Other { layer: *b"SDMX", content: *b"ChBk" });
                c.send(Message::Other { layer: *b"FSEL", content: *b"Sele" });
                c.send(Message::PLoc { port: rnd() as u16, kind: "LightingConsole".into(), name: format!("C{round}"), state: "Go".into() });
            }
            // In order: once this answer is here, everything before it
            // was handled.
            let last = c.send(Message::Msex { version: if cinf { Version::V1_2 } else { Version::V1_0 }, body: Msex::Geli { library_type: MEDIA, parent: wire::ROOT, numbers: Some(vec![0]) } });
            c.wait(|h, m| matches!(m, Message::Msex { body: Msex::Elin { .. }, .. } if h.index == last).then_some(()));
            if round % 2 == 1 {
                // Garbage closes this console, not the server.
                let mut junk: Vec<u8> = (0..300).map(|_| rnd()).collect();
                junk[..4].copy_from_slice(b"CITP");
                c.raw(&junk);
            }
            drop(c);
        }
        // An oversized message closes the connection.
        let mut c = Client::connect(&addr);
        let mut big = wire::encode(&Message::PNam { name: "x".into() }, 0);
        big[8..12].copy_from_slice(&(MAX_MESSAGE as u32 + 1).to_le_bytes());
        c.raw(&big);
        let mut buf = [0u8; 4096];
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match c.stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => assert!(Instant::now() < deadline, "never closed"),
            }
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while hub.status().ignored_messages < 10 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        // A fresh console still gets answers.
        let mut c = Client::connect(&addr);
        c.hello("Après");
        assert!(!elin(&mut c).is_empty());
        running.store(false, Ordering::SeqCst);
        t.join().unwrap();
        assert_eq!(fingerprint(&shared.lock().unwrap()), before, "only counters change");
    }

    #[test]
    fn a_burst_of_picture_requests_never_holds_the_lock() {
        let shared = Arc::new(Mutex::new(shared()));
        let (running, t, addr) = start(&shared);
        let mut c = Client::connect(&addr);
        c.hello("Rafale");
        let pages = elin(&mut c).len().min(4);
        let req = ThumbRequest { format: wire::FMT_JPEG, width: 128, height: 128, flags: 0, library_type: MEDIA };
        let mut want = 0;
        for p in 0..pages {
            want += MediaIndex::build(&shared.lock().unwrap()).pages[p].cues.len();
            c.msex(Msex::Geth { req: req.clone(), library: wire::library_id(p as u8), elements: None });
        }
        let hub = Arc::clone(&shared.lock().unwrap().citp.hub);
        let mut got = 0;
        while got < want {
            if let (_, Message::Msex { body: Msex::Ethn { width: 128, height: 128, .. }, .. }) = c.read() {
                got += 1;
            }
        }
        // The citp thread's own hold time (not another thread's wait,
        // which includes the scheduler): pictures are drawn off the lock,
        // only the index copy happens under it.
        let held = Duration::from_micros(hub.lock_held_us.load(Ordering::Relaxed));
        assert!(held < Duration::from_millis(16), "lock held {held:?}: a frame late");
        running.store(false, Ordering::SeqCst);
        t.join().unwrap();
    }

    #[test]
    fn a_console_without_cinf_gets_msex_1_0_and_bgr() {
        let shared = Arc::new(Mutex::new(shared()));
        let (running, t, addr) = start(&shared);
        let mut c = Client::connect(&addr);
        c.wait(|_, m| matches!(m, Message::Msex { version: Version::V1_0, body: Msex::SInf(_) }).then_some(()));
        c.send(Message::Msex { version: Version::V1_0, body: Msex::Geli { library_type: MEDIA, parent: wire::ROOT, numbers: Some(vec![0]) } });
        let libs = c.wait(|_, m| match m {
            Message::Msex { version: Version::V1_0, body: Msex::Elin { libraries, .. } } => Some(libraries),
            _ => None,
        });
        assert_eq!(libs.len(), 1);
        // The same pictures (every cue of page 1) in 1.0 (BGR) and 1.1
        // (RGB): channels swapped.
        let cues = MediaIndex::build(&shared.lock().unwrap()).pages[0].cues.len() as u8;
        let req = ThumbRequest { format: wire::FMT_RGB8, width: 16, height: 16, flags: 0, library_type: MEDIA };
        let mut pictures = |v: Version| {
            let mut all = Vec::new();
            for n in 0..cues {
                let i = c.send(Message::Msex { version: v, body: Msex::Geth { req: req.clone(), library: wire::library_id(0), elements: Some(vec![n]) } });
                let data = c.wait(|h, m| match m {
                    Message::Msex { version, body: Msex::Ethn { data, .. } } if version == v && h.index == i => Some(data),
                    _ => None,
                });
                assert_eq!(data.len(), 16 * 16 * 3);
                all.extend(data);
            }
            all
        };
        let bgr = pictures(Version::V1_0);
        let rgb = pictures(Version::V1_1);
        assert!(rgb.chunks(3).any(|p| p[0] != p[2]), "a coloured pixel to tell them apart");
        for (b, r) in bgr.chunks(3).zip(rgb.chunks(3)) {
            assert_eq!([b[2], b[1], b[0]], [r[0], r[1], r[2]]);
        }
        running.store(false, Ordering::SeqCst);
        t.join().unwrap();
    }

    #[test]
    fn ploc_is_announced_only_where_told_and_names_the_port() {
        let shared = Arc::new(Mutex::new(shared()));
        let ear = UdpSocket::bind("127.0.0.1:0").unwrap();
        ear.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        shared.lock().unwrap().citp.hub.announce_to(ear.local_addr().unwrap());
        let (running, t, addr) = start(&shared);
        let mut buf = [0u8; 1500];
        let (n, _) = ear.recv_from(&mut buf).unwrap();
        let (_, msg) = wire::decode(&buf[..n]).unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        assert_eq!(msg, Message::PLoc { port, kind: "MediaServer".into(), name: "Laser Studio".into(), state: "Idle".into() });
        running.store(false, Ordering::SeqCst);
        t.join().unwrap();
        // Without the test address, the loopback announces nothing.
        assert!(announce_socket(Ipv4Addr::LOCALHOST, None).is_none());
    }

    #[test]
    fn more_consoles_than_allowed_are_closed() {
        let shared = Arc::new(Mutex::new(shared()));
        let (running, t, addr) = start(&shared);
        let hub = Arc::clone(&shared.lock().unwrap().citp.hub);
        let mut keep: Vec<Client> = (0..MAX_PEERS).map(|_| Client::connect(&addr)).collect();
        for c in &mut keep {
            c.read(); // PNam: accepted
        }
        let mut extra = Client::connect(&addr);
        let mut b = [0u8; 64];
        // Closed by the server: end of stream or a reset, not a timeout.
        match extra.stream.read(&mut b) {
            Ok(0) => {}
            Err(e) => assert!(!matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut), "not refused: {e}"),
            Ok(n) => panic!("accepted: {n} bytes"),
        }
        assert_eq!(hub.status().peers.len(), MAX_PEERS);
        running.store(false, Ordering::SeqCst);
        t.join().unwrap();
    }

    #[test]
    fn a_console_that_never_reads_stays_bounded_and_is_closed() {
        let shared = Arc::new(Mutex::new(shared()));
        let hub = Arc::clone(&shared.lock().unwrap().citp.hub);
        super::super::update(&mut shared.lock().unwrap(), &json!({ "enabled": true, "interface": "127.0.0.1", "thumb_max_px": 256 })).unwrap();
        // The server driven by hand, on a clock of our own.
        let mut server = Server::new(Arc::clone(&shared), Arc::clone(&hub));
        let mut now = Instant::now();
        server.step(now);
        let addr = hub.status().local.expect("listening");
        let mut c = Client::connect(&addr);
        // Asks for big pictures again and again, reads nothing.
        c.msex(Msex::CInf { versions: vec![Version::V1_2.word()] });
        let req = ThumbRequest { format: wire::FMT_RGB8, width: 256, height: 256, flags: 0, library_type: MEDIA };
        for _ in 0..100 {
            c.msex(Msex::Geth { req: req.clone(), library: wire::library_id(0), elements: None });
            // One GELT naming the same page 256 times: answered once.
            c.msex(Msex::Gelt { req: req.clone(), libraries: Some(vec![wire::library_id(0); 256]) });
        }
        let mut worst = 0;
        for _ in 0..400 {
            now += Duration::from_millis(5);
            server.step(now);
            worst = worst.max(server.conns.first().map_or(0, |c| c.output.len()));
        }
        assert_eq!(server.conns.len(), 1, "still connected before the stall delay");
        assert!(worst >= MAX_OUTPUT / 2, "the queue filled ({worst} bytes): the test proves something");
        assert!(worst <= MAX_OUTPUT + MAX_MESSAGE + 1024, "bounded output: {worst} bytes");
        // Nothing drains for STALL: closed, its slot freed.
        now += STALL;
        server.step(now);
        assert!(server.conns.is_empty() && hub.status().peers.is_empty(), "closed");
        // And the slot is usable again.
        let mut d = Client::connect(&addr);
        for _ in 0..1000 {
            now += Duration::from_millis(5);
            server.step(now);
            if !server.conns.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(server.conns.len(), 1);
        server.step(now);
        assert_eq!(d.read().1, Message::PNam { name: "Laser Studio".into() });
        drop(c);
    }

    #[test]
    fn removed_pages_are_named_to_msex_1_0_only() {
        let s = shared();
        let a = MediaIndex::build(&s);
        let mut b = a.clone();
        b.pages.pop();
        let gone = wire::library_id(b.pages.len() as u8);
        let u = updates(&a, &b);
        assert!(u.iter().any(|u| u.library == gone && u.for_version(Version::V1_0)));
        for v in [Version::V1_1, Version::V1_2] {
            let sent: Vec<_> = u.iter().filter(|u| u.for_version(v)).collect();
            assert!(sent.iter().all(|u| u.library != gone), "{v:?} names a gone page");
            assert!(sent.iter().any(|u| u.library == wire::ROOT));
        }
    }

    #[test]
    fn updates_name_what_changed() {
        let s = shared();
        let a = MediaIndex::build(&s);
        let mut b = a.clone();
        assert!(updates(&a, &b).is_empty());
        b.pages[0].cues[1].serial ^= 1;
        b.pages[0].serial ^= 1;
        let u = updates(&a, &b);
        assert_eq!(u.len(), 1);
        assert_eq!((u[0].library, u[0].flags, u[0].elements[0]), (wire::library_id(0), wire::UP_ELEMENTS_UPDATED, 2));
        b.pages.pop();
        let u = updates(&a, &b);
        assert_eq!(u[0].library, wire::ROOT);
        assert!(u[0].flags & wire::UP_LIBRARIES_ADDED_REMOVED != 0);
    }
}
