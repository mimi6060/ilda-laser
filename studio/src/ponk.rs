//! PONK output (T-300): sends the finished frames over UDP to MadMapper /
//! MadLaser, which then drives the ShowNET with its own Laserworld licence.
//! PONK is an open protocol (github.com/madmappersoftware/Ponk,
//! Apache-2.0); the wire format comes from the `ponk-protocol` crate (MIT).
//! Nothing here touches the ShowNET or its protocol.
//!
//! Safety: MadMapper keeps showing the last frame it received when nothing
//! comes in, so this output never goes quiet. Every tick sends a frame, and
//! while disarmed (or after the kill switch) that frame is empty: one path
//! with zero points, which receivers can't mistake for a stray header. The
//! frames it gets are the stage's, after calibration, the safety stage and
//! the arm gate; blanked points end a path and are never sent (MadMapper
//! draws its own blanked travel between paths).

use crate::output::Output;
use crate::patterns::Point;
use anyhow::{Context, Result};
use ponk_protocol::{encode_datagrams, DataFormat, PonkFrame, PonkMetadata, PonkPath, PonkPoint};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

/// MadMapper's conventional PONK port. `--ponk` with no address sends here
/// on this Mac (unicast: a receiver subscribed to the multicast group gets
/// unicast on its port too).
pub const DEFAULT_TARGET: &str = "127.0.0.1:5583";
/// Shown in MadMapper's PONK media list (32 bytes on the wire at most).
const SENDER_NAME: &str = "Laser Studio";
/// Whole datagram, header included: within the spec's 8192 data bytes.
const MAX_DATAGRAM: usize = 8192;

/// Parses `--ponk`: `ip:port` or `host:port`, IPv4 first.
pub fn parse_target(s: &str) -> Result<SocketAddr, String> {
    let addrs: Vec<SocketAddr> = s.to_socket_addrs().map_err(|e| format!("adresse PONK invalide « {s} » : {e}"))?.collect();
    addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()).copied().ok_or_else(|| format!("adresse PONK introuvable : « {s} »"))
}

/// What both the engine thread and the kill switch send through.
struct Link {
    socket: UdpSocket,
    target: SocketAddr,
    sender_id: u32,
    frame_no: AtomicU8,
    armed: AtomicBool,
}

impl Link {
    /// Encodes and sends one frame; `points` are ignored while disarmed.
    fn send(&self, points: &[Point]) -> Result<()> {
        let frame_no = self.frame_no.fetch_add(1, Ordering::Relaxed);
        let lit = if self.armed.load(Ordering::SeqCst) { points } else { &[] };
        let datagrams = match encode(self.sender_id, frame_no, lit) {
            Ok(d) => d,
            Err(e) => {
                // Too big to encode: an empty frame rather than nothing.
                let _ = self.send_datagrams(&encode(self.sender_id, frame_no, &[])?);
                return Err(e);
            }
        };
        self.send_datagrams(&datagrams)
    }

    fn send_datagrams(&self, datagrams: &[Vec<u8>]) -> Result<()> {
        for d in datagrams {
            self.socket.send_to(d, self.target).with_context(|| format!("envoi PONK vers {} impossible", self.target))?;
        }
        Ok(())
    }
}

pub struct PonkOutput {
    name: String,
    link: Arc<Link>,
    /// The last send error, logged once until it changes.
    last_error: Option<String>,
}

