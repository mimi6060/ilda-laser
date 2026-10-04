//! The "osc-in", "artnet-in" and "sacn-in" threads: each owns its UDP
//! socket, binds it when its protocol is switched on (and again when the
//! address changes), decodes datagrams and queues them for the engine
//! without ever blocking (`try_send`; a full queue drops and counts).
//! Blackout is not queued behind anything: the thread trips the e-stop
//! latch itself.
//!
//! sACN joins the multicast groups of its universes only when the operator
//! ticked *Multidiffusion*, on the interface they picked (one socket bound
//! to each group address, never to all interfaces).
//!
//! Receive only: these sockets never send a byte.

use super::artnet::{self, Packet, CHART};
use super::osc::{self, OscCommand};
use super::patch::UniverseEvent;
use super::sacn::{self, Merger, Verdict};
use super::{DmxSource, NetEvent, NetHub, Proto};
use crate::interlock::ArmSource;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Longest wait in `recv`, so a change of settings or a shutdown is seen.
const POLL: Duration = Duration::from_millis(100);
/// The wait when multicast sockets are polled too.
const POLL_FAST: Duration = Duration::from_millis(5);
/// A failed bind is retried this often.
const RETRY: Duration = Duration::from_secs(2);
/// Position of the blackout channel in the chart.
const BLACKOUT: usize = CHART.len() - 1;

/// Starts the listener threads. A thread that can't be created is logged
/// and skipped: the studio runs without that input.
pub fn spawn(hub: Arc<NetHub>, tx: SyncSender<NetEvent>, running: Arc<AtomicBool>) -> Vec<JoinHandle<()>> {
    [(Proto::Osc, "osc-in"), (Proto::ArtNet, "artnet-in"), (Proto::Sacn, "sacn-in")]
        .into_iter()
        .filter_map(|(proto, name)| {
            let (hub, tx, running) = (Arc::clone(&hub), tx.clone(), Arc::clone(&running));
            std::thread::Builder::new()
                .name(name.into())
                .spawn(move || Listener::new(proto, hub, tx).run(&running))
                .map_err(|e| log::warn!("{} : impossible de démarrer le thread : {e}", proto.label()))
                .ok()
        })
        .collect()
}

/// sACN multicast: the interface, the universes and the port joined.
type Joined = (Ipv4Addr, Vec<u16>, u16);

struct Listener {
    proto: Proto,
    hub: Arc<NetHub>,
    tx: SyncSender<NetEvent>,
    /// The address asked for and the socket bound to it.
    socket: Option<(SocketAddr, UdpSocket)>,
    /// Last failed bind, to retry it later rather than in a loop.
    failed: Option<(SocketAddr, Instant)>,
    /// Art-Net: the blackout channel was high in the last packet.
    blackout_high: bool,
    /// sACN: the sources of each universe.
    merger: Merger,
    /// sACN multicast: what was joined, and one socket per group.
    joined: Option<Joined>,
    groups: Vec<UdpSocket>,
}

impl Listener {
    fn new(proto: Proto, hub: Arc<NetHub>, tx: SyncSender<NetEvent>) -> Self {
        Listener { proto, hub, tx, socket: None, failed: None, blackout_high: false, merger: Merger::default(), joined: None, groups: Vec::new() }
    }

