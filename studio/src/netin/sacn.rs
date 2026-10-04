//! sACN in (ANSI E1.31, T-406): a hand-written decoder of E1.31 data
//! packets, from the published standard, and the merge of several sources
//! on one universe (highest priority wins; between equals, the newest
//! packet wins).
//!
//! Receive only: we never send discovery or anything else. Multicast is
//! joined only when the operator asks for it (listener.rs), on the
//! interface they picked; otherwise unicast to this Mac's address.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

/// The sACN UDP port.
pub const PORT: u16 = 5568;
/// Valid universes (E1.31: 1 to 63999).
pub const MIN_UNIVERSE: u16 = 1;
pub const MAX_UNIVERSE: u16 = 63999;
/// Highest source priority.
pub const MAX_PRIORITY: u8 = 200;
/// A source silent this long is gone (E1.31 « network data loss »).
pub const SOURCE_TIMEOUT: Duration = Duration::from_millis(2500);

const ACN_ID: &[u8; 12] = b"ASC-E1.17\0\0\0";
const VECTOR_ROOT_DATA: u32 = 0x0000_0004;
const VECTOR_FRAMING_DATA: u32 = 0x0000_0002;
const VECTOR_DMP_SET: u8 = 0x02;
/// Offset of the first slot (the START code).
const HEADER_LEN: usize = 125;
const OPT_PREVIEW: u8 = 0x80;
const OPT_TERMINATED: u8 = 0x40;

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Data(Data),
    /// Synchronisation, discovery…: ignored.
    Other(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Data {
    /// The sender's component id (CID).
    pub cid: [u8; 16],
    pub name: String,
    pub priority: u8,
    pub sequence: u8,
    pub preview: bool,
    /// The source says it stops sending (stream terminated).
    pub terminated: bool,
    pub universe: u16,
    pub start_code: u8,
    /// The DMX slots after the START code (0 to 512).
    pub slots: Vec<u8>,
}

fn u16_at(p: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([p[at], p[at + 1]])
}

fn u32_at(p: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]])
}

/// The PDU length in a flags-and-length field (low 12 bits).
fn pdu_len(p: &[u8], at: usize) -> usize {
    (u16_at(p, at) & 0x0FFF) as usize
}

/// Decodes one UDP datagram.
pub fn decode(p: &[u8]) -> Result<Packet, &'static str> {
    if p.len() < 22 || u16_at(p, 0) != 0x0010 || u16_at(p, 2) != 0 || &p[4..16] != ACN_ID {
        return Err("pas un paquet sACN");
    }
    let vector = u32_at(p, 18);
    if vector != VECTOR_ROOT_DATA {
        return Ok(Packet::Other(vector));
    }
    if p.len() < HEADER_LEN + 1 {
        return Err("paquet sACN tronqué");
    }
    // Each layer's length runs to the end of the packet.
    if pdu_len(p, 16) != p.len() - 16 || pdu_len(p, 38) != p.len() - 38 || pdu_len(p, 115) != p.len() - 115 {
        return Err("longueur sACN incohérente");
    }
    if u32_at(p, 40) != VECTOR_FRAMING_DATA {
        return Err("trame sACN inconnue");
    }
    if p[117] != VECTOR_DMP_SET || p[118] != 0xA1 || u16_at(p, 119) != 0 || u16_at(p, 121) != 1 {
        return Err("couche DMP sACN invalide");
    }
    let count = u16_at(p, 123) as usize;
    if !(1..=513).contains(&count) || p.len() != HEADER_LEN + count {
        return Err("nombre de canaux sACN invalide");
    }
    let priority = p[108];
    if priority > MAX_PRIORITY {
        return Err("priorité sACN invalide");
    }
    let universe = u16_at(p, 113);
    if !(MIN_UNIVERSE..=MAX_UNIVERSE).contains(&universe) {
        return Err("univers sACN invalide");
    }
    let name_bytes = &p[44..108];
    let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
    let name = String::from_utf8_lossy(&name_bytes[..name_len]).trim().to_string();
    let mut cid = [0u8; 16];
    cid.copy_from_slice(&p[22..38]);
    Ok(Packet::Data(Data {
        cid,
        name,
        priority,
        sequence: p[111],
        preview: p[112] & OPT_PREVIEW != 0,
        terminated: p[112] & OPT_TERMINATED != 0,
        universe,
        start_code: p[HEADER_LEN],
        slots: p[HEADER_LEN + 1..].to_vec(),
    }))
}

/// The multicast group of a universe: 239.255.<hi>.<lo>.
pub fn multicast_group(universe: u16) -> Ipv4Addr {
    let [hi, lo] = universe.to_be_bytes();
    Ipv4Addr::new(239, 255, hi, lo)
}

