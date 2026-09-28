//! Analysis of the captured mono signal, one hop (256 samples) at a time,
//! on the analysis thread (never in the audio callback).
//!
//! For now (T-230) it gives the dBFS meter (RMS and peak) and the three
//! features the engine already uses (`level`, `bass`, `beat`), computed
//! the way the browser does it so a look reacts the same with either
//! source. T-231/T-232 replace `bass` and `beat` with bands, auto-gain
//! and real onsets.

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
/// Browser beat rule: bass > 1.35 × its average (EMA 0.05 per 25 ms
/// step), > 0.12, at most one beat per 200 ms.
const BEAT_RATIO: f32 = 1.35;
const BEAT_MIN: f32 = 0.12;
const BEAT_REFRACTORY_S: f64 = 0.2;
const BEAT_AVG_KEEP_PER_25MS: f32 = 0.95;
/// Unlike the browser, a beat re-arms only once the bass has fallen back
/// under this × its average: the slow decay of one kick is not a second
/// beat.
const BEAT_REARM_RATIO: f32 = 1.1;

/// One hop's result.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meter {
    /// Smoothed RMS level, dBFS.
    pub rms_db: f32,
    /// Peak with a 20 dB/s fall-back, dBFS.
    pub peak_db: f32,
    pub features: AudioFeatures,
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
    bass_avg: f32,
    last_beat_t: f64,
    beat_armed: bool,
    beat: u64,
}

impl Analyzer {
    /// `beat` continues an earlier counter (a reopened stream must not
    /// look like a new beat to the engine).
    pub fn new(sample_rate: u32, beat: u64) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            hop_s: HOP as f32 / rate,
            lp: LowPass::new(BASS_HZ, rate),
            mean_sq: 0.0,
            bass_mean_sq: 0.0,
            peak_db: FLOOR_DB,
            bass_avg: 0.0,
            last_beat_t: f64::NEG_INFINITY,
            beat_armed: true,
            beat,
        }
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
        let keep = BEAT_AVG_KEEP_PER_25MS.powf(dt / 0.025);
        self.bass_avg = self.bass_avg * keep + bass * (1.0 - keep);
        if bass < self.bass_avg * BEAT_REARM_RATIO {
            self.beat_armed = true;
        }
        if self.beat_armed && bass > self.bass_avg * BEAT_RATIO && bass > BEAT_MIN && t - self.last_beat_t > BEAT_REFRACTORY_S {
            self.beat += 1;
            self.last_beat_t = t;
            self.beat_armed = false;
        }
        Meter { rms_db: to_db(self.mean_sq), peak_db: self.peak_db, features: AudioFeatures { level, bass, beat: self.beat } }
    }

    pub fn beat(&self) -> u64 {
        self.beat
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
        let mut a = Analyzer::new(RATE, 0);
        let s = sine(1_000.0, 0.5, RATE, RATE as usize, 0);
        let m = *run(&mut a, 0.5, |i| s[i]).last().unwrap();
        assert!((m.rms_db - (-9.03)).abs() < 0.3, "{m:?}");
        assert!((m.peak_db - (-6.02)).abs() < 0.1, "{m:?}");
        assert!(m.features.level > 0.99, "0.35 RMS × 6 clips to 1");
        let m = *run(&mut Analyzer::new(RATE, 0), 0.5, |i| s[i] * 0.1).last().unwrap();
        assert!((m.features.level - 0.2121).abs() < 0.01, "0.035 RMS × 6: {m:?}");
    }

    #[test]
    fn silence_reads_the_floor() {
        let mut a = Analyzer::new(RATE, 0);
        let m = *run(&mut a, 0.2, |_| 0.0).last().unwrap();
        assert_eq!(m.rms_db, FLOOR_DB);
        assert_eq!(m.peak_db, FLOOR_DB);
        assert_eq!(m.features.level, 0.0);
        assert_eq!(m.features.bass, 0.0);
        assert_eq!(m.features.beat, 0);
    }

    #[test]
    fn the_peak_falls_back_at_20_db_per_second() {
        let mut a = Analyzer::new(RATE, 0);
        run(&mut a, 0.05, |_| 1.0);
        let m = *run(&mut a, 0.5, |_| 0.0).last().unwrap();
        assert!((m.peak_db - (-10.0)).abs() < 0.3, "{m:?}");
    }

    #[test]
    fn bass_follows_low_notes_not_high_ones() {
        let mut low = Analyzer::new(RATE, 0);
        let s = sine(50.0, 0.3, RATE, RATE as usize, 0);
        let bass_low = run(&mut low, 0.5, |i| s[i]).last().unwrap().features.bass;
        let mut high = Analyzer::new(RATE, 0);
        let s = sine(5_000.0, 0.3, RATE, RATE as usize, 0);
        let bass_high = run(&mut high, 0.5, |i| s[i]).last().unwrap().features.bass;
        assert!(bass_low > 0.8, "{bass_low}");
        assert!(bass_high < 0.2, "{bass_high}");
    }

    #[test]
    fn kicks_at_120_bpm_count_beats_a_steady_tone_does_not() {
        // 60 Hz bursts of 80 ms every 0.5 s, for 4 s: about 8 beats.
        let mut a = Analyzer::new(RATE, 5);
        let kick = |i: usize| {
            let t = i as f32 / RATE as f32;
            if t % 0.5 < 0.08 {
                0.5 * (2.0 * std::f32::consts::PI * 60.0 * t).sin()
            } else {
                0.0
            }
        };
        let beats = run(&mut a, 4.0, kick).last().unwrap().features.beat - 5;
        assert!((7..=9).contains(&beats), "{beats} beats");
        let mut steady = Analyzer::new(RATE, 0);
        let s = sine(60.0, 0.5, RATE, 4 * RATE as usize, 0);
        let beats = run(&mut steady, 4.0, |i| s[i]).last().unwrap().features.beat;
        assert!(beats <= 1, "a held bass note is not a stream of beats: {beats}");
    }

    #[test]
    fn non_finite_samples_are_ignored() {
        let mut a = Analyzer::new(RATE, 0);
        let m = a.process(&[f32::NAN, f32::INFINITY, 0.0, 0.0], 0.1);
        assert!(m.rms_db.is_finite() && m.peak_db.is_finite() && m.features.level.is_finite());
    }
}
