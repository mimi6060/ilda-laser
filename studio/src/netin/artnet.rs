//! Art-Net in: a hand-written ArtDmx decoder (from the public Art-Net 4
//! specification) and our own channel chart, the « remote chart » a
//! lighting console patches as one fixture.
//!
//! ArtTimeCode (OpTimeCode, T-409) is decoded by timecode/artnet.rs and
//! handed to the timecode chase, never to a control.
//!
//! Listen only: we never answer an ArtPoll nor send anything, so a console
//! has to send to this Mac's address (unicast) or broadcast on its own.

use crate::controls::ControlInput;
use serde::Serialize;

/// The Art-Net UDP port.
pub const PORT: u16 = 6454;
/// Channels of one universe.
pub const UNIVERSE_LEN: usize = 512;
/// Highest 15-bit Port-Address.
pub const MAX_UNIVERSE: u16 = 0x7FFF;

const ID: &[u8; 8] = b"Art-Net\0";
const OP_DMX: u16 = 0x5000;
/// Oldest protocol revision we accept (Art-Net 3 and 4 are 14).
const MIN_PROT_VER: u16 = 14;

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Dmx { universe: u16, data: Vec<u8> },
    /// ArtTimeCode (T-409): the time a show controller sends.
    TimeCode { tc: crate::timecode::Tc, rate: crate::timecode::Rate },
    /// Any other Art-Net packet (ArtPoll, ArtSync…): ignored.
    Other(u16),
}

/// Decodes one UDP datagram.
pub fn decode(p: &[u8]) -> Result<Packet, &'static str> {
    if p.len() < 10 || &p[..8] != ID {
        return Err("pas un paquet Art-Net");
    }
    let op = u16::from_le_bytes([p[8], p[9]]);
    if op == crate::timecode::artnet::OP_TIMECODE {
        let (tc, rate) = crate::timecode::artnet::decode(p)?;
        return Ok(Packet::TimeCode { tc, rate });
    }
    if op != OP_DMX {
        return Ok(Packet::Other(op));
    }
    if p.len() < 18 {
        return Err("ArtDmx tronqué");
    }
    if u16::from_be_bytes([p[10], p[11]]) < MIN_PROT_VER {
        return Err("version Art-Net trop ancienne");
    }
    // p[12] sequence, p[13] physical: not used.
    let universe = u16::from_le_bytes([p[14], p[15] & 0x7F]);
    let len = u16::from_be_bytes([p[16], p[17]]) as usize;
    if !(2..=UNIVERSE_LEN).contains(&len) {
        return Err("longueur ArtDmx invalide");
    }
    let data = p.get(18..18 + len).ok_or("ArtDmx tronqué")?;
    Ok(Packet::Dmx { universe, data: data.to_vec() })
}

