//! Tempo (BPM) estimation and beat tracking (T-233), on the analysis
//! thread, from the whole-spectrum onset detection function (ODF) of
//! `onsets.rs`. Written from the papers (Scheirer 1998; Ellis 2007;
//! Davies & Plumbley 2007; Stark, Davies & Plumbley 2009; Percival &
//! Tzanetakis 2014), no GPL source read.
//!
//! - **ODF at ~100 Hz**: the hop ODF (187.5 Hz at 48 kHz) is decimated by
//!   the whole number of hops closest to 10 ms (their mean: a peak's place
//!   between two frames survives in their ratio), and the last 8 s are kept.
//!   The ODF is the sum of the low, mid and high band fluxes rather than the
//!   whole-spectrum one, so kicks weigh as much as hi-hats.
//! - **Tempo**, every 0.5 s once 3 s of sound are in: the mean-removed
//!   window, weighted towards its recent end (time constant 4 s, so a
//!   tempo change wins in a couple of seconds), is autocorrelated through
//!   an FFT. Each period τ between 60/250 and 60/40 s (0.1-frame steps) is
//!   scored by a harmonic comb, `A(τ) + ½A(2τ) + ⅓A(3τ) + ¼A(4τ)`, times a
//!   log-Gaussian tempo prior centred on 125 BPM (σ = 1 octave; a *guide*
//!   tap narrows it to ±3 % around the guide). The best period is refined
//!   by parabolic interpolation.
//! - **Octave errors**: the comb only rewards a period whose multiples
//!   are pulses too, so half the tempo (τ × 2) scores lower than the
//!   tempo as soon as there is anything on the beats in between, and
//!   double the tempo (τ / 2) scores low unless the off-beats carry
//!   onsets; the prior settles what the pattern leaves open (a 70 BPM
//!   groove with off-beat hats reads 140, see the tests). Then a
//!   hysteresis: a new estimate within ±4 % of the current tempo is
//!   followed at once (a DJ nudging the pitch, 124 → 128), anything else
//!   must win for 2 s, and a ×2 / ÷2 jump for 4 s.
//! - **Confidence** 0..1 = pulse clarity (the comb's normalised
//!   autocorrelation at the chosen period) × peak-to-average of the score
//!   × *presence* (the ODF's variance over the last 2 s against the whole
//!   window: a break without drums empties it long before its old drums
//!   leave the window) × stability of the last 4 estimates.
//! - **Beats**: the causal dynamic-programming tracker of Ellis 2007 made
//!   online as in Stark et al. 2009. Every ODF frame, the cumulative score
//!   `C(n) = (1 − α)·O(n) + α·max_{d ∈ [P/2, 2P]} W(d)·C(n − d)`, with a
//!   log-Gaussian transition weight `W` around the period `P`. Half a
//!   period after each beat, the next one is predicted: the score is run
//!   one period into the future with no onsets, weighted by a Gaussian
//!   around the expected place, and its maximum is the next beat. When
//!   that frame comes, the beat is placed on the ODF peak next to it
//!   (parabolic interpolation, sub-frame) and reported (`beat_time`), and
//!   `next_beat = beat_time + 60 / bpm`. Beats keep coming through a break.
//! - **States**: *NoInput* while `silent`; *Checking* until the
//!   confidence has stayed ≥ 0.6 for 2 s (*Guided* instead while a guide
//!   is set); *Locked*; *Coasting* once a locked estimate's confidence
//!   falls under 0.4 (a break: the BPM is held and beats keep being
//!   predicted) until it is back ≥ 0.6 for 2 s. More than 3 s of silence,
//!   or *Nouveau morceau*, forgets everything but the last BPM shown.
//!
//! The estimate only *proposes*: it never touches the `TempoClock` (T-234
//! does that, with its own rules). Every buffer and FFT plan is made in
//! `new`: a hop, estimate included, allocates nothing.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use serde::Serialize;
use std::sync::Arc;

/// ODF frame rate aimed at (Hz).
const FRAME_RATE_HZ: f64 = 100.0;
/// The tempo ODF: the low, mid and high band fluxes (each a mean over its
/// bins) weighted like this, so a kick counts as much as a hi-hat (in the
/// whole-spectrum flux, the hats' hundreds of bins drown the kick's three
/// and every eighth looks like a beat).
const BAND_WEIGHTS: [f32; 3] = [1.0, 1.0, 1.0];
/// Analysis window.
const WINDOW_S: f64 = 8.0;
/// A new estimate this often...
const ESTIMATE_EVERY_S: f64 = 0.5;
/// ... once this much sound is in the window.
const MIN_AUDIO_S: f64 = 3.0;
/// The window is weighted by `exp(−age / RECENCY_S)`.
const RECENCY_S: f64 = 4.0;
/// Presence: the ODF's variance over this last stretch against the window's.
const PRESENCE_S: f64 = 2.0;

pub const MIN_BPM: f32 = 40.0;
pub const MAX_BPM: f32 = 250.0;
const PRIOR_BPM: f32 = 125.0;
const PRIOR_OCTAVES: f32 = 1.0;
/// A guide tap's prior: ±3 % (one σ).
const GUIDE_SIGMA: f32 = 0.03;
/// Harmonic comb: A(τ), A(2τ), A(3τ), A(4τ).
const COMB: [f32; 4] = [1.0, 1.0 / 2.0, 1.0 / 3.0, 1.0 / 4.0];
/// ODF smoothing before the autocorrelation.
const SMOOTH: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
/// Period grid step, in ODF frames.
const LAG_STEP: f32 = 0.1;

/// Confidence maps (each clamped to 0..1): clarity from 0.15 to 0.5,
/// peak-to-average from 2 to 6, presence from 0.1 to 0.4.
const CLARITY_MAP: (f32, f32) = (0.15, 0.5);
const PEAK_MAP: (f32, f32) = (2.0, 6.0);
const PRESENCE_MAP: (f32, f32) = (0.1, 0.4);
/// Stability: the last 4 estimates spread by ≤ 1 % → 1, ≥ 4 % → 0.
const STABILITY_N: usize = 4;
const STABILITY_MAP: (f32, f32) = (0.01, 0.04);

