//! Spectral analysis (T-231), one hop at a time on the analysis thread.
//!
//! - **Five bands** by IIR filters on the time signal (a 1024-point FFT
//!   has 47 Hz bins: useless to split 20–60 from 60–150 Hz). Each band is
//!   a Butterworth high-pass + low-pass cascade, 8th order on the inner
//!   edges (60, 150, 500, 2000 Hz) so a sine next to an edge doesn't light
//!   its neighbour; its power is smoothed (a period of the band's lowest
//!   note) and read in dBFS.
//! - **Auto-gain** per band: ceiling = peak with a slow release, floor =
//!   5th percentile over 10 s, normalised to 0..1. So a quiet bar and a
//!   loud festival feed give the same numbers. It is frozen while
//!   `silent` (the noise floor must not be blown up after a track ends).
//! - **FFT** (1024, Hann, every hop, `realfft`, plans and buffers
//!   allocated once): spectral centroid and flatness now (T-232/T-236),
//!   and the power spectrum kept for the onset function (T-232).
//! - **Silence**: every hop under `silence_db` for 300 ms.

use super::analysis::{to_db, FLOOR_DB};
use super::onsets::OnsetConfig;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// FFT length (21.3 ms at 48 kHz); a new transform every hop.
pub const FFT_SIZE: usize = 1024;
pub const BAND_COUNT: usize = 5;
/// `sub`, `bass`, `low_mid`, `mid`, `high`, in Hz.
pub const BAND_EDGES_HZ: [(f32, f32); BAND_COUNT] = [(20.0, 60.0), (60.0, 150.0), (150.0, 500.0), (500.0, 2_000.0), (2_000.0, 12_000.0)];
/// Butterworth order of the inner edges, and of the outer ones (20 Hz is
/// only rumble removal, 12 kHz only an upper limit).
const EDGE_ORDER: usize = 8;
const LOW_OUTER_ORDER: usize = 2;
const HIGH_OUTER_ORDER: usize = 4;
/// Power smoothing per band: about a period of its lowest frequency, so a
/// steady low note reads steady.
const BAND_TAU_S: [f32; BAND_COUNT] = [0.05, 0.03, 0.015, 0.01, 0.01];

/// Auto-gain: the floor is the 5th percentile of 10 s, kept as one value
/// (mean dB) per 100 ms block.
const BLOCK_S: f32 = 0.1;
const HISTORY_LEN: usize = 100;
const FLOOR_PERCENTILE: f32 = 0.05;
/// The ceiling follows a louder hop at once and falls back towards the
/// current level with this time constant (the "5–10 s release").
const CEILING_RELEASE_S: f32 = 7.0;
/// The normalised range spans at least this (a steady tone at the ceiling
/// reads 1, not 0) and at most this (a gap in the music doesn't flatten
/// everything else).
const MIN_SPAN_DB: f32 = 12.0;
const MAX_SPAN_DB: f32 = 48.0;
/// A band's ceiling never sits more than this under the loudest band's:
/// a nearly empty band (a sine's leakage, a track with no highs) is not
/// boosted to full scale.
const MAX_BOOST_DB: f32 = 10.0;
/// Nor under the silence threshold + this: room noise stays low.
const MIN_CEILING_OVER_SILENCE_DB: f32 = 6.0;
/// Without auto-gain: band dBFS (+ manual gain) mapped to 0 and to 1, as
/// the browser's bass meter.
const MANUAL_FLOOR_DB: f32 = -60.0;
const MANUAL_TOP_DB: f32 = -10.0;
/// `silent` once every hop has been under the threshold this long.
pub const SILENCE_HOLD_S: f32 = 0.3;
/// Samples are clamped to this (+12 dBFS): a runaway float input can't
/// overflow the filters.
const SAMPLE_LIMIT: f32 = 4.0;

/// Bands of the display spectrum (`log_spectrum`).
pub const SPECTRUM_BANDS: usize = 64;
const SPECTRUM_LO_HZ: f32 = 20.0;
const SPECTRUM_HI_HZ: f32 = 20_000.0;

/// Band levels normalised to 0..1 (auto-gain or manual gain applied).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bands {
    pub sub: f32,
    pub bass: f32,
    pub low_mid: f32,
    pub mid: f32,
    pub high: f32,
}

