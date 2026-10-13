//! Linear (SMPTE) timecode read from an audio input.
//!
//! LTC is a bi-phase mark code: the level flips at the start of every
//! bit, and a `1` flips again half-way. A frame is 80 bits, sent over the
//! frame's duration (2 000 bit/s at 25 im/s), ending with the sync word
//! `0011 1111 1111 1101`. The decoder only measures the time between
//! level flips (zero crossings with hysteresis): a long interval is a
//! `0`, two short ones a `1`. The bit length it compares against follows
//! the signal, so a slightly fast or slow tape still decodes.
//!
//! Runs on the audio analysis thread (audio/worker.rs), never in the
//! audio callback and never under the `Shared` lock: the callback only
//! copies the chosen channel into a ring. No allocation per sample.

use super::Tc;

/// The sync word as it sits in bits 64..79 of a frame (bit 64 first).
const SYNC: u16 = 0xBFFC;
/// A flip shorter than this fraction of a bit is a half bit.
const HALF_BELOW: f64 = 0.75;
/// Longer than this many bits: a gap in the signal, start over.
const GAP_BITS: f64 = 1.6;
/// Shorter than this fraction of a bit: noise, ignored.
const GLITCH_BELOW: f64 = 0.2;
/// Lowest level that counts as a signal (about -46 dBFS).
const MIN_LEVEL: f32 = 0.005;

/// One frame read off the signal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LtcFrame {
    pub tc: Tc,
    /// The drop-frame flag (bit 10).
    pub drop_frame: bool,
    /// Frames per second measured on the signal.
    pub fps: f64,
    /// Sample index (counted from the first sample fed) at which the
    /// frame ended: the time `tc` + 1 frame.
    pub end_sample: u64,
}

pub struct LtcDecoder {
    rate: f64,
    /// Current level (true = above the upper threshold last).
    high: bool,
    /// Peak follower for the hysteresis threshold.
    env: f32,
    env_decay: f32,
    /// Sample of the last flip.
    last_edge: u64,
    /// Bit length estimate, in samples.
    bit: f64,
    /// A first half bit waits for its second half (its length).
    half: Option<f64>,
    /// The last 80 bits, newest at bit 79.
    reg: u128,
    /// Bits shifted in since the last reset (a frame needs 80).
    bits: u32,
    /// Samples fed so far.
    samples: u64,
    last_sync: Option<u64>,
    /// Frames per second measured between syncs (smoothed).
    fps: Option<f64>,
}

impl LtcDecoder {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(8_000) as f64;
        LtcDecoder {
            rate,
            high: false,
            env: 0.0,
            // About 50 ms to forget a louder passage.
            env_decay: (-1.0 / (0.05 * rate)).exp() as f32,
            last_edge: 0,
            // 2 000 bit/s (25 im/s) to begin with: 24 to 30 im/s are
            // all within the half / whole margins of that.
            bit: rate / 2_000.0,
            half: None,
            reg: 0,
            bits: 0,
            samples: 0,
            last_sync: None,
            fps: None,
        }
    }

    /// Feeds samples (-1..1); calls `out` for every complete frame.
    pub fn feed(&mut self, samples: &[f32], mut out: impl FnMut(LtcFrame)) {
        for &x in samples {
            let a = x.abs();
            self.env = if a > self.env { a } else { self.env * self.env_decay };
            let thr = (self.env * 0.3).max(MIN_LEVEL);
            let flip = if self.high { x < -thr } else { x > thr };
            if flip {
                self.high = !self.high;
                let d = (self.samples - self.last_edge) as f64;
                self.last_edge = self.samples;
                self.interval(d, &mut out);
            }
            self.samples += 1;
        }
    }

    fn resync(&mut self) {
        self.half = None;
        self.bits = 0;
    }

    fn interval(&mut self, d: f64, out: &mut impl FnMut(LtcFrame)) {
        if d < GLITCH_BELOW * self.bit {
            return;
        }
        if d > GAP_BITS * self.bit {
            self.resync();
            return;
        }
        if d < HALF_BELOW * self.bit {
            match self.half.take() {
                Some(first) => {
                    self.adapt(first + d);
                    self.push(true, out);
                }
                None => self.half = Some(d),
            }
        } else {
            if self.half.take().is_some() {
                // A lone half bit: we were out of step. The `0` that
                // follows puts us back.
                self.bits = 0;
            }
            self.adapt(d);
            self.push(false, out);
        }
    }

    fn adapt(&mut self, bit: f64) {
        // Within ±25 % of 24..30 im/s only, so noise can't drag it away.
        let lo = self.rate / (32.0 * 80.0);
        let hi = self.rate / (22.0 * 80.0);
        self.bit = (self.bit + 0.05 * (bit - self.bit)).clamp(lo, hi);
    }

    fn push(&mut self, one: bool, out: &mut impl FnMut(LtcFrame)) {
        self.reg = (self.reg >> 1) | ((one as u128) << 79);
        self.bits = self.bits.saturating_add(1);
        if self.bits < 80 || (self.reg >> 64) as u16 != SYNC {
            return;
        }
        let end = self.samples;
        let measured = self.last_sync.map(|p| (end - p) as f64).filter(|n| (self.rate / 32.0..=self.rate / 22.0).contains(n));
        self.last_sync = Some(end);
        let fps_now = measured.map_or(self.rate / (80.0 * self.bit), |n| self.rate / n);
        let fps = match self.fps {
            Some(f) if measured.is_some() => f + 0.2 * (fps_now - f),
            _ => fps_now,
        };
        self.fps = Some(fps);
        if let Some((tc, drop_frame)) = fields(self.reg) {
            out(LtcFrame { tc, drop_frame, fps, end_sample: end });
        }
    }
}

