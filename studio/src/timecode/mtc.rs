//! MIDI Time Code in: quarter-frame messages (`F1 0nnn dddd`) assembled
//! into a time, and the full-frame SysEx (`F0 7F <dev> 01 01 hh mm ss ff
//! F7`) a source sends when it locates.
//!
//! Eight quarter frames carry one time, two frames long: piece 0 (frame
//! low nibble) is sent at the start of frame F, piece k a quarter frame
//! later each, so piece 7 arrives 1.75 frames after F began. The time is
//! taken only from a forward run 0 → 7 with no piece missing; anything
//! else (reverse play, a lost byte) waits for the next piece 0.
//!
//! Pure and allocation-free: the MIDI worker calls it under the `Shared`
//! lock, once per batch.

use super::{Rate, Tc};

/// Frames between the start of the encoded frame and the arrival of
/// piece 7.
pub const PIECE7_LAG_FRAMES: f64 = 1.75;

#[derive(Clone, Debug, Default)]
pub struct MtcDecoder {
    nibbles: [u8; 8],
    /// The piece expected next; `None` until a piece 0 starts a run.
    next: Option<u8>,
}

impl MtcDecoder {
    /// One quarter-frame data byte. A time when it completes a run.
    pub fn quarter(&mut self, data: u8) -> Option<(Tc, Rate)> {
        let piece = (data >> 4) & 0x07;
        let nibble = data & 0x0F;
        if piece == 0 {
            self.next = Some(0);
        }
        if self.next != Some(piece) {
            self.next = None;
            return None;
        }
        self.nibbles[piece as usize] = nibble;
        if piece < 7 {
            self.next = Some(piece + 1);
            return None;
        }
        self.next = None;
        let n = &self.nibbles;
        let rate = Rate::from_mtc_code((n[7] >> 1) & 0x03);
        let tc = Tc { h: n[6] | ((n[7] & 0x01) << 4), m: n[4] | ((n[5] & 0x03) << 4), s: n[2] | ((n[3] & 0x03) << 4), f: n[0] | ((n[1] & 0x01) << 4) };
        tc.valid(rate).then_some((tc, rate))
    }

    /// Forget a run in progress (source changed).
    pub fn reset(&mut self) {
        self.next = None;
    }
}

/// A full-frame message (`F0 7F <device> 01 01 hr mn sc fr F7`), any
/// device id.
pub fn full_frame(bytes: &[u8]) -> Option<(Tc, Rate)> {
    let [0xF0, 0x7F, _, 0x01, 0x01, hr, mn, sc, fr, 0xF7] = *bytes else { return None };
    let rate = Rate::from_mtc_code((hr >> 5) & 0x03);
    let tc = Tc { h: hr & 0x1F, m: mn, s: sc, f: fr };
    tc.valid(rate).then_some((tc, rate))
}

/// The eight quarter-frame data bytes for `tc` (our own test source).
#[cfg(test)]
pub fn quarter_frames(tc: Tc, rate: Rate) -> [u8; 8] {
    let code = rate.mtc_code();
    let nib = [tc.f & 0x0F, tc.f >> 4, tc.s & 0x0F, tc.s >> 4, tc.m & 0x0F, tc.m >> 4, tc.h & 0x0F, ((tc.h >> 4) & 0x01) | (code << 1)];
    let mut out = [0u8; 8];
    for (k, n) in nib.iter().enumerate() {
        out[k] = ((k as u8) << 4) | (n & 0x0F);
    }
    out
}

/// A full-frame SysEx for `tc` (our own test source).
#[cfg(test)]
pub fn full_frame_bytes(tc: Tc, rate: Rate) -> Vec<u8> {
    vec![0xF0, 0x7F, 0x7F, 0x01, 0x01, (rate.mtc_code() << 5) | tc.h, tc.m, tc.s, tc.f, 0xF7]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(h: u8, m: u8, s: u8, f: u8) -> Tc {
        Tc { h, m, s, f }
    }

    #[test]
    fn eight_pieces_in_order_give_the_time_and_rate() {
        for rate in Rate::ALL {
            let want = tc(1, 2, 3, 4);
            let mut d = MtcDecoder::default();
            let qf = quarter_frames(want, rate);
            for b in &qf[..7] {
                assert_eq!(d.quarter(*b), None);
            }
            assert_eq!(d.quarter(qf[7]), Some((want, rate)), "{rate:?}");
        }
    }

    #[test]
    fn a_run_must_start_at_piece_zero_and_skip_nothing() {
        let qf = quarter_frames(tc(10, 20, 30, 12), Rate::F25);
        let mut d = MtcDecoder::default();
        // Joined mid-run: nothing until the next piece 0.
        for b in &qf[3..] {
            assert_eq!(d.quarter(*b), None);
        }
        // A missing piece drops the run.
        for (k, b) in qf.iter().enumerate() {
            if k != 4 {
                assert_eq!(d.quarter(*b), None);
            }
        }
        // Reverse play (7 → 0) never completes.
        for b in qf.iter().rev() {
            assert_eq!(d.quarter(*b), None);
        }
        let got: Vec<_> = qf.iter().filter_map(|b| d.quarter(*b)).collect();
        assert_eq!(got, [(tc(10, 20, 30, 12), Rate::F25)]);
    }

    #[test]
    fn invalid_times_are_dropped() {
        let mut d = MtcDecoder::default();
        // Frame 27 at 25 im/s.
        assert!(quarter_frames(tc(0, 0, 0, 27), Rate::F25).iter().all(|b| d.quarter(*b).is_none()));
        // A dropped frame label (00:01:00;00) at 29.97 drop.
        assert!(quarter_frames(tc(0, 1, 0, 0), Rate::F2997Df).iter().all(|b| d.quarter(*b).is_none()));
        assert!(quarter_frames(tc(0, 10, 0, 0), Rate::F2997Df).iter().filter_map(|b| d.quarter(*b)).count() == 1);
    }

    #[test]
    fn full_frame_sysex() {
        let want = tc(23, 59, 58, 29);
        assert_eq!(full_frame(&full_frame_bytes(want, Rate::F30)), Some((want, Rate::F30)));
        assert_eq!(full_frame(&[0xF0, 0x7F, 0x00, 0x01, 0x01, 0x21, 2, 3, 4, 0xF7]), Some((tc(1, 2, 3, 4), Rate::F25)));
        // User bits, other universal messages, truncated: not a time.
        assert_eq!(full_frame(&[0xF0, 0x7F, 0x7F, 0x01, 0x02, 0x21, 2, 3, 4, 0xF7]), None);
        assert_eq!(full_frame(&[0xF0, 0x7E, 0x00, 0x06, 0x02, 0xF7]), None);
        assert_eq!(full_frame(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 2, 3]), None);
        assert_eq!(full_frame(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 61, 3, 4, 0xF7]), None);
    }
}