impl Bands {
    pub fn from_array(a: [f32; BAND_COUNT]) -> Self {
        Self { sub: a[0], bass: a[1], low_mid: a[2], mid: a[3], high: a[4] }
    }

    pub fn to_array(self) -> [f32; BAND_COUNT] {
        [self.sub, self.bass, self.low_mid, self.mid, self.high]
    }
}

/// One hop's spectral analysis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SpectralFrame {
    /// Audio time of the end of the hop (studio clock, seconds).
    pub t: f64,
    pub bands: Bands,
    /// Smoothed band levels, dBFS, before any gain (`sub` … `high`).
    pub bands_db: [f32; BAND_COUNT],
    /// Smoothed full-band RMS, dBFS.
    pub level_db: f32,
    /// Spectral centroid, Hz (0 in silence).
    pub centroid_hz: f32,
    /// Spectral flatness, 0 (a pure tone) .. 1 (white noise); 0 in silence.
    pub flatness: f32,
    pub silent: bool,
}

impl Default for SpectralFrame {
    fn default() -> Self {
        Self { t: 0.0, bands: Bands::default(), bands_db: [FLOOR_DB; BAND_COUNT], level_db: FLOOR_DB, centroid_hz: 0.0, flatness: 0.0, silent: true }
    }
}

/// Operator settings of the analysis; in `audio.json` (`analysis`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisConfig {
    /// Normalise each band to its own recent range.
    pub auto_gain: bool,
    /// Used instead of the auto-gain when that is off, dB.
    pub manual_gain_db: f32,
    /// Under this (dBFS) for 300 ms = silence.
    pub silence_db: f32,
    /// Onset detection: sensitivity, look-ahead, kick spacing (T-232).
    pub onsets: OnsetConfig,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self { auto_gain: true, manual_gain_db: 0.0, silence_db: -60.0, onsets: OnsetConfig::default() }
    }
}

impl AnalysisConfig {
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.manual_gain_db = if self.manual_gain_db.is_finite() { self.manual_gain_db.clamp(-40.0, 40.0) } else { d.manual_gain_db };
        self.silence_db = if self.silence_db.is_finite() { self.silence_db.clamp(-100.0, -20.0) } else { d.silence_db };
        self.onsets = self.onsets.sanitized();
        self
    }
}

#[derive(Clone, Copy, Debug)]
enum Pass {
    Low,
    High,
}

