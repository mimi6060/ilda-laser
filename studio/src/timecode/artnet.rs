//! Art-Net timecode (T-409): the ArtTimeCode packet (OpTimeCode 0x9700),
//! decoded from the public Art-Net 4 specification. The listener of
//! netin/ receives it (receive only, never an answer) and hands it to the
//! timecode chase; `encode` builds the same packet for the tests and the
//! `--test-hooks` injector (and, later, a timecode output).
//!
//! Layout: "Art-Net\0", OpCode 0x9700 (little endian), protocol version
//! (big endian, 14), Filler, StreamId, Frames, Seconds, Minutes, Hours,
//! Type (0 film 24, 1 EBU 25, 2 DF 29.97, 3 SMPTE 30). 19 bytes.

use super::{Rate, Tc};

pub const OP_TIMECODE: u16 = 0x9700;
const ID: &[u8; 8] = b"Art-Net\0";
const LEN: usize = 19;
/// Oldest protocol revision we accept (Art-Net 3 and 4 are 14).
const MIN_PROT_VER: u16 = 14;

/// The Type byte of a rate.
pub fn type_code(rate: Rate) -> u8 {
    match rate {
        Rate::F24 => 0,
        Rate::F25 => 1,
        Rate::F2997Df => 2,
        Rate::F30 => 3,
    }
}

fn rate_of(code: u8) -> Option<Rate> {
    Some(match code {
        0 => Rate::F24,
        1 => Rate::F25,
        2 => Rate::F2997Df,
        3 => Rate::F30,
        _ => return None,
    })
}

/// One ArtTimeCode datagram. Anything else (another opcode, a short
/// packet, a label that doesn't exist at its rate) is an error.
pub fn decode(p: &[u8]) -> Result<(Tc, Rate), &'static str> {
    if p.len() < 10 || &p[..8] != ID {
        return Err("pas un paquet Art-Net");
    }
    if u16::from_le_bytes([p[8], p[9]]) != OP_TIMECODE {
        return Err("pas un ArtTimeCode");
    }
    if p.len() < LEN {
        return Err("ArtTimeCode tronqué");
    }
    if u16::from_be_bytes([p[10], p[11]]) < MIN_PROT_VER {
        return Err("version Art-Net trop ancienne");
    }
    // p[12] filler, p[13] stream id: one stream is followed, whichever.
    let rate = rate_of(p[18]).ok_or("type de timecode inconnu")?;
    let tc = Tc { f: p[14], s: p[15], m: p[16], h: p[17] };
    if !tc.valid(rate) {
        return Err("timecode invalide");
    }
    Ok((tc, rate))
}

/// The ArtTimeCode packet of `tc` at `rate`.
pub fn encode(tc: Tc, rate: Rate) -> Vec<u8> {
    let mut p = ID.to_vec();
    p.extend_from_slice(&OP_TIMECODE.to_le_bytes());
    p.extend_from_slice(&MIN_PROT_VER.to_be_bytes());
    p.extend_from_slice(&[0, 0, tc.f, tc.s, tc.m, tc.h, type_code(rate)]);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_at_the_four_rates() {
        for rate in Rate::ALL {
            for n in [0i64, 1, 1_799, 1_800, 17_982, 107_892, 2_000_000] {
                let tc = Tc::from_frames(n, rate);
                let p = encode(tc, rate);
                assert_eq!(p.len(), LEN);
                assert_eq!(decode(&p), Ok((tc, rate)), "{rate:?} {tc}");
            }
        }
        // Bytes as the specification lays them out.
        let p = encode(Tc { h: 1, m: 2, s: 3, f: 4 }, Rate::F25);
        assert_eq!(&p[8..10], &[0x00, 0x97]);
        assert_eq!(&p[14..], &[4, 3, 2, 1, 1]);
    }

    #[test]
    fn truncated_foreign_and_impossible_packets_are_refused() {
        let good = encode(Tc { h: 10, m: 20, s: 30, f: 12 }, Rate::F30);
        for n in 0..good.len() {
            assert!(decode(&good[..n]).is_err(), "truncated at {n}");
        }
        let mut dmx = good.clone();
        dmx[8..10].copy_from_slice(&0x5000u16.to_le_bytes());
        assert_eq!(decode(&dmx), Err("pas un ArtTimeCode"));
        let mut kind = good.clone();
        kind[18] = 4;
        assert!(decode(&kind).is_err());
        let mut old = good.clone();
        old[11] = 13;
        assert!(decode(&old).is_err());
        // Frame 25 at 25 im/s, and a dropped label at 29.97 DF.
        assert!(decode(&encode(Tc { h: 0, m: 0, s: 0, f: 25 }, Rate::F25)).is_err());
        assert!(decode(&encode(Tc { h: 0, m: 1, s: 0, f: 0 }, Rate::F2997Df)).is_err());
        assert!(decode(&encode(Tc { h: 0, m: 10, s: 0, f: 0 }, Rate::F2997Df)).is_ok());
        assert!(decode(b"Not-Art\0\x00\x97").is_err());
    }
}