    fn run(mut self, running: &AtomicBool) {
        let mut buf = vec![0u8; osc::MAX_PACKET];
        while running.load(Ordering::SeqCst) {
            self.reconcile();
            let Some((_, sock)) = &self.socket else {
                std::thread::sleep(POLL);
                continue;
            };
            let _ = sock.set_read_timeout(Some(if self.groups.is_empty() { POLL } else { POLL_FAST }));
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => self.packet(&buf[..n], from),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => {
                    log::debug!("{} : réception : {e}", self.proto.label());
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            // Multicast groups (non-blocking): whatever has arrived.
            let mut got = Vec::new();
            for g in &self.groups {
                while let Ok((n, from)) = g.recv_from(&mut buf) {
                    got.push((buf[..n].to_vec(), from));
                }
            }
            for (data, from) in got {
                self.packet(&data, from);
            }
        }
    }

    /// Binds, rebinds or closes the socket to match the settings.
    fn reconcile(&mut self) {
        let cfg = self.hub.effective();
        if self.proto == Proto::Sacn {
            self.reconcile_groups(cfg.multicast().map(|(ip, u)| (ip, u, cfg.sacn.port)));
        }
        let want = self.hub.listen_addr(&cfg, self.proto);
        if self.socket.as_ref().map(|(a, _)| *a) == want {
            return;
        }
        if self.socket.take().is_some() {
            log::info!("{} : écoute arrêtée", self.proto.label());
        }
        self.blackout_high = false;
        self.merger = Merger::default();
        let Some(addr) = want else {
            self.failed = None;
            self.hub.update_status(self.proto, |st| (st.local, st.error) = (None, None));
            return;
        };
        if self.failed.is_some_and(|(a, at)| a == addr && at.elapsed() < RETRY) {
            return;
        }
        match bind(addr) {
            Ok(sock) => {
                let local = sock.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| addr.to_string());
                log::info!("{} : écoute sur {local}", self.proto.label());
                self.hub.update_status(self.proto, |st| (st.local, st.error) = (Some(local), None));
                self.failed = None;
                self.socket = Some((addr, sock));
            }
            Err(e) => {
                if self.failed.is_none_or(|(a, _)| a != addr) {
                    log::warn!("{} : impossible d'écouter sur {addr} : {e}", self.proto.label());
                }
                let error = format!("impossible d'écouter sur {addr} : {e}");
                self.hub.update_status(self.proto, |st| (st.local, st.error) = (None, Some(error)));
                self.failed = Some((addr, Instant::now()));
            }
        }
    }

    /// Joins or leaves the sACN multicast groups (dropping a socket
    /// leaves its group). Never called with multicast off: nothing joined.
    fn reconcile_groups(&mut self, want: Option<Joined>) {
        if self.joined == want {
            return;
        }
        self.groups.clear();
        self.joined = want.clone();
        let Some((iface, universes, port)) = want else {
            self.hub.update_status(Proto::Sacn, |st| st.groups.clear());
            return;
        };
        let mut joined = Vec::new();
        for u in universes {
            let group = sacn::multicast_group(u);
            match join(group, iface, port) {
                Ok(sock) => {
                    self.groups.push(sock);
                    joined.push(group.to_string());
                }
                Err(e) => {
                    log::warn!("sACN : impossible de rejoindre {group} sur {iface} : {e}");
                    let error = format!("multidiffusion {group} sur {iface} : {e}");
                    self.hub.update_status(Proto::Sacn, |st| st.error = Some(error));
                }
            }
        }
        log::info!("sACN : multidiffusion sur {iface} : {}", joined.join(", "));
        self.hub.update_status(Proto::Sacn, |st| st.groups = joined);
    }

    fn packet(&mut self, data: &[u8], from: SocketAddr) {
        match self.proto {
            Proto::Osc => self.osc(data, from),
            Proto::ArtNet => self.artnet(data, from),
            Proto::Sacn => self.sacn(data, from),
        }
    }

    fn osc(&mut self, data: &[u8], from: SocketAddr) {
        let msgs = match osc::decode(data) {
            Ok(m) => m,
            Err(e) => {
                log::debug!("OSC de {from} : {e}");
                return self.hub.update_status(Proto::Osc, |st| st.rejected += 1);
            }
        };
        self.seen(from);
        for msg in msgs {
            let cmd = osc::command(&msg);
            if cmd == Ok(OscCommand::Blackout) {
                self.hub.estop.trip(ArmSource::Osc);
            }
            self.queue(NetEvent::Osc { from: from.ip(), msg, cmd });
        }
    }

