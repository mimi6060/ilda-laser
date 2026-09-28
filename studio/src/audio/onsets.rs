//! Onsets (T-232), one hop at a time on the analysis thread, from the
//! power spectrum `spectrum.rs` already computes (no second FFT).
//!
//! - **Onset detection function** (ODF): log-compressed, half-wave
//!   rectified spectral flux (Bello et al. 2005, Dixon 2006) in its
//!   **SuperFlux** form (Böck & Widmer 2013): each bin is compared with the
//!   frame two hops back, maximum-filtered over 3 bins, so a note that
//!   slides by a bin (vibrato, a bass line changing note) is not an onset.
//!   Computed over the whole spectrum (the 8 s history is kept for the
//!   tempo, T-233) and per band: 40–150 Hz, 150 Hz–5 kHz, 5–15 kHz. Each
//!   band's flux is the mean over its bins, so one sensitivity `delta`
//!   means the same everywhere.
//! - **Peak picking**, causal (Dixon 2006, Böck et al. 2012): a hop is an
//!   onset if it is the maximum of the last 30 ms and of the `lookahead`
//!   hops after it (0–2, 5–10 ms of delay), at least `delta` above the
//!   mean of the previous 100 ms, and at least 30 ms after the previous
//!   one (the kick's own spacing is `kick_refractory_ms`).
//! - **Kick / snare / hi-hat** by rules on the energy each onset brings
//!   (the power of the frames after the peak minus the lowest of the
//!   ~26 ms before it), not by a learned model:
//!   - kick: a 40–150 Hz onset whose band power jumps ≥ 9 dB over its mean
//!     of the previous ~85 ms (fast rise; a bass line changing note or two
//!     low notes beating don't), whose new energy is not outweighed by the
//!     new energy above 150 Hz and is a real share of the mix;
//!   - snare: a 150 Hz–5 kHz onset whose new energy is spread across the
//!     500 Hz–5 kHz bins (a noise burst, not a note's harmonics), is not
//!     just a kick's leakage (1–5 kHz share) and is not hat-like;
//!   - hi-hat: a 5–15 kHz onset whose new energy per bin is ≥ 6 dB above
//!     the new energy per bin in 1–2 kHz ("little energy below 2 kHz";
//!     lower is left out so a kick under the hat doesn't hide it).
//!
//!   A peak the rules turn down does not start the refractory period.
//!
//! Counters (`u64`, like the legacy `beat`) so a missed snapshot never
//! loses an event, plus a 0..1 strength (the peak against the band's
//! recent loudest onset) and the audio time of the ODF peak.
//!
//! Everything is allocated in `new`: a hop allocates nothing.

use super::analysis::HOP;
use super::spectrum::FFT_SIZE;
use serde::{Deserialize, Serialize};

/// Log compression `log10(1 + GAMMA·|X|)`, with |X| scaled so that a
/// full-scale sine's bin reads 1: above about −70 dBFS per bin the flux
/// is in decibels (the same for a quiet and a loud feed), below it the
/// noise floor is squashed towards 0.
const GAMMA: f32 = 1.0e4;
/// SuperFlux: the reference frame is this many hops back (10.7 ms at
/// 48 kHz, where the 1024-sample windows overlap by half)...
const LAG: usize = 2;
/// ... maximum-filtered over ±1 bin.
const MAX_FILTER_RADIUS: usize = 1;

/// The onset detection functions: whole spectrum, then per band.
const ODF_COUNT: usize = 4;
const ODF_FULL: usize = 0;
const ODF_LOW: usize = 1;
const ODF_MID: usize = 2;
const ODF_HIGH: usize = 3;
const ODF_BANDS_HZ: [(f32, f32); ODF_COUNT] = [(0.0, f32::MAX), (40.0, 150.0), (150.0, 5_000.0), (5_000.0, 15_000.0)];

/// Bands whose power is tracked for the rules.
const POWER_COUNT: usize = 5;
const P_LOW: usize = 0;
const P_LOWMID: usize = 1;
const P_HIGH: usize = 2;
const P_ABOVE_LOW: usize = 3;
const P_CRACK: usize = 4;
const POWER_BANDS_HZ: [(f32, f32); POWER_COUNT] = [(40.0, 150.0), (1_000.0, 2_000.0), (5_000.0, 15_000.0), (150.0, 15_000.0), (1_000.0, 5_000.0)];
/// Where a snare's noise is looked for.
const SPREAD_BAND_HZ: (f32, f32) = (500.0, 5_000.0);

/// Peak picking: local maximum over the previous 30 ms, mean over the
/// previous 100 ms, at least 30 ms between two onsets of one function.
const PRE_MAX_S: f32 = 0.03;
const PRE_AVG_S: f32 = 0.1;
const MIN_SPACING_S: f32 = 0.03;
pub const MAX_LOOKAHEAD_HOPS: u8 = 2;
/// Strength = peak / the band's loudest recent peak, which falls back
/// with this time constant and never under `MIN_PEAK_REF`.
const STRENGTH_RELEASE_S: f32 = 5.0;
const MIN_PEAK_REF: f32 = 0.5;