/// A second-order section (RBJ cookbook), transposed direct form II, in
/// f64: the 20–60 Hz sections at 48 kHz have poles very close to 1.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(pass: Pass, cutoff_hz: f64, q: f64, rate: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * cutoff_hz / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1) = match pass {
            Pass::Low => ((1.0 - cos) / 2.0, 1.0 - cos),
            Pass::High => ((1.0 + cos) / 2.0, -(1.0 + cos)),
        };
        Self { b0: b0 / a0, b1: b1 / a0, b2: b0 / a0, a1: -2.0 * cos / a0, a2: (1.0 - alpha) / a0, z1: 0.0, z2: 0.0 }
    }

    fn step(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The sections of an even-order Butterworth filter.
fn butterworth(pass: Pass, cutoff_hz: f32, order: usize, rate: f32, out: &mut Vec<Biquad>) {
    // Past 0.45 × the rate the edge is meaningless (and unstable): skip it.
    if cutoff_hz >= 0.45 * rate {
        return;
    }
    for k in 0..order / 2 {
        let q = 1.0 / (2.0 * ((2 * k + 1) as f64 * std::f64::consts::PI / (2 * order) as f64).cos());
        out.push(Biquad::new(pass, cutoff_hz as f64, q, rate as f64));
    }
}

/// Per-band auto-gain state.
struct AutoGain {
    ceiling: f32,
    history: [f32; HISTORY_LEN],
    len: usize,
    pos: usize,
    block_sum: f32,
    block_hops: u32,
    block_s: f32,
    /// 5th percentile of `history` (+∞ while empty).
    floor: f32,
    scratch: [f32; HISTORY_LEN],
}

impl AutoGain {
    fn new() -> Self {
        Self { ceiling: FLOOR_DB, history: [0.0; HISTORY_LEN], len: 0, pos: 0, block_sum: 0.0, block_hops: 0, block_s: 0.0, floor: f32::INFINITY, scratch: [0.0; HISTORY_LEN] }
    }

    fn update(&mut self, db: f32, dt: f32) {
        if db > self.ceiling {
            self.ceiling = db;
        } else {
            // Towards the current level, but never aiming lower than the
            // normalised span: the 300 ms before `silent` barely move it.
            let target = db.max(self.ceiling - MAX_SPAN_DB);
            self.ceiling += (target - self.ceiling) * (1.0 - (-dt / CEILING_RELEASE_S).exp());
        }
        self.block_sum += db;
        self.block_hops += 1;
        self.block_s += dt;
        if self.block_s >= BLOCK_S {
            self.history[self.pos] = self.block_sum / self.block_hops as f32;
            self.pos = (self.pos + 1) % HISTORY_LEN;
            self.len = (self.len + 1).min(HISTORY_LEN);
            (self.block_sum, self.block_hops, self.block_s) = (0.0, 0, 0.0);
            let s = &mut self.scratch[..self.len];
            s.copy_from_slice(&self.history[..self.len]);
            let i = ((self.len - 1) as f32 * FLOOR_PERCENTILE).round() as usize;
            let (_, v, _) = s.select_nth_unstable_by(i, f32::total_cmp);
            self.floor = *v;
        }
    }

    /// `ceiling`: the effective one (after the boost limits).
    fn normalise(&self, db: f32, ceiling: f32) -> f32 {
        let floor = self.floor.min(ceiling - MIN_SPAN_DB).max(ceiling - MAX_SPAN_DB);
        ((db - floor) / (ceiling - floor)).clamp(0.0, 1.0)
    }
}

struct Band {
    filters: Vec<Biquad>,
    tau_s: f32,
    mean_sq: f32,
    gain: AutoGain,
}

pub struct SpectralAnalyzer {
    rate: f32,
    config: AnalysisConfig,
    bands: [Band; BAND_COUNT],
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// The last `FFT_SIZE` samples, oldest first.
    frame: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    power: Vec<f32>,
    quiet_s: f32,
}

impl SpectralAnalyzer {
    pub fn new(sample_rate: u32, config: AnalysisConfig) -> Self {
        let rate = sample_rate.max(1) as f32;
        let bands = std::array::from_fn(|i| {
            let (lo, hi) = BAND_EDGES_HZ[i];
            let mut filters = Vec::new();
            butterworth(Pass::High, lo, if i == 0 { LOW_OUTER_ORDER } else { EDGE_ORDER }, rate, &mut filters);
            butterworth(Pass::Low, hi, if i == BAND_COUNT - 1 { HIGH_OUTER_ORDER } else { EDGE_ORDER }, rate, &mut filters);
            Band { filters, tau_s: BAND_TAU_S[i], mean_sq: 0.0, gain: AutoGain::new() }
        });
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos()).collect();
        let (input, spectrum, scratch) = (fft.make_input_vec(), fft.make_output_vec(), fft.make_scratch_vec());
        Self {
            rate,
            config: config.sanitized(),
            bands,
            window,
            frame: vec![0.0; FFT_SIZE],
            input,
            power: vec![0.0; spectrum.len()],
            spectrum,
            scratch,
            fft,
            quiet_s: 0.0,
        }
    }

    pub fn set_config(&mut self, config: AnalysisConfig) {
        self.config = config.sanitized();
    }

    /// Power spectrum of the last hop's FFT (`FFT_SIZE / 2 + 1` bins of
    /// `rate / FFT_SIZE` Hz), for the onset function (T-232).
    pub fn power(&self) -> &[f32] {
        &self.power
    }

    /// The last FFT as `SPECTRUM_BANDS` log-spaced bands from 20 Hz to
    /// 20 kHz (or Nyquist), dBFS (a full-scale sine reads 0), each the
    /// loudest bin in its range (the nearest bin where a low band is
    /// narrower than a bin). For the display (`GET /api/audio/spectrum`);
    /// no allocation.
    pub fn log_spectrum(&self, out: &mut [f32; SPECTRUM_BANDS]) {
        let hz_per_bin = self.rate / FFT_SIZE as f32;
        let nyquist = self.rate / 2.0;
        // A full-scale sine through the Hann window peaks at |X| = N/4.
        let reference = (FFT_SIZE as f32 / 4.0).powi(2);
        let ratio = SPECTRUM_HI_HZ / SPECTRUM_LO_HZ;
        let last = self.power.len() - 1;
        for (i, slot) in out.iter_mut().enumerate() {
            let lo = SPECTRUM_LO_HZ * ratio.powf(i as f32 / SPECTRUM_BANDS as f32);
            let hi = (SPECTRUM_LO_HZ * ratio.powf((i + 1) as f32 / SPECTRUM_BANDS as f32)).min(nyquist);
            if lo >= nyquist {
                *slot = FLOOR_DB;
                continue;
            }
            let (k0, k1) = ((lo / hz_per_bin).ceil() as usize, ((hi / hz_per_bin).ceil() as usize).min(last + 1));
            let p = if k0 < k1 {
                self.power[k0..k1].iter().fold(0.0f32, |m, &p| m.max(p))
            } else {
                self.power[((lo * hi).sqrt() / hz_per_bin).round().min(last as f32) as usize]
            };
            *slot = to_db(p / reference);
        }
    }

    /// One hop of mono samples ending at `t`; `level_db` is the smoothed
    /// full-band RMS (the meter's).
    pub fn process(&mut self, hop: &[f32], t: f64, level_db: f32) -> SpectralFrame {
        let len = hop.len();
        if len < FFT_SIZE {
            self.frame.copy_within(len.., 0);
        }
        let mut sums = [0.0f64; BAND_COUNT];
        let mut hop_sq = 0.0f32;
        for (i, &x) in hop.iter().enumerate() {
            let x = if x.is_finite() { x.clamp(-SAMPLE_LIMIT, SAMPLE_LIMIT) } else { 0.0 };
            hop_sq += x * x;
            for (band, sum) in self.bands.iter_mut().zip(sums.iter_mut()) {
                let y = band.filters.iter_mut().fold(x as f64, |y, f| f.step(y));
                *sum += y * y;
            }
            if len < FFT_SIZE {
                self.frame[FFT_SIZE - len + i] = x;
            } else if i >= len - FFT_SIZE {
                self.frame[i + FFT_SIZE - len] = x;
            }
        }
        let n = len.max(1) as f32;
        let dt = len as f32 / self.rate;

        // Silence: judged on each hop's own RMS, so it comes on in 300 ms
        // whatever the smoothing.
        if to_db(hop_sq / n) < self.config.silence_db {
            self.quiet_s += dt;
        } else {
            self.quiet_s = 0.0;
        }
        let silent = self.quiet_s >= SILENCE_HOLD_S - 1e-4;

        let mut bands_db = [FLOOR_DB; BAND_COUNT];
        for (i, band) in self.bands.iter_mut().enumerate() {
            let k = 1.0 - (-dt / band.tau_s).exp();
            band.mean_sq += (sums[i] as f32 / n - band.mean_sq) * k;
            bands_db[i] = to_db(band.mean_sq);
            if !silent {
                band.gain.update(bands_db[i], dt);
            }
        }

        let mut norm = [0.0f32; BAND_COUNT];
        if self.config.auto_gain {
            let loudest = self.bands.iter().map(|b| b.gain.ceiling).fold(FLOOR_DB, f32::max);
            let lowest = self.config.silence_db + MIN_CEILING_OVER_SILENCE_DB;
            for (i, band) in self.bands.iter().enumerate() {
                let ceiling = band.gain.ceiling.max(loudest - MAX_BOOST_DB).max(lowest);
                norm[i] = band.gain.normalise(bands_db[i], ceiling);
            }
        } else {
            for (v, db) in norm.iter_mut().zip(bands_db) {
                *v = ((db + self.config.manual_gain_db - MANUAL_FLOOR_DB) / (MANUAL_TOP_DB - MANUAL_FLOOR_DB)).clamp(0.0, 1.0);
            }
        }

        let (centroid_hz, flatness) = self.spectrum_shape();
        SpectralFrame { t, bands: Bands::from_array(norm), bands_db, level_db, centroid_hz, flatness, silent }
    }

    /// FFT of the last `FFT_SIZE` samples → centroid (Hz) and flatness.
    fn spectrum_shape(&mut self) -> (f32, f32) {
        for ((dst, &x), &w) in self.input.iter_mut().zip(&self.frame).zip(&self.window) {
            *dst = x * w;
        }
        if self.fft.process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch).is_err() {
            return (0.0, 0.0);
        }
        for (p, c) in self.power.iter_mut().zip(&self.spectrum) {
            *p = c.norm_sqr();
        }
        // DC left out.
        let bins = &self.power[1..];
        let total: f32 = bins.iter().sum();
        if total < 1e-10 {
            return (0.0, 0.0);
        }
        let hz_per_bin = self.rate / FFT_SIZE as f32;
        let weighted: f32 = bins.iter().enumerate().map(|(k, p)| (k + 1) as f32 * p).sum();
        let eps = total * 1e-10;
        let mean_ln = bins.iter().map(|p| (p + eps).ln()).sum::<f32>() / bins.len() as f32;
        let flatness = (mean_ln.exp() / (total / bins.len() as f32)).clamp(0.0, 1.0);
        (weighted / total * hz_per_bin, flatness)
    }
}