    fn artnet(&mut self, data: &[u8], from: SocketAddr) {
        let (universe, data) = match artnet::decode(data) {
            Ok(Packet::Dmx { universe, data }) => (universe, data),
            Ok(Packet::TimeCode { tc, rate }) => {
                // T-409: for the timecode chase only, when it wants it.
                if self.hub.push_timecode((tc, rate, Instant::now())) {
                    return self.seen(from);
                }
                return self.hub.update_status(Proto::ArtNet, |st| st.ignored += 1);
            }
            Ok(Packet::Other(_)) => return, // ArtPoll and co: we never answer
            Err(e) => {
                log::debug!("Art-Net de {from} : {e}");
                return self.hub.update_status(Proto::ArtNet, |st| st.rejected += 1);
            }
        };
        let cfg = self.hub.effective();
        if !cfg.artnet.enabled {
            // Open for the timecode only (T-409): levels are not ours.
            return self.hub.update_status(Proto::ArtNet, |st| st.ignored += 1);
        }
        let chart = universe == cfg.artnet.universe;
        let patched = cfg.dmx.universes().contains(&universe);
        if !chart && !patched {
            return self.hub.update_status(Proto::ArtNet, |st| st.ignored += 1);
        }
        self.seen(from);
        let addr = from.ip().to_string();
        let source = DmxSource { name: addr.clone(), protocol: Proto::ArtNet.id(), addr: addr.clone(), priority: 0, universe, active: true, at: Instant::now() };
        self.hub.set_sources(Proto::ArtNet, universe, vec![source]);
        if patched {
            self.queue(NetEvent::Universe(UniverseEvent { proto: Proto::ArtNet, universe, from: from.ip(), name: addr, data: data.clone() }));
        }
        if !chart {
            return;
        }
        let values = artnet::chart_values(&data, cfg.artnet.address);
        if let Some(v) = values[BLACKOUT] {
            let high = artnet::blackout_on(v);
            if high && !self.blackout_high {
                self.hub.estop.trip(ArmSource::Dmx);
            }
            self.blackout_high = high;
        }
        self.queue(NetEvent::Dmx { from: from.ip(), values });
    }

    fn sacn(&mut self, data: &[u8], from: SocketAddr) {
        let d = match sacn::decode(data) {
            Ok(sacn::Packet::Data(d)) => d,
            Ok(sacn::Packet::Other(_)) => return, // sync, discovery: not levels
            Err(e) => {
                log::debug!("sACN de {from} : {e}");
                return self.hub.update_status(Proto::Sacn, |st| st.rejected += 1);
            }
        };
        if !self.hub.effective().sacn_wants(d.universe) {
            return self.hub.update_status(Proto::Sacn, |st| st.ignored += 1);
        }
        let now = Instant::now();
        let verdict = self.merger.packet(&d, from.ip().to_string(), now);
        let sources = self.merger.sources(d.universe);
        let top = sources.first().map(|s| (s.priority, s.at));
        let list = sources
            .into_iter()
            .map(|s| DmxSource {
                name: s.name.clone(),
                protocol: Proto::Sacn.id(),
                addr: s.addr.clone(),
                priority: s.priority,
                universe: d.universe,
                active: Some((s.priority, s.at)) == top,
                at: s.at,
            })
            .collect();
        self.hub.set_sources(Proto::Sacn, d.universe, list);
        if verdict != Verdict::Take {
            return;
        }
        self.seen(from);
        let name = if d.name.is_empty() { from.ip().to_string() } else { d.name };
        self.queue(NetEvent::Universe(UniverseEvent { proto: Proto::Sacn, universe: d.universe, from: from.ip(), name, data: d.slots }));
    }

    fn seen(&self, from: SocketAddr) {
        self.hub.update_status(self.proto, |st| {
            st.packets += 1;
            st.last_at = Some(Instant::now());
            st.last_from = Some(from.ip().to_string());
        });
    }

    fn queue(&self, ev: NetEvent) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(ev) {
            self.hub.update_status(self.proto, |st| st.dropped += 1);
        }
    }
}

fn bind(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let sock = UdpSocket::bind(addr)?;
    sock.set_read_timeout(Some(POLL))?;
    Ok(sock)
}

/// A socket bound to one sACN group's address, joined on `iface` only.
fn join(group: Ipv4Addr, iface: Ipv4Addr, port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // Other sACN receivers on this Mac may listen to the same group.
    sock.set_reuse_address(true)?;
    sock.bind(&SocketAddrV4::new(group, port).into())?;
    sock.join_multicast_v4(&group, &iface)?;
    sock.set_nonblocking(true)?;
    Ok(sock.into())
}

#[cfg(test)]
mod tests {
    use super::super::osc::tests::encode;
    use super::super::osc::OscArg;
    use super::super::{artnet, frame, NetInConfig, NetInState};
    use super::*;
    use crate::test_support::shared;

    fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Loopback only, ports picked by the system; nothing is ever sent to
    /// another host.
    /// A studio with the external gate open on every group, no access
    /// needed (T-408).
    fn open_studio() -> crate::Shared {
        use crate::external::{self, ExternalConfig, SourceKind, SourceScope, SCOPE_SETS};
        let mut s = shared();
        s.netin = NetInState::new(NetHub::in_memory(Arc::clone(&s.estop)));
        let groups: Vec<String> = SCOPE_SETS.iter().flat_map(|set| set.groups.iter().map(|g| g.to_string())).collect();
        s.external.set_config(ExternalConfig { scopes: SourceKind::ALL.iter().map(|&source| SourceScope { source, groups: groups.clone(), require_access: false }).collect() });
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        s
    }

