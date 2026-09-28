//! Analysis of the captured mono signal, one hop (256 samples) at a time,
//! on the analysis thread (never in the audio callback).
//!
//! It gives the dBFS meter (RMS and peak), the three features the engine
//! already uses (`level`, `bass`, `beat`; `level` and `bass` computed the
//! way the browser does it, T-230), the spectral frame (five bands with
//! auto-gain, centroid, flatness, silence: `spectrum.rs`, T-231) and the
//! onsets (kick / snare / hat: `onsets.rs`, T-232) and the tempo estimate
//! with its beat tracking (`bpm.rs`, T-233). The legacy `beat` is
//! the kick counter: it no longer fires on a bass line. T-237 moves `bass`
//! onto the bands.

use super::bpm::{BpmTracker, TempoEstimate};
use super::onsets::{OnsetDetector, Onsets};
use super::spectrum::{AnalysisConfig, SpectralAnalyzer, SpectralFrame, FFT_SIZE};
use crate::engine::AudioFeatures;

/// Samples per analysis step (5.3 ms at 48 kHz).
pub const HOP: usize = 256;
/// What silence reads as, in dBFS.
pub const FLOOR_DB: f32 = -120.0;

/// Smoothing of the RMS meter, like the browser's 2048-sample window.
const RMS_TAU_S: f32 = 0.04;
/// The peak meter falls back this fast.
const PEAK_RELEASE_DB_PER_S: f32 = 20.0;
/// `level` = RMS × this (the browser: RMS × gain 2 × 3).
const LEVEL_GAIN: f32 = 6.0;
/// Upper edge of the bass band.
const BASS_HZ: f32 = 150.0;
/// Bass level (dBFS) mapped to 0 and to 1.
const BASS_FLOOR_DB: f32 = -60.0;
const BASS_TOP_DB: f32 = -10.0;

/// One hop's result.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meter {
    /// Smoothed RMS level, dBFS.
    pub rms_db: f32,
    /// Peak with a 20 dB/s fall-back, dBFS.
    pub peak_db: f32,
    pub features: AudioFeatures,
    pub spectral: SpectralFrame,
    pub onsets: Onsets,
    /// BPM, confidence, beats, detector state (T-233).
    pub tempo: TempoEstimate,
}