/// Locked once the confidence stays ≥ this for `LOCK_HOLD_S`...
pub const LOCK_CONFIDENCE: f32 = 0.6;
const LOCK_HOLD_S: f64 = 2.0;
/// ... and coasting once it falls under this.
pub const UNLOCK_CONFIDENCE: f32 = 0.4;
/// An estimate under this confidence (before stability) never moves the BPM.
const MIN_UPDATE_CONFIDENCE: f32 = 0.25;
/// Within ±4 %: followed at once; else it must win for 2 s (4 s for ×2 / ÷2).
const FOLLOW_RATIO: f32 = 0.04;
const SWITCH_S: f64 = 2.0;
const OCTAVE_SWITCH_S: f64 = 4.0;
/// A guide accepts an estimate this close to it at once.
const GUIDE_ACCEPT: f32 = 0.06;
/// Silence longer than this is a new track.
pub const NEW_TRACK_SILENCE_S: f64 = 3.0;

/// Dynamic programming (Ellis 2007, Stark 2009): weight of the past score
/// and tightness of the log-Gaussian transition window.
const ALPHA: f32 = 0.9;
const TIGHTNESS: f32 = 5.0;
/// The ODF peak lags the transient by about this share of the FFT window
/// (measured on clicks: see `the_predicted_beats_land_within_20_ms`).
const ODF_DELAY_WINDOWS: f64 = 0.42;

/// What the detector says about the audio's tempo (`/api/state.audio.tempo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectState {
    /// Silence: no estimate (the BPM shown is the last one).
    #[default]
    NoInput,
    /// Listening, not sure yet.
    Checking,
    Locked,
    /// Was locked, lost the pulse (a break): BPM held, beats predicted.
    Coasting,
    /// Checking with a guide tap as a strong prior (T-234).
    Guided,
}

/// The detector's proposal. Times are audio times on the studio clock (s).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct TempoEstimate {
    /// Detected tempo; 0 until the first estimate.
    pub bpm: f32,
    pub confidence: f32,
    /// Time of the last tracked beat (reported ~2 frames after it).
    pub beat_time: f64,
    /// Predicted time of the next one.
    pub next_beat: f64,
    pub state: DetectState,
}