    /// sACN on loopback (unicast, a port the system picks; never
    /// multicast in tests): two sources, the higher priority drives the
    /// patched control; bound to the chosen address only.
    #[test]
    fn sacn_round_trip_on_loopback() {
        use super::super::patch::DmxMapping;
        use super::super::sacn::tests::packet_from;
        let mut s = open_studio();
        let hub = Arc::clone(&s.netin.hub);
        assert!([Proto::Osc, Proto::ArtNet, Proto::Sacn].iter().all(|&p| hub.effective().listen(p).is_none()), "nothing opens by default");
        let mut cfg = NetInConfig::default();
        (cfg.sacn.enabled, cfg.sacn.port, cfg.sacn.universes) = (true, 0, vec![1]);
        cfg.dmx.mappings = vec![DmxMapping { control: "master.perspective".into(), pickup: false, ..DmxMapping::default() }];
        hub.set_file(cfg).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let threads = spawn(Arc::clone(&hub), s.netin.sender(), Arc::clone(&running));
        let local = || hub.status(Proto::Sacn).local.and_then(|a| a.parse::<SocketAddr>().ok());
        wait_for("the sACN socket", || local().is_some());
        let addr = local().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1", "bound to the chosen address, not all interfaces");
        assert!(hub.status(Proto::Sacn).groups.is_empty(), "no multicast unless asked");
        assert!(hub.status(Proto::Osc).local.is_none() && hub.status(Proto::ArtNet).local.is_none());

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.send_to(&packet_from([1; 16], 1, "Basse", 50, 0, 0, &[255]), addr).unwrap();
        client.send_to(&packet_from([2; 16], 1, "Haute", 150, 0, 0, &[64]), addr).unwrap();
        client.send_to(&packet_from([1; 16], 1, "Basse", 50, 1, 0, &[255]), addr).unwrap();
        client.send_to(&packet_from([3; 16], 9, "Autre", 200, 0, 0, &[255]), addr).unwrap(); // not listened to
        client.send_to(b"garbage", addr).unwrap();
        wait_for("the packets", || hub.status(Proto::Sacn).rejected == 1 && hub.status(Proto::Sacn).ignored == 1);
        wait_for("both sources", || hub.sources(Instant::now()).len() == 2);
        frame(&mut s, Instant::now());
        assert!((s.live.perspective - 64.0 / 255.0).abs() < 1e-3, "the priority 150 source wins");
        let sources = hub.sources(Instant::now());
        let active: Vec<&str> = sources.iter().filter(|x| x.active).map(|x| x.name.as_str()).collect();
        assert_eq!(active, ["Haute"]);
        assert!(!s.gate.is_armed());

        let mut cfg = hub.file();
        cfg.sacn.enabled = false;
        hub.set_file(cfg).unwrap();
        wait_for("sACN closed", || hub.status(Proto::Sacn).local.is_none());
        running.store(false, Ordering::SeqCst);
        for t in threads {
            t.join().unwrap();
        }
    }

