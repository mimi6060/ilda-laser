//! The one tempo clock of the app. Everything "on the beat" (synced
//! rotation, colour chases, strobes, evolving cues, the timeline) reads
//! this clock, so effects can never drift apart the way per-effect speed
//! accumulators do.
//!
//! Beats are a pure function of time: `beat_at(t) = (t - origin) * bpm / 60`.
//! Changing the tempo moves `origin` so the beat position never jumps.
//!
//! *Tempo auto* (T-234): with the source on `Audio`, the clock follows the
//! detector's `TempoEstimate` (audio/bpm.rs), but only as a proposal. It
//! takes a locked, confident estimate's BPM once it has been steady for 2 s
//! (4 s for an octave jump), pulls its phase towards the detected beats by
//! at most 1/16 beat per beat (spread over the beat, so nothing ever
//! jumps), and otherwise keeps running at the last BPM and phase (*Maintien*),
//! unlocking after 30 s. A tap, or any manual BPM, takes the clock back.
//!
//! *Décalage de sortie* (T-246): sound, analysis, the engine loop and the
//! DAC's buffer make the light late on the music. The effects read the
//! *predicted* beat, `effect_beat_at(t) = beat_at(t + offset)`, so with
//! +40 ms they cross each beat 40 ms early and the light lands on it. The
//! clock itself (`bpm`, `beat_at`, the follower, taps) never sees the
//! offset. It only moves *what* is computed for a frame, never *when* a
//! frame goes out: nothing is buffered, and arming, blackout, the e-stop
//! and every safety stage act on the very next frame as before.
//!
//! *Temps 1* (T-235): the detector's suggested "1" (audio/downbeat.rs) is
//! shown against the clock's bar (`TempoState::downbeat`); *Accepter*
//! resyncs the bar on it. With *Tempo auto* locked, the bar is realigned by
//! whole beats on its own, but only when the downbeat confidence rises from
//! low (< 0.3) to high (≥ 0.5) or on a drop (which starts a phrase), and at
//! most once every 8 s. Whole beats: the beat phase never moves.

use crate::audio::bpm::{DetectState, TempoEstimate};
use crate::audio::downbeat::{DownbeatDetection, HIGH_CONFIDENCE, SHOW_CONFIDENCE};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Speed groups and the rate master (T-419) live in speed.rs.
pub use crate::speed::SpeedGroups;

pub const MIN_BPM: f64 = 40.0;
pub const MAX_BPM: f64 = 250.0;
const MAX_TAPS: usize = 8;
/// A pause longer than this starts a new tap sequence.
const TAP_RESET_S: f64 = 2.0;
/// Detected BPMs within this ratio of the running average are the same tempo.
const SAME_TEMPO: f64 = 0.02;
/// How long a new tempo must hold before the clock takes it (s)...
const BPM_HOLD_S: f64 = 2.0;
/// ... and for a jump of more than this ratio (×2 / ÷2: usually an error).
const OCTAVE_RATIO: f64 = 1.3;
const OCTAVE_HOLD_S: f64 = 4.0;
/// The detected BPM is averaged over the tempo's life, then over this (s).
const BPM_AVERAGE_S: f64 = 10.0;
/// The clock only moves when the averaged BPM is further than this.
const BPM_DEADBAND: f64 = 0.3;
/// Phase errors beyond this (beats) are ignored unless they persist...
const BIG_PHASE_ERROR: f64 = 0.25;
/// ... for this many beats, agreeing within 1/8 beat: then a resync.
const BIG_PHASE_BEATS: u32 = 4;
/// Beats are compared only when the clock and the detection agree within this.
const PHASE_BPM_RATIO: f64 = 0.04;
/// Integral term: BPM change per beat of phase error, relative to the BPM.
const DRIFT_GAIN: f64 = 0.02;
/// The integral term only acts on errors smaller than this (beats).
const DRIFT_MAX_ERROR: f64 = 1.0 / 16.0;
/// Integral term cap, BPM per bar.
const MAX_DRIFT_PER_BAR: f64 = 0.05;
/// Automatic bar realigns are at least this far apart (s).
const MIN_REALIGN_S: f64 = 8.0;
/// A drop older than this (s) when first seen is not acted on.
const DROP_FRESH_S: f64 = 4.0;

/// The output offset's range (ms), both ways (T-246).
pub const MAX_OUTPUT_OFFSET_MS: f32 = 100.0;

/// Latency compensation (T-246), saved in `latency.json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LatencyConfig {
    /// *Décalage de sortie*: how far ahead the effects read the beat
    /// (ms, −100..+100, 0 = none).
    pub output_offset_ms: f32,
    /// The analysis delay already taken off the detection times (ms):
    /// computed from the audio settings, never read back from the file.
    #[serde(skip_deserializing)]
    pub analysis_delay_ms: f32,
}

impl LatencyConfig {
    /// The offset, or why it is refused: not a number. Clamped to
    /// ±`MAX_OUTPUT_OFFSET_MS` (a file or a request can say anything).
    pub fn checked_offset_ms(ms: f64) -> Result<f32, &'static str> {
        if !ms.is_finite() {
            return Err("décalage de sortie : un nombre de millisecondes est attendu");
        }
        Ok((ms as f32).clamp(-MAX_OUTPUT_OFFSET_MS, MAX_OUTPUT_OFFSET_MS))
    }

    /// A loaded file made safe: a damaged offset is 0.
    pub fn sanitized(self) -> Self {
        Self { output_offset_ms: Self::checked_offset_ms(self.output_offset_ms as f64).unwrap_or(0.0), analysis_delay_ms: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TempoSource {
    Manual,
    Tap,
    /// Follows the audio detection (*Tempo auto*).
    Audio,
    /// Follows an incoming MIDI clock (T-207, midi/clock.rs): Tap and Nudge
    /// are ignored meanwhile.
    MidiClock,
}

/// How the clock follows the detection. Defaults from the task (T-234).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioTempoConfig {
    /// Estimates below this confidence are not followed.
    pub min_confidence: f32,
    /// Share of the phase error corrected per beat.
    pub phase_gain: f32,
    /// Hard cap on the phase correction, beats per beat.
    pub max_phase_step_beats: f64,
    /// *Maintien* longer than this unlocks (s).
    pub coast_timeout_s: f32,
}

impl Default for AudioTempoConfig {
    fn default() -> Self {
        Self { min_confidence: 0.6, phase_gain: 0.2, max_phase_step_beats: 1.0 / 16.0, coast_timeout_s: 30.0 }
    }
}

/// Where *Tempo auto* stands (`/api/state.tempo.follow`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowState {
    /// Source is not `Audio`.
    #[default]
    Off,
    /// *Tempo auto* on, no locked estimate yet: the clock runs as it was.
    Waiting,
    /// Following a locked, confident estimate.
    Locked,
    /// Lost the lock: last BPM and phase kept (*Maintien*).
    Coasting,
    /// *Maintien* timed out: the next lock starts afresh.
    Unlocked,
}

/// Follower state (only meaningful while the source is `Audio`).
#[derive(Clone, Debug, Default)]
struct Follow {
    state: FollowState,
    /// Last `apply_detection` time, for its time steps.
    last_t: Option<f64>,
    last_locked_t: f64,
    confidence: f32,
    detected_bpm: f64,
    /// Detected BPM, averaged, and for how long (locked seconds) it has held.
    avg_bpm: Option<f64>,
    avg_s: f64,
    /// The last detected beat already compared.
    last_beat_time: f64,
    /// Phase shift still to apply (beats) and its rate (beats per beat).
    slew_left: f64,
    slew_rate: f64,
    /// Consecutive big phase errors, and the last one.
    big_errors: u32,
    last_big: f64,
    /// Resyncs done (for the tests and the log).
    resyncs: u32,
}