fn ramp(x: f32, (lo, hi): (f32, f32)) -> f32 {
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

fn octave_distance(a: f32, b: f32) -> f32 {
    (a / b).ln().abs()
}

pub struct BpmTracker {
    /// Hops per ODF frame, and the frame period.
    decim: usize,
    frame_s: f64,
    hop_s: f64,
    odf_delay_s: f64,
    /// The frame being gathered: sums of the ODF and of the hop times, hops.
    acc: f32,
    acc_t: f64,
    acc_n: usize,
    /// Frames by number modulo `cap`: ODF, time, cumulative score.
    cap: usize,
    odf: Vec<f32>,
    times: Vec<f64>,
    cum: Vec<f32>,
    /// Frames since the last reset.
    frames: u64,
    since_estimate: u64,
    estimate_every: u64,
    min_frames: u64,
    /// Autocorrelation through the FFT.
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    buf: Vec<f32>,
    spec: Vec<Complex<f32>>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    acf: Vec<f32>,
    scores: Vec<f32>,
    tau_min: f32,
    /// Mean ODF of the window (the beat refinement's threshold).
    odf_mean: f32,
    /// The last raw estimates, for the stability.
    raw: [f32; STABILITY_N],
    raw_count: usize,
    /// The tempo followed, and a challenger (bpm, since).
    track: Option<f32>,
    pending: Option<(f32, f64)>,
    good_since: Option<f64>,
    ever_locked: bool,
    guide: Option<f32>,
    /// DP: the period (frames) and the transition weights, by distance.
    period: f32,
    weights: Vec<f32>,
    d_lo: usize,
    d_hi: usize,
    future: Vec<f32>,
    predict_at: Option<u64>,
    beat_at: Option<u64>,
    refine_at: Option<u64>,
    had_beat: bool,
    silence_s: f64,
    out: TempoEstimate,
}

impl BpmTracker {
    /// `hop_rate`: ODF values per second (one per analysis hop).
    pub fn new(hop_rate: f32, fft_size: usize) -> Self {
        let hop_rate = if hop_rate.is_finite() && hop_rate > 0.0 { hop_rate as f64 } else { 187.5 };
        let decim = (hop_rate / FRAME_RATE_HZ).round().max(1.0) as usize;
        let hop_s = 1.0 / hop_rate;
        let frame_s = decim as f64 * hop_s;
        let cap = (WINDOW_S / frame_s).ceil() as usize;
        let fft_len = (2 * cap).next_power_of_two();
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(fft_len);
        let inv = planner.plan_fft_inverse(fft_len);
        let tau_min = (60.0 / MAX_BPM as f64 / frame_s) as f32;
        let tau_max = (60.0 / MIN_BPM as f64 / frame_s) as f32;
        let grid = ((tau_max - tau_min) / LAG_STEP).ceil() as usize + 1;
        let max_period = tau_max.ceil() as usize + 1;
        Self {
            decim,
            frame_s,
            hop_s,
            odf_delay_s: ODF_DELAY_WINDOWS * fft_size as f64 * hop_s / super::analysis::HOP as f64,
            acc: 0.0,
            acc_t: 0.0,
            acc_n: 0,
            cap,
            odf: vec![0.0; cap],
            times: vec![0.0; cap],
            cum: vec![0.0; cap],
            frames: 0,
            since_estimate: 0,
            estimate_every: (ESTIMATE_EVERY_S / frame_s).round().max(1.0) as u64,
            min_frames: (MIN_AUDIO_S / frame_s).round() as u64,
            buf: fwd.make_input_vec(),
            spec: fwd.make_output_vec(),
            scratch_fwd: fwd.make_scratch_vec(),
            scratch_inv: inv.make_scratch_vec(),
            fwd,
            inv,
            acf: vec![0.0; fft_len],
            scores: vec![0.0; grid],
            tau_min,
            odf_mean: 0.0,
            raw: [0.0; STABILITY_N],
            raw_count: 0,
            track: None,
            pending: None,
            good_since: None,
            ever_locked: false,
            guide: None,
            period: 0.0,
            weights: vec![0.0; 2 * max_period + 2],
            d_lo: 1,
            d_hi: 0,
            future: vec![0.0; max_period + 2],
            predict_at: None,
            beat_at: None,
            refine_at: None,
            had_beat: false,
            silence_s: 0.0,
            out: TempoEstimate::default(),
        }
    }

    pub fn estimate(&self) -> TempoEstimate {
        self.out
    }

    /// Shows `bpm` until the first estimate (a reopened input).
    pub fn carry_bpm(&mut self, bpm: f32) {
        if self.track.is_none() && bpm.is_finite() {
            self.out.bpm = bpm.clamp(0.0, MAX_BPM);
        }
    }

    /// *Nouveau morceau*: forgets the history, the lock and the guide; the
    /// BPM shown stays until the first new estimate.
    pub fn new_track(&mut self) {
        self.frames = 0;
        self.since_estimate = 0;
        self.acc_n = 0;
        self.acc = 0.0;
        self.acc_t = 0.0;
        self.raw_count = 0;
        self.track = None;
        self.pending = None;
        self.good_since = None;
        self.ever_locked = false;
        self.guide = None;
        self.period = 0.0;
        self.predict_at = None;
        self.beat_at = None;
        self.refine_at = None;
        self.had_beat = false;
        self.out.confidence = 0.0;
        if self.out.state != DetectState::NoInput {
            self.out.state = DetectState::Checking;
        }
    }

    /// A guide tempo (T-234's *Guider*): a strong prior (±3 %) instead of
    /// the broad one, until `None`, a new track or a long silence.
    pub fn set_guide(&mut self, bpm: Option<f32>) {
        self.guide = bpm.filter(|b| b.is_finite()).map(|b| b.clamp(MIN_BPM, MAX_BPM));
        self.pending = None;
        if self.out.state == DetectState::Checking && self.guide.is_some() {
            self.out.state = DetectState::Guided;
        } else if self.out.state == DetectState::Guided && self.guide.is_none() {
            self.out.state = DetectState::Checking;
        }
    }

    /// One analysis hop: its whole-spectrum ODF value, the audio time of
    /// its end, and whether the input is silent.
    pub fn process(&mut self, band_flux: [f32; 3], t: f64, silent: bool) -> TempoEstimate {
        let odf: f32 = band_flux.iter().zip(BAND_WEIGHTS).map(|(f, w)| f * w).sum();
        if silent {
            let before = self.silence_s;
            self.silence_s += self.hop_s;
            self.out.state = DetectState::NoInput;
            self.acc_n = 0;
            self.acc = 0.0;
            self.acc_t = 0.0;
            if before < NEW_TRACK_SILENCE_S && self.silence_s >= NEW_TRACK_SILENCE_S {
                self.new_track();
            }
            return self.out;
        }
        if self.silence_s > 0.0 || self.out.state == DetectState::NoInput {
            self.silence_s = 0.0;
            self.out.state = self.listening_state();
        }
        self.acc += if odf.is_finite() { odf.max(0.0) } else { 0.0 };
        self.acc_t += t;
        self.acc_n += 1;
        if self.acc_n >= self.decim {
            let (v, ft) = (self.acc / self.acc_n as f32, self.acc_t / self.acc_n as f64);
            self.acc = 0.0;
            self.acc_t = 0.0;
            self.acc_n = 0;
            self.push_frame(v, ft);
        }
        self.out
    }

    fn listening_state(&self) -> DetectState {
        if self.ever_locked {
            DetectState::Coasting
        } else if self.guide.is_some() {
            DetectState::Guided
        } else {
            DetectState::Checking
        }
    }

    fn slot(&self, n: u64) -> usize {
        (n % self.cap as u64) as usize
    }

    fn push_frame(&mut self, v: f32, t: f64) {
        let n = self.frames;
        let i = self.slot(n);
        self.odf[i] = v;
        self.times[i] = t;
        self.cum[i] = 0.0;
        self.frames += 1;
        if self.track.is_some() {
            self.cum[i] = self.dp_step(n);
            self.beats(n);
        }
        self.since_estimate += 1;
        if self.since_estimate >= self.estimate_every && self.frames >= self.min_frames {
            self.since_estimate = 0;
            self.estimate_now(t);
        }
    }

    /// The best `W(d)·C(m − d)` over the transition window, where `C` of a
    /// frame after `known` comes from `future` (indexed from `known`).
    fn best_past(&self, m: u64, known: u64) -> f32 {
        let mut best = 0.0f32;
        let oldest = self.frames.saturating_sub(self.cap as u64);
        for d in self.d_lo..=self.d_hi {
            let Some(v) = m.checked_sub(d as u64).filter(|&v| v >= oldest) else { break };
            let c = if v > known { self.future[(v - known) as usize] } else { self.cum[self.slot(v)] };
            best = best.max(self.weights[d] * c);
        }
        best
    }

    fn dp_step(&self, n: u64) -> f32 {
        (1.0 - ALPHA) * self.odf[self.slot(n)] + ALPHA * self.best_past(n, n.saturating_sub(1))
    }

    /// Beat bookkeeping after frame `n`'s score.
    fn beats(&mut self, n: u64) {
        if self.predict_at.is_none() && self.beat_at.is_none() {
            self.predict_at = Some(n);
        }
        if self.predict_at == Some(n) {
            let first = !self.had_beat;
            self.beat_at = Some(n + self.predict(n, first));
            self.predict_at = None;
        }
        if self.beat_at == Some(n) {
            self.beat_at = None;
            self.had_beat = true;
            // Placed once the ODF frame after it is in.
            self.refine_at = Some(n + 2);
            self.predict_at = Some(n + (self.period / 2.0).round().max(1.0) as u64);
        }
        if let Some(r) = self.refine_at.filter(|&r| r == n) {
            self.refine_at = None;
            let tb = self.refine(r - 2);
            self.out.beat_time = tb;
            self.out.next_beat = tb + self.period as f64 * self.frame_s;
        }
    }

    /// Frames from `n` to the next beat: the future score (no onsets) over
    /// one period, weighted around half a period ahead (flat the first time).
    fn predict(&mut self, n: u64, first: bool) -> u64 {
        let p = self.period;
        let span = (p.ceil() as usize).clamp(1, self.future.len() - 1);
        let (mut best, mut best_k) = (f32::MIN, 1usize);
        for k in 1..=span {
            let c = ALPHA * self.best_past(n + k as u64, n);
            self.future[k] = c;
            let g = if first { 1.0 } else { (-0.5 * ((k as f32 - p / 2.0) / (p / 4.0)).powi(2)).exp() };
            if c * g > best {
                (best, best_k) = (c * g, k);
            }
        }
        best_k as u64
    }

    /// The beat at frame `b`, moved onto the ODF peak next to it when there
    /// is one (sub-frame), as an audio time of the transient.
    fn refine(&self, b: u64) -> f64 {
        let at = |j: u64| self.odf[self.slot(j)];
        let mut m = b;
        for j in [b.saturating_sub(1), b + 1] {
            if j + 1 < self.frames && j >= 1 && at(j) > at(m) {
                m = j;
            }
        }
        let mut t = self.times[self.slot(b)];
        if m >= 1 && m + 1 < self.frames && at(m) > 1.5 * self.odf_mean && at(m) >= at(m - 1) && at(m) >= at(m + 1) {
            let (a, c, e) = (at(m - 1), at(m), at(m + 1));
            let den = a - 2.0 * c + e;
            let delta = if den.abs() > 1e-9 { (0.5 * (a - e) / den).clamp(-0.5, 0.5) } else { 0.0 };
            t = self.times[self.slot(m)] + delta as f64 * self.frame_s;
        }
        t - self.odf_delay_s
    }

    /// Sets the DP period (frames) and its transition weights.
    fn set_period(&mut self, bpm: f32) {
        let p = (60.0 / (bpm as f64 * self.frame_s)) as f32;
        let first = self.period == 0.0;
        self.period = p;
        self.d_lo = ((p / 2.0).round() as usize).max(1);
        self.d_hi = ((2.0 * p).round() as usize).min(self.weights.len() - 1).min(self.cap - 1);
        for d in self.d_lo..=self.d_hi {
            self.weights[d] = (-0.5 * (TIGHTNESS * (d as f32 / p).ln()).powi(2)).exp();
        }
        if first {
            // Score the window already there: the beats line up at once.
            let start = self.frames.saturating_sub(self.cap as u64);
            for n in start..self.frames {
                let (i, c) = (self.slot(n), self.dp_step(n));
                self.cum[i] = c;
            }
        }
    }

    /// The autocorrelation at a fractional lag: cubic (Catmull-Rom)
    /// interpolation, smooth across whole lags (a linear one would pull
    /// every maximum onto a whole lag, ~0.5 BPM off at 140).
    fn acf_at(&self, lag: f32, len: usize) -> Option<f32> {
        let i = lag.floor() as usize;
        if i < 1 || i + 2 >= len {
            return None;
        }
        let f = lag - i as f32;
        let (p0, p1, p2, p3) = (self.acf[i - 1], self.acf[i], self.acf[i + 1], self.acf[i + 2]);
        Some(p1 + 0.5 * f * (p2 - p0 + f * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + f * (3.0 * (p1 - p2) + p3 - p0))))
    }

    /// The comb's normalised score at period `tau` (−1..1).
    fn comb(&self, tau: f32, len: usize) -> f32 {
        let (mut sum, mut weight) = (0.0, 0.0);
        for (m, w) in COMB.iter().enumerate() {
            if let Some(a) = self.acf_at(tau * (m + 1) as f32, len) {
                sum += w * a;
                weight += w;
            }
        }
        if weight > 0.0 {
            sum / weight
        } else {
            0.0
        }
    }

    fn prior(&self, bpm: f32) -> f32 {
        let broad = (-0.5 * ((bpm / PRIOR_BPM).log2() / PRIOR_OCTAVES).powi(2)).exp();
        match self.guide {
            Some(g) => (-0.5 * ((bpm / g).ln() / GUIDE_SIGMA).powi(2)).exp(),
            None => broad,
        }
    }

    /// A new estimate from the window (every 0.5 s).
    fn estimate_now(&mut self, now: f64) {
        let len = (self.frames as usize).min(self.cap);
        let start = self.frames - len as u64;
        for k in 0..len {
            self.acf[k] = self.odf[self.slot(start + k as u64)];
        }
        // Binomial smoothing (σ ≈ 1 frame): the autocorrelation's peaks
        // become smooth enough to interpolate between whole lags.
        let mut sum = 0.0f32;
        for k in 0..len {
            let (mut v, mut w) = (0.0, 0.0);
            for (j, kw) in SMOOTH.iter().enumerate() {
                if let Some(x) = (k + j).checked_sub(SMOOTH.len() / 2).and_then(|i| self.acf[..len].get(i)) {
                    v += kw * x;
                    w += kw;
                }
            }
            self.buf[k] = v / w;
            sum += v / w;
        }
        let mean = sum / len as f32;
        self.odf_mean = mean;
        let recent = ((PRESENCE_S / self.frame_s) as usize).min(len);
        let (mut var_all, mut var_recent) = (0.0f32, 0.0f32);
        for k in 0..len {
            let x = self.buf[k] - mean;
            var_all += x * x;
            if k >= len - recent {
                var_recent += x * x;
            }
            let age = (len - 1 - k) as f64 * self.frame_s;
            self.buf[k] = x * (-age / RECENCY_S).exp() as f32;
        }
        let presence = if var_all > 1e-12 { (var_recent / recent.max(1) as f32) / (var_all / len as f32) } else { 0.0 };
        self.buf[len..].iter_mut().for_each(|x| *x = 0.0);

        // Autocorrelation = inverse FFT of the power spectrum (zero-padded
        // to twice the window: no wrap-around).
        let ok = self.fwd.process_with_scratch(&mut self.buf, &mut self.spec, &mut self.scratch_fwd).is_ok();
        for c in self.spec.iter_mut() {
            *c = Complex::new(c.norm_sqr(), 0.0);
        }
        let ok = ok && self.inv.process_with_scratch(&mut self.spec, &mut self.acf, &mut self.scratch_inv).is_ok();
        let a0 = self.acf[0];
        let (mut conf_raw, mut raw_bpm) = (0.0f32, None);
        if ok && a0 > 1e-12 && a0.is_finite() {
            for a in self.acf[..len].iter_mut() {
                *a /= a0;
            }
            // Comb × prior over the period grid.
            let (mut best, mut best_i, mut abs_sum) = (f32::MIN, 0usize, 0.0f32);
            for i in 0..self.scores.len() {
                let tau = self.tau_min + i as f32 * LAG_STEP;
                let bpm = (60.0 / (tau as f64 * self.frame_s)) as f32;
                let s = self.comb(tau, len).max(0.0) * self.prior(bpm);
                self.scores[i] = s;
                abs_sum += s;
                if s > best {
                    (best, best_i) = (s, i);
                }
            }
            let mut tau = self.tau_min + best_i as f32 * LAG_STEP;
            if best_i > 0 && best_i + 1 < self.scores.len() {
                let (a, b, c) = (self.scores[best_i - 1], best, self.scores[best_i + 1]);
                let den = a - 2.0 * b + c;
                if den.abs() > 1e-12 {
                    tau += (0.5 * (a - c) / den).clamp(-0.5, 0.5) * LAG_STEP;
                }
            }
            let bpm = ((60.0 / (tau as f64 * self.frame_s)) as f32).clamp(MIN_BPM, MAX_BPM);
            let clarity = self.comb(tau, len);
            let peak = if abs_sum > 0.0 { best * self.scores.len() as f32 / abs_sum } else { 0.0 };
            conf_raw = ramp(clarity, CLARITY_MAP) * ramp(peak, PEAK_MAP) * ramp(presence, PRESENCE_MAP);
            if best > 0.0 {
                raw_bpm = Some(bpm);
            }
        }

        // Stability of the last estimates.
        let stability = match raw_bpm {
            Some(bpm) => {
                self.raw.rotate_right(1);
                self.raw[0] = bpm;
                self.raw_count = (self.raw_count + 1).min(STABILITY_N);
                let last = &self.raw[..self.raw_count];
                let (lo, hi) = last.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &b| (lo.min(b), hi.max(b)));
                let spread = (hi - lo) / (0.5 * (hi + lo));
                (1.0 - ramp(spread, STABILITY_MAP)) * self.raw_count as f32 / STABILITY_N as f32
            }
            None => {
                self.raw_count = 0;
                0.0
            }
        };
        let confidence = conf_raw * stability;

        if let Some(bpm) = raw_bpm.filter(|_| conf_raw >= MIN_UPDATE_CONFIDENCE) {
            self.follow(bpm, now);
        } else {
            self.pending = None;
        }

        // States.
        if confidence >= LOCK_CONFIDENCE {
            let since = *self.good_since.get_or_insert(now);
            if now - since >= LOCK_HOLD_S - 1e-6 || self.out.state == DetectState::Locked {
                self.out.state = DetectState::Locked;
                self.ever_locked = true;
            } else {
                self.out.state = self.listening_state();
            }
        } else {
            self.good_since = None;
            if !(self.out.state == DetectState::Locked && confidence >= UNLOCK_CONFIDENCE) {
                self.out.state = self.listening_state();
            }
        }
        self.out.confidence = confidence;
        if let Some(t) = self.track {
            self.out.bpm = t;
        }
    }

    /// The hysteresis between the raw estimate and the tempo followed.
    fn follow(&mut self, bpm: f32, now: f64) {
        let Some(track) = self.track else {
            self.track = Some(bpm);
            self.set_period(bpm);
            return;
        };
        let near = |a: f32, b: f32, r: f32| octave_distance(a, b) <= (1.0 + r).ln();
        let guided = self.guide.is_some_and(|g| near(bpm, g, GUIDE_ACCEPT) && !near(track, g, GUIDE_ACCEPT));
        if near(bpm, track, FOLLOW_RATIO) || guided {
            self.track = Some(bpm);
            self.pending = None;
        } else {
            let since = match self.pending {
                Some((p, since)) if near(bpm, p, FOLLOW_RATIO) => since,
                _ => now,
            };
            self.pending = Some((bpm, since));
            let octave = near(bpm, track * 2.0, FOLLOW_RATIO) || near(bpm, track / 2.0, FOLLOW_RATIO);
            let need = if octave { OCTAVE_SWITCH_S } else { SWITCH_S };
            if now - since >= need - 1e-6 {
                self.track = Some(bpm);
                self.pending = None;
            } else {
                return;
            }
        }
        self.set_period(bpm);
    }
}