/// A second-order low-pass (RBJ cookbook, Q = 1/√2), transposed direct
/// form II.
#[derive(Clone, Copy, Debug, Default)]
struct LowPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl LowPass {
    fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * (cutoff_hz / sample_rate).min(0.49);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cos) / a0;
        Self { b0: b1 / 2.0, b1, b2: b1 / 2.0, a1: -2.0 * cos / a0, a2: (1.0 - alpha) / a0, z1: 0.0, z2: 0.0 }
    }

    fn step(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

pub fn to_db(power: f32) -> f32 {
    if power <= 1e-12 {
        FLOOR_DB
    } else {
        (10.0 * power.log10()).max(FLOOR_DB)
    }
}

pub struct Analyzer {
    hop_s: f32,
    lp: LowPass,
    mean_sq: f32,
    bass_mean_sq: f32,
    peak_db: f32,
    spectral: SpectralAnalyzer,
    onsets: OnsetDetector,
    bpm: BpmTracker,
}

impl Analyzer {
    #[cfg(test)]
    pub fn new(sample_rate: u32) -> Self {
        Self::with_config(sample_rate, Onsets::default(), AnalysisConfig::default())
    }

    /// `carried` continues earlier onset counters (a reopened stream must
    /// not look like a new beat to the engine).
    pub fn with_config(sample_rate: u32, carried: Onsets, config: AnalysisConfig) -> Self {
        let rate = sample_rate.max(1) as f32;
        let onsets = OnsetDetector::new(sample_rate, carried, config.onsets);
        let bpm = BpmTracker::new(onsets.odf_rate(), FFT_SIZE);
        Self {
            hop_s: HOP as f32 / rate,
            lp: LowPass::new(BASS_HZ, rate),
            mean_sq: 0.0,
            bass_mean_sq: 0.0,
            peak_db: FLOOR_DB,
            spectral: SpectralAnalyzer::new(sample_rate, config),
            onsets,
            bpm,
        }
    }

    pub fn set_config(&mut self, config: AnalysisConfig) {
        self.spectral.set_config(config);
        self.onsets.set_config(config.onsets);
    }

    /// One hop of mono samples ending at `t` (seconds, studio clock).
    pub fn process(&mut self, hop: &[f32], t: f64) -> Meter {
        let n = hop.len().max(1) as f32;
        let (mut sum, mut bass_sum, mut peak) = (0.0f32, 0.0f32, 0.0f32);
        for &x in hop {
            let x = if x.is_finite() { x } else { 0.0 };
            sum += x * x;
            peak = peak.max(x.abs());
            let b = self.lp.step(x);
            bass_sum += b * b;
        }
        let dt = hop.len() as f32 * self.hop_s / HOP as f32;
        let k = 1.0 - (-dt / RMS_TAU_S).exp();
        self.mean_sq += (sum / n - self.mean_sq) * k;
        self.bass_mean_sq += (bass_sum / n - self.bass_mean_sq) * k;
        let peak_now = to_db(peak * peak);
        self.peak_db = peak_now.max(self.peak_db - PEAK_RELEASE_DB_PER_S * dt).max(FLOOR_DB);

        let level = (self.mean_sq.sqrt() * LEVEL_GAIN).min(1.0);
        let bass = ((to_db(self.bass_mean_sq) - BASS_FLOOR_DB) / (BASS_TOP_DB - BASS_FLOOR_DB)).clamp(0.0, 1.0);
        let rms_db = to_db(self.mean_sq);
        let spectral = self.spectral.process(hop, t, rms_db);
        let onsets = self.onsets.process(self.spectral.power(), t, spectral.silent);
        let tempo = self.bpm.process(self.onsets.band_flux(), t, spectral.silent);
        let features = AudioFeatures { level, bass, beat: onsets.kick };
        Meter { rms_db, peak_db: self.peak_db, features, spectral, onsets, tempo }
    }

    pub fn onsets(&self) -> Onsets {
        self.onsets.onsets()
    }

    pub fn tempo(&self) -> TempoEstimate {
        self.bpm.estimate()
    }

    /// A reopened input shows the last BPM until its first estimate.
    pub fn carry_bpm(&mut self, bpm: f32) {
        self.bpm.carry_bpm(bpm);
    }

    /// *Nouveau morceau*: the tempo history is forgotten.
    pub fn new_track(&mut self) {
        self.bpm.new_track();
    }

    /// A guide tempo for the estimator (T-234's *Guider*), or none.
    #[allow(dead_code)] // wired to the tap by T-234
    pub fn set_guide(&mut self, bpm: Option<f32>) {
        self.bpm.set_guide(bpm);
    }
}

#[cfg(test)]
pub mod testsig {
    //! Synthetic signals for the audio tests.

    pub fn sine(freq: f32, amp: f32, rate: u32, n: usize, phase0: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * freq * (i + phase0) as f32 / rate as f32).sin()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testsig::sine;
    use super::*;

    const RATE: u32 = 48_000;

    /// Runs `seconds` of `signal(sample_index)` and returns every meter.
    fn run(a: &mut Analyzer, seconds: f32, signal: impl Fn(usize) -> f32) -> Vec<Meter> {
        let hops = (seconds * RATE as f32) as usize / HOP;
        (0..hops)
            .map(|h| {
                let buf: Vec<f32> = (0..HOP).map(|i| signal(h * HOP + i)).collect();
                a.process(&buf, ((h + 1) * HOP) as f64 / RATE as f64)
            })
            .collect()
    }

    #[test]
    fn a_half_scale_sine_reads_minus_9_rms_and_minus_6_peak() {
        let mut a = Analyzer::new(RATE);
        let s = sine(1_000.0, 0.5, RATE, RATE as usize, 0);
        let m = *run(&mut a, 0.5, |i| s[i]).last().unwrap();
        assert!((m.rms_db - (-9.03)).abs() < 0.3, "{m:?}");
        assert!((m.peak_db - (-6.02)).abs() < 0.1, "{m:?}");
        assert!(m.features.level > 0.99, "0.35 RMS × 6 clips to 1");
        let m = *run(&mut Analyzer::new(RATE), 0.5, |i| s[i] * 0.1).last().unwrap();
        assert!((m.features.level - 0.2121).abs() < 0.01, "0.035 RMS × 6: {m:?}");
    }

    #[test]
    fn silence_reads_the_floor() {
        let mut a = Analyzer::new(RATE);
        let m = *run(&mut a, 0.2, |_| 0.0).last().unwrap();
        assert_eq!(m.rms_db, FLOOR_DB);
        assert_eq!(m.peak_db, FLOOR_DB);
        assert_eq!(m.features.level, 0.0);
        assert_eq!(m.features.bass, 0.0);
        assert_eq!(m.features.beat, 0);
    }

    #[test]
    fn the_peak_falls_back_at_20_db_per_second() {
        let mut a = Analyzer::new(RATE);
        run(&mut a, 0.05, |_| 1.0);
        let m = *run(&mut a, 0.5, |_| 0.0).last().unwrap();
        assert!((m.peak_db - (-10.0)).abs() < 0.3, "{m:?}");
    }

    #[test]
    fn bass_follows_low_notes_not_high_ones() {
        let mut low = Analyzer::new(RATE);
        let s = sine(50.0, 0.3, RATE, RATE as usize, 0);
        let bass_low = run(&mut low, 0.5, |i| s[i]).last().unwrap().features.bass;
        let mut high = Analyzer::new(RATE);
        let s = sine(5_000.0, 0.3, RATE, RATE as usize, 0);
        let bass_high = run(&mut high, 0.5, |i| s[i]).last().unwrap().features.bass;
        assert!(bass_low > 0.8, "{bass_low}");
        assert!(bass_high < 0.2, "{bass_high}");
    }

    #[test]
    fn kicks_at_120_bpm_count_beats_a_steady_tone_does_not() {
        // 60 Hz bursts of 80 ms every 0.5 s, for 4 s: 8 kicks, and the
        // legacy beat is the kick counter (carried over from 5).
        let carried = Onsets { kick: 5, ..Default::default() };
        let mut a = Analyzer::with_config(RATE, carried, AnalysisConfig::default());
        let kick = |i: usize| {
            let t = i as f32 / RATE as f32;
            if t % 0.5 < 0.08 {
                0.5 * (2.0 * std::f32::consts::PI * 60.0 * t).sin()
            } else {
                0.0
            }
        };
        let m = *run(&mut a, 4.0, kick).last().unwrap();
        assert_eq!((m.features.beat, m.onsets.kick), (13, 13), "{m:?}");
        let mut steady = Analyzer::new(RATE);
        let s = sine(60.0, 0.5, RATE, 4 * RATE as usize, 0);
        let beats = run(&mut steady, 4.0, |i| s[i]).last().unwrap().features.beat;
        assert!(beats <= 1, "a held bass note is not a stream of beats: {beats}");
    }

    #[test]
    fn non_finite_samples_are_ignored() {
        let mut a = Analyzer::new(RATE);
        let m = a.process(&[f32::NAN, f32::INFINITY, 0.0, 0.0], 0.1);
        assert!(m.rms_db.is_finite() && m.peak_db.is_finite() && m.features.level.is_finite());
        assert!(m.spectral.bands_db.iter().all(|d| d.is_finite()));
    }

    #[test]
    fn each_hop_also_gives_the_spectral_frame() {
        let mut a = Analyzer::new(RATE);
        let s = sine(5_000.0, 0.3, RATE, RATE as usize, 0);
        let m = *run(&mut a, 0.5, |i| s[i]).last().unwrap();
        assert!(m.spectral.bands.high > 0.9, "{m:?}");
        assert_eq!(m.spectral.level_db, m.rms_db, "one meter");
        assert!((m.spectral.t - 0.4960).abs() < 1e-3, "{m:?}");
        a.set_config(AnalysisConfig { auto_gain: false, ..Default::default() });
        let m = *run(&mut a, 0.1, |i| s[i]).last().unwrap();
        assert!((m.spectral.bands.high - 0.93).abs() < 0.03, "manual: (-13.5 + 60) / 50: {m:?}");
    }
}
