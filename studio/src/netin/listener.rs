//! The "osc-in" and "artnet-in" threads: each owns its UDP socket, binds
//! it when its protocol is switched on (and again when the address
//! changes), decodes datagrams and queues them for the engine without ever
//! blocking (`try_send`; a full queue drops and counts). Blackout is not
//! queued behind anything: the thread trips the e-stop latch itself.
//!
//! Receive only: these sockets never send a byte.

use super::artnet::{self, Packet, CHART};
use super::osc::{self, OscCommand};
use super::{NetEvent, NetHub, Proto};
use crate::interlock::ArmSource;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Longest wait in `recv`, so a change of settings or a shutdown is seen.
const POLL: Duration = Duration::from_millis(100);
/// A failed bind is retried this often.
const RETRY: Duration = Duration::from_secs(2);
/// Position of the blackout channel in the chart.
const BLACKOUT: usize = CHART.len() - 1;

/// Starts both listener threads. A thread that can't be created is logged
/// and skipped: the studio runs without that input.
pub fn spawn(hub: Arc<NetHub>, tx: SyncSender<NetEvent>, running: Arc<AtomicBool>) -> Vec<JoinHandle<()>> {
    [(Proto::Osc, "osc-in"), (Proto::ArtNet, "artnet-in")]
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
}

impl Listener {
    fn new(proto: Proto, hub: Arc<NetHub>, tx: SyncSender<NetEvent>) -> Self {
        Listener { proto, hub, tx, socket: None, failed: None, blackout_high: false }
    }

    fn run(mut self, running: &AtomicBool) {
        let mut buf = vec![0u8; osc::MAX_PACKET];
        while running.load(Ordering::SeqCst) {
            self.reconcile();
            let Some((_, sock)) = &self.socket else {
                std::thread::sleep(POLL);
                continue;
            };
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => self.packet(&buf[..n], from),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => {
                    log::debug!("{} : réception : {e}", self.proto.label());
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    /// Binds, rebinds or closes the socket to match the settings.
    fn reconcile(&mut self) {
        let want = self.hub.effective().listen(self.proto);
        if self.socket.as_ref().map(|(a, _)| *a) == want {
            return;
        }
        if self.socket.take().is_some() {
            log::info!("{} : écoute arrêtée", self.proto.label());
        }
        self.blackout_high = false;
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

    fn packet(&mut self, data: &[u8], from: SocketAddr) {
        match self.proto {
            Proto::Osc => self.osc(data, from),
            Proto::ArtNet => self.artnet(data, from),
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
                self.hub.estop.trip(ArmSource::Network);
            }
            self.queue(NetEvent::Osc { from: from.ip(), msg, cmd });
        }
    }

    fn artnet(&mut self, data: &[u8], from: SocketAddr) {
        let (universe, data) = match artnet::decode(data) {
            Ok(Packet::Dmx { universe, data }) => (universe, data),
            Ok(Packet::Other(_)) => return, // ArtPoll and co: we never answer
            Err(e) => {
                log::debug!("Art-Net de {from} : {e}");
                return self.hub.update_status(Proto::ArtNet, |st| st.rejected += 1);
            }
        };
        let cfg = self.hub.effective().artnet;
        if universe != cfg.universe {
            return self.hub.update_status(Proto::ArtNet, |st| st.ignored += 1);
        }
        self.seen(from);
        let values = artnet::chart_values(&data, cfg.address);
        if let Some(v) = values[BLACKOUT] {
            let high = artnet::blackout_on(v);
            if high && !self.blackout_high {
                self.hub.estop.trip(ArmSource::Network);
            }
            self.blackout_high = high;
        }
        self.queue(NetEvent::Dmx { from: from.ip(), values });
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
    #[test]
    fn udp_round_trip_on_loopback() {
        let mut s = shared();
        s.netin = NetInState::new(NetHub::in_memory(Arc::clone(&s.estop)));
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