/// E1.31 6.7.2: a packet whose sequence is at most 20 behind the last
/// one is late (out of order) and dropped.
pub fn out_of_order(last: u8, new: u8) -> bool {
    let diff = new.wrapping_sub(last) as i8;
    diff <= 0 && diff > -20
}

#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub addr: String,
    pub priority: u8,
    pub sequence: u8,
    pub at: Instant,
}

/// What the merge decided for one packet.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// This source leads the universe: its slots are the universe's.
    Take,
    /// Accepted, but a higher priority source leads.
    Outranked,
    /// Late, preview, another START code, or the source just stopped.
    Dropped,
}

/// The sources of every universe (the sACN listener's own state).
#[derive(Default)]
pub struct Merger {
    sources: HashMap<(u16, [u8; 16]), Source>,
}

impl Merger {
    pub fn packet(&mut self, d: &Data, addr: String, now: Instant) -> Verdict {
        self.expire(now);
        let key = (d.universe, d.cid);
        if d.terminated {
            self.sources.remove(&key);
            return Verdict::Dropped;
        }
        if let Some(src) = self.sources.get(&key) {
            if out_of_order(src.sequence, d.sequence) {
                return Verdict::Dropped;
            }
        }
        // Preview data is for visualisers; alternate START codes are not
        // levels. Neither counts as the source being alive for levels.
        if d.preview || d.start_code != 0 {
            return Verdict::Dropped;
        }
        self.sources.insert(key, Source { name: d.name.clone(), addr, priority: d.priority, sequence: d.sequence, at: now });
        let top = self.sources.iter().filter(|((u, _), _)| *u == d.universe).map(|(_, s)| s.priority).max().unwrap_or(0);
        if d.priority >= top {
            Verdict::Take
        } else {
            Verdict::Outranked
        }
    }

    fn expire(&mut self, now: Instant) {
        self.sources.retain(|_, s| now.saturating_duration_since(s.at) <= SOURCE_TIMEOUT);
    }