#[cfg(test)]
mod tests {
    use super::super::analysis::{Analyzer, HOP};
    use super::super::onsets::tests::{noise, render_at, Hit};
    use super::*;

    const RATE: u32 = 48_000;

    /// A −50 dBFS noise bed, so the input is never `silent`.
    fn bed(i: usize) -> f32 {
        0.005 * noise(i, 7)
    }

    /// Beat times from `start` to `end` at `bpm`.
    fn beats(bpm: f64, start: f64, end: f64) -> Vec<f64> {
        let p = 60.0 / bpm;
        (0..).map(|k| start + k as f64 * p).take_while(|&t| t < end).collect()
    }

    /// Kick on every beat, snare on 2 and 4, hats on the eighths
    /// (`swing`: the off-beat eighth at this share of the beat, 0.5 = straight).
    fn groove(beat_times: &[f64], swing: f64) -> Vec<(f64, Hit)> {
        let mut hits = Vec::new();
        for (k, w) in beat_times.windows(2).enumerate() {
            let (b, p) = (w[0], w[1] - w[0]);
            hits.push((b, Hit::Kick));
            if k % 2 == 1 {
                hits.push((b, Hit::Snare));
            }
            hits.push((b, Hit::Hat));
            hits.push((b + swing * p, Hit::Hat));
        }
        hits
    }