/// The energy an onset brings: the band power after the peak (the peak
/// hop and the look-ahead ones) against the lowest of this many hops
/// before it (~26 ms at 48 kHz)...
const BEFORE_HOPS: u64 = 5;
/// ... or, for the kick, against their mean over this many (~85 ms): the
/// slow swell of two low notes beating is not a kick (its trough is
/// shorter than that), and the mean of the three flickering low bins of a
/// rumble is steadier than their minimum.
const KICK_BEFORE_HOPS: u64 = 16;
/// Per-hop band powers kept for that (> KICK_BEFORE_HOPS + look-ahead).
const HISTORY: usize = 24;
/// Kick: the 40–150 Hz power jumps at least this much (a kick over a
/// bass line is some 12 dB; the three low bins of a rumble flicker by
/// 6 dB)...
const KICK_RISE_DB: f32 = 9.0;
/// ... its new energy is at least this × the new energy above 150 Hz...
const KICK_DOMINANCE: f32 = 0.5;
/// ... and this × the whole power above 150 Hz (the 3 bins of the low
/// band flicker in noise; a kick is a big share of the mix).
const KICK_SHARE: f32 = 0.2;
/// Snare: its 1–5 kHz new energy is at least this × the low band's (a
/// kick's own leakage up there is some 30 dB under its body).
const SNARE_SHARE: f32 = 0.003;
/// Snare: at least this share of the 500 Hz–5 kHz bins rose by
/// `SPREAD_RISE` (log10 units, ≈ 3 dB) and are within 30 dB of the
/// band's loudest bin (a kick's leakage falls off far faster).
const SNARE_SPREAD: f32 = 0.4;
const SPREAD_RISE: f32 = 0.15;
const SPREAD_FLOOR: f32 = 1.0e-3;
/// Hat-like: new energy per bin in 5–15 kHz ≥ this × that in 1–2 kHz
/// (6 dB); a band must also rise by `MIN_RISE_DB` to count at all.
const HAT_RATIO: f32 = 4.0;
const MIN_RISE_DB: f32 = 3.0;

/// Operator settings; `analysis.onsets` in `audio.json`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OnsetConfig {
    /// Sensitivity: how far (mean log10 flux per bin) a peak must stand
    /// above the recent mean. Lower = more onsets. To be tuned on the
    /// T-244 corpus.
    pub delta: f64,
    /// Hops (5.3 ms at 48 kHz) a peak is confirmed after: 0..2. More =
    /// fewer double detections, more delay.
    pub lookahead_hops: u8,
    /// Minimum time between two kicks.
    pub kick_refractory_ms: f32,
}

impl Default for OnsetConfig {
    fn default() -> Self {
        Self { delta: 0.1, lookahead_hops: 1, kick_refractory_ms: 100.0 }
    }
}

impl OnsetConfig {
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        self.delta = if self.delta.is_finite() { self.delta.clamp(0.01, 2.0) } else { d.delta };
        self.lookahead_hops = self.lookahead_hops.min(MAX_LOOKAHEAD_HOPS);
        self.kick_refractory_ms = if self.kick_refractory_ms.is_finite() { self.kick_refractory_ms.clamp(30.0, 1_000.0) } else { d.kick_refractory_ms };
        self
    }
}

/// What the engine and the UI get: counters (like `beat`), the strength
/// and the audio time (studio clock, s) of each kind's last onset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Onsets {
    /// Any onset, whole spectrum.
    pub onset: u64,
    pub kick: u64,
    pub snare: u64,
    pub hat: u64,
    pub onset_strength: f32,
    pub kick_strength: f32,
    pub snare_strength: f32,
    pub hat_strength: f32,
    /// Audio time of the ODF peak (the end of that hop), before the
    /// look-ahead: T-246 subtracts the analysis delay from the event time.
    pub last_onset_t: f64,
    pub last_kick_t: f64,
    pub last_snare_t: f64,
    pub last_hat_t: f64,
}

/// A range of FFT bins, `lo..hi`.
#[derive(Clone, Copy, Debug)]
struct Bins {
    lo: usize,
    hi: usize,
}

impl Bins {
    /// The bins whose centre is in `lo_hz..=hi_hz` (at least one while the
    /// band is under Nyquist; DC is never included).
    fn new(lo_hz: f32, hi_hz: f32, hz_per_bin: f32, bins: usize) -> Self {
        let lo = ((lo_hz / hz_per_bin).ceil() as usize).max(1);
        let hi = ((hi_hz / hz_per_bin).floor() as usize).saturating_add(1).min(bins);
        if lo >= bins {
            Self { lo: bins, hi: bins }
        } else {
            Self { lo, hi: hi.max(lo + 1) }
        }
    }

    fn contains(&self, k: usize) -> bool {
        (self.lo..self.hi).contains(&k)
    }

    fn len(&self) -> usize {
        self.hi - self.lo
    }
}

/// Causal peak picking on one onset detection function.
struct Picker {
    /// The last values, by hop number modulo the length.
    values: Vec<f32>,
    pre_max: u64,
    pre_avg: u64,
    last: Option<u64>,
    peak_ref: f32,
}

impl Picker {
    fn new(hop_s: f32) -> Self {
        let pre_max = hops(PRE_MAX_S, hop_s);
        let pre_avg = hops(PRE_AVG_S, hop_s).max(pre_max);
        Self { values: vec![0.0; (pre_avg + MAX_LOOKAHEAD_HOPS as u64 + 1) as usize], pre_max, pre_avg, last: None, peak_ref: MIN_PEAK_REF }
    }

    fn at(&self, hop: u64) -> f32 {
        self.values[(hop % self.values.len() as u64) as usize]
    }

    /// Records hop `n`'s value; returns the candidate peak confirmed at
    /// hop `n − lookahead` (hop, value), if any. The refractory period
    /// only starts once a candidate is `accept`ed: a peak the rules turn
    /// down (low-band noise before a kick) must not hide the next one.
    fn push(&mut self, n: u64, v: f32, delta: f32, lookahead: u64, refractory: u64, decay: f32) -> Option<(u64, f32)> {
        let len = self.values.len() as u64;
        self.values[(n % len) as usize] = v;
        self.peak_ref = (self.peak_ref * decay).max(MIN_PEAK_REF);
        let c = n.checked_sub(lookahead)?;
        let vc = self.at(c);
        if (c + 1..=n).any(|j| self.at(j) > vc) {
            return None;
        }
        let (mut max_before, mut sum, mut count) = (0.0f32, 0.0f32, 0u32);
        for j in 1..=self.pre_avg.min(c) {
            let x = self.at(c - j);
            if j <= self.pre_max {
                max_before = max_before.max(x);
            }
            sum += x;
            count += 1;
        }
        let mean = if count > 0 { sum / count as f32 } else { 0.0 };
        if vc <= max_before || vc < mean + delta || self.last.is_some_and(|l| c - l < refractory) {
            return None;
        }
        Some((c, vc))
    }

