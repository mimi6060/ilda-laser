//! Target tracking over PosiStageNet (T-457): a tracking system sends the
//! position of markers in metres; a marker *linked* to a mobile target
//! moves that target, which the « Cibles miroir » generator (T-109) lights
//! like a fixed one. Typical use: a beam that follows a mirror on a moving
//! piece of scenery. Following a person (on stage or in the audience) is
//! not what this is for: the zones forbid it.
//!
//! **Protocol.** Implemented from the public specification only:
//! « PosiStageNet Protocol description v2.02 » (VYV Corporation and MA
//! Lighting, 2016-09-15, <https://posistage.net/>), released into the
//! public domain; the protocol may be used by third parties without
//! royalty. No code was copied from any implementation. What we read:
//! UDP, default multicast group 236.10.10.10 port 56565, little-endian,
//! packets of at most 1500 bytes made of chunks (a 32-bit header: id in
//! bits 0-15, data length in bits 16-30, « has sub-chunks » in bit 31);
//! `PSN_DATA_PACKET` (0x6755) with its header (0x0000: timestamp, version,
//! frame id, packet count) and tracker list (0x0001), each tracker (chunk
//! id = tracker id) with its position (0x0000, three floats, metres) and
//! status (0x0003, a validity float); `PSN_INFO_PACKET` (0x6756) with the
//! system name (0x0001) and the tracker names (0x0002 › id › 0x0000).
//! Unknown chunks are skipped, as the specification asks. Speed,
//! orientation, acceleration and target position are not used. Every
//! tracker chunk is complete within its packet, so a frame split over
//! several packets needs no reassembly here: each sample stands alone.
//!
//! **Untrusted input.** Positions come from the network. A sample is
//! taken only if every coordinate is a number within `MAX_COORD_M` and its
//! validity (when sent) is a positive number; the specification gives no
//! scale for the validity, so 0 or less counts as « not valid » (fail
//! dark). The listener keeps the newest sample per tracker (`MAX_TRACKERS`
//! at most, a packet rate cap), the engine reads them once per frame.
//!
//! **What tracking can do.** Move the mobile targets already linked, and
//! nothing else: no arming, no cue, no setting. The data goes through the
//! external gate (T-408, `external::tracking`) with its own source
//! (*PosiStageNet*) and scope (*Suivi de cible*): gate closed, out of
//! scope or switch off = every mobile target dark.
//!
//! **Fail dark.** A mobile target is lit only when all of these hold:
//! the receiver is on and the gate lets the data through; its link has a
//! valid calibration (at least four reference points, not degenerate, a
//! residual error under `MAX_ERROR`); a valid sample arrived less than
//! `lost_after_ms` ago (else « perdu »); the sample is inside the
//! calibrated area (grown by `REGION_MARGIN`) and maps inside the field;
//! it is not a jump faster than `max_speed` (a jump darkens it until a
//! next sample confirms the new place); and the zones, the horizon and
//! the outdoor sectors leave light there (`targets::status`; « hors
//! zone »). Coming back after a loss or a jump never sweeps: the smoothing
//! restarts at the new place.
//!
//! **No exemption.** The generator only makes points; the whole safety
//! stage (zones, horizon, outdoor, the static-beam guard, the strobe
//! limiter, the caps, the arm gate) applies to them as to any look.
//!
//! **Settings.** `<data-dir>/psn.json`, machine settings (never in a
//! project). Switching the receiver on, adding or changing a link,
//! raising a link's speed limit or the loss delay, or changing where the
//! data comes from while on are loosenings: confirmation, refused armed,
//! under the safety lock and in show mode (like the mirror targets).
//! Switching off, removing a link and lowering a limit always run. No
//! socket exists while the receiver is off.

use crate::external::Refusal;
use crate::targets::{self, Target};
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const DEFAULT_GROUP: &str = "236.10.10.10";
pub const DEFAULT_PORT: u16 = 56565;
/// The specification's largest packet.
pub const MAX_PACKET: usize = 1500;
pub const MAX_LINKS: usize = 16;
pub const MAX_REFS: usize = 16;
/// Trackers the listener keeps at most (more are ignored and counted).
pub const MAX_TRACKERS: usize = 64;
pub const MAX_NAME: usize = 40;
/// Positions beyond this many metres from the origin are not taken.
pub const MAX_COORD_M: f32 = 1000.0;
pub const LOST_MIN_MS: u32 = 100;
pub const LOST_MAX_MS: u32 = 2000;
pub const SMOOTH_MAX_MS: u32 = 1000;
pub const SPEED_MIN: f32 = 0.1;
pub const SPEED_MAX: f32 = 20.0;
/// Mean residual of a calibration at most (output units; the field is 2
/// wide, so 0.1 = 5 %).
pub const MAX_ERROR: f32 = 0.1;
/// The calibrated area is the box of the reference points grown by this
/// share of its size on each side.
pub const REGION_MARGIN: f32 = 0.25;
/// Packets the listener takes per second at most.
const MAX_PACKETS_PER_S: u32 = 2000;
/// A tracker silent this long leaves the list.
const FORGET: Duration = Duration::from_secs(10);
/// Shortest interval the speed check divides by (a burst of packets
/// arriving together is not an infinite speed).
const MIN_DT: f32 = 1.0 / 120.0;
const POLL: Duration = Duration::from_millis(100);
const RETRY: Duration = Duration::from_secs(2);

// ---------- settings ----------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    X,
    Y,
    Z,
    NegX,
    NegY,
    NegZ,
}

impl Axis {
    fn base(self) -> usize {
        match self {
            Axis::X | Axis::NegX => 0,
            Axis::Y | Axis::NegY => 1,
            Axis::Z | Axis::NegZ => 2,
        }
    }