impl Follow {
    fn new(state: FollowState) -> Self {
        Self { state, ..Default::default() }
    }

    /// Forget the tempo and phase history, keep the state and readings.
    fn forget(&mut self) {
        *self = Self { state: self.state, last_t: self.last_t, last_locked_t: self.last_locked_t, confidence: self.confidence, detected_bpm: self.detected_bpm, ..Default::default() };
    }
}

/// The suggested "1" against the clock (`/api/state.tempo.downbeat`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DownbeatEstimate {
    /// The clock's `beat_in_bar` at the detected "1" (0 = already aligned).
    pub beat_in_bar_offset: u8,
    pub confidence: f32,
    /// Start of the current phrase (audio time, studio clock), if any.
    pub phrase_start: Option<f64>,
}

/// What the clock keeps of the downbeat detection.
#[derive(Clone, Copy, Debug)]
struct DownbeatFollow {
    det: Option<DownbeatDetection>,
    /// Hysteresis: the confidence went ≥ `HIGH_CONFIDENCE` and has not
    /// fallen below `SHOW_CONFIDENCE` since.
    high: bool,
    /// The phrase counter already seen.
    phrases: Option<u64>,
    last_realign_t: f64,
    /// Automatic realigns done (tests, log).
    realigns: u32,
}

impl Default for DownbeatFollow {
    fn default() -> Self {
        Self { det: None, high: false, phrases: None, last_realign_t: f64::NEG_INFINITY, realigns: 0 }
    }
}

#[derive(Clone, Debug)]
pub struct TempoClock {
    pub bpm: f64,
    pub beats_per_bar: u8,
    pub source: TempoSource,
    pub follow_config: AudioTempoConfig,
    origin_s: f64,
    taps: VecDeque<f64>,
    guide_taps: VecDeque<f64>,
    /// The guide given to the estimator (*Guider*), if any.
    guide_bpm: Option<f64>,
    follow: Follow,
    /// *Décalage de sortie* (s, T-246): only `effect_beat_at` reads it.
    output_offset_s: f64,
    downbeat: DownbeatFollow,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct TempoState {
    pub bpm: f64,
    /// Beats since the clock's origin (fractional).
    pub beat: f64,
    pub bar: i64,
    /// 0-based beat within the bar (0 = the "one").
    pub beat_in_bar: u8,
    /// Position inside the current beat, 0..1.
    pub phase: f64,
    pub beats_per_bar: u8,
    pub source: TempoSource,
    /// *Tempo auto*: where the follower stands, and the last estimate seen.
    pub follow: FollowState,
    pub confidence: f32,
    pub detected_bpm: f64,
    /// The *Guider* tempo sent to the estimator.
    pub guide_bpm: Option<f64>,
    /// *Décalage de sortie* (T-246): the effects read the beat this far ahead.
    pub output_offset_ms: f32,
    /// The detected "1" against this clock (T-235); none without a fresh
    /// detection at this tempo, or outside 4/4.
    pub downbeat: Option<DownbeatEstimate>,
    /// Show *Temps 1 suggéré*: confident enough and not where the clock's 1 is.
    pub downbeat_suggested: bool,
    /// Bar within the current phrase (0-based), and the phrase length.
    pub phrase_bar: Option<u8>,
    pub phrase_bars: u8,
}

impl Default for TempoClock {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            beats_per_bar: 4,
            source: TempoSource::Manual,
            follow_config: AudioTempoConfig::default(),
            origin_s: 0.0,
            taps: VecDeque::new(),
            guide_taps: VecDeque::new(),
            guide_bpm: None,
            follow: Follow::default(),
            output_offset_s: 0.0,
            downbeat: DownbeatFollow::default(),
        }
    }
}

/// Adds a tap to `taps`; from the third, the tapped BPM (60 / median interval).
fn tapped_bpm(taps: &mut VecDeque<f64>, t: f64) -> Option<f64> {
    if taps.back().is_some_and(|&last| t - last > TAP_RESET_S || t < last) {
        taps.clear();
    }
    taps.push_back(t);
    while taps.len() > MAX_TAPS {
        taps.pop_front();
    }
    if taps.len() < 3 {
        return None;
    }
    let mut intervals: Vec<f64> = taps.iter().zip(taps.iter().skip(1)).map(|(a, b)| b - a).collect();
    intervals.sort_by(|a, b| a.total_cmp(b));
    let median = intervals[intervals.len() / 2];
    (median > 0.0).then(|| 60.0 / median)
}

/// The one launch quantisation (T-159), used by the cue grid and the
/// evolving cues (T-111): the first multiple of `grid` beats at or after
/// `beat`, or the one just passed when `beat` is less than `catch_up`
/// beats after it (a press a hair late still starts on that beat). A grid
/// of 0 (or less) is no quantisation: `beat` itself.
pub fn quantize(beat: f64, grid: f64, catch_up: f64) -> f64 {
    if grid.is_nan() || grid <= 0.0 || !beat.is_finite() {
        return beat;
    }
    let before = (beat / grid).floor() * grid;
    if beat - before < catch_up.max(0.0) { before } else { before + grid }
}

/// `x` wrapped to -½..½.
fn wrap_half(x: f64) -> f64 {
    x - x.round()
}

impl TempoClock {
    pub fn beat_at(&self, t: f64) -> f64 {
        (t - self.origin_s) * self.bpm / 60.0
    }

    /// The beat the effects play at `t` (T-246): the clock's beat
    /// `output_offset` later, so the light, late by the output's latency,
    /// lands on the beat. Everything rendered reads this one; the clock,
    /// the follower and the operator's own beat display read `beat_at`.
    pub fn effect_beat_at(&self, t: f64) -> f64 {
        self.beat_at(self.effect_t(t))
    }

    /// The seconds the effects play at `t` (T-246): what a free-BPM speed
    /// group (T-419) reads, ahead like `effect_beat_at`.
    pub fn effect_t(&self, t: f64) -> f64 {
        t + self.output_offset_s
    }