/// What one channel of the chart does.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Function {
    /// A control over its whole range; `centred`: 128 is exactly the middle
    /// (size 100 %, position 0, rotation stopped). `neutral` is the native
    /// value the « neutre » loss policy gives back.
    Control { id: &'static str, centred: bool, neutral: Option<f32> },
    /// The cue page: 256 values shared between the pages.
    Page,
    /// 0 = stop the cues; 1–40 = that cell of the page's grid.
    Cue,
    /// 128 and above: blackout (latches the emergency stop), on the way up.
    Blackout,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Channel {
    /// From the start address: 0 is the first channel.
    pub offset: usize,
    pub label_fr: &'static str,
    pub function: Function,
}

const fn ctl(id: &'static str, centred: bool, neutral: Option<f32>) -> Function {
    Function::Control { id, centred, neutral }
}

/// Our chart (our own layout). Intensity first, as consoles expect.
pub const CHART: [Channel; 11] = [
    Channel { offset: 0, label_fr: "Intensité maître", function: ctl("master.brightness", false, Some(0.0)) },
    Channel { offset: 1, label_fr: "Page de cues", function: Function::Page },
    Channel { offset: 2, label_fr: "Cue (0 = arrêt, 1–40 = case)", function: Function::Cue },
    Channel { offset: 3, label_fr: "Taille (128 = 100 %)", function: ctl("master.size", true, Some(1.0)) },
    Channel { offset: 4, label_fr: "Position X (128 = centre)", function: ctl("master.pos_x", true, Some(0.0)) },
    Channel { offset: 5, label_fr: "Position Y (128 = centre)", function: ctl("master.pos_y", true, Some(0.0)) },
    Channel { offset: 6, label_fr: "Rotation (128 = arrêt)", function: ctl("master.rot_z.speed", true, Some(0.0)) },
    Channel { offset: 7, label_fr: "Vitesse d'animation", function: ctl("master.speed", false, Some(1.0)) },
    Channel { offset: 8, label_fr: "Mode couleur", function: ctl("master.color.mode", false, None) },
    Channel { offset: 9, label_fr: "Teinte", function: ctl("master.color.hue", false, None) },
    Channel { offset: 10, label_fr: "Blackout (≥ 128)", function: Function::Blackout },
];

/// Highest start address that fits the whole chart in a universe.
pub const MAX_ADDRESS: u16 = (UNIVERSE_LEN - CHART.len() + 1) as u16;

/// A DMX value as a control input. Centred channels: 0 → 0, 128 → ½
/// exactly, 255 → 1.
pub fn input(v: u8, centred: bool) -> ControlInput {
    let v = v as f32;
    ControlInput::Norm(match centred {
        false => v / 255.0,
        true if v <= 128.0 => v / 256.0,
        true => 0.5 + (v - 128.0) / 254.0,
    })
}

/// The cue page a value picks (`pages` pages).
pub fn page(v: u8, pages: usize) -> usize {
    (v as usize * pages / 256).min(pages.saturating_sub(1))
}

/// Above the blackout threshold.
pub fn blackout_on(v: u8) -> bool {
    v >= 128
}

/// The chart's values in a universe from the 1-based `address`; `None`
/// past the end of a short packet.
pub fn chart_values(data: &[u8], address: u16) -> [Option<u8>; CHART.len()] {
    let start = address.max(1) as usize - 1;
    std::array::from_fn(|i| data.get(start + i).copied())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// An ArtDmx packet (tests and the UDP round trip).
    pub fn art_dmx(universe: u16, data: &[u8]) -> Vec<u8> {
        let mut p = ID.to_vec();
        p.extend_from_slice(&OP_DMX.to_le_bytes());
        p.extend_from_slice(&14u16.to_be_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&universe.to_le_bytes());
        p.extend_from_slice(&(data.len() as u16).to_be_bytes());
        p.extend_from_slice(data);
        p
    }

    #[test]
    fn decodes_art_dmx() {
        let mut data = vec![0u8; 512];
        data[0] = 255;
        data[511] = 7;
        assert_eq!(decode(&art_dmx(0x0123, &data)), Ok(Packet::Dmx { universe: 0x0123, data: data.clone() }));
        assert_eq!(decode(&art_dmx(0, &[1, 2])), Ok(Packet::Dmx { universe: 0, data: vec![1, 2] }));
        // The top bit of Net is not part of the 15-bit Port-Address.
        let mut p = art_dmx(0x7FFF, &[0, 0]);
        p[15] |= 0x80;
        assert!(matches!(decode(&p), Ok(Packet::Dmx { universe: 0x7FFF, .. })));
    }

    #[test]
    fn decodes_art_timecode() {
        use crate::timecode::{artnet::encode, Rate, Tc};
        let tc = Tc { h: 1, m: 0, s: 59, f: 29 };
        assert_eq!(decode(&encode(tc, Rate::F2997Df)), Ok(Packet::TimeCode { tc, rate: Rate::F2997Df }));
        let short = encode(tc, Rate::F30);
        assert!(decode(&short[..18]).is_err(), "truncated ArtTimeCode");
    }

    #[test]
    fn other_opcodes_are_ignored_and_garbage_refused() {
        let mut poll = ID.to_vec();
        poll.extend_from_slice(&0x2000u16.to_le_bytes());
        poll.extend_from_slice(&[0, 14, 0, 0]);
        assert_eq!(decode(&poll), Ok(Packet::Other(0x2000)));
        assert!(decode(b"Art-Net").is_err());
        assert!(decode(b"Not-Art\0\0\x50").is_err());
        let good = art_dmx(1, &[9; 20]);
        for n in 0..good.len() {
            assert!(decode(&good[..n]).is_err(), "truncated at {n}");
        }
        let mut old = good.clone();
        old[11] = 13;
        assert!(decode(&old).is_err(), "protocol revision 13");
        let mut long = good.clone();
        long[16..18].copy_from_slice(&600u16.to_be_bytes());
        assert!(decode(&long).is_err());
        let mut one = art_dmx(1, &[9, 9]);
        one[16..18].copy_from_slice(&1u16.to_be_bytes());
        assert!(decode(&one).is_err(), "length 1 is not allowed");
    }

    #[test]
    fn values_map_with_a_true_centre() {
        let n = |v, c| match input(v, c) {
            ControlInput::Norm(x) => x,
            ControlInput::Value(_) => unreachable!(),
        };
        assert_eq!((n(0, false), n(255, false)), (0.0, 1.0));
        assert_eq!((n(0, true), n(128, true), n(255, true)), (0.0, 0.5, 1.0));
        assert!(n(127, true) < 0.5 && n(129, true) > 0.5);
        assert_eq!((page(0, 9), page(255, 9), page(128, 9)), (0, 8, 4));
        assert!(!blackout_on(127) && blackout_on(128));
    }

    #[test]
    fn chart_values_follow_the_start_address() {
        let data: Vec<u8> = (0..20).collect();
        assert_eq!(chart_values(&data, 1)[0], Some(0));
        assert_eq!(chart_values(&data, 5)[10], Some(14));
        assert_eq!(chart_values(&data, 15)[5], Some(19));
        assert_eq!(chart_values(&data, 15)[6], None, "past a short packet");
        assert_eq!(MAX_ADDRESS, 502);
        assert!(CHART.iter().enumerate().all(|(i, c)| c.offset == i));
    }
}