    fn clicks(beat_times: &[f64]) -> Vec<(f64, Hit)> {
        beat_times.iter().map(|&t| (t, Hit::Click)).collect()
    }

    /// Every hop's estimate, with the hop's end time.
    fn track(signal: &[f32]) -> Vec<(f64, TempoEstimate)> {
        track_with(&mut Analyzer::new(RATE), signal, 0)
    }

    fn track_with(a: &mut Analyzer, signal: &[f32], first_hop: usize) -> Vec<(f64, TempoEstimate)> {
        signal
            .as_chunks::<HOP>()
            .0
            .iter()
            .enumerate()
            .map(|(h, hop)| {
                let t = ((first_hop + h + 1) * HOP) as f64 / RATE as f64;
                (t, a.process(hop, t).tempo)
            })
            .collect()
    }

    fn at(est: &[(f64, TempoEstimate)], t: f64) -> TempoEstimate {
        est.iter().rev().find(|(ht, _)| *ht <= t).map(|e| e.1).unwrap()
    }

    /// Worst distance (s) from each new `next_beat` (and `beat_time`)
    /// after `from` to the nearest true beat.
    fn phase_errors(est: &[(f64, TempoEstimate)], truth: &[f64], from: f64) -> (f64, f64, f64) {
        let near = |t: f64| truth.iter().map(|b| t - b).min_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap();
        let (mut worst_next, mut worst_beat, mut sum, mut n) = (0.0f64, 0.0f64, 0.0, 0);
        for w in est.windows(2) {
            let (a, b) = (w[0].1, w[1].1);
            if w[1].0 < from || b.next_beat == a.next_beat || b.next_beat > truth[truth.len() - 1] {
                continue;
            }
            let e = near(b.next_beat);
            worst_next = worst_next.max(e.abs());
            worst_beat = worst_beat.max(near(b.beat_time).abs());
            sum += e;
            n += 1;
        }
        (worst_next, worst_beat, sum / n.max(1) as f64)
    }