/// The time and the drop-frame flag of a frame's 64 data bits, if valid.
fn fields(reg: u128) -> Option<(Tc, bool)> {
    let nib = |at: u32, n: u32| ((reg >> at) & ((1 << n) - 1)) as u8;
    let units = [nib(0, 4), nib(16, 4), nib(32, 4), nib(48, 4)];
    if units.iter().any(|&u| u > 9) {
        return None;
    }
    let tc = Tc { f: units[0] + 10 * nib(8, 2), s: units[1] + 10 * nib(24, 3), m: units[2] + 10 * nib(40, 3), h: units[3] + 10 * nib(56, 2) };
    (tc.f < 30 && tc.s < 60 && tc.m < 60 && tc.h < 24).then_some((tc, nib(10, 1) == 1))
}

/// Our own LTC source for the tests: `frames` frames from `start`, as a
/// square wave of amplitude `amp`, softened by a one-pole low-pass (a
/// real cable's rise time).
#[cfg(test)]
pub fn encode(start: Tc, rate: super::Rate, frames: usize, sample_rate: u32, amp: f32) -> Vec<f32> {
    let bit_len = sample_rate as f64 / (rate.fps() * 80.0);
    let first = start.frames(rate);
    let mut out = Vec::with_capacity((bit_len * 80.0 * frames as f64) as usize + 2);
    let mut level = 1.0f32;
    let mut smooth = 0.0f32;
    let mut k = 0u64;
    for i in 0..frames {
        let tc = Tc::from_frames(first + i as i64, rate);
        let bits = frame_bits(tc, rate);
        for (b, one) in bits.iter().enumerate() {
            let start_t = ((i * 80 + b) as f64) * bit_len;
            let mid_t = start_t + bit_len / 2.0;
            let end_t = start_t + bit_len;
            // A flip at every bit start, and half-way through a `1`.
            level = -level;
            while (k as f64) < end_t {
                let v = if *one && (k as f64) >= mid_t { -level } else { level };
                smooth += 0.5 * (v * amp - smooth);
                out.push(smooth);
                k += 1;
            }
            if *one {
                level = -level;
            }
        }
    }
    out
}

/// The 80 bits of a frame, in the order they are sent (SMPTE 12M): the
/// time in BCD, the drop-frame flag (bit 10), user bits and the colour
/// flag left at 0, the sync word, and the polarity correction bit (27, or
/// 59 at 25 im/s) set so the frame holds an even number of `1`s: every
/// frame then starts on the same level. Shared by the tests' encoder and
/// the timecode output (ltc_out.rs).
pub fn frame_bits(tc: Tc, rate: super::Rate) -> [bool; 80] {
    let mut bits = [false; 80];
    let mut put = |at: usize, n: usize, v: u8| {
        for i in 0..n {
            bits[at + i] = (v >> i) & 1 == 1;
        }
    };
    put(0, 4, tc.f % 10);
    put(8, 2, tc.f / 10);
    put(10, 1, rate.drop_frame() as u8);
    put(16, 4, tc.s % 10);
    put(24, 3, tc.s / 10);
    put(32, 4, tc.m % 10);
    put(40, 3, tc.m / 10);
    put(48, 4, tc.h % 10);
    put(56, 2, tc.h / 10);
    for i in 0..16 {
        bits[64 + i] = (SYNC >> i) & 1 == 1;
    }
    let polarity = if rate == super::Rate::F25 { 59 } else { 27 };
    bits[polarity] = bits.iter().filter(|&&b| b).count() % 2 == 1;
    bits
}

#[cfg(test)]
mod tests {
    use super::super::Rate;
    use super::*;

    fn decode_all(signal: &[f32], sample_rate: u32, chunk: usize) -> Vec<LtcFrame> {
        let mut d = LtcDecoder::new(sample_rate);
        let mut got = Vec::new();
        for c in signal.chunks(chunk) {
            d.feed(c, |f| got.push(f));
        }
        got
    }