    #[test]
    fn udp_round_trip_on_loopback() {
        let mut s = open_studio();
        let mut cfg = NetInConfig::default();
        (cfg.osc.enabled, cfg.osc.port, cfg.artnet.enabled, cfg.artnet.port, cfg.artnet.universe) = (true, 0, true, 0, 5);
        s.netin.hub.set_file(cfg).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let threads = spawn(Arc::clone(&s.netin.hub), s.netin.sender(), Arc::clone(&running));
        let hub = Arc::clone(&s.netin.hub);
        let local = |p| hub.status(p).local.and_then(|a| a.parse::<SocketAddr>().ok());
        wait_for("both sockets", || local(Proto::Osc).is_some() && local(Proto::ArtNet).is_some());
        let (osc_addr, art_addr) = (local(Proto::Osc).unwrap(), local(Proto::ArtNet).unwrap());
        assert!(osc_addr.ip().is_loopback() && art_addr.ip().is_loopback());

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.send_to(&encode("/ls/control/master.perspective", &[OscArg::Float(0.5)]), osc_addr).unwrap();
        client.send_to(b"garbage", osc_addr).unwrap();
        let mut data = [0u8; 16];
        data[0] = 51;
        client.send_to(&artnet::tests::art_dmx(4, &[255; 16]), art_addr).unwrap(); // another universe
        client.send_to(&artnet::tests::art_dmx(5, &data), art_addr).unwrap();
        wait_for("the packets", || hub.status(Proto::Osc).rejected == 1 && hub.status(Proto::ArtNet).packets == 1);
        frame(&mut s, Instant::now());
        assert_eq!(s.live.perspective, 0.5);
        assert!((s.live.brightness - 0.2).abs() < 1e-6);
        assert_eq!(hub.status(Proto::ArtNet).ignored, 1);
        assert_eq!(hub.status(Proto::Osc).last_from.as_deref(), Some("127.0.0.1"));

        // Blackout trips the latch from the listener, before any frame.
        client.send_to(&encode("/ls/blackout", &[]), osc_addr).unwrap();
        wait_for("the e-stop", || s.estop.is_latched());
        assert!(!s.gate.is_armed());

        // Switched off: the socket closes.
        let mut cfg = hub.file();
        cfg.osc.enabled = false;
        hub.set_file(cfg).unwrap();
        wait_for("OSC closed", || hub.status(Proto::Osc).local.is_none());
        running.store(false, Ordering::SeqCst);
        for t in threads {
            t.join().unwrap();
        }
    }

    /// Art-Net timecode (T-409) on loopback: the socket opens only while
    /// the timecode wants it, takes ArtTimeCode and ignores levels (the
    /// chart is off), and closes again.
    #[test]
    fn art_timecode_round_trip_on_loopback() {
        use crate::timecode::{artnet::encode, Rate, Tc};
        let hub = Arc::new(NetHub::in_memory(Arc::default()));
        let mut cfg = NetInConfig::default();
        cfg.artnet.port = 0;
        hub.set_file(cfg).unwrap();
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let running = Arc::new(AtomicBool::new(true));
        let thread = {
            let (hub, running) = (Arc::clone(&hub), Arc::clone(&running));
            std::thread::spawn(move || Listener::new(Proto::ArtNet, hub, tx).run(&running))
        };
        std::thread::sleep(Duration::from_millis(250));
        assert!(hub.status(Proto::ArtNet).local.is_none(), "closed until the timecode wants it");
        hub.set_timecode(true, Some(bind));
        let local = || hub.status(Proto::ArtNet).local.and_then(|a| a.parse::<SocketAddr>().ok());
        wait_for("the Art-Net socket", || local().is_some());
        let addr = local().unwrap();
        assert!(addr.ip().is_loopback());
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let tc = Tc { h: 1, m: 2, s: 3, f: 4 };
        client.send_to(&artnet::tests::art_dmx(0, &[255; 16]), addr).unwrap();
        client.send_to(&encode(tc, Rate::F25), addr).unwrap();
        wait_for("the timecode", || hub.status(Proto::ArtNet).packets == 1);
        assert_eq!(hub.status(Proto::ArtNet).ignored, 1, "levels ignored without the chart");
        let got = hub.take_timecode();
        assert_eq!((got.len(), got[0].0, got[0].1), (1, tc, Rate::F25));
        assert!(rx.try_recv().is_err(), "nothing queued for the controls");
        hub.set_timecode(false, Some(bind));
        wait_for("Art-Net closed", || hub.status(Proto::ArtNet).local.is_none());
        assert!(!hub.push_timecode((tc, Rate::F25, Instant::now())), "not wanted: ignored");
        running.store(false, Ordering::SeqCst);
        thread.join().unwrap();
    }

    #[test]
    fn a_busy_port_is_reported_not_retried_in_a_loop() {
        let taken = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let hub = Arc::new(NetHub::in_memory(Arc::default()));
        let mut cfg = NetInConfig::default();
        (cfg.osc.enabled, cfg.osc.port) = (true, port);
        hub.set_file(cfg).unwrap();
        let (tx, _rx) = std::sync::mpsc::sync_channel(4);
        let mut l = Listener::new(Proto::Osc, Arc::clone(&hub), tx);
        l.reconcile();
        assert!(l.socket.is_none());
        assert!(hub.status(Proto::Osc).error.unwrap().contains("impossible d'écouter"));
        let first = l.failed.unwrap().1;
        l.reconcile();
        assert_eq!(l.failed.unwrap().1, first, "not retried before RETRY");
    }
}