    /// The live sources of a universe, highest priority first.
    pub fn sources(&self, universe: u16) -> Vec<&Source> {
        let mut v: Vec<&Source> = self.sources.iter().filter(|((u, _), _)| *u == universe).map(|(_, s)| s).collect();
        v.sort_by(|a, b| b.priority.cmp(&a.priority).then(b.at.cmp(&a.at)));
        v
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// An E1.31 data packet (tests and the UDP round trips).
    pub fn packet(universe: u16, name: &str, priority: u8, sequence: u8, options: u8, slots: &[u8]) -> Vec<u8> {
        packet_from([7; 16], universe, name, priority, sequence, options, slots)
    }

    pub fn packet_from(cid: [u8; 16], universe: u16, name: &str, priority: u8, sequence: u8, options: u8, slots: &[u8]) -> Vec<u8> {
        let len = HEADER_LEN + 1 + slots.len();
        let flags = |n: usize| (0x7000 | n as u16).to_be_bytes();
        let mut p = Vec::with_capacity(len);
        p.extend_from_slice(&0x0010u16.to_be_bytes());
        p.extend_from_slice(&0u16.to_be_bytes());
        p.extend_from_slice(ACN_ID);
        p.extend_from_slice(&flags(len - 16));
        p.extend_from_slice(&VECTOR_ROOT_DATA.to_be_bytes());
        p.extend_from_slice(&cid);
        p.extend_from_slice(&flags(len - 38));
        p.extend_from_slice(&VECTOR_FRAMING_DATA.to_be_bytes());
        let mut n = [0u8; 64];
        n[..name.len().min(63)].copy_from_slice(&name.as_bytes()[..name.len().min(63)]);
        p.extend_from_slice(&n);
        p.push(priority);
        p.extend_from_slice(&0u16.to_be_bytes()); // sync address
        p.push(sequence);
        p.push(options);
        p.extend_from_slice(&universe.to_be_bytes());
        p.extend_from_slice(&flags(len - 115));
        p.extend_from_slice(&[VECTOR_DMP_SET, 0xA1, 0, 0, 0, 1]);
        p.extend_from_slice(&((slots.len() + 1) as u16).to_be_bytes());
        p.push(0); // START code
        p.extend_from_slice(slots);
        assert_eq!(p.len(), len);
        p
    }

    fn data(p: &[u8]) -> Data {
        match decode(p).unwrap() {
            Packet::Data(d) => d,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decodes_a_data_packet() {
        let mut slots = vec![0u8; 512];
        (slots[0], slots[511]) = (255, 9);
        let d = data(&packet(1, "Console A", 100, 42, 0, &slots));
        assert_eq!((d.universe, d.priority, d.sequence, d.name.as_str()), (1, 100, 42, "Console A"));
        assert_eq!(d.slots, slots);
        assert!(!d.preview && !d.terminated);
        let d = data(&packet(63999, "x", 0, 0, OPT_TERMINATED | OPT_PREVIEW, &[1, 2]));
        assert!(d.terminated && d.preview);
        assert_eq!(d.slots, [1, 2]);
    }

    #[test]
    fn refuses_bad_ids_lengths_and_values() {
        let good = packet(1, "c", 100, 0, 0, &[1, 2, 3]);
        assert!(decode(b"Art-Net\0\0P").is_err());
        let mut bad_id = good.clone();
        bad_id[4] = b'X';
        assert!(decode(&bad_id).is_err());
        for n in 0..good.len() {
            assert!(decode(&good[..n]).is_err(), "truncated at {n}");
        }
        let mut longer = good.clone();
        longer.push(0);
        assert!(decode(&longer).is_err(), "the lengths no longer match");
        let mut count = good.clone();
        count[123..125].copy_from_slice(&600u16.to_be_bytes());
        assert!(decode(&count).is_err());
        let mut priority = good.clone();
        priority[108] = 201;
        assert!(decode(&priority).is_err());
        assert!(decode(&packet(0, "c", 100, 0, 0, &[1])).is_err(), "universe 0");
        assert!(decode(&packet(64000, "c", 100, 0, 0, &[1])).is_err());
        let mut sync = good.clone();
        sync[18..22].copy_from_slice(&8u32.to_be_bytes());
        assert_eq!(decode(&sync), Ok(Packet::Other(8)));
        let mut framing = good;
        framing[43] = 9;
        assert!(decode(&framing).is_err());
    }

    #[test]
    fn sequence_and_groups() {
        assert!(out_of_order(10, 10) && out_of_order(10, 9) && out_of_order(10, 247u8.wrapping_add(10)));
        assert!(!out_of_order(10, 11) && !out_of_order(255, 0) && !out_of_order(10, 246));
        assert_eq!(multicast_group(1), Ipv4Addr::new(239, 255, 0, 1));
        assert_eq!(multicast_group(0x1234), Ipv4Addr::new(239, 255, 0x12, 0x34));
    }

    #[test]
    fn highest_priority_wins_then_the_newest() {
        let mut m = Merger::default();
        let t0 = Instant::now();
        let a = |seq, pri| data(&packet_from([1; 16], 1, "A", pri, seq, 0, &[10]));
        let b = |seq, pri| data(&packet_from([2; 16], 1, "B", pri, seq, 0, &[20]));
        assert_eq!(m.packet(&a(1, 100), "a".into(), t0), Verdict::Take);
        assert_eq!(m.packet(&b(1, 150), "b".into(), t0), Verdict::Take);
        assert_eq!(m.packet(&a(2, 100), "a".into(), t0), Verdict::Outranked);
        assert_eq!(m.sources(1)[0].name, "B");
        // Equal priorities: the newest packet wins.
        assert_eq!(m.packet(&a(3, 150), "a".into(), t0), Verdict::Take);
        assert_eq!(m.packet(&b(2, 150), "b".into(), t0), Verdict::Take);
        // Late packets are dropped.
        assert_eq!(m.packet(&b(1, 150), "b".into(), t0), Verdict::Dropped);
        // B stops: A leads again on its next packet.
        assert_eq!(m.packet(&data(&packet_from([2; 16], 1, "B", 150, 3, OPT_TERMINATED, &[0])), "b".into(), t0), Verdict::Dropped);
        assert_eq!(m.packet(&a(4, 100), "a".into(), t0), Verdict::Take);
        // Another universe doesn't compete.
        assert_eq!(m.packet(&data(&packet_from([3; 16], 2, "C", 10, 0, 0, &[1])), "c".into(), t0), Verdict::Take);
        // Preview data never counts.
        assert_eq!(m.packet(&data(&packet_from([4; 16], 1, "P", 200, 0, OPT_PREVIEW, &[1])), "p".into(), t0), Verdict::Dropped);
    }

    #[test]
    fn a_silent_source_expires() {
        let mut m = Merger::default();
        let t0 = Instant::now();
        let hi = data(&packet_from([1; 16], 1, "Hi", 200, 0, 0, &[1]));
        let lo = data(&packet_from([2; 16], 1, "Lo", 50, 0, 0, &[2]));
        m.packet(&hi, "a".into(), t0);
        assert_eq!(m.packet(&lo, "b".into(), t0 + Duration::from_secs(2)), Verdict::Outranked);
        let mut lo2 = lo.clone();
        lo2.sequence = 1;
        assert_eq!(m.packet(&lo2, "b".into(), t0 + Duration::from_secs(3)), Verdict::Take, "Hi silent for 3 s");
        assert_eq!(m.sources(1).len(), 1);
    }
}