#[cfg(test)]
mod tests {
    use super::super::analysis::testsig::sine;
    use super::super::analysis::HOP;
    use super::*;

    const RATE: u32 = 48_000;

    /// A deterministic noise source (xorshift), uniform in -1..1.
    struct Noise(u32);

    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
        }
    }

    /// Runs `seconds` of `signal(sample_index)` and returns every frame.
    fn run(a: &mut SpectralAnalyzer, from: usize, seconds: f32, mut signal: impl FnMut(usize) -> f32) -> Vec<SpectralFrame> {
        let hops = (seconds * RATE as f32) as usize / HOP;
        let mut buf = [0.0f32; HOP];
        (0..hops)
            .map(|h| {
                let start = from + h * HOP;
                let mut sq = 0.0;
                for (i, s) in buf.iter_mut().enumerate() {
                    *s = signal(start + i);
                    sq += *s * *s;
                }
                a.process(&buf, (start + HOP) as f64 / RATE as f64, to_db(sq / HOP as f32))
            })
            .collect()
    }

    fn tone(freq: f32, amp: f32) -> impl FnMut(usize) -> f32 {
        let s = sine(freq, amp, RATE, 4 * RATE as usize, 0);
        move |i| s[i % s.len()]
    }

    fn mean_bands(frames: &[SpectralFrame]) -> [f32; BAND_COUNT] {
        let mut m = [0.0; BAND_COUNT];
        for f in frames {
            for (acc, v) in m.iter_mut().zip(f.bands.to_array()) {
                *acc += v / frames.len() as f32;
            }
        }
        m
    }

    fn argmax(a: [f32; BAND_COUNT]) -> usize {
        (0..BAND_COUNT).max_by(|&i, &j| a[i].total_cmp(&a[j])).unwrap()
    }

    #[test]
    fn a_40_hz_sine_is_sub_and_nothing_else() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        let frames = run(&mut a, 0, 3.0, tone(40.0, 0.3));
        let last = frames.last().unwrap();
        let b = last.bands;
        assert!(b.sub > 0.9, "{b:?}");
        for (name, v) in [("bass", b.bass), ("low_mid", b.low_mid), ("mid", b.mid), ("high", b.high)] {
            assert!(v < 0.1, "{name} = {v}: {b:?}");
        }
        // 0.3 peak = -13.5 dBFS RMS, all of it in the sub band.
        assert!((last.bands_db[0] - (-13.5)).abs() < 1.0, "{:?}", last.bands_db);
        assert!(last.bands_db[1] < -35.0, "{:?}", last.bands_db);
        assert!(!last.silent);
    }

    #[test]
    fn a_5_khz_sine_is_high() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        let b = run(&mut a, 0, 3.0, tone(5_000.0, 0.3)).last().unwrap().bands;
        assert!(b.high > 0.9, "{b:?}");
        for v in [b.sub, b.bass, b.low_mid, b.mid] {
            assert!(v < 0.1, "{b:?}");
        }
    }

    #[test]
    fn sines_land_in_their_band() {
        for (freq, band) in [(30.0, 0), (45.0, 0), (90.0, 1), (120.0, 1), (250.0, 2), (400.0, 2), (800.0, 3), (1_500.0, 3), (3_000.0, 4), (9_000.0, 4)] {
            let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
            let last = *run(&mut a, 0, 2.0, tone(freq, 0.2)).last().unwrap();
            assert_eq!(argmax(last.bands_db), band, "{freq} Hz: {:?}", last.bands_db);
            assert_eq!(argmax(last.bands.to_array()), band, "{freq} Hz: {:?}", last.bands);
            // Passband: the level is the sine's (-17 dBFS RMS) within 3.5 dB.
            assert!((last.bands_db[band] - (-17.0)).abs() < 3.5, "{freq} Hz: {:?}", last.bands_db);
        }
    }

    /// Synthetic "music" at `gain`: noise through the bands' natural tilt,
    /// a 50 Hz kick every 0.5 s, a hat every 0.25 s, a 400 Hz chord.
    fn music(gain: f32) -> impl FnMut(usize) -> f32 {
        let mut noise = Noise(0x1234_5678);
        let mut lp = 0.0f32;
        move |i| {
            let t = i as f32 / RATE as f32;
            let tau = std::f32::consts::TAU;
            let n = noise.next();
            lp += (n - lp) * 0.05;
            let kick_t = t % 0.5;
            let kick = if kick_t < 0.12 { (1.0 - kick_t / 0.12) * (tau * 50.0 * t).sin() } else { 0.0 };
            let hat = if t % 0.25 < 0.03 { 0.3 * n } else { 0.0 };
            let chord = 0.15 * ((tau * 400.0 * t).sin() + (tau * 1_000.0 * t).sin());
            gain * (0.8 * kick + 0.3 * lp + hat + chord)
        }
    }

    #[test]
    fn the_same_music_at_minus_30_and_minus_10_dbfs_reads_the_same() {
        let run_at = |gain: f32| {
            let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
            let frames = run(&mut a, 0, 12.0, music(gain));
            (mean_bands(&frames[frames.len() - 375..]), frames.last().unwrap().level_db)
        };
        // Scaled to about -10 and -30 dBFS RMS.
        let (loud, loud_db) = run_at(0.8);
        let (quiet, quiet_db) = run_at(0.08);
        assert!((loud_db - quiet_db - 20.0).abs() < 1.0, "{loud_db} vs {quiet_db}");
        for i in 0..BAND_COUNT {
            assert!((loud[i] - quiet[i]).abs() <= 0.1, "band {i}: {loud:?} vs {quiet:?}");
        }
        // And the music actually moves the bands.
        assert!(loud.iter().all(|&v| v > 0.05), "{loud:?}");
    }

    #[test]
    fn digital_silence_is_detected_in_300_ms() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        let music_s = 2.0;
        let frames = run(&mut a, 0, music_s, music(0.5));
        assert!(frames.iter().all(|f| !f.silent));
        let from = (music_s * RATE as f32) as usize / HOP * HOP;
        let frames = run(&mut a, from, 1.0, |_| 0.0);
        let first = frames.iter().position(|f| f.silent).expect("silent at some point");
        let hop_s = HOP as f32 / RATE as f32;
        let at = (first + 1) as f32 * hop_s;
        assert!((at - SILENCE_HOLD_S).abs() <= hop_s, "silent after {at} s");
        assert!(frames[first..].iter().all(|f| f.silent));
        let last = frames.last().unwrap();
        assert_eq!(last.bands, Bands::default(), "silence reads 0 everywhere");
        assert_eq!((last.centroid_hz, last.flatness), (0.0, 0.0));
    }

    #[test]
    fn the_auto_gain_is_frozen_during_silence() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        run(&mut a, 0, 3.0, tone(90.0, 0.5));
        let before = a.bands[1].gain.ceiling;
        run(&mut a, 0, 0.35, |_| 0.0);
        let ceiling = a.bands[1].gain.ceiling;
        assert!(before - ceiling < 3.0, "the 300 ms before `silent` barely release it: {before} → {ceiling}");
        run(&mut a, 0, 20.0, |_| 0.0);
        assert_eq!(a.bands[1].gain.ceiling, ceiling, "no release while silent");
        // The same note 26 dB quieter just after the silence reads low: the
        // gain comes back slowly, the background isn't blown up.
        let b = run(&mut a, 0, 0.2, tone(90.0, 0.025)).last().unwrap().bands;
        assert!(b.bass < 0.4, "{b:?}");
        // ... and catches up after a while.
        let b = run(&mut a, 0, 20.0, tone(90.0, 0.025)).last().unwrap().bands;
        assert!(b.bass > 0.8, "{b:?}");
    }

    #[test]
    fn clipping_and_garbage_stay_bounded() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        // A sine driven 4× into a hard clip, then runaway float samples.
        let mut s = tone(100.0, 4.0);
        let frames = run(&mut a, 0, 1.0, |i| s(i).clamp(-1.0, 1.0));
        let garbage = [f32::NAN, f32::INFINITY, -1e30, 1e30];
        let more = run(&mut a, 0, 0.5, |i| garbage[i % 4]);
        for f in frames.iter().chain(&more) {
            for v in f.bands.to_array() {
                assert!((0.0..=1.0).contains(&v), "{f:?}");
            }
            assert!(f.bands_db.iter().all(|d| d.is_finite() && *d <= 13.0), "{f:?}");
            assert!(f.centroid_hz.is_finite() && (0.0..=1.0).contains(&f.flatness), "{f:?}");
        }
        let last = frames.last().unwrap();
        assert!(last.bands.bass > 0.9, "{last:?}");
        assert!(last.bands_db[1] > -4.0, "a clipped 100 Hz square is loud in the bass: {:?}", last.bands_db);
        assert!(last.bands.mid > 0.0, "its harmonics reach the mids: {:?}", last.bands);
    }

    #[test]
    fn manual_gain_replaces_the_auto_gain() {
        let config = AnalysisConfig { auto_gain: false, ..Default::default() };
        let mut a = SpectralAnalyzer::new(RATE, config);
        // 0.1 peak = -23 dBFS RMS → (−23 + 60) / 50.
        let b = run(&mut a, 0, 1.0, tone(40.0, 0.1)).last().unwrap().bands;
        assert!((b.sub - 0.74).abs() < 0.03, "{b:?}");
        a.set_config(AnalysisConfig { manual_gain_db: 10.0, ..config });
        let b = run(&mut a, 0, 0.1, tone(40.0, 0.1)).last().unwrap().bands;
        assert!((b.sub - 0.94).abs() < 0.03, "{b:?}");
        // Back to auto: full scale at once (the auto-gain kept tracking).
        a.set_config(AnalysisConfig::default());
        let b = run(&mut a, 0, 0.1, tone(40.0, 0.1)).last().unwrap().bands;
        assert!(b.sub > 0.9, "{b:?}");
    }

    #[test]
    fn the_silence_threshold_is_configurable() {
        // -50 dBFS of noise: music for the default threshold, silence at -40.
        let quiet = |a: &mut SpectralAnalyzer| {
            let mut n = Noise(7);
            run(a, 0, 1.0, move |_| 0.0055 * n.next()).last().unwrap().silent
        };
        assert!(!quiet(&mut SpectralAnalyzer::new(RATE, AnalysisConfig::default())));
        assert!(quiet(&mut SpectralAnalyzer::new(RATE, AnalysisConfig { silence_db: -40.0, ..Default::default() })));
    }

    #[test]
    fn the_display_spectrum_puts_a_tone_in_its_band_at_its_level() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        run(&mut a, 0, 0.3, tone(1_000.0, 0.5));
        let mut out = [0.0; SPECTRUM_BANDS];
        a.log_spectrum(&mut out);
        let loudest = (0..SPECTRUM_BANDS).max_by(|&i, &j| out[i].total_cmp(&out[j])).unwrap();
        // Band i spans 20 × 1000^(i/64) .. 20 × 1000^((i+1)/64) Hz: 1 kHz is band 36.
        assert_eq!(loudest, 36, "{out:?}");
        assert!((out[loudest] + 6.0).abs() < 1.5, "a half-scale sine reads about −6 dBFS: {}", out[loudest]);
        assert!(out[10] < out[loudest] - 40.0 && out[60] < out[loudest] - 40.0, "{out:?}");
        assert!(out.iter().all(|v| v.is_finite()));
        // Silence: everything at the floor; 16 kHz: bands past 8 kHz too.
        let mut quiet = SpectralAnalyzer::new(16_000, AnalysisConfig::default());
        run(&mut quiet, 0, 0.1, |_| 0.0);
        quiet.log_spectrum(&mut out);
        assert!(out.iter().all(|&v| v == FLOOR_DB), "{out:?}");
        let n = crate::audio::capture::tests::allocations_during(|| a.log_spectrum(&mut out));
        assert_eq!(n, 0);
    }

    #[test]
    fn centroid_and_flatness() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        let f = *run(&mut a, 0, 0.5, tone(1_000.0, 0.5)).last().unwrap();
        assert!((f.centroid_hz - 1_000.0).abs() < 60.0, "{f:?}");
        assert!(f.flatness < 0.05, "a tone is not flat: {f:?}");
        let mut n = Noise(99);
        let f = *run(&mut a, 0, 0.5, move |_| 0.5 * n.next()).last().unwrap();
        assert!(f.flatness > 0.4, "white noise is flat: {f:?}");
        assert!((f.centroid_hz - 12_000.0).abs() < 1_000.0, "white noise centres on rate / 4: {f:?}");
        assert_eq!(a.power().len(), FFT_SIZE / 2 + 1);
    }

    #[test]
    fn config_defaults_and_sanitising() {
        let c: AnalysisConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, AnalysisConfig { auto_gain: true, manual_gain_db: 0.0, silence_db: -60.0, onsets: OnsetConfig::default() });
        let c = AnalysisConfig { manual_gain_db: 99.0, silence_db: f32::NAN, auto_gain: false, ..Default::default() }.sanitized();
        assert_eq!(c, AnalysisConfig { auto_gain: false, manual_gain_db: 40.0, silence_db: -60.0, ..Default::default() });
        assert_eq!(AnalysisConfig { silence_db: -5.0, ..Default::default() }.sanitized().silence_db, -20.0);
    }

    #[test]
    fn other_sample_rates_work() {
        for rate in [16_000u32, 44_100, 96_000] {
            let mut a = SpectralAnalyzer::new(rate, AnalysisConfig::default());
            let s = sine(40.0, 0.3, rate, rate as usize, 0);
            let mut last = SpectralFrame::default();
            for (h, chunk) in s.as_chunks::<HOP>().0.iter().enumerate() {
                last = a.process(chunk, h as f64, -10.0);
            }
            assert_eq!(argmax(last.bands.to_array()), 0, "{rate} Hz: {last:?}");
            assert!(last.bands.high < 0.1 && last.bands.bass < 0.1, "{rate} Hz: {last:?}");
        }
    }

    #[test]
    fn a_hop_does_not_allocate() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        let hop = sine(440.0, 0.5, RATE, HOP, 0);
        let long = sine(440.0, 0.5, RATE, 2 * FFT_SIZE, 0);
        a.process(&hop, 0.0, -10.0);
        let n = crate::audio::capture::tests::allocations_during(|| {
            for i in 0..200 {
                std::hint::black_box(a.process(&hop, i as f64, -10.0));
            }
            std::hint::black_box(a.process(&long, 1.0, -10.0));
            std::hint::black_box(a.process(&hop[..7], 1.0, -10.0));
        });
        assert_eq!(n, 0, "the FFT plan and buffers are allocated once");
    }

    /// Acceptance: under 2 % of a core. Timing-sensitive, so opt-in; run
    /// in release: `cargo test -p laser-studio --release -- --ignored spectral_cost`.
    #[test]
    #[ignore]
    fn spectral_cost_is_under_2_percent_of_a_core() {
        let mut a = SpectralAnalyzer::new(RATE, AnalysisConfig::default());
        let seconds = 30.0;
        let mut m = music(0.5);
        let signal: Vec<f32> = (0..(seconds * RATE as f32) as usize).map(&mut m).collect();
        let start = std::time::Instant::now();
        for (h, chunk) in signal.as_chunks::<HOP>().0.iter().enumerate() {
            std::hint::black_box(a.process(chunk, h as f64, -20.0));
        }
        let share = start.elapsed().as_secs_f32() / seconds;
        eprintln!("spectral analysis: {:.3} % of a core", share * 100.0);
        assert!(share < 0.02, "{:.3} % of a core", share * 100.0);
    }
}