    /// Counts the candidate `(c, value)` as an onset; returns its strength.
    fn accept(&mut self, c: u64, v: f32) -> f32 {
        self.last = Some(c);
        self.peak_ref = self.peak_ref.max(v);
        (v / self.peak_ref).clamp(0.0, 1.0)
    }
}

fn db_ratio(a: f32, b: f32) -> f32 {
    10.0 * ((a + 1e-12) / (b + 1e-12)).log10()
}

fn hops(seconds: f32, hop_s: f32) -> u64 {
    (seconds / hop_s).round().max(1.0) as u64
}

/// What the rules need of each hop.
#[derive(Clone, Copy, Debug, Default)]
struct HopInfo {
    power: [f32; POWER_COUNT],
    spread: f32,
}

pub struct OnsetDetector {
    config: OnsetConfig,
    hop_s: f32,
    bins: usize,
    odf_bins: [Bins; ODF_COUNT],
    power_bins: [Bins; POWER_COUNT],
    spread_bins: Bins,
    /// Log-compressed magnitudes of the last `LAG + 1` frames, one after
    /// the other.
    logs: Vec<f32>,
    pickers: [Picker; ODF_COUNT],
    history: [HopInfo; HISTORY],
    /// The whole-spectrum ODF of the last 8 s, by hop number modulo the
    /// length (for the tempo estimation, T-233).
    odf: Vec<f32>,
    /// This hop's flux: whole spectrum, low, mid, high.
    flux: [f32; ODF_COUNT],
    /// Hops processed.
    n: u64,
    out: Onsets,
}

impl OnsetDetector {
    /// `carried`: the counters of a previous stream (a reopened input must
    /// not look like new onsets, nor restart them from 0).
    pub fn new(sample_rate: u32, carried: Onsets, config: OnsetConfig) -> Self {
        let rate = sample_rate.max(1) as f32;
        let hop_s = HOP as f32 / rate;
        let hz_per_bin = rate / FFT_SIZE as f32;
        let bins = FFT_SIZE / 2 + 1;
        let odf_len = (8.0 / hop_s).ceil() as usize;
        Self {
            config: config.sanitized(),
            hop_s,
            bins,
            odf_bins: ODF_BANDS_HZ.map(|(lo, hi)| Bins::new(lo, hi, hz_per_bin, bins)),
            power_bins: POWER_BANDS_HZ.map(|(lo, hi)| Bins::new(lo, hi, hz_per_bin, bins)),
            spread_bins: Bins::new(SPREAD_BAND_HZ.0, SPREAD_BAND_HZ.1, hz_per_bin, bins),
            logs: vec![0.0; (LAG + 1) * bins],
            pickers: std::array::from_fn(|_| Picker::new(hop_s)),
            history: [HopInfo::default(); HISTORY],
            odf: vec![0.0; odf_len.max(1)],
            flux: [0.0; ODF_COUNT],
            n: 0,
            out: Onsets { onset_strength: 0.0, kick_strength: 0.0, snare_strength: 0.0, hat_strength: 0.0, ..carried },
        }
    }

    pub fn set_config(&mut self, config: OnsetConfig) {
        self.config = config.sanitized();
    }

    pub fn onsets(&self) -> Onsets {
        self.out
    }

    /// The last hop's flux in the low (40–150 Hz), mid (150 Hz–5 kHz) and
    /// high (5–15 kHz) bands, each a mean over its bins (T-233's tempo ODF).
    pub fn band_flux(&self) -> [f32; 3] {
        [self.flux[ODF_LOW], self.flux[ODF_MID], self.flux[ODF_HIGH]]
    }

    /// Hops per second of the ODF.
    pub fn odf_rate(&self) -> f32 {
        1.0 / self.hop_s
    }

    /// The whole-spectrum ODF of the last 8 s (or less since the start),
    /// oldest first, as two slices. (The tempo, T-233, reads the band
    /// fluxes instead; kept for an ODF display.)
    #[allow(dead_code)]
    pub fn odf_history(&self) -> (&[f32], &[f32]) {
        let len = self.odf.len();
        if (self.n as usize) < len {
            (&self.odf[..self.n as usize], &[])
        } else {
            let pos = (self.n % len as u64) as usize;
            (&self.odf[pos..], &self.odf[..pos])
        }
    }