    fn of(self, p: [f32; 3]) -> f32 {
        let v = p[self.base()];
        match self {
            Axis::NegX | Axis::NegY | Axis::NegZ => -v,
            _ => v,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RefPoint {
    /// The tracker's position (metres).
    pub tracker: [f32; 3],
    /// Where the laser was aimed (look coordinates, like a mirror target).
    pub output: [f32; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackerLink {
    pub tracker_id: u16,
    /// The mobile target's name.
    pub target: String,
    /// Which tracker axes make the plane (horizontal, vertical).
    pub axes: [Axis; 2],
    pub refs: Vec<RefPoint>,
    /// Output units per second; a faster move is a jump.
    pub max_speed: f32,
}

impl Default for TrackerLink {
    fn default() -> Self {
        Self { tracker_id: 1, target: "Cible mobile".into(), axes: [Axis::X, Axis::Y], refs: Vec::new(), max_speed: 4.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PsnSettings {
    pub enabled: bool,
    /// The IPv4 address of the interface to listen on; `None` = this Mac
    /// only (127.0.0.1, unicast, no multicast).
    pub interface: Option<String>,
    pub group: String,
    /// 0: the system picks one (tests); the status shows which.
    pub port: u16,
    pub lost_after_ms: u32,
    pub smoothing_ms: u32,
    pub links: Vec<TrackerLink>,
}

impl Default for PsnSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interface: None,
            group: DEFAULT_GROUP.into(),
            port: DEFAULT_PORT,
            lost_after_ms: 500,
            smoothing_ms: 50,
            links: Vec::new(),
        }
    }
}

fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect::<String>().trim().to_string()
}

fn finite3(p: [f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD_M)
}

impl PsnSettings {
    /// Refuses what the store must never hold (French messages).
    pub fn validate(&self) -> Result<(), String> {
        let group: Ipv4Addr = self.group.trim().parse().map_err(|_| format!("groupe « {} » : adresse IPv4 attendue", self.group))?;
        if !group.is_multicast() {
            return Err(format!("groupe {group} : une adresse de multidiffusion est attendue (224.0.0.0 à 239.255.255.255)"));
        }
        if let Some(iface) = &self.interface {
            let ip: Ipv4Addr = iface.trim().parse().map_err(|_| format!("interface « {iface} » : adresse IPv4 attendue"))?;
            if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
                return Err("interface : choisissez l'adresse IPv4 d'une interface de ce Mac (ni 0.0.0.0, ni multidiffusion)".into());
            }
        }
        if !(LOST_MIN_MS..=LOST_MAX_MS).contains(&self.lost_after_ms) {
            return Err(format!("extinction après perte : de {LOST_MIN_MS} à {LOST_MAX_MS} ms"));
        }
        if self.smoothing_ms > SMOOTH_MAX_MS {
            return Err(format!("lissage : de 0 à {SMOOTH_MAX_MS} ms"));
        }
        if self.links.len() > MAX_LINKS {
            return Err(format!("{} affectations : {MAX_LINKS} au plus", self.links.len()));
        }
        for (i, l) in self.links.iter().enumerate() {
            let n = i + 1;
            let name = l.target.trim();
            if name.is_empty() {
                return Err(format!("Affectation {n} : nom de cible obligatoire"));
            }
            if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
                return Err(format!("Affectation {n} : nom invalide ({MAX_NAME} caractères au plus)"));
            }
            if self.links[..i].iter().any(|o| o.target.trim() == name) {
                return Err(format!("Affectation {n} : la cible « {name} » existe déjà"));
            }
            if l.axes[0].base() == l.axes[1].base() {
                return Err(format!("Affectation {n} : deux axes différents sont attendus"));
            }
            if !(l.max_speed.is_finite() && (SPEED_MIN..=SPEED_MAX).contains(&l.max_speed)) {
                return Err(format!("Affectation {n} : vitesse max de {SPEED_MIN} à {SPEED_MAX}"));
            }
            if l.refs.len() > MAX_REFS {
                return Err(format!("Affectation {n} : {MAX_REFS} points de référence au plus"));
            }
            for r in &l.refs {
                if !finite3(r.tracker) || !r.output.iter().all(|v| v.is_finite() && (-1.0..=1.0).contains(v)) {
                    return Err(format!("Affectation {n} : point de référence invalide"));
                }
            }
        }
        Ok(())
    }

    /// For a file edited by hand: invalid parts dropped or brought into
    /// range; whatever cannot be repaired switches the receiver off.
    pub fn sanitized(&self) -> Self {
        let mut s = self.clone();
        s.lost_after_ms = s.lost_after_ms.clamp(LOST_MIN_MS, LOST_MAX_MS);
        s.smoothing_ms = s.smoothing_ms.min(SMOOTH_MAX_MS);
        let mut links: Vec<TrackerLink> = Vec::new();
        for l in s.links.iter().take(MAX_LINKS) {
            let name = clean_name(&l.target);
            if name.is_empty() || links.iter().any(|o| o.target == name) || l.axes[0].base() == l.axes[1].base() {
                continue;
            }
            let refs = l
                .refs
                .iter()
                .filter(|r| finite3(r.tracker) && r.output.iter().all(|v| v.is_finite() && (-1.0..=1.0).contains(v)))
                .take(MAX_REFS)
                .copied()
                .collect();
            let max_speed = if l.max_speed.is_finite() { l.max_speed.clamp(SPEED_MIN, SPEED_MAX) } else { TrackerLink::default().max_speed };
            links.push(TrackerLink { target: name, refs, max_speed, ..l.clone() });
        }
        s.links = links;
        if s.validate().is_err() {
            s = PsnSettings { links: s.links, ..PsnSettings::default() };
        }
        s
    }

    /// What going from `self` to `new` would loosen (French), for the
    /// operator's confirmation. Empty = only tighter or equal.
    pub fn loosenings(&self, new: &PsnSettings) -> Vec<String> {
        let mut out = Vec::new();
        if new.enabled && !self.enabled {
            out.push("Suivi activé : des cibles mobiles pourront s'allumer".to_string());
        }
        if new.enabled && self.enabled && (new.interface != self.interface || new.group != self.group || new.port != self.port) {
            out.push("Source du suivi changée : d'autres données pourront déplacer les cibles".to_string());
        }
        if new.lost_after_ms > self.lost_after_ms {
            out.push(format!("Extinction après perte allongée de {} à {} ms", self.lost_after_ms, new.lost_after_ms));
        }
        for l in &new.links {
            match self.links.iter().find(|o| o.target.trim() == l.target.trim()) {
                None => out.push(format!("Cible mobile « {} » ajoutée", l.target.trim())),
                Some(o) if o.tracker_id != l.tracker_id || o.axes != l.axes || o.refs != l.refs => {
                    out.push(format!("Cible mobile « {} » : traceur ou calibrage changé", l.target.trim()))
                }
                Some(o) if l.max_speed > o.max_speed => out.push(format!("Cible mobile « {} » : vitesse max relevée", l.target.trim())),
                Some(_) => {}
            }
        }
        // A target lights by its place after the fixed ones: a link that
        // takes the place of another one sends light somewhere else.
        for (i, l) in new.links.iter().enumerate() {
            if self.links.get(i).is_some_and(|o| o.target.trim() != l.target.trim()) && self.links.iter().any(|o| o.target.trim() == l.target.trim()) {
                out.push(format!("Cible mobile « {} » : place changée", l.target.trim()));
            }
        }
        out
    }
}

// ---------- calibration ----------

/// A plane-to-plane homography from the tracker's plane (metres) to the
/// look's coordinates, from at least four reference points.
#[derive(Clone, Debug, PartialEq)]
pub struct Calib {
    h: [f64; 9],
    /// The sign the denominator has at the reference points.
    sign: f64,
    /// The calibrated area (tracker plane), grown by `REGION_MARGIN`.
    region: [f32; 4],
    /// Mean residual in output units.
    pub error: f32,
}

fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn collinear(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let d2 = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
    let side = d2(a, b).max(d2(b, c)).max(d2(a, c));
    side <= 1e-12 || cross(a, b, c).abs() / side < 1e-3
}

/// Whether some four of `pts` have no three on a line.
fn has_quad(pts: &[[f64; 2]]) -> bool {
    let n = pts.len();
    for a in 0..n {
        for b in a + 1..n {
            for c in b + 1..n {
                if collinear(pts[a], pts[b], pts[c]) {
                    continue;
                }
                for d in c + 1..n {
                    let (p, q, r, s) = (pts[a], pts[b], pts[c], pts[d]);
                    if !collinear(p, q, s) && !collinear(p, r, s) && !collinear(q, r, s) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Translation and scale bringing `pts` to centroid 0, mean distance √2.
fn normalizer(pts: &[[f64; 2]]) -> [f64; 3] {
    let n = pts.len() as f64;
    let (cx, cy) = (pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n);
    let mean = pts.iter().map(|p| ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()).sum::<f64>() / n;
    let s = if mean > 1e-12 { std::f64::consts::SQRT_2 / mean } else { 1.0 };
    [s, cx, cy]
}

/// Solves `m · x = v` (8 unknowns) with partial pivoting; `None` when
/// singular.
fn solve8(mut m: [[f64; 8]; 8], mut v: [f64; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let piv = (col..8).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[piv][col].abs() < 1e-10 {
            return None;
        }
        m.swap(col, piv);
        v.swap(col, piv);
        let (pivot_row, pivot_v) = (m[col], v[col]);
        for (r, row) in m.iter_mut().enumerate() {
            if r != col {
                let f = row[col] / pivot_row[col];
                for (x, p) in row[col..].iter_mut().zip(&pivot_row[col..]) {
                    *x -= f * p;
                }
                v[r] -= f * pivot_v;
            }
        }
    }
    let mut x = [0.0; 8];
    for i in 0..8 {
        x[i] = v[i] / m[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

fn mat_mul(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    let mut r = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            r[i * 3 + j] = (0..3).map(|k| a[i * 3 + k] * b[k * 3 + j]).sum();
        }
    }
    r
}

impl Calib {
    /// The tracker's plane position for `link`'s axes.
    pub fn plane(axes: [Axis; 2], p: [f32; 3]) -> [f32; 2] {
        [axes[0].of(p), axes[1].of(p)]
    }

    /// The correspondence for `link`, or why there is none (French).
    pub fn from_link(link: &TrackerLink) -> Result<Calib, String> {
        if link.axes[0].base() == link.axes[1].base() {
            return Err("deux axes différents sont attendus".into());
        }
        if link.refs.len() < 4 {
            return Err(format!("{} point(s) de référence sur 4 au moins", link.refs.len()));
        }
        let src: Vec<[f64; 2]> = link.refs.iter().map(|r| Calib::plane(link.axes, r.tracker)).map(|p| [p[0] as f64, p[1] as f64]).collect();
        let dst: Vec<[f64; 2]> = link.refs.iter().map(|r| [r.output[0] as f64, r.output[1] as f64]).collect();
        if !src.iter().chain(&dst).all(|p| p[0].is_finite() && p[1].is_finite()) {
            return Err("point de référence invalide".into());
        }
        if !has_quad(&src) || !has_quad(&dst) {
            return Err("points alignés ou confondus : placez-les aux coins de la zone parcourue".into());
        }
        let ns = normalizer(&src);
        let nd = normalizer(&dst);
        let norm = |n: [f64; 3], p: [f64; 2]| [(p[0] - n[1]) * n[0], (p[1] - n[2]) * n[0]];
        // Least squares on h (h8 = 1): normal equations.
        let mut ata = [[0.0f64; 8]; 8];
        let mut atb = [0.0f64; 8];
        for (s, d) in src.iter().zip(&dst) {
            let [x, y] = norm(ns, *s);
            let [u, v] = norm(nd, *d);
            for (row, rhs) in [([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u), ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v)] {
                for i in 0..8 {
                    for j in 0..8 {
                        ata[i][j] += row[i] * row[j];
                    }
                    atb[i] += row[i] * rhs;
                }
            }
        }
        let hn = solve8(ata, atb).ok_or("calibrage impossible : points dégénérés")?;
        let hn = [hn[0], hn[1], hn[2], hn[3], hn[4], hn[5], hn[6], hn[7], 1.0];
        // H = Td⁻¹ · Hn · Ts.
        let ts = [ns[0], 0.0, -ns[0] * ns[1], 0.0, ns[0], -ns[0] * ns[2], 0.0, 0.0, 1.0];
        let td_inv = [1.0 / nd[0], 0.0, nd[1], 0.0, 1.0 / nd[0], nd[2], 0.0, 0.0, 1.0];
        let h = mat_mul(&td_inv, &mat_mul(&hn, &ts));
        let w = |p: [f64; 2]| h[6] * p[0] + h[7] * p[1] + h[8];
        let ws: Vec<f64> = src.iter().map(|&p| w(p)).collect();
        let sign = ws[0].signum();
        if ws.iter().any(|&v| v.abs() < 1e-9 || v.signum() != sign) {
            return Err("calibrage impossible : correspondance retournée".into());
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &src {
            (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
        }
        let size = (x1 - x0).max(y1 - y0) * REGION_MARGIN as f64;
        let region = [(x0 - size) as f32, (y0 - size) as f32, (x1 + size) as f32, (y1 + size) as f32];
        let mut c = Calib { h, sign, region, error: 0.0 };
        let mut err = 0.0f32;
        for r in &link.refs {
            let Some(q) = c.map_plane(Calib::plane(link.axes, r.tracker)) else {
                return Err("calibrage impossible : un point de référence ne se projette pas".into());
            };
            err += ((q[0] - r.output[0]).powi(2) + (q[1] - r.output[1]).powi(2)).sqrt();
        }
        c.error = err / link.refs.len() as f32;
        if !(c.error.is_finite() && c.error <= MAX_ERROR) {
            return Err(format!("calibrage incohérent : erreur moyenne {:.1} % (au plus {:.0} %)", c.error * 50.0, MAX_ERROR * 50.0));
        }
        Ok(c)
    }

    /// A plane point through the homography, without the area check.
    fn map_plane(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let (x, y) = (p[0] as f64, p[1] as f64);
        let h = &self.h;
        let w = h[6] * x + h[7] * y + h[8];
        if !w.is_finite() || w.abs() < 1e-9 || w.signum() != self.sign {
            return None;
        }
        let q = [((h[0] * x + h[1] * y + h[2]) / w) as f32, ((h[3] * x + h[4] * y + h[5]) / w) as f32];
        q.iter().all(|v| v.is_finite()).then_some(q)
    }

    /// Where a tracker plane point sends the target (look coordinates):
    /// `None` outside the calibrated area or outside the field.
    pub fn map(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let [x0, y0, x1, y1] = self.region;
        if !(p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1) {
            return None;
        }
        self.map_plane(p).filter(|q| q.iter().all(|v| (-1.0..=1.0).contains(v)))
    }

    /// The mean residual in percent of the field's width.
    pub fn error_pct(&self) -> f32 {
        self.error * 50.0
    }
}

// ---------- protocol ----------

pub const DATA_PACKET: u16 = 0x6755;
pub const INFO_PACKET: u16 = 0x6756;
const PACKET_HEADER: u16 = 0x0000;
const DATA_TRACKER_LIST: u16 = 0x0001;
const TRACKER_POS: u16 = 0x0000;
const TRACKER_STATUS: u16 = 0x0003;
const INFO_SYSTEM_NAME: u16 = 0x0001;
const INFO_TRACKER_LIST: u16 = 0x0002;
const INFO_TRACKER_NAME: u16 = 0x0000;
/// The protocol version we read (the high number must match).
const VERSION_HIGH: u8 = 2;
/// Chunks per level at most (a 1500-byte packet holds fewer).
const MAX_CHUNKS: usize = 400;

struct Chunk<'a> {
    id: u16,
    data: &'a [u8],
    sub: bool,
}

fn chunks(mut buf: &[u8]) -> Result<Vec<Chunk<'_>>, String> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        if buf.len() < 4 {
            return Err("en-tête de bloc tronqué".into());
        }
        let v = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let (id, len, sub) = ((v & 0xFFFF) as u16, ((v >> 16) & 0x7FFF) as usize, v >> 31 == 1);
        if buf.len() < 4 + len {
            return Err("bloc tronqué".into());
        }
        out.push(Chunk { id, data: &buf[4..4 + len], sub });
        if out.len() > MAX_CHUNKS {
            return Err("trop de blocs".into());
        }
        buf = &buf[4 + len..];
    }
    Ok(out)
}

fn f32_at(d: &[u8], i: usize) -> f32 {
    f32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackerData {
    pub id: u16,
    pub pos: Option<[f32; 3]>,
    pub validity: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Data { trackers: Vec<TrackerData> },
    Info { system: String, names: Vec<(u16, String)> },
}

fn check_header(root: &[Chunk]) -> Result<(), String> {
    let h = root.iter().find(|c| c.id == PACKET_HEADER && !c.sub).ok_or("en-tête de paquet absent")?;
    if h.data.len() < 12 {
        return Err("en-tête de paquet trop court".into());
    }
    if h.data[8] != VERSION_HIGH {
        return Err(format!("version {}.{} non prise en charge", h.data[8], h.data[9]));
    }
    Ok(())
}

fn text_of(d: &[u8]) -> String {
    clean_name(&String::from_utf8_lossy(d))
}

/// One datagram. Malformed ones are an error (never a panic); unknown
/// chunks are skipped.
pub fn decode(buf: &[u8]) -> Result<Packet, String> {
    if buf.len() > MAX_PACKET {
        return Err(format!("paquet de {} octets : {MAX_PACKET} au plus", buf.len()));
    }
    let top = chunks(buf)?;
    let [root] = top.as_slice() else {
        return Err("un bloc racine attendu".into());
    };
    if !root.sub {
        return Err("bloc racine sans sous-blocs".into());
    }
    let kids = chunks(root.data)?;
    match root.id {
        DATA_PACKET => {
            check_header(&kids)?;
            let mut trackers = Vec::new();
            for list in kids.iter().filter(|c| c.id == DATA_TRACKER_LIST && c.sub) {
                for t in chunks(list.data)?.into_iter().filter(|t| t.sub) {
                    let mut td = TrackerData { id: t.id, pos: None, validity: None };
                    for f in chunks(t.data)? {
                        match f.id {
                            TRACKER_POS if f.data.len() >= 12 && !f.sub => td.pos = Some([f32_at(f.data, 0), f32_at(f.data, 4), f32_at(f.data, 8)]),
                            TRACKER_STATUS if f.data.len() >= 4 && !f.sub => td.validity = Some(f32_at(f.data, 0)),
                            _ => {}
                        }
                    }
                    if trackers.len() < MAX_TRACKERS {
                        trackers.push(td);
                    }
                }
            }
            Ok(Packet::Data { trackers })
        }
        INFO_PACKET => {
            check_header(&kids)?;
            let mut system = String::new();
            let mut names = Vec::new();
            for c in &kids {
                match c.id {
                    INFO_SYSTEM_NAME if !c.sub => system = text_of(c.data),
                    INFO_TRACKER_LIST if c.sub => {
                        for t in chunks(c.data)?.into_iter().filter(|t| t.sub) {
                            if let Some(n) = chunks(t.data)?.into_iter().find(|n| n.id == INFO_TRACKER_NAME && !n.sub) {
                                if names.len() < MAX_TRACKERS {
                                    names.push((t.id, text_of(n.data)));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Packet::Info { system, names })
        }
        id => Err(format!("bloc racine 0x{id:04x} inconnu")),
    }
}

fn chunk(id: u16, data: &[u8], sub: bool) -> Vec<u8> {
    let len = data.len().min(0x7FFF) as u32;
    let v = id as u32 | (len << 16) | ((sub as u32) << 31);
    let mut out = v.to_le_bytes().to_vec();
    out.extend_from_slice(&data[..len as usize]);
    out
}

fn header_chunk(frame_id: u8) -> Vec<u8> {
    let mut h = 0u64.to_le_bytes().to_vec();
    h.extend_from_slice(&[VERSION_HIGH, 3, frame_id, 1]);
    chunk(PACKET_HEADER, &h, false)
}

/// Our encoder, for the tests and the test hook: a data packet.
pub fn encode_data(frame_id: u8, trackers: &[(u16, [f32; 3], Option<f32>)]) -> Vec<u8> {
    let mut list = Vec::new();
    for (id, p, validity) in trackers {
        let mut fields = chunk(TRACKER_POS, &p.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>(), false);
        if let Some(v) = validity {
            fields.extend(chunk(TRACKER_STATUS, &v.to_le_bytes(), false));
        }
        list.extend(chunk(*id, &fields, true));
    }
    let mut body = header_chunk(frame_id);
    body.extend(chunk(DATA_TRACKER_LIST, &list, true));
    chunk(DATA_PACKET, &body, true)
}

/// Our encoder: an info packet (system and tracker names).
pub fn encode_info(system: &str, names: &[(u16, &str)]) -> Vec<u8> {
    let mut list = Vec::new();
    for (id, name) in names {
        list.extend(chunk(*id, &chunk(INFO_TRACKER_NAME, name.as_bytes(), false), true));
    }
    let mut body = header_chunk(0);
    body.extend(chunk(INFO_SYSTEM_NAME, system.as_bytes(), false));
    body.extend(chunk(INFO_TRACKER_LIST, &list, true));
    chunk(INFO_PACKET, &body, true)
}

// ---------- the hub (shared with the listener thread) ----------

/// What the listener should bind: the address, and the group to join on
/// an interface (`None`: unicast on 127.0.0.1 only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    pub bind: SocketAddrV4,
    pub join: Option<(Ipv4Addr, Ipv4Addr)>,
}

impl Want {
    pub fn of(s: &PsnSettings) -> Option<Want> {
        if !s.enabled {
            return None;
        }
        let iface: Option<Ipv4Addr> = s.interface.as_deref().and_then(|i| i.trim().parse().ok());
        match iface {
            Some(ip) if !ip.is_loopback() => {
                let group: Ipv4Addr = s.group.trim().parse().ok()?;
                Some(Want { bind: SocketAddrV4::new(group, s.port), join: Some((group, ip)) })
            }
            _ => Some(Want { bind: SocketAddrV4::new(Ipv4Addr::LOCALHOST, s.port), join: None }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub pos: [f32; 3],
    pub valid: bool,
    pub at: Instant,
    /// Increases with every sample taken (any tracker).
    pub seq: u64,
}

#[derive(Debug, Default)]
struct HubInner {
    want: Option<Want>,
    samples: HashMap<u16, Sample>,
    names: HashMap<u16, String>,
    system: String,
    seq: u64,
    local: Option<String>,
    error: Option<String>,
    packets: u64,
    rejected: u64,
    ignored: u64,
    dropped: u64,
    last_from: Option<String>,
    window: (Option<Instant>, u32),
}

#[derive(Debug, Default)]
pub struct PsnHub {
    inner: Mutex<HubInner>,
}

impl PsnHub {
    fn lock(&self) -> MutexGuard<'_, HubInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_want(&self, want: Option<Want>) {
        let mut h = self.lock();
        if h.want != want && want.is_none() {
            h.samples.clear();
        }
        h.want = want;
    }

    pub fn want(&self) -> Option<Want> {
        self.lock().want
    }

    /// The bound address (`None`: no socket).
    pub fn local(&self) -> Option<String> {
        self.lock().local.clone()
    }

    /// A datagram as the listener got it at `at`. Never panics.
    pub fn ingest_bytes(&self, buf: &[u8], from: Option<IpAddr>, at: Instant) -> Result<(), String> {
        {
            let mut h = self.lock();
            if h.want.is_none() {
                h.ignored += 1;
                return Err("suivi désactivé".into());
            }
            let (start, n) = h.window;
            if start.is_none_or(|t| at.saturating_duration_since(t) >= Duration::from_secs(1)) {
                h.window = (Some(at), 1);
            } else if n >= MAX_PACKETS_PER_S {
                h.dropped += 1;
                return Err("débit trop élevé : paquet ignoré".into());
            } else {
                h.window.1 += 1;
            }
        }
        match decode(buf) {
            Ok(p) => {
                self.ingest(p, from, at);
                Ok(())
            }
            Err(e) => {
                self.lock().rejected += 1;
                Err(e)
            }
        }
    }

    fn ingest(&self, p: Packet, from: Option<IpAddr>, at: Instant) {
        let mut h = self.lock();
        h.packets += 1;
        h.last_from = from.map(|f| f.to_string());
        h.samples.retain(|_, s| at.saturating_duration_since(s.at) < FORGET);
        match p {
            Packet::Data { trackers } => {
                for t in trackers {
                    if !h.samples.contains_key(&t.id) && h.samples.len() >= MAX_TRACKERS {
                        h.ignored += 1;
                        continue;
                    }
                    let pos = t.pos.unwrap_or([f32::NAN; 3]);
                    let valid = t.pos.is_some() && finite3(pos) && t.validity.is_none_or(|v| v.is_finite() && v > 0.0);
                    let pos = if finite3(pos) { pos } else { [0.0; 3] };
                    h.seq += 1;
                    let seq = h.seq;
                    h.samples.insert(t.id, Sample { pos, valid, at, seq });
                }
            }
            Packet::Info { system, names } => {
                h.system = system;
                for (id, n) in names {
                    if h.names.len() < MAX_TRACKERS || h.names.contains_key(&id) {
                        h.names.insert(id, n);
                    }
                }
            }
        }
    }

    fn snapshot(&self) -> HashMap<u16, Sample> {
        self.lock().samples.clone()
    }

    fn name(&self, id: u16) -> String {
        self.lock().names.get(&id).cloned().unwrap_or_default()
    }

    fn status(&self) -> Value {
        let h = self.lock();
        json!({
            "local": h.local, "error": h.error, "packets": h.packets, "rejected": h.rejected,
            "ignored": h.ignored, "dropped": h.dropped, "last_from": h.last_from, "system": h.system,
        })
    }
}

// ---------- the listener thread ----------

/// Starts the "psn-in" thread: idle (no socket) while the receiver is off.
pub fn spawn(hub: Arc<PsnHub>, running: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("psn-in".into())
        .spawn(move || listen(&hub, &running))
        .map_err(|e| log::warn!("PosiStageNet : impossible de démarrer le thread : {e}"))
        .ok()
}

fn open(want: Want) -> std::io::Result<UdpSocket> {
    let sock = match want.join {
        None => UdpSocket::bind(SocketAddr::V4(want.bind))?,
        Some((group, iface)) => {
            use socket2::{Domain, Protocol, Socket, Type};
            let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
            // Other PSN receivers on this Mac may listen to the same group.
            s.set_reuse_address(true)?;
            s.bind(&want.bind.into())?;
            s.join_multicast_v4(&group, &iface)?;
            s.into()
        }
    };
    sock.set_read_timeout(Some(POLL))?;
    Ok(sock)
}

/// Receive only: this socket never sends a byte.
fn listen(hub: &PsnHub, running: &AtomicBool) {
    let mut socket: Option<(Want, UdpSocket)> = None;
    let mut failed: Option<(Want, Instant)> = None;
    let mut buf = vec![0u8; 2048];
    while running.load(Ordering::SeqCst) {
        let want = hub.want();
        if socket.as_ref().map(|(w, _)| *w) != want {
            if socket.take().is_some() {
                log::info!("PosiStageNet : écoute arrêtée");
            }
            let mut h = hub.lock();
            (h.local, h.error) = (None, None);
            drop(h);
            if let Some(w) = want.filter(|w| failed.is_none_or(|(f, at)| f != *w || at.elapsed() >= RETRY)) {
                match open(w) {
                    Ok(sock) => {
                        let local = sock.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| w.bind.to_string());
                        log::info!("PosiStageNet : écoute sur {local}{}", w.join.map(|(g, i)| format!(" (groupe {g} sur {i})")).unwrap_or_default());
                        hub.lock().local = Some(local);
                        failed = None;
                        socket = Some((w, sock));
                    }
                    Err(e) => {
                        log::warn!("PosiStageNet : impossible d'écouter sur {} : {e}", w.bind);
                        hub.lock().error = Some(format!("impossible d'écouter sur {} : {e}", w.bind));
                        failed = Some((w, Instant::now()));
                    }
                }
            }
        }
        let Some((_, sock)) = &socket else {
            std::thread::sleep(POLL);
            continue;
        };
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Err(e) = hub.ingest_bytes(&buf[..n], Some(from.ip()), Instant::now()) {
                    log::debug!("PosiStageNet de {from} : {e}");
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => {
                log::debug!("PosiStageNet : réception : {e}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

// ---------- the engine side ----------

/// Why a mobile target is dark this frame, or that it is lit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LinkStatus {
    pub target: String,
    pub tracker_id: u16,
    /// Its bit in the « Cibles miroir » mask (0-based; past 15 unreachable).
    pub bit: usize,
    /// « suivi », « perdu », « hors zone », « non calibré », « saut »,
    /// « entrées fermées », « désactivé ».
    pub state: String,
    pub lit: bool,
    /// Look coordinates of the target when computed.
    pub xy: Option<[f32; 2]>,
    /// Output coordinates (after the calibration).
    pub out: Option<[f32; 2]>,
    pub reasons: Vec<String>,
    pub calibrated: bool,
    pub calib_error: Option<String>,
    pub error_pct: Option<f32>,
}

#[derive(Clone, Debug, Default)]
struct LinkRun {
    last_seq: u64,
    /// Last raw position (look coordinates) and when it was received.
    raw: Option<([f32; 2], Instant)>,
    smooth: Option<[f32; 2]>,
    last_valid: Option<Instant>,
    /// After a jump: dark until a next sample confirms the new place.
    settling: bool,
    /// The jump reference was cleared by an invalid sample, an
    /// out-of-area one or a loss: the next valid sample has nothing to be
    /// checked against, so it stays dark until a second one confirms it.
    reset: bool,
}

const RECONFIRM_FR: &str = "Position à confirmer après une donnée invalide, hors zone ou perdue, ou à la réouverture des entrées externes";
const JUMP_FR: &str = "Déplacement plus rapide que la vitesse max";

pub enum SetError {
    Invalid(String),
    Loosens(Vec<String>),
    Armed(Vec<String>),
}

/// `Shared.psn`.
pub struct PsnState {
    path: Option<PathBuf>,
    settings: PsnSettings,
    calibs: Vec<Result<Calib, String>>,
    pub hub: Arc<PsnHub>,
    runs: Vec<LinkRun>,
    last_frame: Option<Instant>,
    statuses: Vec<LinkStatus>,
    /// The gate's last verdict on the tracking data.
    refusal: Option<Refusal>,
}

impl PsnState {
    pub fn load(path: PathBuf) -> Self {
        let s: PsnSettings = crate::load_json(&path);
        Self::with(Some(path), s.sanitized())
    }

    pub fn in_memory() -> Self {
        Self::with(None, PsnSettings::default())
    }

    fn with(path: Option<PathBuf>, settings: PsnSettings) -> Self {
        let mut st = PsnState {
            path,
            settings: PsnSettings::default(),
            calibs: Vec::new(),
            hub: Arc::new(PsnHub::default()),
            runs: Vec::new(),
            last_frame: None,
            statuses: Vec::new(),
            refusal: None,
        };
        st.install(settings);
        st
    }

    fn install(&mut self, settings: PsnSettings) {
        self.calibs = settings.links.iter().map(Calib::from_link).collect();
        self.runs = vec![LinkRun::default(); settings.links.len()];
        self.hub.set_want(Want::of(&settings));
        self.settings = settings;
        self.statuses.clear();
        if !self.settings.enabled {
            targets::publish_mobile(Vec::new());
        }
    }

    pub fn settings(&self) -> &PsnSettings {
        &self.settings
    }

    /// Replaces the settings. `locked`: armed or under the safety lock (no
    /// loosening at all); `confirm`: the operator confirmed the loosening.
    pub fn set(&mut self, new: PsnSettings, locked: bool, confirm: bool) -> Result<PsnSettings, SetError> {
        new.validate().map_err(SetError::Invalid)?;
        let mut new = new;
        new.group = new.group.trim().to_string();
        new.interface = new.interface.map(|i| i.trim().to_string());
        for l in &mut new.links {
            l.target = l.target.trim().to_string();
        }
        let loosen = self.settings.loosenings(&new);
        if !loosen.is_empty() {
            if locked {
                return Err(SetError::Armed(loosen));
            }
            if !confirm {
                return Err(SetError::Loosens(loosen));
            }
        }
        self.install(new);
        if let Some(path) = &self.path {
            crate::save_json(path, &self.settings);
        }
        Ok(self.settings.clone())
    }

    /// Tests: settings as a hand-edited file could hold them (sanitised,
    /// no confirmation).
    #[cfg(test)]
    pub fn insert_for_tests(&mut self, s: PsnSettings) {
        self.install(s.sanitized());
    }

    pub fn statuses(&self) -> &[LinkStatus] {
        &self.statuses
    }
}

fn dark(link: &TrackerLink, bit: usize, state: &str, reason: Option<String>, calib: &Result<Calib, String>) -> LinkStatus {
    LinkStatus {
        target: link.target.clone(),
        tracker_id: link.tracker_id,
        bit,
        state: state.into(),
        lit: false,
        xy: None,
        out: None,
        reasons: reason.into_iter().collect(),
        calibrated: calib.is_ok(),
        calib_error: calib.as_ref().err().cloned(),
        error_pct: calib.as_ref().ok().map(Calib::error_pct),
    }
}

/// One engine frame: reads the newest samples, moves the mobile targets
/// and publishes them for the « Cibles miroir » generator. Writes runtime
/// state only: never a setting, never the arming.
pub fn frame(s: &mut Shared, now: Instant) {
    if !s.psn.settings.enabled {
        if !s.psn.statuses.is_empty() || s.psn.last_frame.is_some() {
            s.psn.statuses.clear();
            s.psn.last_frame = None;
            targets::publish_mobile(Vec::new());
        }
        return;
    }
    let dt = s.psn.last_frame.map_or(1.0 / 60.0, |t| now.saturating_duration_since(t).as_secs_f32().min(0.1));
    s.psn.last_frame = Some(now);
    let samples = s.psn.hub.snapshot();
    let fresh = s.psn.settings.links.iter().zip(&s.psn.runs).any(|(l, r)| samples.get(&l.tracker_id).is_some_and(|sm| sm.seq > r.last_seq));
    let allowed = crate::external::tracking(s, fresh, now);
    let (cfg, cal) = (s.effective_safety(), s.calibration);
    let statics = s.targets.get().items.len();
    let st = &mut s.psn;
    st.refusal = allowed.err();
    let lost_after = Duration::from_millis(st.settings.lost_after_ms as u64);
    let tau = st.settings.smoothing_ms as f32 / 1000.0;
    let alpha = if tau <= 0.0 { 1.0 } else { 1.0 - (-dt / tau).exp() };
    let mut statuses = Vec::with_capacity(st.settings.links.len());
    let mut mobile = Vec::with_capacity(st.settings.links.len());
    for (i, link) in st.settings.links.iter().enumerate() {
        let bit = statics + i;
        let calib = &st.calibs[i];
        let run = &mut st.runs[i];
        let sample = samples.get(&link.tracker_id).copied();
        let status = (|| {
            if let Err(why) = allowed {
                // Reopening the gate is like a loss: the first sample after
                // it waits for a second consistent one before it lights.
                *run = LinkRun { last_seq: sample.map_or(run.last_seq, |s| s.seq), reset: true, ..Default::default() };
                return dark(link, bit, "entrées fermées", Some(why.label_fr().to_string()), calib);
            }
            let c = match calib {
                Ok(c) => c,
                Err(e) => return dark(link, bit, "non calibré", Some(e.clone()), calib),
            };
            if let Some(sm) = sample.filter(|sm| sm.seq > run.last_seq) {
                run.last_seq = sm.seq;
                if !sm.valid {
                    // Fail dark at once: no last good place kept lit.
                    *run = LinkRun { last_seq: run.last_seq, reset: true, ..Default::default() };
                    return dark(link, bit, "perdu", Some("Donnée de suivi invalide".into()), calib);
                }
                {
                    run.last_valid = Some(sm.at);
                    let Some(q) = c.map(Calib::plane(link.axes, sm.pos)) else {
                        run.raw = None;
                        run.smooth = None;
                        run.reset = true;
                        return dark(link, bit, "hors zone", Some("Hors de la zone calibrée ou du champ".into()), calib);
                    };
                    let jump = match run.raw {
                        Some((p, at)) => {
                            let d = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
                            d / sm.at.saturating_duration_since(at).as_secs_f32().max(MIN_DT) > link.max_speed
                        }
                        // No reference after a reset: never lit unchecked.
                        None => run.reset,
                    };
                    run.raw = Some((q, sm.at));
                    if jump {
                        run.settling = true;
                        run.smooth = Some(q);
                        return dark(link, bit, "saut", Some(if run.reset { RECONFIRM_FR } else { JUMP_FR }.into()), calib);
                    }
                    run.settling = false;
                    run.reset = false;
                }
            }
            if run.last_valid.is_none_or(|t| now.saturating_duration_since(t) > lost_after) {
                let reset = run.reset || run.last_valid.is_some();
                *run = LinkRun { last_seq: run.last_seq, reset, ..Default::default() };
                return dark(link, bit, "perdu", Some(format!("Aucune donnée valide depuis {} ms", lost_after.as_millis())), calib);
            }
            let Some((q, _)) = run.raw else {
                return dark(link, bit, "hors zone", Some("Hors de la zone calibrée ou du champ".into()), calib);
            };
            if run.settling {
                return dark(link, bit, "saut", Some(if run.reset { RECONFIRM_FR } else { JUMP_FR }.into()), calib);
            }
            let p = match run.smooth {
                Some(p) => [p[0] + alpha * (q[0] - p[0]), p[1] + alpha * (q[1] - p[1])],
                None => q,
            };
            run.smooth = Some(p);
            let ts = targets::status(&Target { name: link.target.clone(), x: p[0], y: p[1] }, &cfg, &cal);
            let mut out = dark(link, bit, "suivi", None, calib);
            (out.xy, out.out, out.reasons) = (Some(p), Some(ts.out), ts.reasons);
            if ts.level <= 0.0 {
                out.state = "hors zone".into();
            } else {
                out.lit = true;
            }
            out
        })();
        mobile.push(status.xy.filter(|_| status.lit).map(|p| (p[0], p[1])));
        statuses.push(status);
    }
    st.statuses = statuses;
    targets::publish_mobile(mobile);
}

/// Test hook (`POST /api/test/psn`) and fuzz: positions handed to the hub
/// as packets built by our encoder, as the listener would (no socket).
pub fn inject(s: &Shared, trackers: &[(u16, [f32; 3], Option<f32>)], names: &[(u16, &str)], at: Instant) -> Result<(), String> {
    if !names.is_empty() {
        s.psn.hub.ingest_bytes(&encode_info("test", names), None, at)?;
    }
    for part in trackers.chunks(32) {
        s.psn.hub.ingest_bytes(&encode_data(0, part), None, at)?;
    }
    Ok(())
}

/// `GET /api/psn/trackers`: the trackers seen and the mobile targets.
pub fn trackers_view(s: &Shared, now: Instant) -> Value {
    let samples = s.psn.hub.snapshot();
    let mut ids: Vec<u16> = samples.keys().copied().collect();
    ids.sort_unstable();
    let trackers: Vec<Value> = ids
        .iter()
        .map(|id| {
            let sm = samples[id];
            let link = s.psn.statuses.iter().find(|l| l.tracker_id == *id);
            let state = match link {
                Some(l) => l.state.clone(),
                None if !sm.valid => "invalide".into(),
                None => "non affecté".into(),
            };
            json!({
                "id": id,
                "name": s.psn.hub.name(*id),
                "pos": sm.pos,
                "valid": sm.valid,
                "age_ms": now.saturating_duration_since(sm.at).as_millis().min(u32::MAX as u128) as u32,
                "target_xy": link.and_then(|l| l.xy),
                "state": state,
            })
        })
        .collect();
    json!({
        "enabled": s.psn.settings.enabled,
        "trackers": trackers,
        "links": s.psn.statuses,
        "gate": s.psn.refusal.map(|r| r.label_fr()),
        "listen": s.psn.hub.status(),
    })
}

/// `GET /api/psn`.
pub fn view(s: &Shared, now: Instant) -> Value {
    let calibs: Vec<Value> = s
        .psn
        .calibs
        .iter()
        .map(|c| match c {
            Ok(c) => json!({ "ok": true, "error_pct": c.error_pct() }),
            Err(e) => json!({ "ok": false, "why": e }),
        })
        .collect();
    json!({
        "settings": s.psn.settings,
        "calibration": calibs,
        "max_links": MAX_LINKS,
        "defaults": { "group": DEFAULT_GROUP, "port": DEFAULT_PORT },
        "status": trackers_view(s, now),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Calibration;
    use crate::safety::SafetySettings;

    fn square_refs() -> Vec<RefPoint> {
        // Tracker plane x 0..4 m, y 0..2 m → look -0.5..0.5, 0.2..0.8.
        vec![
            RefPoint { tracker: [0.0, 0.0, 9.0], output: [-0.5, 0.2] },
            RefPoint { tracker: [4.0, 0.0, 9.0], output: [0.5, 0.2] },
            RefPoint { tracker: [4.0, 2.0, 9.0], output: [0.5, 0.8] },
            RefPoint { tracker: [0.0, 2.0, 9.0], output: [-0.5, 0.8] },
        ]
    }

    pub(crate) fn link(id: u16) -> TrackerLink {
        TrackerLink { tracker_id: id, target: format!("Mobile {id}"), refs: square_refs(), ..Default::default() }
    }

    pub(crate) fn on(links: Vec<TrackerLink>) -> PsnSettings {
        PsnSettings { enabled: true, smoothing_ms: 0, links, ..Default::default() }
    }

    #[test]
    fn packets_round_trip_through_our_encoder() {
        let p = encode_data(7, &[(3, [1.5, -2.0, 0.25], None), (900, [0.0, 1.0, 2.0], Some(1.0))]);
        assert!(p.len() <= MAX_PACKET);
        assert_eq!(
            decode(&p).unwrap(),
            Packet::Data {
                trackers: vec![
                    TrackerData { id: 3, pos: Some([1.5, -2.0, 0.25]), validity: None },
                    TrackerData { id: 900, pos: Some([0.0, 1.0, 2.0]), validity: Some(1.0) }
                ]
            }
        );
        let i = encode_info("Serveur", &[(3, "Miroir"), (4, "Chariot\u{7}")]);
        assert_eq!(decode(&i).unwrap(), Packet::Info { system: "Serveur".into(), names: vec![(3, "Miroir".into()), (4, "Chariot".into())] });
        // The header bits: id low 16, length 15 bits, sub-chunk flag on top.
        assert_eq!(&p[..2], &DATA_PACKET.to_le_bytes());
        assert_eq!(p[3] & 0x80, 0x80);
    }

    #[test]
    fn malformed_packets_are_refused_without_panicking() {
        let good = encode_data(0, &[(1, [1.0, 1.0, 1.0], None)]);
        for n in 0..good.len() {
            assert!(decode(&good[..n]).is_err(), "truncated at {n}");
        }
        assert!(decode(&[]).is_err());
        assert!(decode(&vec![0u8; MAX_PACKET + 1]).is_err());
        let mut bad_version = good.clone();
        bad_version[4 + 4 + 8] = 1; // root hdr, header chunk hdr, timestamp
        assert!(decode(&bad_version).unwrap_err().contains("version"));
        let mut two = good.clone();
        two.extend(&good);
        assert!(decode(&two).is_err(), "one root chunk");
        // Unknown root and unknown chunks.
        assert!(decode(&chunk(0x1234, &[], true)).is_err());
        let mut body = header_chunk(0);
        body.extend(chunk(0x0042, &[1, 2, 3], false));
        assert_eq!(decode(&chunk(DATA_PACKET, &body, true)).unwrap(), Packet::Data { trackers: vec![] });
        // Random bytes: never a panic.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..20_000 {
            let n = (x % 64) as usize;
            let mut buf: Vec<u8> = (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                })
                .collect();
            if n > 8 && x.is_multiple_of(3) {
                // A plausible root to reach the inner decoding.
                buf[..4].copy_from_slice(&(DATA_PACKET as u32 | (((n - 4) as u32) << 16) | (1 << 31)).to_le_bytes());
            }
            let _ = decode(&buf);
        }
    }

    #[test]
    fn a_homography_on_four_points_maps_them_and_reports_its_error() {
        let c = Calib::from_link(&link(1)).unwrap();
        assert!(c.error < 1e-4, "{}", c.error);
        let q = c.map([2.0, 1.0]).unwrap();
        assert!((q[0] - 0.0).abs() < 1e-4 && (q[1] - 0.5).abs() < 1e-4, "{q:?}");
        // A perspective: a trapezoid.
        let mut l = link(1);
        l.refs[2].output = [0.3, 0.7];
        l.refs[3].output = [-0.3, 0.7];
        let c = Calib::from_link(&l).unwrap();
        for r in &l.refs {
            let q = c.map(Calib::plane(l.axes, r.tracker)).unwrap();
            assert!((q[0] - r.output[0]).abs() < 1e-4 && (q[1] - r.output[1]).abs() < 1e-4);
        }
        // Inverted axes: x mirrored.
        let mut l = link(1);
        l.axes = [Axis::NegX, Axis::Y];
        for r in &mut l.refs {
            r.tracker[0] = -r.tracker[0];
        }
        assert!(Calib::from_link(&l).is_ok());
        // Five noisy points: least squares, a residual error.
        let mut l = link(1);
        l.refs.push(RefPoint { tracker: [2.0, 1.0, 0.0], output: [0.03, 0.5] });
        let c = Calib::from_link(&l).unwrap();
        assert!(c.error > 0.0 && c.error < MAX_ERROR, "{}", c.error);
        // Outside the calibrated area (grown by 25 %): none.
        let c = Calib::from_link(&link(1)).unwrap();
        assert!(c.map([4.9, 1.0]).is_some());
        assert!(c.map([5.5, 1.0]).is_none());
        assert!(c.map([f32::NAN, 1.0]).is_none());
    }

    #[test]
    fn degenerate_calibrations_are_refused() {
        let mut l = link(1);
        l.refs.truncate(3);
        assert!(Calib::from_link(&l).unwrap_err().contains("4 au moins"));
        // Aligned.
        let mut l = link(1);
        for (i, r) in l.refs.iter_mut().enumerate() {
            r.tracker = [i as f32, 0.0, 0.0];
        }
        assert!(Calib::from_link(&l).is_err());
        // Two at the same place.
        let mut l = link(1);
        l.refs[1].tracker = l.refs[0].tracker;
        assert!(Calib::from_link(&l).is_err());
        // Aimed at a line.
        let mut l = link(1);
        for (i, r) in l.refs.iter_mut().enumerate() {
            r.output = [i as f32 * 0.1, 0.5];
        }
        assert!(Calib::from_link(&l).is_err());
        // Same base axis twice.
        let mut l = link(1);
        l.axes = [Axis::X, Axis::NegX];
        assert!(Calib::from_link(&l).is_err());
        // Incoherent: a fifth point far from the others' mapping.
        let mut l = link(1);
        l.refs.push(RefPoint { tracker: [2.0, 1.0, 0.0], output: [0.9, -0.9] });
        assert!(Calib::from_link(&l).unwrap_err().contains("incohérent"));
    }

    #[test]
    fn validation_and_loosenings() {
        assert!(PsnSettings::default().validate().is_ok());
        assert!(!PsnSettings::default().enabled, "off by default");
        assert_eq!((PsnSettings::default().group.as_str(), PsnSettings::default().port), ("236.10.10.10", 56565));
        let bad = [
            PsnSettings { group: "10.0.0.1".into(), ..Default::default() },
            PsnSettings { group: "x".into(), ..Default::default() },
            PsnSettings { interface: Some("0.0.0.0".into()), ..Default::default() },
            PsnSettings { interface: Some("abc".into()), ..Default::default() },
            PsnSettings { lost_after_ms: 50, ..Default::default() },
            PsnSettings { lost_after_ms: 60_000, ..Default::default() },
            PsnSettings { smoothing_ms: 5000, ..Default::default() },
            PsnSettings { links: vec![TrackerLink { max_speed: f32::NAN, ..link(1) }], ..Default::default() },
            PsnSettings { links: vec![TrackerLink { target: " ".into(), ..link(1) }], ..Default::default() },
            PsnSettings { links: vec![link(1), TrackerLink { tracker_id: 2, ..link(1) }], ..Default::default() },
            PsnSettings { links: (0..=MAX_LINKS as u16).map(link).collect(), ..Default::default() },
        ];
        for b in &bad {
            assert!(b.validate().is_err(), "{b:?}");
        }
        let mut bad_ref = link(1);
        bad_ref.refs[0].tracker[0] = f32::INFINITY;
        assert!(PsnSettings { links: vec![bad_ref], ..Default::default() }.validate().is_err());

        let off = PsnSettings::default();
        let one = on(vec![link(1)]);
        let l = off.loosenings(&one);
        assert!(l.iter().any(|m| m.contains("Suivi activé")) && l.iter().any(|m| m.contains("ajoutée")), "{l:?}");
        assert!(one.loosenings(&off).is_empty(), "switching off and removing tighten");
        assert!(one.loosenings(&PsnSettings { lost_after_ms: 200, ..one.clone() }).is_empty());
        assert!(!one.loosenings(&PsnSettings { lost_after_ms: 900, ..one.clone() }).is_empty());
        assert!(!one.loosenings(&PsnSettings { port: 5000, ..one.clone() }).is_empty());
        assert!(one.loosenings(&PsnSettings { smoothing_ms: 300, ..one.clone() }).is_empty());
        let mut moved = one.clone();
        moved.links[0].refs[0].output = [-0.4, 0.2];
        assert!(!one.loosenings(&moved).is_empty());
        let mut faster = one.clone();
        faster.links[0].max_speed = 8.0;
        assert!(!one.loosenings(&faster).is_empty());
        faster.links[0].max_speed = 1.0;
        assert!(one.loosenings(&faster).is_empty());
        let two = on(vec![link(1), link(2)]);
        assert!(!two.loosenings(&on(vec![link(2)])).is_empty(), "a link taking another one's place");
    }

    #[test]
    fn the_store_needs_a_confirmation_and_refuses_armed() {
        let mut st = PsnState::in_memory();
        assert!(st.hub.want().is_none(), "no socket wanted while off");
        assert!(matches!(st.set(on(vec![link(1)]), false, false), Err(SetError::Loosens(_))));
        assert!(matches!(st.set(on(vec![link(1)]), true, true), Err(SetError::Armed(_))));
        assert!(!st.settings().enabled);
        assert!(st.set(on(vec![link(1)]), false, true).is_ok());
        assert_eq!(st.hub.want(), Some(Want { bind: SocketAddrV4::new(Ipv4Addr::LOCALHOST, DEFAULT_PORT), join: None }));
        // Off: always, even locked.
        assert!(st.set(PsnSettings::default(), true, false).is_ok());
        assert!(st.hub.want().is_none());
        // An interface: the group joined on it only.
        let w = Want::of(&PsnSettings { enabled: true, interface: Some("192.168.1.20".into()), ..Default::default() }).unwrap();
        assert_eq!(w.join, Some(("236.10.10.10".parse().unwrap(), "192.168.1.20".parse().unwrap())));
        assert!(st.set(PsnSettings { group: "1.2.3.4".into(), ..Default::default() }, false, true).is_err());
    }

    /// A studio with one mobile target linked to tracker 1, the gate open.
    pub(crate) fn studio(settings: PsnSettings) -> Shared {
        let mut s = crate::test_support::shared();
        s.psn.insert_for_tests(settings);
        crate::external::set_open(&mut s, true, crate::interlock::ArmSource::Ui).unwrap();
        s
    }

    fn step(s: &mut Shared, at: Instant, pos: Option<[f32; 3]>) -> LinkStatus {
        if let Some(p) = pos {
            inject(s, &[(1, p, None)], &[], at).unwrap();
        }
        frame(s, at);
        s.psn.statuses()[0].clone()
    }

    #[test]
    fn a_test_tracker_moves_the_target_and_loss_darkens_it_after_500_ms() {
        let mut s = studio(on(vec![link(1)]));
        let t0 = Instant::now();
        let st = step(&mut s, t0, Some([2.0, 1.0, 0.0]));
        assert_eq!(st.state, "suivi", "{st:?}");
        assert!(st.lit);
        let xy = st.xy.unwrap();
        assert!(xy[0].abs() < 1e-3 && (xy[1] - 0.5).abs() < 1e-3);
        assert_eq!(crate::targets::current_mobile_for_tests(), vec![Some((xy[0], xy[1]))]);
        // It follows.
        let mut t = t0;
        for k in 1..=30 {
            t += Duration::from_millis(16);
            let st = step(&mut s, t, Some([2.0 + k as f32 * 0.01, 1.0, 0.0]));
            assert!(st.lit, "{st:?}");
        }
        assert!(s.psn.statuses()[0].xy.unwrap()[0] > 0.05);
        // Data stops: still lit at 490 ms, dark after 500 ms.
        let last = t;
        let st = step(&mut s, last + Duration::from_millis(490), None);
        assert!(st.lit);
        let st = step(&mut s, last + Duration::from_millis(510), None);
        assert_eq!((st.state.as_str(), st.lit), ("perdu", false));
        assert_eq!(crate::targets::current_mobile_for_tests(), vec![None]);
        // It comes back at the new place once a second sample confirms
        // it (the loss cleared the jump reference), never armed.
        let st = step(&mut s, last + Duration::from_millis(600), Some([1.0, 1.0, 0.0]));
        assert_eq!((st.state.as_str(), st.lit), ("saut", false), "{st:?}");
        let st = step(&mut s, last + Duration::from_millis(616), Some([1.0, 1.0, 0.0]));
        assert!(st.lit && (st.xy.unwrap()[0] + 0.25).abs() < 1e-3, "{st:?}");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn smoothing_follows_with_its_time_constant() {
        let mut s = studio(PsnSettings { smoothing_ms: 50, ..on(vec![link(1)]) });
        let t0 = Instant::now();
        step(&mut s, t0, Some([2.0, 1.0, 0.0]));
        // A step of 0.05 m (0.0125 in x): slow enough to be no jump.
        let st = step(&mut s, t0 + Duration::from_millis(16), Some([2.05, 1.0, 0.0]));
        let a = 1.0 - (-0.016f32 / 0.05).exp();
        let xy = st.xy.unwrap_or_else(|| panic!("{st:?}"));
        assert!((xy[0] - 0.0125 * a).abs() < 1e-4, "{st:?}");
        let mut t = t0 + Duration::from_millis(16);
        for _ in 0..20 {
            t += Duration::from_millis(16);
            step(&mut s, t, None);
        }
        assert!((s.psn.statuses()[0].xy.unwrap()[0] - 0.0125).abs() < 1e-4, "converges");
    }

    #[test]
    fn a_jump_darkens_the_target_until_confirmed() {
        let mut s = studio(on(vec![link(1)]));
        let t0 = Instant::now();
        assert!(step(&mut s, t0, Some([0.5, 1.0, 0.0])).lit);
        // 3 m in 16 ms: far above 4 units/s.
        let t1 = t0 + Duration::from_millis(16);
        let st = step(&mut s, t1, Some([3.5, 1.0, 0.0]));
        assert_eq!((st.state.as_str(), st.lit), ("saut", false));
        assert!(!step(&mut s, t1 + Duration::from_millis(16), None).lit, "no confirmation yet");
        let st = step(&mut s, t1 + Duration::from_millis(32), Some([3.5, 1.0, 0.0]));
        assert!(st.lit && (st.xy.unwrap()[0] - 0.375).abs() < 1e-3, "{st:?}");
    }

    /// An invalid sample, an out-of-area one or a loss clears the jump
    /// reference: the next valid sample, wherever it is, stays dark until
    /// a second consistent one confirms it (A, reset, far B → dark).
    #[test]
    fn after_a_reset_the_next_sample_waits_for_a_confirmation() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        type Reset = fn(&mut Shared, Instant);
        let resets: [(&str, Reset); 4] = [
            ("validity 0", |s, at| {
                inject(s, &[(1, [0.5, 1.0, 0.0], Some(0.0))], &[], at).unwrap();
                frame(s, at);
            }),
            ("NaN", |s, at| {
                step(s, at, Some([f32::NAN, 1.0, 0.0]));
            }),
            ("out of the area", |s, at| {
                step(s, at, Some([40.0, 1.0, 0.0]));
            }),
            ("loss", |s, at| {
                step(s, at + Duration::from_millis(600), None);
            }),
        ];
        for (what, reset) in resets {
            let mut s = studio(on(vec![link(1)]));
            assert!(step(&mut s, ms(0), Some([0.5, 1.0, 0.0])).lit, "{what}");
            reset(&mut s, ms(16));
            assert!(!s.psn.statuses()[0].lit, "{what}");
            // Far B (3 m away), 16 ms after the reset (600 ms for the loss).
            let tb = if what == "loss" { ms(632) } else { ms(32) };
            let st = step(&mut s, tb, Some([3.5, 1.0, 0.0]));
            assert_eq!((st.state.as_str(), st.lit), ("saut", false), "{what}: {st:?}");
            assert_eq!(crate::targets::current_mobile_for_tests(), vec![None], "{what}");
            assert!(!step(&mut s, tb + Duration::from_millis(8), None).lit, "{what}: not without a second sample");
            // A second sample far from B again: still dark.
            let st = step(&mut s, tb + Duration::from_millis(16), Some([0.5, 1.0, 0.0]));
            assert!(!st.lit, "{what}: {st:?}");
            // A consistent one confirms it.
            let st = step(&mut s, tb + Duration::from_millis(32), Some([0.5, 1.0, 0.0]));
            assert!(st.lit, "{what}: {st:?}");
            assert!(!s.gate.is_armed());
        }
        // The very first sample after switching on still lights at once.
        let mut s = studio(on(vec![link(1)]));
        assert!(step(&mut s, ms(0), Some([3.5, 1.0, 0.0])).lit);
    }

    #[test]
    fn without_a_valid_calibration_or_with_bad_data_the_target_stays_dark() {
        let mut l = link(1);
        l.refs.truncate(3);
        let mut s = studio(on(vec![l]));
        let st = step(&mut s, Instant::now(), Some([2.0, 1.0, 0.0]));
        assert_eq!((st.state.as_str(), st.lit), ("non calibré", false));
        let mut s = studio(on(vec![link(1)]));
        let t0 = Instant::now();
        // Not numbers, absurd, outside the calibrated area, invalid status.
        for (k, p) in [[f32::NAN, 1.0, 0.0], [1e9, 1.0, 0.0], [40.0, 1.0, 0.0]].into_iter().enumerate() {
            let st = step(&mut s, t0 + Duration::from_millis(16 * k as u64), Some(p));
            assert!(!st.lit, "{p:?}: {st:?}");
        }
        inject(&s, &[(1, [2.0, 1.0, 0.0], Some(0.0))], &[], t0).unwrap();
        frame(&mut s, t0 + Duration::from_millis(60));
        assert!(!s.psn.statuses()[0].lit, "validity 0");
    }

    #[test]
    fn a_target_in_a_blank_zone_lights_nothing_there() {
        use crate::zones::{Zone, ZoneKind};
        let mut s = studio(on(vec![link(1)]));
        let zone = Zone { id: 1, name: "Régie".into(), kind: ZoneKind::Blank, points: vec![[-0.1, 0.4], [0.1, 0.4], [0.1, 0.6], [-0.1, 0.6]], ..Default::default() };
        let mut cfg = s.safety.get();
        cfg.zones = vec![zone.clone()];
        s.safety.set(cfg, true).unwrap();
        let t0 = Instant::now();
        let st = step(&mut s, t0, Some([2.0, 1.0, 0.0]));
        assert_eq!((st.state.as_str(), st.lit), ("hors zone", false));
        assert!(st.reasons[0].contains("Régie"), "{st:?}");
        // Through the engine and the whole safety stage: see
        // safety_invariants (`psn_targets_never_light_a_cut_spot_nor_pass_the_guard_dose`).
        assert_eq!(crate::targets::current_mobile_for_tests(), vec![None]);
        // Elsewhere: lit (the look draws it).
        let st = step(&mut s, t0 + Duration::from_millis(400), Some([3.0, 1.0, 0.0]));
        assert!(st.state == "saut" || st.lit, "{st:?}");
        let st = step(&mut s, t0 + Duration::from_millis(800), Some([3.0, 1.0, 0.0]));
        assert!(st.lit, "{st:?}");
    }

    /// T-492: reopening the external-input gate is like a loss: the first
    /// sample after it stays dark until a second consistent one confirms it.
    #[test]
    fn after_the_gate_reopens_the_first_sample_waits_for_a_confirmation() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut s = studio(on(vec![link(1)]));
        assert!(step(&mut s, ms(0), Some([0.5, 1.0, 0.0])).lit);
        crate::external::set_open(&mut s, false, crate::interlock::ArmSource::Ui).unwrap();
        let st = step(&mut s, ms(16), Some([0.5, 1.0, 0.0]));
        assert_eq!((st.state.as_str(), st.lit), ("entrées fermées", false));
        crate::external::set_open(&mut s, true, crate::interlock::ArmSource::Ui).unwrap();
        let st = step(&mut s, ms(32), Some([3.5, 1.0, 0.0]));
        assert_eq!((st.state.as_str(), st.lit), ("saut", false), "{st:?}");
        assert!(st.reasons.iter().any(|r| r == RECONFIRM_FR), "{st:?}");
        assert_eq!(crate::targets::current_mobile_for_tests(), vec![None]);
        let st = step(&mut s, ms(48), Some([3.5, 1.0, 0.0]));
        assert!(st.lit, "{st:?}");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_closed_gate_or_scope_darkens_everything_and_tracking_never_arms() {
        let mut s = studio(on(vec![link(1)]));
        crate::external::set_open(&mut s, false, crate::interlock::ArmSource::Ui).unwrap();
        let st = step(&mut s, Instant::now(), Some([2.0, 1.0, 0.0]));
        assert_eq!((st.state.as_str(), st.lit), ("entrées fermées", false));
        assert!(s.external.gate.refused.get(&crate::external::SourceKind::Psn).copied().unwrap_or(0) >= 1);
        // Generated packets of every kind: arming and settings never move.
        let mut s = studio(on(vec![link(1), link(2)]));
        let before = s.psn.settings().clone();
        let (safety, cal, targets) = (s.safety.get(), s.calibration, s.targets.get().clone());
        let t0 = Instant::now();
        let mut x: u32 = 12345;
        for k in 0..600u64 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let v = (x >> 8) as f32 / (1 << 24) as f32;
            let p = match x % 5 {
                0 => [f32::NAN, v, 0.0],
                1 => [v * 1e6, 0.0, 0.0],
                _ => [v * 4.0, v * 2.0, 0.0],
            };
            let at = t0 + Duration::from_millis(16 * k);
            let _ = inject(&s, &[((x % 3) as u16, p, x.is_multiple_of(7).then_some(v - 0.5))], &[((x % 3) as u16, "x")], at);
            let _ = s.psn.hub.ingest_bytes(&x.to_le_bytes(), None, at);
            frame(&mut s, at);
            assert!(!s.gate.is_armed());
        }
        assert_eq!(s.psn.settings(), &before);
        assert_eq!((s.safety.get(), s.calibration, s.targets.get().clone()), (safety, cal, targets));
        // Armed by the operator: tracking never disarms nor re-arms either.
        s.request_arm(crate::interlock::ArmSource::Ui).unwrap();
        frame(&mut s, t0 + Duration::from_secs(20));
        assert!(s.gate.is_armed());
    }

    #[test]
    fn the_target_status_uses_the_output_calibration() {
        let link = link(1);
        let c = Calib::from_link(&link).unwrap();
        let q = c.map([2.0, 1.0]).unwrap();
        let shifted = Calibration { offset_y: -0.6, ..Default::default() };
        let st = targets::status(&Target { name: "m".into(), x: q[0], y: q[1] }, &SafetySettings::default(), &shifted);
        assert_eq!(st.level, 0.0, "below the horizon once calibrated");
    }

    #[test]
    fn a_test_sender_on_loopback_reaches_the_listener() {
        let mut st = PsnState::in_memory();
        st.insert_for_tests(PsnSettings { port: 0, ..on(vec![link(1)]) });
        let running = Arc::new(AtomicBool::new(true));
        let hub = Arc::clone(&st.hub);
        let thread = spawn(Arc::clone(&hub), Arc::clone(&running)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let local = loop {
            if let Some(l) = hub.local() {
                break l;
            }
            assert!(Instant::now() < deadline, "never bound");
            std::thread::sleep(Duration::from_millis(10));
        };
        let addr: SocketAddr = local.parse().unwrap();
        assert!(addr.ip().is_loopback(), "this Mac only by default");
        let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let pkt = encode_data(1, &[(1, [2.0, 1.0, 0.0], Some(1.0))]);
        loop {
            tx.send_to(&pkt, addr).unwrap();
            tx.send_to(b"junk", addr).unwrap();
            if hub.snapshot().get(&1).is_some_and(|s| s.valid) && hub.status()["rejected"].as_u64().unwrap() >= 1 {
                break;
            }
            assert!(Instant::now() < deadline, "no sample");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Switched off: the socket goes.
        st.insert_for_tests(PsnSettings::default());
        while hub.local().is_some() {
            assert!(Instant::now() < deadline + Duration::from_secs(2), "socket kept");
            std::thread::sleep(Duration::from_millis(10));
        }
        running.store(false, Ordering::SeqCst);
        thread.join().unwrap();
    }

    #[test]
    fn the_hub_caps_trackers_and_ignores_data_while_off() {
        let hub = PsnHub::default();
        let t = Instant::now();
        assert!(hub.ingest_bytes(&encode_data(0, &[(1, [0.0; 3], None)]), None, t).is_err(), "off");
        hub.set_want(Want::of(&on(vec![])));
        let many: Vec<(u16, [f32; 3], Option<f32>)> = (0..100).map(|i| (i, [0.0; 3], None)).collect();
        for part in many.chunks(32) {
            hub.ingest_bytes(&encode_data(0, part), None, t).unwrap();
        }
        assert_eq!(hub.snapshot().len(), MAX_TRACKERS);
        // The packet rate cap.
        let p = encode_data(0, &[(1, [0.0; 3], None)]);
        let refused = (0..MAX_PACKETS_PER_S + 10).filter(|_| hub.ingest_bytes(&p, None, t).is_err()).count();
        assert!(refused >= 10);
    }
}