impl PonkOutput {
    /// Opens a UDP socket towards `target`. The sender id is stable across
    /// runs (kept in `<data_dir>/ponk.json`) so MadMapper reconnects the
    /// same PONK media. Starts disarmed: only empty frames go out.
    pub fn open(target: SocketAddr, data_dir: &Path) -> Result<Self> {
        let bind: SocketAddr = match target.ip() {
            IpAddr::V4(ip) if ip.is_loopback() => (Ipv4Addr::LOCALHOST, 0).into(),
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(ip) if ip.is_loopback() => (Ipv6Addr::LOCALHOST, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(bind).context("failed to open the PONK socket")?;
        if target.ip().is_multicast() && target.is_ipv4() {
            // Only if the user asked for a group: stay on the local network.
            socket.set_multicast_ttl_v4(1).context("failed to set the PONK multicast TTL")?;
        }
        // The engine thread never waits on the network: a full buffer drops
        // the frame (the next one comes 16 ms later).
        socket.set_nonblocking(true).context("failed to configure the PONK socket")?;
        let link = Link {
            socket,
            target,
            sender_id: sender_id(data_dir),
            frame_no: AtomicU8::new(0),
            armed: AtomicBool::new(false),
        };
        Ok(Self { name: format!("PONK → MadMapper ({target})"), link: Arc::new(link), last_error: None })
    }

    fn report(&mut self, result: Result<()>) {
        match result {
            Ok(()) => self.last_error = None,
            Err(e) => {
                let msg = format!("{e:#}");
                if self.last_error.as_deref() != Some(&msg) {
                    log::warn!("{msg}");
                    self.last_error = Some(msg);
                }
            }
        }
    }
}

impl Output for PonkOutput {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> &'static str {
        "ponk"
    }
    fn set_armed(&mut self, armed: bool) -> Result<()> {
        self.link.armed.store(armed, Ordering::SeqCst);
        Ok(())
    }
    fn send(&mut self, points: &[Point]) {
        let result = self.link.send(points);
        self.report(result);
    }
    fn blank_now(&mut self) -> Result<()> {
        self.link.send(&[])
    }
    /// The e-stop: disarm and send an empty frame at once, from whichever
    /// thread trips it.
    fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        let link = Arc::clone(&self.link);
        Some(Box::new(move || {
            link.armed.store(false, Ordering::SeqCst);
            if let Err(e) = link.send(&[]) {
                log::warn!("emergency blank of the PONK output failed: {e:#}");
            }
        }))
    }
}

/// One PONK frame as datagrams (≤ 8192 bytes each). No lit point gives
/// the empty frame.
fn encode(sender_id: u32, frame_number: u8, points: &[Point]) -> Result<Vec<Vec<u8>>> {
    let frame = PonkFrame { sender_id, sender_name: SENDER_NAME.into(), frame_number, paths: lit_paths(points) };
    encode_datagrams(&frame, DataFormat::XyF32RgbU8, MAX_DATAGRAM).map_err(|e| anyhow::anyhow!("PONK frame not encodable: {e}"))
}