    /// One hop: `power` is the hop's power spectrum (`FFT_SIZE / 2 + 1`
    /// bins), `t` the audio time of its end. No onset is reported while
    /// `silent`.
    pub fn process(&mut self, power: &[f32], t: f64, silent: bool) -> Onsets {
        let n = self.n;
        let bins = self.bins.min(power.len());
        let slot = (n % (LAG as u64 + 1)) as usize;
        let reference = ((n + 1) % (LAG as u64 + 1)) as usize; // n − LAG
        // |X| of a full-scale sine under a Hann window is N/4.
        let scale = 4.0 / FFT_SIZE as f32;

        // Band powers (for the rules) and this frame's log magnitudes.
        let mut info = HopInfo::default();
        let mut spread_max = 0.0f32;
        {
            let cur = &mut self.logs[slot * self.bins..(slot + 1) * self.bins];
            for (k, (l, &p)) in cur.iter_mut().zip(power).enumerate() {
                let p = if p.is_finite() { p.max(0.0) * scale * scale } else { 0.0 };
                *l = (1.0 + GAMMA * p.sqrt()).log10();
                for (sum, b) in info.power.iter_mut().zip(&self.power_bins) {
                    if b.contains(k) {
                        *sum += p;
                    }
                }
                if self.spread_bins.contains(k) {
                    spread_max = spread_max.max(p);
                }
            }
        }

        // SuperFlux per band, and the snare's spread.
        let cur = &self.logs[slot * self.bins..(slot + 1) * self.bins];
        let prev = &self.logs[reference * self.bins..(reference + 1) * self.bins];
        let mut flux = [0.0f32; ODF_COUNT];
        let mut spread = 0usize;
        for k in 1..bins {
            let from = k.saturating_sub(MAX_FILTER_RADIUS);
            let to = (k + MAX_FILTER_RADIUS + 1).min(bins);
            let r = prev[from..to].iter().fold(0.0f32, |m, &x| m.max(x));
            let d = (cur[k] - r).max(0.0);
            for (f, b) in flux.iter_mut().zip(&self.odf_bins) {
                if b.contains(k) {
                    *f += d;
                }
            }
            if self.spread_bins.contains(k) && d > SPREAD_RISE {
                let p = power[k].max(0.0) * scale * scale;
                if p >= spread_max * SPREAD_FLOOR {
                    spread += 1;
                }
            }
        }
        for (f, b) in flux.iter_mut().zip(&self.odf_bins) {
            *f = if b.len() > 0 { *f / b.len() as f32 } else { 0.0 };
        }
        info.spread = if self.spread_bins.len() > 0 { spread as f32 / self.spread_bins.len() as f32 } else { 0.0 };
        self.history[(n % HISTORY as u64) as usize] = info;
        self.flux = flux;
        let odf_len = self.odf.len() as u64;
        self.odf[(n % odf_len) as usize] = flux[ODF_FULL];

        // Peak picking and the rules.
        let lookahead = self.config.lookahead_hops.min(MAX_LOOKAHEAD_HOPS) as u64;
        let spacing = hops(MIN_SPACING_S, self.hop_s);
        let kick_spacing = hops(self.config.kick_refractory_ms / 1_000.0, self.hop_s);
        let decay = (-self.hop_s / STRENGTH_RELEASE_S).exp();
        let delta = self.config.delta as f32;
        let at = |c: u64| t - (n - c) as f64 * self.hop_s as f64;
        let mut fired = [None; ODF_COUNT];
        for (i, (picker, &v)) in self.pickers.iter_mut().zip(&flux).enumerate() {
            let refractory = if i == ODF_LOW { kick_spacing } else { spacing };
            fired[i] = picker.push(n, v, delta, lookahead, refractory, decay);
        }
        if !silent {
            let kinds = [
                fired[ODF_FULL],
                fired[ODF_LOW].filter(|&(c, _)| self.is_kick(c, n)),
                fired[ODF_MID].filter(|&(c, _)| self.is_snare(c, n)),
                fired[ODF_HIGH].filter(|&(c, _)| self.is_hat(c, n)),
            ];
            for (i, kind) in kinds.into_iter().enumerate() {
                let Some((c, v)) = kind else { continue };
                let s = self.pickers[i].accept(c, v);
                let o = &mut self.out;
                let (count, strength, time) = match i {
                    ODF_FULL => (&mut o.onset, &mut o.onset_strength, &mut o.last_onset_t),
                    ODF_LOW => (&mut o.kick, &mut o.kick_strength, &mut o.last_kick_t),
                    ODF_MID => (&mut o.snare, &mut o.snare_strength, &mut o.last_snare_t),
                    _ => (&mut o.hat, &mut o.hat_strength, &mut o.last_hat_t),
                };
                *count += 1;
                (*strength, *time) = (s, at(c));
            }
        }
        self.n += 1;
        self.out
    }

    fn info(&self, hop: u64) -> &HopInfo {
        &self.history[(hop % HISTORY as u64) as usize]
    }

    /// The power band `b` gained with the onset at hop `c` (confirmed at
    /// `n`): (new power, rise in dB).
    fn rise(&self, b: usize, c: u64, n: u64) -> (f32, f32) {
        let before = (c.saturating_sub(BEFORE_HOPS)..c).map(|j| self.info(j).power[b]).fold(f32::INFINITY, f32::min);
        let before = if before.is_finite() { before } else { 0.0 };
        let after = self.after(b, c, n);
        ((after - before).max(0.0), db_ratio(after, before))
    }

    /// Band `b`'s highest power from the peak hop `c` to `n`.
    fn after(&self, b: usize, c: u64, n: u64) -> f32 {
        (c..=n).map(|j| self.info(j).power[b]).fold(0.0f32, f32::max)
    }

    fn is_kick(&self, c: u64, n: u64) -> bool {
        let (low, _) = self.rise(P_LOW, c, n);
        let (above, _) = self.rise(P_ABOVE_LOW, c, n);
        let from = c.saturating_sub(KICK_BEFORE_HOPS);
        let mean_before = (from..c).map(|j| self.info(j).power[P_LOW]).sum::<f32>() / (c - from).max(1) as f32;
        let jump_db = db_ratio(self.after(P_LOW, c, n), mean_before);
        jump_db >= KICK_RISE_DB && low >= KICK_DOMINANCE * above && low >= KICK_SHARE * self.after(P_ABOVE_LOW, c, n)
    }