    /// Sets the *Décalage de sortie* (ms): refused when not a number,
    /// clamped to ±`MAX_OUTPUT_OFFSET_MS`. Returns the value applied. The
    /// BPM and `beat_at` don't change.
    pub fn set_output_offset_ms(&mut self, ms: f64) -> Result<f32, &'static str> {
        let ms = LatencyConfig::checked_offset_ms(ms)?;
        self.output_offset_s = ms as f64 / 1_000.0;
        Ok(ms)
    }

    pub fn output_offset_ms(&self) -> f32 {
        (self.output_offset_s * 1_000.0) as f32
    }

    pub fn state(&self, t: f64) -> TempoState {
        let beat = self.beat_at(t);
        let est = self.downbeat_estimate();
        let whole = beat.floor();
        let bpb = self.beats_per_bar as f64;
        TempoState {
            bpm: self.bpm,
            beat,
            bar: (whole / bpb).floor() as i64,
            beat_in_bar: whole.rem_euclid(bpb) as u8,
            phase: beat - whole,
            beats_per_bar: self.beats_per_bar,
            source: self.source,
            follow: self.follow.state,
            confidence: self.follow.confidence,
            detected_bpm: self.follow.detected_bpm,
            guide_bpm: self.guide_bpm,
            output_offset_ms: self.output_offset_ms(),
            downbeat: est,
            downbeat_suggested: est.is_some_and(|e| e.confidence >= SHOW_CONFIDENCE && (e.beat_in_bar_offset != 0 || self.downbeat_phase_off())),
            phrase_bar: self.phrase_bar(beat),
            phrase_bars: self.downbeat.det.map_or(8, |d| d.phrase_bars),
        }
    }

    /// Change tempo without moving the current beat position.
    pub fn set_bpm(&mut self, bpm: f64, t: f64) {
        let beat = self.beat_at(t);
        self.bpm = bpm.clamp(MIN_BPM, MAX_BPM);
        self.origin_s = t - beat * 60.0 / self.bpm;
    }

    /// T-456: a backup machine in standby follows the main machine's
    /// clock: `bpm` and the meter, with beat `beat` at `t` (this machine's
    /// clock). Manual source: nothing on this machine drives it meanwhile.
    /// Values that are not numbers are ignored.
    pub fn mirror(&mut self, bpm: f64, beats_per_bar: u8, beat: f64, t: f64) {
        if !bpm.is_finite() || !beat.is_finite() || !t.is_finite() {
            return;
        }
        self.bpm = bpm.clamp(MIN_BPM, MAX_BPM);
        self.beats_per_bar = beats_per_bar.clamp(1, 16);
        self.origin_s = t - beat * 60.0 / self.bpm;
        self.set_source(TempoSource::Manual);
    }

    /// Sets the source; leaving `Audio` stops following at once.
    fn set_source(&mut self, source: TempoSource) {
        if source != TempoSource::Audio {
            self.follow = Follow::new(FollowState::Off);
        }
        self.source = source;
    }

    /// MIDI clock (T-207): on, the clock is driven by the incoming ticks
    /// (*Tempo auto* stops); off, back to manual at the current BPM and
    /// phase, so nothing jumps. Off does nothing under another source.
    pub fn set_midi_clock(&mut self, on: bool) {
        if on {
            self.set_source(TempoSource::MidiClock);
        } else if self.source == TempoSource::MidiClock {
            self.set_source(TempoSource::Manual);
        }
    }

    pub fn set_bpm_manual(&mut self, bpm: f64, t: f64) {
        self.set_bpm(bpm, t);
        self.set_source(TempoSource::Manual);
    }

    /// Tap tempo: from the third tap, BPM = 60 / median interval, and the
    /// last tap lands exactly on a whole beat. The tap always wins: the
    /// first tap already takes the clock back from *Tempo auto*.
    pub fn tap(&mut self, t: f64) {
        if self.source == TempoSource::Audio {
            self.set_source(TempoSource::Tap);
        }
        let Some(bpm) = tapped_bpm(&mut self.taps, t) else { return };
        self.set_bpm(bpm, t);
        self.set_source(TempoSource::Tap);
        self.align_to_whole_beat(t);
    }

    /// The present instant becomes the first beat of a bar.
    pub fn resync(&mut self, t: f64) {
        let bpb = self.beats_per_bar as f64;
        let target = (self.beat_at(t) / bpb).round() * bpb;
        self.origin_s = t - target * 60.0 / self.bpm;
    }

    /// Shift the phase by `beats` (positive = later beats arrive sooner).
    pub fn nudge(&mut self, beats: f64) {
        self.origin_s -= beats * 60.0 / self.bpm;
    }

    fn align_to_whole_beat(&mut self, t: f64) {
        let beat = self.beat_at(t);
        self.origin_s += (beat - beat.round()) * 60.0 / self.bpm;
    }

    /// *Tempo auto* on: follow the detection (from `Waiting`); off: back to
    /// manual at the current BPM and phase.
    pub fn set_auto(&mut self, on: bool) {
        if on && self.source != TempoSource::Audio {
            self.source = TempoSource::Audio;
            self.follow = Follow::new(FollowState::Waiting);
        } else if !on && self.source == TempoSource::Audio {
            self.set_source(TempoSource::Manual);
        }
    }

    /// *Guider*: taps that never set the clock; from the third, their tempo
    /// becomes the estimator's guide (a ±3 % prior, audio/bpm.rs). Returns
    /// the guide when it changed.
    pub fn guide_tap(&mut self, t: f64) -> Option<f64> {
        let bpm = tapped_bpm(&mut self.guide_taps, t)?.clamp(MIN_BPM, MAX_BPM);
        self.guide_bpm = Some(bpm);
        Some(bpm)
    }

    pub fn guide_bpm(&self) -> Option<f64> {
        self.guide_bpm
    }

    /// *Nouveau morceau*: the guide, the follower's history and the
    /// downbeat are forgotten (the clock keeps its BPM and phase).
    pub fn new_track(&mut self) {
        self.guide_bpm = None;
        self.guide_taps.clear();
        self.follow.forget();
        self.downbeat = DownbeatFollow { phrases: self.downbeat.phrases, last_realign_t: self.downbeat.last_realign_t, realigns: self.downbeat.realigns, ..Default::default() };
    }

    /// The detection at this clock's tempo (within 4 %) in 4/4.
    fn matching_downbeat(&self) -> Option<DownbeatDetection> {
        self.downbeat.det.filter(|d| self.beats_per_bar == 4 && d.bpm > 0.0 && (d.bpm as f64 / self.bpm - 1.0).abs() < PHASE_BPM_RATIO)
    }

    /// ... with a "1" found.
    fn usable_downbeat(&self) -> Option<DownbeatDetection> {
        self.matching_downbeat().filter(|d| d.downbeat_t > 0.0)
    }

    /// The detected "1" against the clock's bar (T-235's `DownbeatEstimate`).
    pub fn downbeat_estimate(&self) -> Option<DownbeatEstimate> {
        let d = self.usable_downbeat()?;
        let offset = self.beat_at(d.downbeat_t).round().rem_euclid(self.beats_per_bar as f64) as u8;
        let confidence = (d.confidence.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        Some(DownbeatEstimate { beat_in_bar_offset: offset, confidence, phrase_start: d.phrase_start })
    }

    /// The detected "1" falls between the clock's beats (more than ¼ off).
    fn downbeat_phase_off(&self) -> bool {
        self.usable_downbeat().is_some_and(|d| wrap_half(self.beat_at(d.downbeat_t)).abs() > BIG_PHASE_ERROR)
    }

    /// Bar within the phrase at `beat`, counted from the phrase start's
    /// nearest whole beat.
    fn phrase_bar(&self, beat: f64) -> Option<u8> {
        let d = self.matching_downbeat()?;
        let start = self.beat_at(d.phrase_start?).round();
        let bars = ((beat - start) / self.beats_per_bar as f64).floor();
        (bars >= 0.0).then(|| bars.rem_euclid(d.phrase_bars.max(1) as f64) as u8)
    }

    /// *Accepter* (T-235): a resync on the suggested "1": the detected
    /// downbeat becomes the nearest bar start, phase included. False (and
    /// nothing moves) without a usable suggestion.
    pub fn accept_downbeat(&mut self) -> bool {
        let Some(d) = self.usable_downbeat() else { return false };
        let bpb = self.beats_per_bar as f64;
        let target = (self.beat_at(d.downbeat_t) / bpb).round() * bpb;
        self.origin_s = d.downbeat_t - target * 60.0 / self.bpm;
        true
    }

    /// Shifts the bar by whole beats so the audio time `at` is a bar start
    /// (the nearer way round). Returns the shift.
    fn align_bar_on(&mut self, at: f64) -> i64 {
        let bpb = self.beats_per_bar.max(1) as i64;
        let off = (self.beat_at(at).round() as i64).rem_euclid(bpb);
        let shift = if off * 2 <= bpb { -off } else { bpb - off };
        if shift != 0 {
            self.nudge(shift as f64);
        }
        shift
    }

    /// Called every engine frame with the latest fresh downbeat detection
    /// (none when stale). Keeps it for the state; with *Tempo auto* locked,
    /// realigns the bar on a rise to high confidence or on a fresh drop,
    /// at most once every `MIN_REALIGN_S`. Never touches the BPM or phase.
    pub fn apply_downbeat(&mut self, det: Option<DownbeatDetection>, now: f64) {
        self.downbeat.det = det;
        let Some(d) = det else { return };
        let seen = self.downbeat.phrases.replace(d.phrases);
        let drop = seen.is_some_and(|n| n != d.phrases) && d.phrase_from_drop && d.phrase_start.is_some_and(|p| (now - p).abs() < DROP_FRESH_S);
        let rising = match self.downbeat_estimate() {
            Some(e) => {
                let was = self.downbeat.high;
                if e.confidence >= HIGH_CONFIDENCE {
                    self.downbeat.high = true;
                } else if e.confidence < SHOW_CONFIDENCE {
                    self.downbeat.high = false;
                }
                !was && self.downbeat.high
            }
            None => false,
        };
        let following = self.source == TempoSource::Audio && self.follow.state == FollowState::Locked;
        if !following || now - self.downbeat.last_realign_t < MIN_REALIGN_S || self.matching_downbeat().is_none() {
            return;
        }
        let at = if drop { d.phrase_start } else if rising { Some(d.downbeat_t) } else { None };
        let Some(at) = at else { return };
        self.downbeat.last_realign_t = now;
        let shift = self.align_bar_on(at);
        if shift != 0 {
            self.downbeat.realigns += 1;
            log::info!("tempo auto : mesure recalée de {shift:+} temps ({})", if drop { "drop" } else { "temps 1 détecté" });
        }
    }

    /// Called every engine frame while the source is `Audio`, with the
    /// latest estimate (`TempoEstimate::default()`, i.e. *no input*, when
    /// none is fresh). Never jumps the beat: BPM changes go through
    /// `set_bpm`, phase corrections are spread over a beat and capped.
    pub fn apply_detection(&mut self, est: &TempoEstimate, now: f64) {
        if self.source != TempoSource::Audio {
            self.follow.state = FollowState::Off;
            return;
        }
        let cfg = self.follow_config;
        let dt = self.follow.last_t.map_or(0.0, |last| (now - last).clamp(0.0, 0.25));
        self.follow.last_t = Some(now);
        self.slew(dt);

        let bpm = est.bpm as f64;
        let confidence = if est.confidence.is_finite() { est.confidence } else { 0.0 };
        self.follow.confidence = confidence;
        if bpm.is_finite() && bpm > 0.0 {
            self.follow.detected_bpm = bpm;
        }
        let good = est.state == DetectState::Locked && confidence >= cfg.min_confidence && (MIN_BPM..=MAX_BPM).contains(&bpm);
        if !good {
            match self.follow.state {
                FollowState::Locked => self.follow.state = FollowState::Coasting,
                FollowState::Coasting if now - self.follow.last_locked_t > cfg.coast_timeout_s as f64 => {
                    log::info!("tempo auto : déverrouillé après {} s de maintien", cfg.coast_timeout_s);
                    self.follow.forget();
                    self.follow.state = FollowState::Unlocked;
                }
                _ => {}
            }
            return;
        }
        if matches!(self.follow.state, FollowState::Waiting | FollowState::Unlocked | FollowState::Off) {
            self.follow.forget();
        }
        self.follow.state = FollowState::Locked;
        self.follow.last_locked_t = now;
        self.follow_bpm(bpm, dt, now);
        if est.beat_time > self.follow.last_beat_time {
            self.follow.last_beat_time = est.beat_time;
            let fresh = est.beat_time <= now + 0.05 && now - est.beat_time < 2.0 * 60.0 / self.bpm;
            if fresh && (bpm / self.bpm - 1.0).abs() < PHASE_BPM_RATIO {
                self.follow_phase(est.beat_time, now);
            }
        }
    }

    /// BPM: an average of the detected tempo (the whole life of this tempo,
    /// then the last `BPM_AVERAGE_S`), taken once it has held 2 s (4 s for
    /// an octave jump) and is more than 0.3 BPM away.
    fn follow_bpm(&mut self, bpm: f64, dt: f64, now: f64) {
        let f = &mut self.follow;
        match f.avg_bpm {
            Some(avg) if (bpm / avg - 1.0).abs() <= SAME_TEMPO => {
                f.avg_s += dt;
                let alpha = if f.avg_s > 0.0 { (dt / f.avg_s).max(dt / BPM_AVERAGE_S) } else { 0.0 };
                f.avg_bpm = Some(avg + alpha.min(1.0) * (bpm - avg));
            }
            _ => {
                f.avg_bpm = Some(bpm);
                f.avg_s = 0.0;
            }
        }
        let avg = f.avg_bpm.unwrap_or(bpm);
        let ratio = avg / self.bpm;
        let hold = if !(1.0 / OCTAVE_RATIO..=OCTAVE_RATIO).contains(&ratio) { OCTAVE_HOLD_S } else { BPM_HOLD_S };
        if f.avg_s >= hold && (avg - self.bpm).abs() > BPM_DEADBAND {
            if hold == OCTAVE_HOLD_S {
                log::info!("tempo auto : saut de tempo {:.1} → {avg:.1} BPM", self.bpm);
            }
            self.set_bpm(avg, now);
        }
    }

    /// Phase-locked loop on one detected beat: error `e` = clock's nearest
    /// whole beat − clock's beat at the detected time (beats, ±½). Corrects
    /// `k·e` over the next beat, capped; a big error is ignored unless it
    /// persists `BIG_PHASE_BEATS` beats (then the whole error, still at the
    /// capped rate). A small integral term trims the BPM.
    fn follow_phase(&mut self, beat_time: f64, now: f64) {
        let cfg = self.follow_config;
        let cap = cfg.max_phase_step_beats.abs();
        let err = -wrap_half(self.beat_at(beat_time));
        let f = &mut self.follow;
        if err.abs() > BIG_PHASE_ERROR {
            let agrees = f.big_errors > 0 && wrap_half(err - f.last_big).abs() < 0.125;
            f.big_errors = if agrees { f.big_errors + 1 } else { 1 };
            f.last_big = err;
            if f.big_errors >= BIG_PHASE_BEATS {
                log::info!("tempo auto : recalage de phase sur l'audio ({err:+.2} temps)");
                f.big_errors = 0;
                f.resyncs += 1;
                f.slew_left = err;
                f.slew_rate = cap;
            }
            return;
        }
        f.big_errors = 0;
        if f.slew_left.abs() > cap {
            // A resync is under way: let it finish.
            return;
        }
        let step = (cfg.phase_gain as f64 * err).clamp(-cap, cap);
        f.slew_left = step;
        f.slew_rate = step.abs();
        if err.abs() < DRIFT_MAX_ERROR {
            // Only once tracking: pulling in a big offset is not drift.
            let max_drift = MAX_DRIFT_PER_BAR / self.beats_per_bar.max(1) as f64;
            let drift = (DRIFT_GAIN * err * self.bpm).clamp(-max_drift, max_drift);
            self.set_bpm(self.bpm + drift, now);
        }
    }

    /// Applies the pending phase shift at `slew_rate` beats per beat.
    fn slew(&mut self, dt: f64) {
        let f = &mut self.follow;
        if f.slew_left == 0.0 || dt <= 0.0 {
            return;
        }
        let beats = dt * self.bpm / 60.0;
        let step = f.slew_left.abs().min(f.slew_rate * beats).copysign(f.slew_left);
        f.slew_left -= step;
        if f.slew_left.abs() < 1e-12 {
            f.slew_left = 0.0;
        }
        self.nudge(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_to_the_next_boundary_with_catch_up() {
        let eighth = 0.125;
        assert_eq!(quantize(10.4, 1.0, eighth), 11.0);
        assert_eq!(quantize(11.05, 1.0, eighth), 11.0, "just after the beat: catch up");
        assert_eq!(quantize(11.2, 1.0, eighth), 12.0);
        assert_eq!(quantize(10.4, 4.0, eighth), 12.0);
        assert_eq!(quantize(12.1, 4.0, eighth), 12.0);
        assert_eq!(quantize(10.4, 0.0, eighth), 10.4, "no grid: no quantisation");
        assert_eq!(quantize(-0.5, 1.0, eighth), 0.0);
    }

    #[test]
    fn four_regular_taps_give_the_tapped_tempo() {
        let mut c = TempoClock::default();
        c.set_bpm_manual(90.0, 0.0);
        for i in 0..4 {
            c.tap(10.0 + i as f64 * 0.5);
        }
        assert!((c.bpm - 120.0).abs() < 0.1, "bpm {}", c.bpm);
        assert_eq!(c.source, TempoSource::Tap);
        let b = c.beat_at(11.5);
        assert!((b - b.round()).abs() < 1e-9, "last tap not on a whole beat: {b}");
    }

    #[test]
    fn a_lone_tap_after_silence_keeps_the_tempo() {
        let mut c = TempoClock::default();
        for i in 0..4 {
            c.tap(i as f64 * 0.5);
        }
        let before = c.bpm;
        c.tap(1.5 + 3.0);
        assert_eq!(c.bpm, before);
    }

    #[test]
    fn two_taps_are_not_enough() {
        let mut c = TempoClock::default();
        c.tap(0.0);
        c.tap(0.3);
        assert_eq!(c.bpm, 120.0);
    }

    #[test]
    fn median_ignores_one_sloppy_tap() {
        let mut c = TempoClock::default();
        for t in [0.0, 0.5, 1.0, 1.8, 2.3, 2.8] {
            c.tap(t);
        }
        assert!((c.bpm - 120.0).abs() < 0.1, "bpm {}", c.bpm);
    }

    #[test]
    fn changing_bpm_does_not_jump_the_beat() {
        let mut c = TempoClock::default();
        let t = 37.3;
        let before = c.beat_at(t);
        c.set_bpm_manual(128.0, t);
        assert!((c.beat_at(t) - before).abs() < 1e-6);
        assert!(c.beat_at(t + 1.0) > before);
    }

    #[test]
    fn resync_puts_now_on_the_one() {
        let mut c = TempoClock::default();
        c.resync(12.34);
        let s = c.state(12.34);
        assert_eq!(s.beat_in_bar, 0);
        assert!(s.phase.abs() < 1e-9 || (1.0 - s.phase).abs() < 1e-9);
    }

    #[test]
    fn no_drift_after_an_hour() {
        let mut c = TempoClock::default();
        c.set_bpm_manual(128.0, 0.0);
        assert!((c.beat_at(3600.0) - 7680.0).abs() < 1e-6);
    }

    #[test]
    fn nudge_shifts_phase_and_bpm_is_bounded() {
        let mut c = TempoClock::default();
        let before = c.beat_at(5.0);
        c.nudge(1.0 / 32.0);
        assert!((c.beat_at(5.0) - before - 1.0 / 32.0).abs() < 1e-9);
        c.set_bpm_manual(1000.0, 5.0);
        assert_eq!(c.bpm, MAX_BPM);
        c.set_bpm_manual(1.0, 5.0);
        assert_eq!(c.bpm, MIN_BPM);
    }

    // ---- Décalage de sortie (T-246), simulated time ----

    /// (beat number, time) each time `beat(t)` crosses a whole beat,
    /// sampled every 1 ms over `secs`.
    fn crossings(secs: f64, beat: impl Fn(f64) -> f64) -> Vec<(i64, f64)> {
        let mut out = Vec::new();
        let mut last = beat(0.0).floor();
        for ms in 1..=(secs * 1_000.0) as i64 {
            let t = ms as f64 / 1_000.0;
            let b = beat(t).floor();
            if b != last {
                out.push((b as i64, t));
                last = b;
            }
        }
        out
    }

    #[test]
    fn an_offset_of_40_ms_crosses_every_beat_40_ms_early() {
        for offset in [40.0, -25.0] {
            let mut c = TempoClock::default();
            c.set_bpm_manual(128.0, 0.0);
            c.nudge(0.3);
            assert_eq!(c.set_output_offset_ms(offset), Ok(offset as f32));
            let clock = crossings(10.0, |t| c.beat_at(t));
            let effects = crossings(10.0, |t| c.effect_beat_at(t));
            assert!(clock.len() >= 20);
            // Each beat, by its number: when the clock crosses it, minus
            // when the effects do.
            let lead: Vec<f64> = clock.iter().filter_map(|&(n, t)| effects.iter().find(|e| e.0 == n).map(|e| t - e.1)).collect();
            assert!(lead.len() >= 20);
            for l in lead {
                assert!((l - offset / 1_000.0).abs() <= 0.0011, "offset {offset}: effects lead by {:.1} ms", l * 1e3);
            }
        }
    }

    #[test]
    fn the_offset_moves_neither_the_bpm_nor_the_clock() {
        let mut c = TempoClock::default();
        c.set_bpm_manual(128.0, 3.0);
        let (bpm, beats): (f64, Vec<f64>) = (c.bpm, (0..50).map(|i| c.beat_at(i as f64 * 0.37)).collect());
        c.set_output_offset_ms(80.0).unwrap();
        assert_eq!(c.bpm, bpm);
        assert_eq!((0..50).map(|i| c.beat_at(i as f64 * 0.37)).collect::<Vec<_>>(), beats);
        assert_eq!(c.state(5.0).beat, c.beat_at(5.0), "the state shows the clock's beat");
        assert_eq!(c.state(5.0).output_offset_ms, 80.0);
        assert!((c.effect_beat_at(5.0) - c.beat_at(5.08)).abs() < 1e-9);
        // Tap, tempo changes and resync work on the clock as before.
        c.set_bpm_manual(140.0, 6.0);
        assert!((c.effect_beat_at(6.0) - c.beat_at(6.08)).abs() < 1e-9);
        c.set_output_offset_ms(0.0).unwrap();
        assert_eq!(c.effect_beat_at(6.0), c.beat_at(6.0));
    }

    #[test]
    fn the_offset_is_clamped_and_never_nan() {
        let mut c = TempoClock::default();
        assert_eq!(c.set_output_offset_ms(5_000.0), Ok(MAX_OUTPUT_OFFSET_MS));
        assert_eq!(c.set_output_offset_ms(-5_000.0), Ok(-MAX_OUTPUT_OFFSET_MS));
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(c.set_output_offset_ms(bad).is_err());
            assert_eq!(c.output_offset_ms(), -MAX_OUTPUT_OFFSET_MS, "a refused value changes nothing");
            assert!(c.effect_beat_at(1.0).is_finite());
        }
        // Old files and damaged ones load.
        let old: LatencyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old, LatencyConfig::default());
        let file: LatencyConfig = serde_json::from_str(r#"{ "output_offset_ms": 900, "analysis_delay_ms": 55 }"#).unwrap();
        assert_eq!(file.sanitized(), LatencyConfig { output_offset_ms: 100.0, analysis_delay_ms: 0.0 });
        assert_eq!(LatencyConfig { output_offset_ms: f32::NAN, analysis_delay_ms: 0.0 }.sanitized().output_offset_ms, 0.0);
    }

    #[test]
    fn state_reports_bar_and_beat_in_bar() {
        let c = TempoClock::default(); // 120 bpm: 2 beats per second
        let s = c.state(2.75); // beat 5.5
        assert_eq!((s.bar, s.beat_in_bar), (1, 1));
        assert!((s.phase - 0.5).abs() < 1e-9);
    }

    // ---- Tempo auto (T-234): simulated estimates and time, no device ----

    const FRAME: f64 = 1.0 / 60.0;

    /// A detector that hears a steady grid: beats at `offset + n·60/bpm`,
    /// each reported 21 ms late (as bpm.rs does), with `bpm_at` as its BPM.
    fn detector(bpm: f64, offset: f64, state: DetectState, confidence: f32) -> impl Fn(f64) -> TempoEstimate {
        move |t: f64| {
            let spb = 60.0 / bpm;
            let n = ((t - 0.021 - offset) / spb).floor();
            let beat_time = offset + n * spb;
            TempoEstimate { bpm: bpm as f32, confidence, beat_time, next_beat: beat_time + spb, state }
        }
    }

    fn locked(bpm: f64, offset: f64) -> impl Fn(f64) -> TempoEstimate {
        detector(bpm, offset, DetectState::Locked, 0.8)
    }

    /// The engine loop: 60 fps, one `apply_detection` per frame. Records
    /// every frame's beat discontinuity (beat_at(t) before vs after).
    struct Sim {
        clock: TempoClock,
        t: f64,
        jumps: Vec<(f64, f64, f64)>,
    }

    impl Sim {
        fn new(bpm: f64) -> Self {
            let mut clock = TempoClock::default();
            clock.set_bpm_manual(bpm, 0.0);
            clock.set_auto(true);
            Self { clock, t: 0.0, jumps: Vec::new() }
        }

        fn run(&mut self, seconds: f64, est: impl Fn(f64) -> TempoEstimate) {
            let end = self.t + seconds;
            while self.t < end {
                self.t += FRAME;
                let before = self.clock.beat_at(self.t);
                self.clock.apply_detection(&est(self.t), self.t);
                let after = self.clock.beat_at(self.t);
                self.jumps.push((self.t, after - before, self.clock.bpm));
            }
        }

        fn max_jump(&self) -> f64 {
            self.jumps.iter().map(|j| j.1.abs()).fold(0.0, f64::max)
        }

        /// The biggest phase shift over any one-beat window: the frames
        /// that together last at most one beat at the clock's tempo.
        fn max_shift_per_beat(&self) -> f64 {
            let mut worst: f64 = 0.0;
            for i in 0..self.jumps.len() {
                let (mut beats, mut sum) = (0.0, 0.0);
                for j in self.jumps[..=i].iter().rev() {
                    beats += FRAME * j.2 / 60.0;
                    if beats > 1.0 + 1e-9 {
                        break;
                    }
                    sum += j.1;
                }
                worst = worst.max(f64::abs(sum));
            }
            worst
        }

        /// Clock phase error against the grid `offset + n·60/bpm` (beats).
        fn phase_error(&self, bpm: f64, offset: f64) -> f64 {
            let spb = 60.0 / bpm;
            let beat = offset + ((self.t - offset) / spb).floor() * spb;
            wrap_half(self.clock.beat_at(beat))
        }
    }

    const CAP: f64 = 1.0 / 16.0;

    #[test]
    fn a_locked_estimate_brings_the_clock_to_the_music_without_jumps() {
        let mut sim = Sim::new(120.0);
        assert_eq!(sim.clock.state(0.0).follow, FollowState::Waiting);
        sim.run(1.0, locked(128.0, 0.37));
        assert_eq!(sim.clock.bpm, 120.0, "a tempo must hold 2 s before the clock takes it");
        assert_eq!(sim.clock.state(sim.t).follow, FollowState::Locked);
        sim.run(29.0, locked(128.0, 0.37));
        assert!((sim.clock.bpm - 128.0).abs() < 0.05, "bpm {}", sim.clock.bpm);
        let e = sim.phase_error(128.0, 0.37);
        assert!(e.abs() < 0.02, "phase error {e}");
        assert!(sim.max_jump() <= CAP + 1e-9, "jump {}", sim.max_jump());
        assert!(sim.max_shift_per_beat() <= CAP + 1e-9);
        assert_eq!(sim.clock.source, TempoSource::Audio);
    }

    #[test]
    fn a_quarter_beat_error_is_corrected_by_at_most_a_sixteenth_per_beat() {
        // Right tempo, phase a quarter beat off (just under, so it is
        // corrected at once rather than after 4 beats).
        for gain in [0.2, 1.0] {
            let mut sim = Sim::new(128.0);
            sim.clock.follow_config.phase_gain = gain;
            let offset = 0.2499 * 60.0 / 128.0;
            sim.run(20.0, locked(128.0, offset));
            let per_beat = sim.max_shift_per_beat();
            assert!(per_beat <= CAP + 1e-9, "gain {gain}: {per_beat} beats per beat");
            assert!(per_beat > 0.03, "gain {gain}: corrected {per_beat}");
            assert!(sim.phase_error(128.0, offset).abs() < 0.02, "gain {gain}: {}", sim.phase_error(128.0, offset));
            assert!((sim.clock.bpm - 128.0).abs() < 0.05, "gain {gain}: bpm {}", sim.clock.bpm);
        }
    }

    #[test]
    fn a_big_error_waits_four_beats_then_resyncs_at_the_capped_rate() {
        let mut sim = Sim::new(128.0);
        let spb = 60.0 / 128.0;
        let offset = 0.4 * spb;
        sim.run(3.0 * spb, locked(128.0, offset));
        assert_eq!(sim.clock.follow.resyncs, 0, "3 beats: still ignored");
        assert!(sim.max_jump() == 0.0);
        sim.run(20.0, locked(128.0, offset));
        assert_eq!(sim.clock.follow.resyncs, 1);
        assert!(sim.phase_error(128.0, offset).abs() < 0.02, "{}", sim.phase_error(128.0, offset));
        assert!(sim.max_shift_per_beat() <= CAP + 1e-9);
    }

    #[test]
    fn noisy_estimates_keep_the_shown_bpm_steady() {
        let mut sim = Sim::new(128.0);
        // ±1 BPM of noise, a new value every 0.5 s (a fixed LCG: repeatable).
        let noise = |t: f64| {
            let n = (t / 0.5).floor() as u64;
            let x = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) >> 33;
            (x % 2001) as f64 / 1000.0 - 1.0
        };
        let est = |t: f64| TempoEstimate { bpm: (128.0 + noise(t)) as f32, ..locked(128.0, 0.1)(t) };
        sim.run(10.0, est);
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for _ in 0..60 * 60 {
            sim.run(FRAME, est);
            lo = lo.min(sim.clock.bpm);
            hi = hi.max(sim.clock.bpm);
        }
        assert!(lo >= 127.7 && hi <= 128.3, "shown bpm {lo:.3}..{hi:.3}");
        assert!(sim.max_jump() <= CAP + 1e-9);
    }

    #[test]
    fn a_slow_drift_is_followed() {
        // The DJ's pitch creeps 128 → 129 over a minute.
        let mut sim = Sim::new(128.0);
        sim.run(5.0, locked(128.0, 0.0));
        for i in 0..60 {
            sim.run(1.0, locked(128.0 + i as f64 / 60.0, 0.0));
        }
        sim.run(10.0, locked(129.0, 0.0));
        assert!((sim.clock.bpm - 129.0).abs() < 0.3, "bpm {}", sim.clock.bpm);
        assert!(sim.max_jump() <= CAP + 1e-9);
    }

    #[test]
    fn an_octave_jump_needs_four_seconds() {
        let mut sim = Sim::new(128.0);
        sim.run(5.0, locked(128.0, 0.0));
        sim.run(3.0, locked(64.0, 0.0));
        assert!((sim.clock.bpm - 128.0).abs() < 0.1, "{}", sim.clock.bpm);
        sim.run(2.0, locked(64.0, 0.0));
        assert!((sim.clock.bpm - 64.0).abs() < 0.1, "{}", sim.clock.bpm);
        assert!(sim.max_jump() <= CAP + 1e-9);
    }

    #[test]
    fn unsure_estimates_never_move_the_clock() {
        let mut sim = Sim::new(120.0);
        sim.run(10.0, detector(140.0, 0.1, DetectState::Checking, 0.9));
        sim.run(10.0, detector(140.0, 0.1, DetectState::Guided, 0.9));
        sim.run(10.0, detector(140.0, 0.1, DetectState::Locked, 0.5));
        sim.run(10.0, |_| TempoEstimate { bpm: f32::NAN, confidence: f32::NAN, ..locked(140.0, 0.1)(0.0) });
        assert_eq!(sim.clock.bpm, 120.0);
        assert_eq!(sim.max_jump(), 0.0);
        assert_eq!(sim.clock.state(sim.t).follow, FollowState::Waiting);
    }

    #[test]
    fn coasting_keeps_the_beat_going_then_unlocks_after_30_s() {
        let mut sim = Sim::new(128.0);
        sim.run(10.0, locked(128.0, 0.2));
        let bpm = sim.clock.bpm;
        // The break: the detector coasts, then hears nothing.
        sim.run(5.0, detector(128.0, 0.2, DetectState::Coasting, 0.3));
        assert_eq!(sim.clock.state(sim.t).follow, FollowState::Coasting);
        sim.run(20.0, |_| TempoEstimate::default());
        assert_eq!(sim.clock.state(sim.t).follow, FollowState::Coasting);
        assert!((sim.clock.bpm - bpm).abs() < 1e-9, "the BPM is held");
        sim.run(6.0, |_| TempoEstimate::default());
        assert_eq!(sim.clock.state(sim.t).follow, FollowState::Unlocked);
        assert!((sim.clock.bpm - bpm).abs() < 1e-9);
        // beat_at kept running smoothly through all of it.
        let steps: Vec<f64> = sim.jumps.iter().map(|j| j.1).collect();
        assert!(steps.iter().all(|j| j.abs() <= CAP + 1e-9));
        let (t0, b0) = (sim.t, sim.clock.beat_at(sim.t));
        sim.run(1.0, |_| TempoEstimate::default());
        assert!((sim.clock.beat_at(sim.t) - b0 - (sim.t - t0) * bpm / 60.0).abs() < 1e-6);
        // A new lock after the timeout starts afresh.
        sim.run(20.0, locked(132.0, 0.0));
        assert!((sim.clock.bpm - 132.0).abs() < 0.1, "{}", sim.clock.bpm);
        assert_eq!(sim.clock.state(sim.t).follow, FollowState::Locked);
    }

    #[test]
    fn a_tap_takes_over_until_auto() {
        let mut sim = Sim::new(120.0);
        sim.run(10.0, locked(128.0, 0.0));
        assert!((sim.clock.bpm - 128.0).abs() < 0.1);
        let t = sim.t;
        sim.clock.tap(t);
        assert_eq!(sim.clock.source, TempoSource::Tap, "the first tap already wins");
        assert_eq!(sim.clock.state(t).follow, FollowState::Off);
        for i in 1..4 {
            sim.clock.tap(t + i as f64 * 0.5);
        }
        assert!((sim.clock.bpm - 120.0).abs() < 0.1);
        let (bpm, before) = (sim.clock.bpm, sim.jumps.len());
        sim.run(20.0, locked(140.0, 0.1));
        assert_eq!(sim.clock.source, TempoSource::Tap);
        assert_eq!(sim.clock.bpm, bpm, "detection ignored after a tap");
        assert!(sim.jumps[before..].iter().all(|j| j.1 == 0.0));
        sim.clock.set_auto(true);
        assert_eq!(sim.clock.source, TempoSource::Audio);
        sim.run(20.0, locked(140.0, 0.1));
        assert!((sim.clock.bpm - 140.0).abs() < 0.1);
        sim.clock.set_auto(false);
        assert_eq!((sim.clock.source, sim.clock.state(sim.t).follow), (TempoSource::Manual, FollowState::Off));
        // A manual BPM also leaves Tempo auto.
        sim.clock.set_auto(true);
        let t = sim.t;
        sim.clock.set_bpm_manual(100.0, t);
        assert_eq!(sim.clock.source, TempoSource::Manual);
    }

    #[test]
    fn guide_taps_guide_the_estimator_without_touching_the_clock() {
        let mut sim = Sim::new(128.0);
        sim.run(5.0, locked(128.0, 0.0));
        let (bpm, beat) = (sim.clock.bpm, sim.clock.beat_at(sim.t));
        let t = sim.t;
        assert_eq!(sim.clock.guide_tap(t), None);
        assert_eq!(sim.clock.guide_tap(t + 0.47), None);
        let g = sim.clock.guide_tap(t + 0.94).expect("a guide from the third tap");
        assert!((g - 127.66).abs() < 0.1, "{g}");
        assert_eq!(sim.clock.guide_bpm(), Some(g));
        assert_eq!(sim.clock.source, TempoSource::Audio);
        assert_eq!(sim.clock.bpm, bpm);
        assert_eq!(sim.clock.beat_at(t), beat);
        assert_eq!(sim.clock.state(t).guide_bpm, Some(g));
        sim.clock.new_track();
        assert_eq!(sim.clock.guide_bpm(), None);
        assert_eq!(sim.clock.source, TempoSource::Audio);
    }

    #[test]
    fn detection_is_ignored_unless_the_source_is_audio() {
        let mut c = TempoClock::default();
        for i in 0..600 {
            let t = i as f64 * FRAME;
            c.apply_detection(&locked(140.0, 0.1)(t), t);
        }
        assert_eq!((c.bpm, c.source, c.state(10.0).follow), (120.0, TempoSource::Manual, FollowState::Off));
    }

    #[test]
    fn a_full_scenario_never_jumps_more_than_a_sixteenth() {
        // Lock, noise, a break, a new track off-phase, a slower tempo.
        let mut sim = Sim::new(120.0);
        sim.run(15.0, locked(126.0, 0.3));
        sim.run(8.0, detector(126.0, 0.3, DetectState::Coasting, 0.35));
        sim.run(4.0, |_| TempoEstimate::default());
        sim.run(20.0, locked(126.0, 0.05));
        sim.run(20.0, locked(122.0, 0.4));
        assert!((sim.clock.bpm - 122.0).abs() < 0.1, "{}", sim.clock.bpm);
        assert!(sim.max_jump() <= CAP + 1e-9, "{}", sim.max_jump());
        let per_beat = sim.max_shift_per_beat();
        assert!(per_beat <= CAP + 1e-9, "{per_beat}");
    }

    // --- Temps 1 (T-235) ---

    /// A detection at 120 BPM whose "1" is the grid beat `one` (mod 4) of
    /// `n·0.5 s`, the last one at or before `t`.
    fn downbeat(t: f64, one: i64, confidence: f32) -> DownbeatDetection {
        let n = (t / 0.5).floor() as i64;
        let last = n - (n - one).rem_euclid(4);
        DownbeatDetection { downbeat_t: last as f64 * 0.5, confidence, bpm: 120.0, bars: 8, ..Default::default() }
    }

    /// Tempo auto locked at 120 BPM on the grid `n·0.5 s` (clock origin 0).
    fn locked_clock() -> (TempoClock, f64) {
        let mut c = TempoClock::default();
        c.set_auto(true);
        let est = locked(120.0, 0.0);
        let mut t = 0.0;
        while t < 3.0 {
            t += FRAME;
            c.apply_detection(&est(t), t);
        }
        assert_eq!(c.follow.state, FollowState::Locked);
        (c, t)
    }

    /// Runs the engine loop for `seconds`, the detection from `det(t)`.
    fn run_downbeat(c: &mut TempoClock, t: &mut f64, seconds: f64, det: impl Fn(f64) -> Option<DownbeatDetection>) {
        let est = locked(120.0, 0.0);
        let end = *t + seconds;
        while *t < end {
            *t += FRAME;
            c.apply_detection(&est(*t), *t);
            c.apply_downbeat(det(*t), *t);
        }
    }

    #[test]
    fn the_suggestion_is_shown_against_the_bar_and_accept_resyncs_on_it() {
        let mut c = TempoClock::default();
        c.apply_downbeat(Some(downbeat(10.6, 1, 0.7)), 10.6);
        let s = c.state(10.6);
        let e = s.downbeat.unwrap();
        assert_eq!((e.beat_in_bar_offset, e.confidence, s.downbeat_suggested), (1, 0.7, true));
        // Manual source: shown, never applied by itself.
        assert_eq!(c.downbeat.realigns, 0);
        assert!(c.accept_downbeat());
        let s = c.state(10.6);
        assert_eq!((s.downbeat.unwrap().beat_in_bar_offset, s.downbeat_suggested), (0, false));
        // The detected "1" is now a bar start, the beat phase unchanged.
        assert!((c.beat_at(10.5) / 4.0 - (c.beat_at(10.5) / 4.0).round()).abs() < 1e-9);
        assert!((c.beat_at(10.6) - c.beat_at(10.6).floor() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn no_suggestion_when_stale_unsure_at_another_tempo_or_outside_4_4() {
        let mut c = TempoClock::default();
        c.apply_downbeat(None, 1.0);
        assert!(c.state(1.0).downbeat.is_none());
        assert!(!c.accept_downbeat());
        c.apply_downbeat(Some(downbeat(10.6, 1, 0.2)), 10.6);
        assert!(!c.state(10.6).downbeat_suggested, "below 0.3: no suggestion");
        c.apply_downbeat(Some(DownbeatDetection { bpm: 140.0, ..downbeat(10.6, 1, 0.9) }), 10.6);
        assert!(c.state(10.6).downbeat.is_none());
        assert!(!c.accept_downbeat());
        c.beats_per_bar = 3;
        c.apply_downbeat(Some(downbeat(10.6, 1, 0.9)), 10.6);
        assert!(c.state(10.6).downbeat.is_none());
    }

    #[test]
    fn tempo_auto_realigns_once_when_the_confidence_rises() {
        let (mut c, mut t) = locked_clock();
        let start = t;
        // Unsure first, then sure: one realign, at the rise.
        run_downbeat(&mut c, &mut t, 10.0, |t| Some(downbeat(t, 3, if t - start < 5.0 { 0.2 } else { 0.8 })));
        assert_eq!(c.downbeat.realigns, 1);
        assert_eq!(c.state(t).downbeat.unwrap().beat_in_bar_offset, 0);
        // Still sure, the music's "1" moves (a wrong guess elsewhere): only
        // suggested, no realign without a new rise.
        run_downbeat(&mut c, &mut t, 20.0, |t| Some(downbeat(t, 0, 0.8)));
        assert_eq!(c.downbeat.realigns, 1);
        assert!(c.state(t).downbeat_suggested);
    }

    #[test]
    fn an_unaccented_groove_never_realigns() {
        let (mut c, mut t) = locked_clock();
        // Confidence wandering under 0.3 for two minutes, the "1" anywhere.
        run_downbeat(&mut c, &mut t, 120.0, |t| Some(downbeat(t, (t / 7.0) as i64 % 4, 0.1 + 0.15 * ((t * 0.7).sin() as f32 + 1.0) / 2.0)));
        assert_eq!(c.downbeat.realigns, 0);
        assert!(c.downbeat.last_realign_t.is_infinite());
    }

    #[test]
    fn a_drop_realigns_on_its_beat_even_unsure() {
        let (mut c, mut t) = locked_clock();
        let drop_at = 20.5; // grid beat 41: the clock's beat_in_bar 1
        let det = move |t: f64| {
            let mut d = downbeat(t, 0, 0.1);
            if t > drop_at + 0.2 {
                d.phrases = 1;
                d.phrase_start = Some(drop_at);
                d.phrase_from_drop = true;
            }
            Some(d)
        };
        let left = 30.0 - t;
        run_downbeat(&mut c, &mut t, left, det);
        assert_eq!(c.downbeat.realigns, 1);
        let b = c.beat_at(drop_at);
        assert!((b / 4.0 - (b / 4.0).round()).abs() < 1e-3, "the drop is a 1: {b}");
        // Phrase: counted from the drop.
        let s = c.state(drop_at + 4.0 * 2.0 + 0.1);
        assert_eq!((s.phrase_bar, s.phrase_bars), (Some(4), 8));
        // Not under Tempo auto: a drop only updates the display.
        let mut m = TempoClock::default();
        m.apply_downbeat(det(20.0), 20.0);
        m.apply_downbeat(det(21.0), 21.0);
        assert_eq!(m.downbeat.realigns, 0);
    }

    #[test]
    fn realigns_are_at_least_8_s_apart() {
        let (mut c, mut t) = locked_clock();
        // A drop every 2 s (a detector gone wrong), each on another beat.
        run_downbeat(&mut c, &mut t, 30.0, |t| {
            let k = (t / 2.0).floor() as u64;
            let at = k as f64 * 2.0 + 0.5 * (k % 3) as f64;
            Some(DownbeatDetection { phrases: k, phrase_start: Some(at), phrase_from_drop: true, ..downbeat(t, 0, 0.1) })
        });
        assert!(c.downbeat.realigns <= 4, "{}", c.downbeat.realigns);
    }

    #[test]
    fn a_realign_moves_whole_beats_only() {
        let (mut c, mut t) = locked_clock();
        let before = c.beat_at(t) - c.beat_at(t).floor();
        run_downbeat(&mut c, &mut t, 6.0, |t| Some(downbeat(t, 2, 0.9)));
        assert_eq!(c.downbeat.realigns, 1);
        let after = c.beat_at(t) - c.beat_at(t).floor();
        assert!((after - before).abs() < 0.02, "phase {before} → {after}");
    }
}