/// Splits a frame into PONK paths: one per run of lit points. Blanked
/// points (and any non-finite coordinate) end the current path and are
/// dropped. Each path asks MadMapper to keep our order (`PRESRVOR`) and
/// carries its index (`PATHNUMB`); the rest (angle optimisation, minimum
/// points, speed) stays on the MadMapper surface's settings.
fn lit_paths(points: &[Point]) -> Vec<PonkPath> {
    let mut paths = Vec::new();
    let mut current: Vec<PonkPoint> = Vec::new();
    fn close(current: &mut Vec<PonkPoint>, paths: &mut Vec<PonkPath>) {
        if !current.is_empty() {
            let metadata = vec![
                PonkMetadata { key: "PATHNUMB".into(), value: paths.len() as f32 },
                PonkMetadata { key: "PRESRVOR".into(), value: 1.0 },
            ];
            paths.push(PonkPath { metadata, points: std::mem::take(current) });
        }
    }
    for p in points {
        if p.is_lit() && p.x.is_finite() && p.y.is_finite() {
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            current.push(PonkPoint { x: p.x.clamp(-1.0, 1.0), y: p.y.clamp(-1.0, 1.0), rgb: [c(p.r), c(p.g), c(p.b)] });
        } else {
            close(&mut current, &mut paths);
        }
    }
    close(&mut current, &mut paths);
    paths
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SenderFile {
    sender_id: u32,
}

/// The stable sender id, created on first use.
fn sender_id(data_dir: &Path) -> u32 {
    let path = data_dir.join("ponk.json");
    if let Some(f) = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<SenderFile>(&s).ok()) {
        return f.sender_id;
    }
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    h.write_u32(std::process::id());
    let id = h.finish() as u32;
    crate::save_json(&path, &SenderFile { sender_id: id });
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::{ArmSource, EStop};
    use crate::output::OutputStage;
    use ponk_protocol::{decode_datagram, PonkAssembler};
    use std::time::Duration;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-ponk-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A local receiver on 127.0.0.1 and a port the OS picks: never 5583,
    /// never multicast, never a real MadMapper.
    fn receiver() -> (UdpSocket, SocketAddr) {
        let rx = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let addr = rx.local_addr().unwrap();
        assert_ne!(addr.port(), 5583);
        (rx, addr)
    }

    /// The next complete frame on the receiver.
    fn next_frame(rx: &UdpSocket, asm: &mut PonkAssembler) -> PonkFrame {
        let mut buf = vec![0u8; 65_536];
        loop {
            let (n, from) = rx.recv_from(&mut buf).expect("a PONK datagram");
            if let Some(f) = asm.push_datagram(&buf[..n], from).unwrap() {
                return f;
            }
        }
    }

    fn lit_count(f: &PonkFrame) -> usize {
        f.paths.iter().map(|p| p.points.len()).sum()
    }

    fn square() -> Vec<Point> {
        vec![
            Point::blanked(-0.5, -0.5),
            Point::lit(-0.5, -0.5, 1.0, 0.0, 0.0),
            Point::lit(0.5, -0.5, 0.0, 1.0, 0.0),
            Point::lit(0.5, 0.5, 0.0, 0.0, 1.0),
            Point::blanked(0.5, 0.5),
            Point::blanked(-0.2, 0.3),
            Point::lit(-0.2, 0.3, 1.0, 1.0, 1.0),
            Point::lit(0.1, 0.3, 0.5, 0.25, 0.0),
        ]
    }

    #[test]
    fn encode_then_decode_gives_the_same_paths() {
        let datagrams = encode(7, 42, &square()).unwrap();
        assert_eq!(datagrams.len(), 1);
        let f = decode_datagram(&datagrams[0]).unwrap().unwrap();
        assert_eq!((f.sender_id, f.frame_number, f.sender_name.as_str()), (7, 42, "Laser Studio"));
        assert_eq!(f.paths.len(), 2, "split at the blanked points");
        let xy: Vec<Vec<(f32, f32, [u8; 3])>> = f.paths.iter().map(|p| p.points.iter().map(|q| (q.x, q.y, q.rgb)).collect()).collect();
        assert_eq!(
            xy,
            vec![
                vec![(-0.5, -0.5, [255, 0, 0]), (0.5, -0.5, [0, 255, 0]), (0.5, 0.5, [0, 0, 255])],
                vec![(-0.2, 0.3, [255, 255, 255]), (0.1, 0.3, [128, 64, 0])],
            ]
        );
        for (i, p) in f.paths.iter().enumerate() {
            let meta: Vec<(&str, f32)> = p.metadata.iter().map(|m| (m.key.as_str(), m.value)).collect();
            assert_eq!(meta, vec![("PATHNUMB", i as f32), ("PRESRVOR", 1.0)]);
        }
    }

    #[test]
    fn a_dark_frame_is_an_empty_frame_not_nothing() {
        for points in [vec![], vec![Point::blanked(0.0, 0.0), Point::blanked(0.3, 0.3)]] {
            let datagrams = encode(1, 0, &points).unwrap();
            assert_eq!(datagrams.len(), 1, "always one datagram");
            let f = decode_datagram(&datagrams[0]).unwrap().unwrap();
            assert_eq!(lit_count(&f), 0);
        }
    }

    #[test]
    fn out_of_range_and_non_finite_points_never_leave() {
        let paths = lit_paths(&[
            Point::lit(3.0, -2.0, 2.0, -1.0, 0.5),
            Point::lit(f32::NAN, 0.0, 1.0, 1.0, 1.0),
            Point::lit(0.0, 0.0, 1.0, 1.0, 1.0),
        ]);
        assert_eq!(paths.len(), 2, "the NaN point breaks the path");
        assert_eq!((paths[0].points[0].x, paths[0].points[0].y, paths[0].points[0].rgb), (1.0, -1.0, [255, 0, 128]));
        assert_eq!(paths[1].points.len(), 1);
    }

    #[test]
    fn a_big_frame_is_chunked_and_reassembled_identically() {
        // ~3000 lit points × 11 bytes: several 8 KB datagrams.
        let points: Vec<Point> =
            (0..3000).map(|i| if i % 500 == 499 { Point::blanked(0.0, 0.0) } else { Point::lit((i as f32 / 3000.0) * 2.0 - 1.0, 0.25, 1.0, 0.5, 0.0) }).collect();
        let datagrams = encode(9, 3, &points).unwrap();
        assert!(datagrams.len() > 1);
        assert!(datagrams.iter().all(|d| d.len() <= 8192));
        let mut asm = PonkAssembler::new();
        let from: SocketAddr = (Ipv4Addr::LOCALHOST, 1).into();
        let mut frame = None;
        for d in &datagrams {
            frame = asm.push_datagram(d, from).unwrap().or(frame);
        }
        let frame = frame.expect("reassembled");
        assert_eq!(frame.paths, lit_paths(&points));
        assert_eq!(frame.paths.len(), 6);
        assert_eq!(lit_count(&frame), 2994);
    }

    #[test]
    fn the_sender_id_is_kept_across_runs() {
        let dir = temp_dir("id");
        let _ = std::fs::remove_file(dir.join("ponk.json"));
        let a = sender_id(&dir);
        assert_eq!(sender_id(&dir), a);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parse_target_takes_ip_and_host() {
        assert_eq!(parse_target("127.0.0.1:6000").unwrap(), "127.0.0.1:6000".parse().unwrap());
        assert!(parse_target("localhost:6000").unwrap().ip().is_loopback());
        assert!(parse_target("127.0.0.1").is_err(), "a port is required");
        assert!(parse_target("nope").is_err());
    }

    /// Over a real (local) socket through the output stage: disarmed ticks
    /// send empty frames, armed ones the frame, `blank_now` and the kill
    /// switch an empty one at once.
    #[test]
    fn the_stage_drives_empty_frames_unless_armed() {
        let (rx, addr) = receiver();
        let dir = temp_dir("stage");
        let out = PonkOutput::open(addr, &dir).unwrap();
        assert_eq!(out.name(), format!("PONK → MadMapper ({addr})"));
        let kill = out.kill_switch().unwrap();
        let link = Arc::clone(&out.link);
        let mut stage = OutputStage::new(Some(Box::new(out)));
        let mut asm = PonkAssembler::new();
        let estop = EStop::default();

        // Disarmed: one empty frame per tick, never silence.
        for _ in 0..3 {
            assert_eq!(stage.emit(&square(), false, true, &estop).lit, 0);
            assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 0);
        }
        // Armed: the frame, as two paths.
        stage.emit(&square(), true, true, &estop);
        let f = next_frame(&rx, &mut asm);
        assert_eq!((f.paths.len(), lit_count(&f)), (2, 5));
        // Hold-to-run released: dark frames.
        stage.emit(&square(), true, false, &estop);
        assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 0);
        // The e-stop's kill switch: an empty frame right away, then the
        // stage keeps sending empty ones.
        stage.emit(&square(), true, true, &estop);
        assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 5);
        estop.trip(ArmSource::Keyboard);
        kill();
        assert!(!link.armed.load(Ordering::SeqCst));
        assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 0);
        stage.emit(&square(), true, true, &estop);
        assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 0);
        // Shutdown: three empty frames.
        stage.shutdown();
        for _ in 0..3 {
            assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 0);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Even if the stage were bypassed, a disarmed PONK output sends lit
    /// points as an empty frame.
    #[test]
    fn a_disarmed_output_ignores_lit_points() {
        let (rx, addr) = receiver();
        let dir = temp_dir("direct");
        let mut out = PonkOutput::open(addr, &dir).unwrap();
        let mut asm = PonkAssembler::new();
        out.send(&square());
        assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 0);
        out.set_armed(true).unwrap();
        out.send(&square());
        assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 5);
        out.blank_now().unwrap();
        assert_eq!(lit_count(&next_frame(&rx, &mut asm)), 0);
        out.set_armed(false).unwrap();
        out.send(&square());
        let f = next_frame(&rx, &mut asm);
        assert_eq!(lit_count(&f), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