    /// New energy per bin in 5–15 kHz ≥ HAT_RATIO × that in 500 Hz–2 kHz.
    fn hat_like(&self, c: u64, n: u64) -> Option<bool> {
        let (high, high_db) = self.rise(P_HIGH, c, n);
        if high_db < MIN_RISE_DB || self.power_bins[P_HIGH].len() == 0 {
            return None;
        }
        let (lowmid, _) = self.rise(P_LOWMID, c, n);
        let per_bin = |p: f32, b: usize| p / self.power_bins[b].len().max(1) as f32;
        Some(per_bin(high, P_HIGH) >= HAT_RATIO * per_bin(lowmid, P_LOWMID))
    }

    fn is_snare(&self, c: u64, n: u64) -> bool {
        let spread = (c..=n).map(|j| self.info(j).spread).fold(0.0f32, f32::max);
        let (crack, _) = self.rise(P_CRACK, c, n);
        let (low, _) = self.rise(P_LOW, c, n);
        spread >= SNARE_SPREAD && crack >= SNARE_SHARE * low && self.hat_like(c, n) != Some(true)
    }

    fn is_hat(&self, c: u64, n: u64) -> bool {
        self.hat_like(c, n) == Some(true)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::analysis::{Analyzer, HOP};
    use super::super::spectrum::AnalysisConfig;
    use super::*;
    use std::f32::consts::PI;

    const RATE: u32 = 48_000;

    /// Deterministic white noise in -1..1, a pure function of the sample
    /// index (so every generator is stateless).
    pub(crate) fn noise(i: usize, seed: u64) -> f32 {
        let mut z = (i as u64).wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) enum Hit {
        /// A sine gliding 150 → 50 Hz.
        Kick,
        /// Noise + 200 Hz.
        Snare,
        /// High-passed noise.
        Hat,
        /// One-sample click.
        Click,
    }

    /// `hit` sounding `dt` seconds after its start, at sample `i`.
    fn voice(hit: Hit, dt: f32, i: usize) -> f32 {
        match hit {
            Hit::Kick if dt < 0.35 => {
                let tau = 0.03;
                let phase = 2.0 * PI * (50.0 * dt + 100.0 * tau * (1.0 - (-dt / tau).exp()));
                0.8 * (-dt / 0.15).exp() * phase.sin()
            }
            Hit::Snare if dt < 0.25 => 0.3 * (-dt / 0.05).exp() * noise(i, 1) + 0.3 * (-dt / 0.08).exp() * (2.0 * PI * 200.0 * dt).sin(),
            Hit::Hat if dt < 0.1 => {
                // Second difference of white noise: +12 dB/octave, most of it above 8 kHz.
                let hp = (noise(i, 2) - 2.0 * noise(i.wrapping_sub(1), 2) + noise(i.wrapping_sub(2), 2)) / 4.0;
                0.5 * (-dt / 0.03).exp() * hp
            }
            Hit::Click if dt == 0.0 => 0.9,
            _ => 0.0,
        }
    }

    /// Renders `seconds` of `hits` (start time, kind) over `bed(i)`.
    fn render(seconds: f32, hits: &[(f64, Hit)], bed: impl Fn(usize) -> f32) -> Vec<f32> {
        render_at(RATE, seconds, hits, bed)
    }

    pub(crate) fn render_at(rate: u32, seconds: f32, hits: &[(f64, Hit)], bed: impl Fn(usize) -> f32) -> Vec<f32> {
        let n = (seconds * rate as f32) as usize;
        let mut out: Vec<f32> = (0..n).map(&bed).collect();
        for &(t0, hit) in hits {
            let start = (t0 * rate as f64).round() as usize;
            for (i, x) in out.iter_mut().enumerate().skip(start) {
                let dt = (i - start) as f32 / rate as f32;
                if dt > 0.4 {
                    break;
                }
                *x += voice(hit, dt, i);
            }
        }
        out
    }

    /// When each counter moved (the end of the hop that reported it, s).
    #[derive(Debug, Default)]
    struct Events {
        onset: Vec<f64>,
        kick: Vec<f64>,
        snare: Vec<f64>,
        hat: Vec<f64>,
        /// The reported audio times of the kicks' ODF peaks.
        kick_t: Vec<f64>,
    }

    fn detect_with(signal: &[f32], rate: u32, config: AnalysisConfig) -> Events {
        let mut a = Analyzer::with_config(rate, Onsets::default(), config);
        let mut prev = Onsets::default();
        let mut e = Events::default();
        for (h, hop) in signal.as_chunks::<HOP>().0.iter().enumerate() {
            let t = ((h + 1) * HOP) as f64 / rate as f64;
            let o = a.process(hop, t).onsets;
            let note = |now: u64, before: u64, v: &mut Vec<f64>| {
                if now > before {
                    assert_eq!(now, before + 1, "one event per hop at most");
                    v.push(t);
                }
            };
            note(o.onset, prev.onset, &mut e.onset);
            note(o.kick, prev.kick, &mut e.kick);
            note(o.snare, prev.snare, &mut e.snare);
            note(o.hat, prev.hat, &mut e.hat);
            if o.kick > prev.kick {
                e.kick_t.push(o.last_kick_t);
            }
            prev = o;
        }
        e
    }

    fn detect(signal: &[f32]) -> Events {
        detect_with(signal, RATE, AnalysisConfig::default())
    }

    /// F-measure of `found` against `truth`, one-to-one within ±`tol` s.
    fn f_measure(truth: &[f64], found: &[f64], tol: f64) -> f32 {
        let mut used = vec![false; found.len()];
        let mut hits = 0;
        for &t in truth {
            if let Some(j) = (0..found.len()).filter(|&j| !used[j] && (found[j] - t).abs() <= tol).min_by(|&a, &b| (found[a] - t).abs().total_cmp(&(found[b] - t).abs())) {
                used[j] = true;
                hits += 1;
            }
        }
        if truth.is_empty() && found.is_empty() {
            return 1.0;
        }
        2.0 * hits as f32 / (truth.len() + found.len()) as f32
    }

    /// Four-on-the-floor at 128 BPM: kick on every beat, snare on 2 and 4,
    /// hat on every eighth, over a −60 dBFS noise bed. `offset` shifts it
    /// off the hop grid.
    fn pattern(bars: usize, offset: f64) -> (Vec<f32>, Vec<(f64, Hit)>) {
        let beat = 60.0 / 128.0;
        let mut hits = Vec::new();
        for b in 0..bars * 4 {
            let t = offset + b as f64 * beat;
            hits.push((t, Hit::Kick));
            if b % 2 == 1 {
                hits.push((t, Hit::Snare));
            }
            hits.push((t, Hit::Hat));
            hits.push((t + beat / 2.0, Hit::Hat));
        }
        let seconds = (offset + (bars * 4) as f64 * beat + 0.5) as f32;
        (render(seconds, &hits, |i| 0.001 * noise(i, 7)), hits)
    }

    fn times(hits: &[(f64, Hit)], kind: Hit) -> Vec<f64> {
        hits.iter().filter(|h| h.1 == kind).map(|h| h.0).collect()
    }

    const BEAT: f64 = 60.0 / 128.0;

    /// Start times off the hop grid (a hop is 5.33 ms).
    const OFFSETS: [f64; 3] = [0.25, 0.2513, 0.2527];

    /// Acceptance: 4-on-the-floor + snare on 2 and 4 + eighth hats at
    /// 128 BPM, ±50 ms: kick F ≥ 0.95, snare ≥ 0.85, hat ≥ 0.8.
    #[test]
    fn the_128_bpm_pattern_is_classified() {
        for offset in OFFSETS {
            let (signal, hits) = pattern(8, offset);
            let e = detect(&signal);
            let f = |k, v: &[f64]| f_measure(&times(&hits, k), v, 0.05);
            let (kick, snare, hat) = (f(Hit::Kick, &e.kick), f(Hit::Snare, &e.snare), f(Hit::Hat, &e.hat));
            assert!(kick >= 0.95, "offset {offset}: kick F {kick} ({} found)", e.kick.len());
            assert!(snare >= 0.85, "offset {offset}: snare F {snare} ({} found)", e.snare.len());
            assert!(hat >= 0.8, "offset {offset}: hat F {hat} ({} found)", e.hat.len());
            // One onset (whole spectrum) per eighth note, no more.
            assert!(f_measure(&times(&hits, Hit::Hat), &e.onset, 0.05) >= 0.95, "{} onsets", e.onset.len());
        }
    }

    #[test]
    fn the_pattern_reads_the_same_30_db_quieter() {
        let (signal, hits) = pattern(8, 0.25);
        let quiet: Vec<f32> = signal.iter().map(|x| x * 0.0316).collect();
        let e = detect(&quiet);
        let f = |k, v: &[f64]| f_measure(&times(&hits, k), v, 0.05);
        assert!(f(Hit::Kick, &e.kick) >= 0.95, "{:?}", e.kick.len());
        assert!(f(Hit::Snare, &e.snare) >= 0.85, "{:?}", e.snare.len());
        assert!(f(Hit::Hat, &e.hat) >= 0.8, "{:?}", e.hat.len());
    }

    /// Acceptance: ≤ 25 ms between the transient and the event, counted in
    /// samples (the end of the hop that reports it).
    #[test]
    fn kicks_and_clicks_are_reported_within_25_ms() {
        for offset in OFFSETS {
            let hits: Vec<(f64, Hit)> = (0..16).map(|b| (offset + b as f64 * BEAT, Hit::Kick)).collect();
            let signal = render(8.0, &hits, |i| 0.001 * noise(i, 3));
            let e = detect(&signal);
            assert_eq!(e.kick.len(), 16, "offset {offset}: {:?}", e.kick);
            for ((&(truth, _), &found), &peak) in hits.iter().zip(&e.kick).zip(&e.kick_t) {
                let latency = found - truth;
                assert!((0.0..=0.025).contains(&latency), "kick at {truth}: reported {:.1} ms later", latency * 1e3);
                assert!((peak - truth).abs() <= 0.015 && peak <= found, "ODF peak time {peak} for {truth}");
            }
            let clicks: Vec<(f64, Hit)> = [0.3, 0.55, 0.61, 0.9, 1.4, 1.44, 2.0, 2.7, 3.05, 3.9].iter().map(|&t| (t + offset, Hit::Click)).collect();
            let e = detect(&render(4.5, &clicks, |_| 0.0));
            assert_eq!(e.onset.len(), clicks.len(), "offset {offset}: {:?}", e.onset);
            for (&(truth, _), &found) in clicks.iter().zip(&e.onset) {
                assert!((0.0..=0.025).contains(&(found - truth)), "click at {truth}: {found}");
            }
            // A click is flat and broadband: it may read as a snare, never as a kick.
            assert!(e.kick.is_empty() && e.hat.is_empty(), "{e:?}");
        }
    }

    #[test]
    fn no_lookahead_is_faster_by_a_hop() {
        let hits: Vec<(f64, Hit)> = (0..8).map(|b| (0.25 + b as f64 * BEAT, Hit::Kick)).collect();
        let signal = render(4.0, &hits, |i| 0.001 * noise(i, 3));
        let mean = |e: &Events| e.kick.iter().zip(&hits).map(|(f, h)| f - h.0).sum::<f64>() / e.kick.len() as f64;
        let one = detect(&signal);
        let zero = detect_with(&signal, RATE, AnalysisConfig { onsets: OnsetConfig { lookahead_hops: 0, ..Default::default() }, ..Default::default() });
        assert_eq!((one.kick.len(), zero.kick.len()), (8, 8));
        let hop = HOP as f64 / RATE as f64;
        assert!(mean(&zero) <= mean(&one) - hop + 1e-6, "{} vs {}", mean(&one), mean(&zero));
        assert!(mean(&zero) >= 0.0);
    }

    /// Acceptance: a held bass line without a kick is not a stream of kicks
    /// (the legacy `beat` fired on every note of it).
    #[test]
    fn a_bass_line_is_not_a_kick() {
        let notes = [55.0, 73.4, 82.4, 65.4, 41.2, 98.0, 61.7, 110.0];
        let note_len = (BEAT / 2.0 * RATE as f64) as usize;
        // Legato: one continuous phase, the pitch changes every eighth.
        let mut phase = 0.0f64;
        let legato: Vec<f32> = (0..10 * RATE as usize)
            .map(|i| {
                phase += notes[(i / note_len) % notes.len()] / RATE as f64;
                0.5 * (2.0 * std::f64::consts::PI * phase).sin() as f32
            })
            .collect();
        let e = detect(&legato);
        assert!(e.kick.len() <= 1, "only the very first note may count: {:?}", e.kick);
        // With hats and snares on top, the kicks stay absent.
        let hits: Vec<(f64, Hit)> = (0..20).flat_map(|b| [(0.3 + b as f64 * BEAT, Hit::Hat), (0.3 + (b as f64 + 0.5) * BEAT, if b % 2 == 1 { Hit::Snare } else { Hit::Hat })]).collect();
        let over = render(10.0, &hits, |i| legato[i]);
        let e = detect(&over);
        assert!(e.kick.len() <= 1, "{:?}", e.kick);
        assert!(e.hat.len() >= 20, "the hats are still heard: {}", e.hat.len());
    }

    #[test]
    fn a_kick_over_a_bass_line_is_still_a_kick() {
        let bass = |i: usize| 0.2 * (2.0 * PI * 55.0 * i as f32 / RATE as f32).sin();
        let hits: Vec<(f64, Hit)> = (0..16).map(|b| (0.3 + b as f64 * BEAT, Hit::Kick)).collect();
        let e = detect(&render(8.0, &hits, bass));
        assert!(f_measure(&times(&hits, Hit::Kick), &e.kick, 0.05) >= 0.95, "{:?}", e.kick);
    }

    #[test]
    fn each_drum_alone_is_only_itself() {
        for (kind, name) in [(Hit::Kick, "kick"), (Hit::Snare, "snare"), (Hit::Hat, "hat")] {
            let hits: Vec<(f64, Hit)> = (0..8).map(|b| (0.3 + b as f64 * BEAT, kind)).collect();
            let e = detect(&render(4.5, &hits, |i| 0.001 * noise(i, 5)));
            let counts = (e.kick.len(), e.snare.len(), e.hat.len());
            let want = match kind {
                Hit::Kick => (8, 0, 0),
                Hit::Snare => (0, 8, 0),
                _ => (0, 0, 8),
            };
            assert_eq!(counts, want, "{name} alone: (kick, snare, hat)");
            assert_eq!(e.onset.len(), 8, "{name}: one onset per hit");
        }
    }

    /// False positives: stationary noise, a steady chord, digital silence.
    #[test]
    fn noise_tones_and_silence_give_no_onsets() {
        let white = |i: usize| 0.1 * noise(i, 11);
        // Brown-ish noise: most of its energy in the low band.
        let brown: Vec<f32> = {
            let mut y = 0.0f32;
            (0..10 * RATE as usize)
                .map(|i| {
                    y = 0.995 * y + 0.05 * noise(i, 12);
                    y
                })
                .collect()
        };
        let chord = |i: usize| {
            let t = i as f32 / RATE as f32;
            [65.4f32, 130.8, 196.0, 261.6, 329.6, 523.2].iter().map(|f| 0.08 * (2.0 * PI * f * t).sin()).sum::<f32>()
        };
        let signals: [(&str, Vec<f32>); 4] = [
            ("white", (0..10 * RATE as usize).map(white).collect()),
            ("brown", brown),
            ("chord", (0..10 * RATE as usize).map(chord).collect()),
            ("silence", vec![0.0; 10 * RATE as usize]),
        ];
        for (name, s) in signals {
            let e = detect(&s);
            // The start of the signal (out of digital silence) may count once.
            let late = |v: &[f64]| v.iter().filter(|&&t| t > 0.3).count();
            // A loud rumble (brown noise, almost all under 60 Hz) makes the
            // three low bins flicker: a few false kicks are its known
            // budget (≤ 0.3/s, to be checked on the T-244 corpus).
            let kick_budget = if name == "brown" { 3 } else { 0 };
            assert!(late(&e.kick) <= kick_budget, "{name}: {} kicks: {e:?}", late(&e.kick));
            assert_eq!((late(&e.snare), late(&e.hat)), (0, 0), "{name}: {e:?}");
            assert!(late(&e.onset) <= 1, "{name}: {} onsets in 10 s", late(&e.onset));
            if name == "silence" {
                assert!(e.onset.is_empty() && e.kick.is_empty());
            }
        }
    }

    #[test]
    fn the_kick_spacing_is_configurable() {
        // Kicks 250 ms apart, then a lone one.
        let hits = [(0.3, Hit::Kick), (0.55, Hit::Kick), (1.2, Hit::Kick)];
        let signal = render(1.7, &hits, |i| 0.001 * noise(i, 4));
        assert_eq!(detect(&signal).kick.len(), 3, "100 ms by default");
        let long = AnalysisConfig { onsets: OnsetConfig { kick_refractory_ms: 300.0, ..Default::default() }, ..Default::default() };
        assert_eq!(detect_with(&signal, RATE, long).kick.len(), 2);
    }

    #[test]
    fn a_higher_delta_means_fewer_onsets() {
        let (signal, _) = pattern(4, 0.25);
        let quiet_hats: Vec<f32> = signal.iter().map(|x| x * 0.5).collect();
        let n = |delta| detect_with(&quiet_hats, RATE, AnalysisConfig { onsets: OnsetConfig { delta, ..Default::default() }, ..Default::default() }).onset.len();
        let (sensitive, deaf) = (n(0.05), n(1.5));
        assert!(sensitive >= 32 && deaf < sensitive, "{sensitive} vs {deaf}");
    }

    #[test]
    fn other_sample_rates_work() {
        for rate in [16_000u32, 44_100, 96_000] {
            let hits: Vec<(f64, Hit)> = (0..8).map(|b| (0.3 + b as f64 * BEAT, Hit::Kick)).collect();
            let signal = render_at(rate, 4.5, &hits, |i| 0.001 * noise(i, 6));
            let e = detect_with(&signal, rate, AnalysisConfig::default());
            assert_eq!(e.kick.len(), 8, "{rate} Hz: {:?}", e.kick);
            for (&(truth, _), &found) in hits.iter().zip(&e.kick) {
                // 25 ms; at 16 kHz a hop is 16 ms and the window 64 ms: 65 ms.
                let limit = if rate < 44_100 { 0.065 } else { 0.025 };
                assert!((0.0..=limit).contains(&(found - truth)), "{rate} Hz: {truth} → {found}");
            }
        }
    }

    #[test]
    fn counters_carry_over_and_strengths_are_bounded() {
        let carried = Onsets { onset: 100, kick: 40, snare: 20, hat: 70, kick_strength: 0.9, ..Default::default() };
        let mut a = Analyzer::with_config(RATE, carried, AnalysisConfig::default());
        assert_eq!(a.onsets().kick, 40);
        assert_eq!(a.onsets().kick_strength, 0.0, "strengths are not carried");
        let (signal, _) = pattern(2, 0.25);
        let mut last = Onsets::default();
        for (h, hop) in signal.as_chunks::<HOP>().0.iter().enumerate() {
            last = a.process(hop, ((h + 1) * HOP) as f64 / RATE as f64).onsets;
            for s in [last.onset_strength, last.kick_strength, last.snare_strength, last.hat_strength] {
                assert!((0.0..=1.0).contains(&s), "{last:?}");
            }
        }
        assert_eq!((last.kick, last.snare), (48, 24), "{last:?}");
        assert!(last.onset > 100 + 8 && last.hat > 70);
        assert!(last.kick_strength > 0.5 && last.last_kick_t > 3.0, "{last:?}");
    }

    #[test]
    fn the_odf_history_keeps_8_seconds() {
        let mut d = OnsetDetector::new(RATE, Onsets::default(), OnsetConfig::default());
        let power = vec![0.0f32; FFT_SIZE / 2 + 1];
        for h in 0..10 {
            d.process(&power, h as f64, false);
        }
        let (a, b) = d.odf_history();
        assert_eq!((a.len(), b.len()), (10, 0));
        for h in 10..2_000 {
            d.process(&power, h as f64, false);
        }
        let (a, b) = d.odf_history();
        assert_eq!(a.len() + b.len(), (8.0 * d.odf_rate()).ceil() as usize);
        assert!((d.odf_rate() - 187.5).abs() < 1e-3);
    }

    #[test]
    fn the_settings_are_sanitised() {
        let c = OnsetConfig { delta: f64::NAN, lookahead_hops: 9, kick_refractory_ms: 1.0 }.sanitized();
        assert_eq!(c, OnsetConfig { delta: 0.1, lookahead_hops: 2, kick_refractory_ms: 30.0 });
        let c: OnsetConfig = serde_json::from_str(r#"{"delta": 0.3}"#).unwrap();
        assert_eq!(c, OnsetConfig { delta: 0.3, ..Default::default() });
    }

    /// Opt-in timing: `cargo test -p laser-studio --release -- --ignored onset_cost`.
    #[test]
    #[ignore]
    fn onset_cost_is_small() {
        let (signal, _) = pattern(16, 0.25);
        let mut d = OnsetDetector::new(RATE, Onsets::default(), OnsetConfig::default());
        let mut power = vec![0.0f32; FFT_SIZE / 2 + 1];
        let start = std::time::Instant::now();
        for (h, hop) in signal.as_chunks::<HOP>().0.iter().enumerate() {
            // Any changing spectrum will do for the timing.
            for (k, p) in power.iter_mut().enumerate() {
                *p = hop[k % HOP] * hop[k % HOP] * 1e4;
            }
            std::hint::black_box(d.process(&power, h as f64, false));
        }
        let share = start.elapsed().as_secs_f32() / (signal.len() as f32 / RATE as f32);
        eprintln!("onsets: {:.3} % of a core", share * 100.0);
        assert!(share < 0.01);
    }

    #[test]
    fn a_hop_does_not_allocate() {
        let mut a = Analyzer::new(RATE);
        let (signal, _) = pattern(1, 0.25);
        let hops: Vec<&[f32; HOP]> = signal.as_chunks::<HOP>().0.iter().collect();
        a.process(hops[0], 0.0);
        let n = crate::audio::capture::tests::allocations_during(|| {
            for (h, hop) in hops.iter().enumerate().skip(1) {
                std::hint::black_box(a.process(&hop[..], h as f64 * 0.005));
            }
        });
        assert_eq!(n, 0, "buffers are allocated once, in new()");
        assert!(a.onsets().kick >= 3, "the loop did detect: {:?}", a.onsets());
    }
}