    #[test]
    fn the_sync_word_is_where_the_spec_puts_it() {
        let bits = frame_bits(Tc::default(), Rate::F30);
        let word: String = bits[64..].iter().map(|&b| if b { '1' } else { '0' }).collect();
        assert_eq!(word, "0011111111111101");
    }

    /// Generated LTC at every rate and two sample rates: every frame after
    /// the first is read back, in order, each within one frame of when it
    /// was sent, and the rate is recognised.
    #[test]
    fn generated_ltc_decodes_frame_by_frame() {
        let start = Tc { h: 1, m: 0, s: 59, f: 0 };
        for sample_rate in [48_000, 44_100] {
            for rate in Rate::ALL {
                let frames = 3 * rate.nominal() as usize;
                let signal = encode(start, rate, frames, sample_rate, 0.4);
                let got = decode_all(&signal, sample_rate, 256);
                assert!(got.len() >= frames - 1, "{rate:?} at {sample_rate}: {} of {frames}", got.len());
                let first = start.frames(rate);
                let frame_s = 1.0 / rate.fps();
                for f in &got {
                    let n = f.tc.frames(rate) - first;
                    assert!((0..frames as i64).contains(&n), "{:?}", f.tc);
                    // Sent from n × frame_s to (n + 1) × frame_s.
                    let end_s = f.end_sample as f64 / sample_rate as f64;
                    assert!((end_s - (n + 1) as f64 * frame_s).abs() < frame_s, "{rate:?} {:?} at {end_s}", f.tc);
                    assert_eq!(f.drop_frame, rate.drop_frame());
                }
                let last = got.last().unwrap();
                assert_eq!(Rate::from_ltc(last.fps, last.drop_frame), rate, "{rate:?} measured {}", last.fps);
                // In order, no frame skipped.
                assert!(got.windows(2).all(|w| w[1].tc.frames(rate) == w[0].tc.frames(rate) + 1));
            }
        }
    }

    #[test]
    fn quiet_noisy_and_varispeed_signals_still_decode() {
        let start = Tc { h: 0, m: 9, s: 59, f: 20 };
        // -30 dBFS with noise from a fixed-seed generator.
        let mut seed = 0x1234_5678u32;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 - 0.5) * 0.004
        };
        let quiet: Vec<f32> = encode(start, Rate::F25, 50, 48_000, 0.03).into_iter().map(|x| x + noise()).collect();
        assert!(decode_all(&quiet, 48_000, 512).len() >= 48);
        // 4 % fast: the 25 im/s signal of a 48 kHz file read as 49.92 kHz.
        let fast = encode(start, Rate::F25, 50, 48_000, 0.4);
        let got = decode_all(&fast, 49_920, 128);
        assert!(got.len() >= 48);
        assert!((got.last().unwrap().fps - 26.0).abs() < 0.2, "{}", got.last().unwrap().fps);
        assert_eq!(Rate::from_ltc(got.last().unwrap().fps, false), Rate::F25);
    }

    #[test]
    fn silence_noise_and_music_give_nothing() {
        assert!(decode_all(&vec![0.0; 48_000], 48_000, 256).is_empty());
        let mut seed = 99u32;
        let noise: Vec<f32> = (0..96_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
            })
            .collect();
        assert!(decode_all(&noise, 48_000, 256).is_empty());
        let tone: Vec<f32> = (0..96_000).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1_000.0 / 48_000.0).sin() * 0.5).collect();
        assert!(decode_all(&tone, 48_000, 256).is_empty());
    }

    #[test]
    fn a_dropout_resumes_on_the_next_frame() {
        let start = Tc { h: 2, m: 0, s: 0, f: 0 };
        let mut signal = encode(start, Rate::F30, 30, 48_000, 0.4);
        // 100 ms of silence, then the signal again from a later frame.
        signal.extend(std::iter::repeat_n(0.0, 4_800));
        let resume = Tc { h: 2, m: 0, s: 10, f: 0 };
        signal.extend(encode(resume, Rate::F30, 30, 48_000, 0.4));
        let got = decode_all(&signal, 48_000, 300);
        assert!(got.iter().any(|f| f.tc == Tc { h: 2, m: 0, s: 0, f: 28 }));
        assert!(got.iter().any(|f| f.tc == Tc { h: 2, m: 0, s: 10, f: 28 }));
        assert!(got.iter().all(|f| f.tc.h == 2));
    }

    #[test]
    fn feeding_does_not_allocate() {
        let signal = encode(Tc::default(), Rate::F25, 10, 48_000, 0.4);
        let mut d = LtcDecoder::new(48_000);
        let mut n = 0;
        let allocs = crate::audio::capture::tests::allocations_during(|| d.feed(&signal, |_| n += 1));
        assert_eq!(allocs, 0);
        assert!(n >= 9);
    }
}