    /// Beat times at `from_bpm` until `switch`, then at `to_bpm`.
    fn beats_changing(from_bpm: f64, switch: f64, to_bpm: f64, end: f64) -> Vec<f64> {
        let mut b = beats(from_bpm, 0.1, switch);
        let last = *b.last().unwrap();
        b.extend(beats(to_bpm, last + 60.0 / to_bpm, end));
        b
    }

    /// When the state first became `state`.
    fn first(est: &[(f64, TempoEstimate)], state: DetectState) -> Option<f64> {
        est.iter().find(|e| e.1.state == state).map(|e| e.0)
    }

    /// Worst |bpm − `bpm`| from `from` on.
    fn worst_error(est: &[(f64, TempoEstimate)], bpm: f64, from: f64) -> f64 {
        est.iter().filter(|e| e.0 >= from).map(|e| (e.1.bpm as f64 - bpm).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn clicks_at_128_lock_within_8_s_and_read_within_half_a_bpm() {
        let b = beats(128.0, 0.1, 16.0);
        let est = track(&render_at(RATE, 16.0, &clicks(&b), bed));
        assert_eq!(first(&est, DetectState::Checking), Some(est[0].0), "listening from the first hop");
        let lock = first(&est, DetectState::Locked).expect("locked");
        assert!(lock < 8.0, "locked after {lock} s");
        assert!(worst_error(&est, 128.0, lock) < 0.5, "{}", worst_error(&est, 128.0, lock));
        assert!(est.iter().filter(|e| e.0 >= lock).all(|e| e.1.state == DetectState::Locked && e.1.confidence >= LOCK_CONFIDENCE));
        // Nothing but the estimate before the first 3 s of sound.
        assert_eq!(at(&est, 2.9).bpm, 0.0);
    }

    #[test]
    fn fixed_tempos_from_70_to_180_are_found() {
        for bpm in [70.0, 87.0, 100.0, 124.0, 140.0, 150.0, 174.0, 180.0] {
            let b = beats(bpm, 0.1, 14.0);
            let est = track(&render_at(RATE, 14.0, &clicks(&b), bed));
            let lock = first(&est, DetectState::Locked).unwrap_or(f64::MAX);
            assert!(lock < 8.0, "clicks {bpm}: locked after {lock} s");
            assert!(worst_error(&est, bpm, lock) < 0.5, "clicks {bpm}: off by {}", worst_error(&est, bpm, lock));
            if bpm > 80.0 {
                // A full kit: kick, snare on 2 and 4, straight eighth hats.
                let est = track(&render_at(RATE, 14.0, &groove(&b, 0.5), bed));
                let lock = first(&est, DetectState::Locked).unwrap_or(f64::MAX);
                assert!(lock < 8.0, "groove {bpm}: locked after {lock} s");
                assert!(worst_error(&est, bpm, lock) < 0.5, "groove {bpm}: off by {}", worst_error(&est, bpm, lock));
            }
        }
    }

    #[test]
    fn a_174_four_beat_pattern_reads_174_not_87() {
        // The task accepts 87 (documented); the comb and the prior give 174.
        let b = beats(174.0, 0.1, 14.0);
        let est = track(&render_at(RATE, 14.0, &groove(&b, 0.5), bed));
        assert!(worst_error(&est, 174.0, 8.0) < 0.5, "{:?}", at(&est, 13.9));
        assert_eq!(at(&est, 13.9).state, DetectState::Locked);
    }

    #[test]
    fn at_70_bpm_off_beat_hats_read_140_and_a_plain_70_reads_70() {
        // Documented choice: eighth hats as loud as the kick make every
        // eighth a pulse; 140 is the tempo the prior prefers (and a
        // dance-floor reading). Without them it is 70.
        let b = beats(70.0, 0.1, 16.0);
        let est = track(&render_at(RATE, 16.0, &groove(&b, 0.5), bed));
        assert!(worst_error(&est, 140.0, 8.0) < 0.5, "{:?}", at(&est, 15.9));
        let kick_snare: Vec<(f64, Hit)> = groove(&b, 0.5).into_iter().filter(|h| h.1 != Hit::Hat).collect();
        let est = track(&render_at(RATE, 16.0, &kick_snare, bed));
        assert!(worst_error(&est, 70.0, 8.0) < 0.5, "{:?}", at(&est, 15.9));
    }

    #[test]
    fn swing_keeps_the_beat_tempo() {
        for swing in [0.6, 0.667] {
            let b = beats(120.0, 0.1, 14.0);
            let est = track(&render_at(RATE, 14.0, &groove(&b, swing), bed));
            let lock = first(&est, DetectState::Locked).unwrap_or(f64::MAX);
            assert!(lock < 8.0, "swing {swing}: locked after {lock}");
            assert!(worst_error(&est, 120.0, lock) < 0.5, "swing {swing}: {:?}", at(&est, 13.9));
        }
    }

    #[test]
    fn a_tempo_change_from_124_to_128_is_followed_within_4_s() {
        let b = beats_changing(124.0, 12.0, 128.0, 24.0);
        let change = b.iter().copied().find(|&t| t >= 12.0).unwrap();
        let est = track(&render_at(RATE, 24.0, &groove(&b, 0.5), bed));
        assert!((at(&est, 11.9).bpm - 124.0).abs() < 0.5, "{:?}", at(&est, 11.9));
        // From when on it reads 128 (± 0.5) for good.
        let adopted = est.iter().rev().take_while(|e| (e.1.bpm - 128.0).abs() < 0.5).last().unwrap().0;
        assert!(adopted - change < 4.0, "adopted {:.2} s after the change", adopted - change);
        assert_eq!(at(&est, 23.9).state, DetectState::Locked);
    }

    /// A held three-note chord: no onsets, but not silence either.
    fn pad(i: usize) -> f32 {
        let t = i as f32 / RATE as f32;
        let tau = 2.0 * std::f32::consts::PI;
        bed(i) + 0.05 * ((tau * 220.0 * t).sin() + (tau * 277.2 * t).sin() + (tau * 329.6 * t).sin())
    }

    #[test]
    fn a_16_beat_break_coasts_on_the_same_bpm_and_keeps_predicting_beats() {
        let b = beats(128.0, 0.1, 30.0);
        let p = 60.0 / 128.0;
        let (break_from, break_to) = (12.0, 12.0 + 16.0 * p);
        let drums: Vec<(f64, Hit)> = groove(&b, 0.5).into_iter().filter(|h| h.0 < break_from - 0.05 || h.0 >= break_to - 0.01).collect();
        let est = track(&render_at(RATE, 30.0, &drums, pad));
        assert_eq!(at(&est, break_from).state, DetectState::Locked);
        let bpm = at(&est, break_from).bpm;
        let during: Vec<_> = est.iter().filter(|e| e.0 >= break_from && e.0 < break_to).collect();
        // Until the drums have left the last 2 s, the estimate may still
        // refine by a hair; once coasting, it is frozen.
        assert!(during.iter().all(|e| (e.1.bpm - bpm).abs() < 0.05), "the BPM does not move in the break");
        let coast = during.iter().position(|e| e.1.state == DetectState::Coasting).expect("coasting in the break");
        let coast_bpm = during[coast].1.bpm;
        assert!(during[coast..].iter().all(|e| e.1.state == DetectState::Coasting && e.1.bpm == coast_bpm));
        let coast = during[coast].0;
        assert!(coast - break_from < 4.0, "coasting {:.2} s into the break", coast - break_from);
        // The beats keep being predicted, on the grid.
        let during: Vec<(f64, TempoEstimate)> = during.iter().map(|e| **e).collect();
        let (worst_next, _, _) = phase_errors(&during, &b, break_from);
        assert!(worst_next < 0.03, "{worst_next}");
        let predicted = during.windows(2).filter(|w| w[1].1.next_beat != w[0].1.next_beat).count();
        assert!(predicted >= 14, "{predicted} beats predicted in the break");
        // The drums are back: locked again.
        let relock = est.iter().find(|e| e.0 > break_to && e.1.state == DetectState::Locked).expect("relocked").0;
        assert!(relock - break_to < 6.0, "relocked {:.2} s after the break", relock - break_to);
        assert!(worst_error(&est, 128.0, break_from).min(worst_error(&est, 128.0, 8.0)) < 0.5);
    }

    #[test]
    fn silence_is_no_input_with_the_bpm_kept_and_a_long_one_is_a_new_track() {
        let b = beats(128.0, 0.1, 10.0);
        let mut signal = render_at(RATE, 10.0, &groove(&b, 0.5), bed);
        signal.resize(signal.len() + 5 * RATE as usize, 0.0);
        let mut a = Analyzer::new(RATE);
        let est = track_with(&mut a, &signal, 0);
        assert_eq!(at(&est, 9.9).state, DetectState::Locked);
        let quiet: Vec<_> = est.iter().filter(|e| e.0 > 10.5).collect();
        let bpm = quiet[0].1.bpm;
        assert!((bpm - 128.0).abs() < 0.5);
        assert!(quiet.iter().all(|e| e.1.state == DetectState::NoInput && e.1.bpm == bpm), "{:?}", quiet[0]);
        assert_eq!(at(&est, 14.9).confidence, 0.0, "forgotten after 3 s");
        // A new track at 100 BPM: no hysteresis to fight, locked as from scratch.
        let b = beats(100.0, 0.1, 12.0);
        let est = track_with(&mut a, &render_at(RATE, 12.0, &groove(&b, 0.5), bed), signal.len() / HOP);
        let start = est[0].0;
        let lock = first(&est, DetectState::Locked).expect("locked") - start;
        assert!(lock < 8.0, "{lock}");
        assert!((at(&est, start + 11.9).bpm - 100.0).abs() < 0.5);
        assert_eq!(at(&est, start + 2.0).bpm, bpm, "the old BPM shown until the first estimate");
    }

    #[test]
    fn a_short_silence_keeps_the_history() {
        let b = beats(128.0, 0.1, 20.0);
        let mut signal = render_at(RATE, 20.0, &groove(&b, 0.5), bed);
        // 1.5 s of digital silence in the middle.
        signal[10 * RATE as usize..(11.5 * RATE as f32) as usize].iter_mut().for_each(|x| *x = 0.0);
        let est = track(&signal);
        assert_eq!(at(&est, 11.4).state, DetectState::NoInput);
        let back = est.iter().find(|e| e.0 > 11.5 && e.1.state != DetectState::NoInput).unwrap();
        assert_eq!(back.1.state, DetectState::Coasting, "was locked: coasting until it relocks");
        assert!((back.1.bpm - 128.0).abs() < 0.5);
        assert_eq!(at(&est, 19.9).state, DetectState::Locked);
    }

    #[test]
    fn noise_without_a_pulse_has_a_low_confidence() {
        for amp in [0.02, 0.3] {
            let signal: Vec<f32> = (0..15 * RATE as usize).map(|i| amp * noise(i, 11)).collect();
            let est = track(&signal);
            let worst = est.iter().map(|e| e.1.confidence).fold(0.0, f32::max);
            assert!(worst < 0.3, "noise at {amp}: confidence up to {worst}");
            assert!(est.iter().all(|e| e.1.state == DetectState::Checking));
        }
    }

    #[test]
    fn the_predicted_beats_land_within_20_ms() {
        for bpm in [90.0, 128.0, 174.0] {
            let b = beats(bpm, 0.1, 20.0);
            for (name, hits) in [("clicks", clicks(&b)), ("groove", groove(&b, 0.5))] {
                let est = track(&render_at(RATE, 20.0, &hits, bed));
                let (next, last, mean) = phase_errors(&est, &b, 10.0);
                assert!(next < 0.02 && last < 0.02, "{name} {bpm}: next {next}, beat {last}");
                assert!(mean.abs() < 0.005, "{name} {bpm}: bias {mean}");
            }
        }
    }

    #[test]
    fn a_guide_settles_the_octave_and_a_new_track_forgets_it() {
        let b = beats(70.0, 0.1, 30.0);
        let signal = render_at(RATE, 30.0, &groove(&b, 0.5), bed);
        let mut a = Analyzer::new(RATE);
        a.set_guide(Some(71.0));
        let first_part = 15 * RATE as usize / HOP * HOP;
        let est = track_with(&mut a, &signal[..first_part], 0);
        assert_eq!(at(&est, 2.0).state, DetectState::Guided);
        assert!(worst_error(&est, 70.0, 8.0) < 0.5, "{:?}", at(&est, 14.9));
        a.new_track();
        assert_eq!(a.tempo().state, DetectState::Checking);
        assert_eq!(a.tempo().confidence, 0.0);
        let est = track_with(&mut a, &signal[first_part..], first_part / HOP);
        assert!((at(&est, 29.9).bpm - 140.0).abs() < 0.5, "no guide any more: {:?}", at(&est, 29.9));
    }

    #[test]
    fn other_sample_rates_work() {
        for rate in [44_100, 96_000] {
            let b = beats(128.0, 0.1, 14.0);
            let signal = render_at(rate, 14.0, &groove(&b, 0.5), |i| 0.005 * noise(i, 7));
            let mut a = Analyzer::new(rate);
            let est: Vec<(f64, TempoEstimate)> =
                signal.as_chunks::<HOP>().0.iter().enumerate().map(|(h, hop)| (((h + 1) * HOP) as f64 / rate as f64, a.process(hop, ((h + 1) * HOP) as f64 / rate as f64).tempo)).collect();
            let lock = first(&est, DetectState::Locked).unwrap_or(f64::MAX);
            assert!(lock < 8.0, "{rate} Hz: locked after {lock}");
            assert!(worst_error(&est, 128.0, lock) < 0.5, "{rate} Hz: {:?}", at(&est, 13.9));
            let (next, _, _) = phase_errors(&est, &b, 10.0);
            assert!(next < 0.02, "{rate} Hz: {next}");
        }
    }

    #[test]
    fn a_hop_does_not_allocate_estimates_included() {
        let b = beats(128.0, 0.1, 8.0);
        let signal = render_at(RATE, 8.0, &groove(&b, 0.5), bed);
        let hops: Vec<&[f32; HOP]> = signal.as_chunks::<HOP>().0.iter().collect();
        let mut a = Analyzer::new(RATE);
        a.process(hops[0], 0.0);
        let n = crate::audio::capture::tests::allocations_during(|| {
            for (h, hop) in hops.iter().enumerate().skip(1) {
                std::hint::black_box(a.process(&hop[..], ((h + 1) * HOP) as f64 / RATE as f64));
            }
        });
        assert_eq!(n, 0, "buffers and FFT plans are made once, in new()");
        assert!(a.tempo().bpm > 0.0 && a.tempo().next_beat > 0.0, "the loop did estimate: {:?}", a.tempo());
    }

    #[test]
    fn the_tracker_alone_handles_odd_input() {
        let mut t = BpmTracker::new(187.5, 1024);
        for h in 0..3_000 {
            let v = if h % 7 == 0 { f32::NAN } else if h % 11 == 0 { f32::INFINITY } else { -1.0 };
            let e = t.process([v, 0.0, 0.0], h as f64 / 187.5, false);
            assert!(e.bpm.is_finite() && e.confidence.is_finite() && e.beat_time.is_finite() && e.next_beat.is_finite());
        }
        assert!(t.estimate().confidence < 0.3);
        t.carry_bpm(f32::NAN);
        t.carry_bpm(128.0);
        assert_eq!(t.estimate().bpm, 128.0);
        t.set_guide(Some(1_000.0));
        assert_eq!(t.guide, Some(MAX_BPM));
        t.set_guide(None);
        assert_eq!(t.estimate().state, DetectState::Checking);
        // Degenerate hop rates do not panic.
        let _ = BpmTracker::new(0.0, 1024);
        let _ = BpmTracker::new(f32::NAN, 1024);
        let mut slow = BpmTracker::new(62.5, 1024); // 16 kHz: one hop per frame
        for h in 0..2_000 {
            slow.process([if h % 31 == 0 { 1.0 } else { 0.0 }, 0.0, 0.0], h as f64 / 62.5, false);
        }
        assert!((slow.estimate().bpm - 60.0 * 62.5 / 31.0).abs() < 0.5, "{:?}", slow.estimate());
    }

    /// Opt-in timing: `cargo test -p laser-studio --release -- --ignored bpm_cost`.
    #[test]
    #[ignore]
    fn bpm_cost_is_small() {
        let mut t = BpmTracker::new(187.5, 1024);
        let hops = 187 * 60;
        let start = std::time::Instant::now();
        for h in 0..hops {
            let v = if h % 88 == 0 { 1.0 } else { 0.01 };
            std::hint::black_box(t.process([v, v, v], h as f64 / 187.5, false));
        }
        let share = start.elapsed().as_secs_f32() / 60.0;
        eprintln!("bpm: {:.3} % of a core", share * 100.0);
        assert!(share < 0.01);
    }
}
