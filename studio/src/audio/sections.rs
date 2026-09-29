//! Musical sections (T-236): *silence*, *normal*, *break*, *build-up* and
//! *drop*, on the analysis thread, from what the other analysers already
//! give every hop (band levels and centroid of `spectrum.rs`, kicks and
//! onsets of `onsets.rs`, the beat period and grid of `bpm.rs`). Our own
//! heuristics after docs/research/audio-analysis.md § 2.6; no source read.
//!
//! - **Trend frames** of ~20 ms: the mean power of the low bands
//!   (20–150 Hz), of the rest (150 Hz–12 kHz) and of the high band
//!   (2–12 kHz), the mean log-centroid, the onsets and kicks counted. The
//!   last 8 s are kept in a ring. Windows are counted in **beats** (the
//!   detected tempo, 125 BPM until there is one, or the groove's kick
//!   spacing when longer): the *short* window is the last beat, the *long*
//!   one the 4 beats before it. Silence is left out of them.
//! - **References**: the low and rest levels of the groove (a one-beat
//!   mean), averaged with a time constant of 8 beats (≈ a 16-beat mean),
//!   and only while the section is *normal* or *drop*: a break must not
//!   become the new normal. Nothing but *normal* is reported before
//!   8 beats of groove have been heard.
//! - **Break**: the last beat's low level ≥ 10 dB under its reference,
//!   the rest ≤ 6 dB under its own (the overall level is held: a global
//!   fader move lowers both and is not a break) and no kick for 1.75 beats
//!   (a kick expected and missed; the research note says 2 beats, which
//!   would make the detection always a full beat late). Its start is put
//!   one beat after the last kick.
//! - **Build-up**: rising evidence over the last beat against the 4
//!   before, held for ~80 ms (the centroid up 0.2 octave with the high
//!   band up 2 dB, the high band up 4 dB, or 2 more onsets per beat: a
//!   snare roll), with the low band still ≥ 6 dB down (from a break, or
//!   from a groove whose low end was filtered out). `buildup`
//!   0..1 is how far the high band (18 dB), the centroid (2 octaves) and
//!   the onset rate (6 per beat) have climbed since it started, held at its
//!   maximum (a riser only climbs). It falls back to a break when the high
//!   band falls 6 dB from its peak while the rest of the mix holds (not
//!   when everything fades: that is a blackout coming).
//! - **Drop**, checked over the 100 ms after each kick during a break or
//!   a build-up that has lasted ≥ 2 beats: the low band (30 ms smoothing)
//!   ≥ 8 dB over its peak of the beat before (peaks against peaks: a
//!   kick fading back in a little louder each beat is no drop) and ≤ 10 dB
//!   under the groove reference. The kick rules (missed kick, kicks back,
//!   the beat before a drop) count in the groove's measured kick spacing,
//!   not in beats: a tempo read an octave off must not fake a missed kick. Then
//!   `drop` counts one, `last_drop_t` is the kick's time, snapped onto the
//!   beat grid when the tempo is locked or coasting (if within ¼ beat),
//!   and the section is *drop* for 16 beats, then *normal*. Kicks that
//!   come back without that jump end the section as *normal*, no drop: a
//!   false drop is worse than a missed one.
//! - **Silence**: the spectrum's `silent` (300 ms under the threshold).
//!   A silence shorter than 4 beats inside a break or a build-up (the
//!   blackout before the drop) returns to that section; after 3 s the
//!   references are forgotten (a new track).
//!
//! Detection only: nothing here changes the laser (T-240 will react to
//! it). Every buffer is made in `new`: a hop allocates nothing.

use super::analysis::to_db;
use super::bpm::{DetectState, TempoEstimate};
use super::onsets::Onsets;
use super::spectrum::{SpectralFrame, SILENCE_HOLD_S};
use crate::engine::Section;
use serde::{Serialize, Serializer};

/// Trend frame length aimed at (s).
const FRAME_S: f64 = 0.02;
/// History kept (s): 5 beats at 40 BPM.
const RING_S: f64 = 8.0;
/// Beat period used while the tempo is unknown, and the range trusted.
const DEFAULT_BPM: f32 = 125.0;
const MIN_BPM: f32 = 60.0;
const MAX_BPM: f32 = 200.0;
/// The long window: this many beats before the last one.
const LONG_BEATS: f64 = 4.0;
/// Groove references: time constant, and the groove needed before
/// anything but *normal* is reported (beats).
const REF_TAU_BEATS: f64 = 8.0;
const WARMUP_BEATS: f64 = 8.0;
/// Break: low band this far under its reference, the rest at most this
/// far under its own, no kick for this many beats.
const BREAK_LOW_DB: f32 = 10.0;
const HELD_REST_DB: f32 = 6.0;
const KICK_GAP_BEATS: f64 = 1.75;
/// Kick spacings trusted for the kick period (s), and its smoothing.
const KICK_SPACING_S: (f64, f64) = (0.25, 2.0);
const KICK_PERIOD_K: f64 = 0.2;
/// Build-up: the low band still this far down, and one of these rises
/// (last beat against the 4 before).
const BUILD_LOW_DB: f32 = 6.0;
const RISE_CENTROID_OCT: f32 = 0.2;
const RISE_HIGH_DB: f32 = 4.0;
const RISE_ONSETS: f32 = 2.0;
/// A centroid rise counts only with at least this much more high band
/// (a kick leaving the mix raises the centroid of what stays).
const RISE_HIGH_WITH_CENTROID_DB: f32 = 2.0;
/// The build-up value: full at these climbs.
const BUILD_FULL_HIGH_DB: f32 = 18.0;
const BUILD_FULL_OCT: f32 = 2.0;
const BUILD_FULL_ONSETS: f32 = 6.0;
/// A build-up whose high band falls this far from its peak is a break,
/// unless the newest frame is this far under the groove (a blackout).
const BUILD_FALL_DB: f32 = 6.0;
const FADING_OUT_DB: f32 = 20.0;
/// Drop: the low band rises this much over the beat before, and comes
/// back within this of the groove reference.
const DROP_RISE_DB: f32 = 8.0;
const DROP_NEAR_REF_DB: f32 = 10.0;
/// A break or build-up must have lasted this long before a drop, and a
/// drop before anything else (beats).
const MIN_SECTION_BEATS: f64 = 2.0;
/// Rising evidence must hold this many trend frames in a row (~80 ms:
/// longer than a kick takes to be reported, so the hats of a returning
/// groove don't read as a build-up just before its drop).
const RISING_FRAMES: u32 = 4;
/// *Drop* lasts this long, then *normal* (beats).
const DROP_HOLD_BEATS: f64 = 16.0;
/// Snap the drop onto the beat grid when it is this close (beats).
const SNAP_BEATS: f64 = 0.25;
/// Smoothing of the low band for the drop test (s), and how long after
/// a kick the jump may take to show (the band filters lag the onset).
const LOW_FAST_TAU_S: f64 = 0.03;
const DROP_WINDOW_S: f64 = 0.1;
/// A silence shorter than this (beats) keeps a break or build-up going.
const SHORT_SILENCE_BEATS: f64 = 4.0;
/// A silence longer than this (s) forgets the references.
const FORGET_SILENCE_S: f64 = 3.0;
pub const HISTORY_LEN: usize = 5;

/// One change of section: its (estimated) start, audio time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct SectionChange {
    pub t: f64,
    pub section: Section,
}

/// The last changes, newest first; serialised as a list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct History {
    items: [SectionChange; HISTORY_LEN],
    len: usize,
}

impl History {
    fn push(&mut self, change: SectionChange) {
        self.items.copy_within(0..HISTORY_LEN - 1, 1);
        self.items[0] = change;
        self.len = (self.len + 1).min(HISTORY_LEN);
    }

    pub fn as_slice(&self) -> &[SectionChange] {
        &self.items[..self.len]
    }
}

impl Serialize for History {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.as_slice())
    }
}

/// What the detector says (`/api/state.audio.sections`; the engine gets
/// `section`, `buildup` and `drop` in `AudioFeatures`). Times are audio
/// times on the studio clock (s).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SectionState {
    pub section: Section,
    /// How clearly the current section's criteria hold, 0..1.
    pub confidence: f32,
    /// Build-up progress 0..1 (0 outside a build-up).
    pub buildup: f32,
    /// Drops detected (a counter: an event is a change).
    pub drop: u64,
    /// Time of the last drop (on the beat grid when the tempo is locked).
    pub last_drop_t: f64,
    /// Estimated start of the current section.
    pub start_t: f64,
    /// Seconds since that start.
    pub since_s: f32,
    /// The last 5 changes, newest first.
    pub history: History,
}

impl Default for SectionState {
    /// Nothing heard yet: silence.
    fn default() -> Self {
        Self { section: Section::Silence, confidence: 0.0, buildup: 0.0, drop: 0, last_drop_t: 0.0, start_t: 0.0, since_s: 0.0, history: History::default() }
    }
}

fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

fn power(db: f32) -> f32 {
    if db.is_finite() {
        10f32.powf(db / 10.0)
    } else {
        0.0
    }
}

/// One trend frame: mean powers, mean log2 centroid, counts.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    low: f32,
    /// Peak of the fast low level (the drop compares peaks with peaks).
    low_peak: f32,
    rest: f32,
    high: f32,
    cent: f32,
    onsets: f32,
    kicks: f32,
}

pub struct SectionDetector {
    hop_s: f64,
    frame_hops: usize,
    frame_s: f64,
    ring: Vec<Frame>,
    /// Frames pushed since the last reset.
    frames: usize,
    acc: Frame,
    acc_n: usize,
    acc_cent_n: usize,
    last_cent: f32,
    prev_onset: Option<u64>,
    prev_kick: Option<u64>,
    last_kick_t: f64,
    /// The groove's kick spacing (s), learnt from its kicks; the kick
    /// rules count in it rather than in beats (a tempo read an octave off,
    /// or a kick every other beat, must not fake a missed kick).
    kick_period: Option<f64>,
    low_fast: f32,
    /// Groove references (dB) and how much groove they have seen (s).
    low_ref: f32,
    rest_ref: f32,
    ref_s: f64,
    ref_n: u32,
    /// The current section's entry levels (long window), peak high band.
    entry_high: f32,
    entry_cent: f32,
    high_max: f32,
    /// Start of the current run of break / build-up (a drop needs 2 beats of it).
    quiet_since: f64,
    /// A kick in a break or build-up, being checked for a drop: its
    /// time, the low level of the beat before it (dB), the deadline.
    pending_drop: Option<(f64, f32, f64)>,
    /// Consecutive trend frames with rising evidence.
    rising_n: u32,
    /// Before the current silence: the section, its start, the silence's
    /// start, the run's start.
    before_silence: Option<(Section, f64, f64, f64)>,
    out: SectionState,
}

impl SectionDetector {
    pub fn new(sample_rate: u32, hop: usize) -> Self {
        let hop_s = hop.max(1) as f64 / sample_rate.max(1) as f64;
        let frame_hops = (FRAME_S / hop_s).round().max(1.0) as usize;
        let frame_s = frame_hops as f64 * hop_s;
        let cap = (RING_S / frame_s).ceil() as usize + 1;
        Self {
            hop_s,
            frame_hops,
            frame_s,
            ring: vec![Frame::default(); cap],
            frames: 0,
            acc: Frame::default(),
            acc_n: 0,
            acc_cent_n: 0,
            last_cent: 0.0,
            prev_onset: None,
            prev_kick: None,
            last_kick_t: f64::NEG_INFINITY,
            kick_period: None,
            low_fast: 0.0,
            low_ref: 0.0,
            rest_ref: 0.0,
            ref_s: 0.0,
            ref_n: 0,
            entry_high: 0.0,
            entry_cent: 0.0,
            high_max: 0.0,
            quiet_since: 0.0,
            pending_drop: None,
            rising_n: 0,
            before_silence: None,
            out: SectionState::default(),
        }
    }

    pub fn state(&self) -> SectionState {
        self.out
    }

    /// Continues the drop counter and history of an earlier stream (a
    /// reopened input must not look like a new drop).
    pub fn carry(&mut self, s: &SectionState) {
        self.out.drop = s.drop;
        self.out.last_drop_t = s.last_drop_t;
        self.out.history = s.history;
    }

    /// Forgets the references and the trends (a new track); the drop
    /// counter and the history stay.
    pub fn forget(&mut self) {
        self.forget_trends();
        if matches!(self.out.section, Section::Break | Section::Buildup | Section::Drop) {
            let t = self.out.start_t + self.out.since_s as f64;
            self.set(Section::Normal, t, 0.0);
        }
    }

    fn period(tempo: &TempoEstimate) -> f64 {
        let bpm = if tempo.bpm.is_finite() && tempo.bpm > 0.0 { tempo.bpm.clamp(MIN_BPM, MAX_BPM) } else { DEFAULT_BPM };
        60.0 / bpm as f64
    }

    fn set(&mut self, section: Section, start_t: f64, confidence: f32) {
        let quiet = |s| matches!(s, Section::Break | Section::Buildup);
        if quiet(section) && !quiet(self.out.section) && self.out.section != Section::Silence {
            self.quiet_since = start_t;
        }
        if section != self.out.section {
            self.out.history.push(SectionChange { t: start_t, section });
        }
        self.out.section = section;
        self.out.start_t = start_t;
        self.out.confidence = confidence;
        if section != Section::Buildup {
            self.out.buildup = 0.0;
        }
    }

    /// One hop: the spectral frame, onsets and tempo of this hop, ending at `t`.
    pub fn process(&mut self, s: &SpectralFrame, o: &Onsets, tempo: &TempoEstimate, t: f64) -> SectionState {
        // The beat: the tempo's, or the kick spacing when that is longer
        // (a groove with loud eighths reads double tempo).
        let p = Self::period(tempo).max(self.kick_period.unwrap_or(0.0));
        let b = s.bands_db;
        let low = power(b[0]) + power(b[1]);
        let rest = power(b[2]) + power(b[3]) + power(b[4]);
        let high = power(b[4]);
        let new_onsets = self.prev_onset.map_or(0, |v| o.onset.saturating_sub(v));
        let new_kicks = self.prev_kick.map_or(0, |v| o.kick.saturating_sub(v));
        self.prev_onset = Some(o.onset);
        self.prev_kick = Some(o.kick);
        if new_kicks > 0 && o.last_kick_t.is_finite() {
            let spacing = o.last_kick_t - self.last_kick_t;
            if matches!(self.out.section, Section::Normal | Section::Drop) && (KICK_SPACING_S.0..=KICK_SPACING_S.1).contains(&spacing) {
                let kp = self.kick_period.unwrap_or(spacing);
                self.kick_period = Some(kp + (spacing - kp) * KICK_PERIOD_K);
            }
            self.last_kick_t = o.last_kick_t;
        }
        let k = 1.0 - (-self.hop_s / LOW_FAST_TAU_S).exp() as f32;
        self.low_fast += (low - self.low_fast) * k;
        if !self.low_fast.is_finite() {
            self.low_fast = 0.0;
        }

        if s.silent {
            self.silent(t);
        } else {
            if self.out.section == Section::Silence {
                self.sound_back(t, p);
            }
            self.acc.low += low;
            self.acc.low_peak = self.acc.low_peak.max(self.low_fast);
            self.acc.rest += rest;
            self.acc.high += high;
            if s.centroid_hz.is_finite() && s.centroid_hz > 1.0 {
                self.acc.cent += s.centroid_hz.log2();
                self.acc_cent_n += 1;
            }
            self.acc.onsets += new_onsets as f32;
            self.acc.kicks += new_kicks as f32;
            self.acc_n += 1;
            if new_kicks > 0 {
                self.kick_in_quiet(p, self.kick_period.unwrap_or(p), t);
            }
            self.check_drop(tempo, t);
            if self.acc_n >= self.frame_hops {
                self.push_frame();
                self.update(t, p);
            }
        }
        self.out.since_s = (t - self.out.start_t).max(0.0) as f32;
        self.out
    }

    fn silent(&mut self, t: f64) {
        match self.before_silence {
            None => {
                let start = t - SILENCE_HOLD_S as f64;
                if self.out.section != Section::Silence {
                    self.before_silence = Some((self.out.section, self.out.start_t, start, self.quiet_since));
                    // The hold before `silent` was silence too: out of the trends.
                    let held = (SILENCE_HOLD_S as f64 / self.frame_s).round() as usize;
                    self.frames = self.frames.saturating_sub(held);
                    self.acc = Frame::default();
                    self.acc_n = 0;
                    self.acc_cent_n = 0;
                }
                self.set(Section::Silence, start.max(0.0), 1.0);
            }
            Some((_, _, since, _)) if t - since > FORGET_SILENCE_S && self.frames > 0 => self.forget_trends(),
            Some(_) => {}
        }
    }

    /// References and trends only (also inside a long silence).
    fn forget_trends(&mut self) {
        self.frames = 0;
        self.acc = Frame::default();
        self.acc_n = 0;
        self.acc_cent_n = 0;
        self.ref_s = 0.0;
        self.ref_n = 0;
        self.last_kick_t = f64::NEG_INFINITY;
        self.pending_drop = None;
        self.kick_period = None;
        if let Some(b) = self.before_silence.as_mut() {
            b.0 = Section::Normal;
        }
    }

    fn sound_back(&mut self, t: f64, p: f64) {
        let before = self.before_silence.take();
        match before {
            Some((section @ (Section::Break | Section::Buildup), start, since, quiet_since)) if t - since < SHORT_SILENCE_BEATS * p => {
                // The blackout before a drop: the section goes on.
                self.set(section, start, 0.5);
                self.quiet_since = quiet_since;
            }
            _ => self.set(Section::Normal, t, 0.0),
        }
    }

    fn push_frame(&mut self) {
        let n = self.acc_n as f32;
        if self.acc_cent_n > 0 {
            self.last_cent = self.acc.cent / self.acc_cent_n as f32;
        }
        let f = Frame { low: self.acc.low / n, low_peak: self.acc.low_peak, rest: self.acc.rest / n, high: self.acc.high / n, cent: self.last_cent, onsets: self.acc.onsets, kicks: self.acc.kicks };
        let cap = self.ring.len();
        self.ring[self.frames % cap] = f;
        self.frames += 1;
        self.acc = Frame::default();
        self.acc_n = 0;
        self.acc_cent_n = 0;
    }

    /// Mean (or sum) of `field` over the frames `back .. back + len` before
    /// the newest (0 = the newest); `None` if none is there.
    fn window(&self, back: usize, len: usize, sum: bool, field: impl Fn(&Frame) -> f32) -> Option<f32> {
        let cap = self.ring.len();
        let avail = self.frames.min(cap);
        let end = (back + len).min(avail);
        if back >= end {
            return None;
        }
        let total: f32 = (back..end).map(|j| field(&self.ring[(self.frames - 1 - j) % cap])).sum();
        Some(if sum { total } else { total / (end - back) as f32 })
    }

    /// Maximum of `field` over the same frames as `window`.
    fn window_max(&self, back: usize, len: usize, field: impl Fn(&Frame) -> f32) -> Option<f32> {
        let cap = self.ring.len();
        let end = (back + len).min(self.frames.min(cap));
        (back..end).map(|j| field(&self.ring[(self.frames - 1 - j) % cap])).reduce(f32::max)
    }

    fn frames_per(&self, beats: f64, p: f64) -> usize {
        ((beats * p / self.frame_s).round() as usize).max(1)
    }

    /// A kick during a break or a build-up: a drop to check for, over the
    /// next `DROP_WINDOW_S`.
    fn kick_in_quiet(&mut self, p: f64, kp: f64, t: f64) {
        if !matches!(self.out.section, Section::Break | Section::Buildup) || t - self.quiet_since < MIN_SECTION_BEATS * p {
            return;
        }
        let nb = self.frames_per(1.0, kp);
        let back = self.frames_per(0.25, kp);
        if let Some(prev_peak) = self.window_max(back, nb, |f| f.low_peak) {
            self.pending_drop = Some((self.last_kick_t.min(t), to_db(prev_peak), t + DROP_WINDOW_S));
        }
    }

    fn check_drop(&mut self, tempo: &TempoEstimate, t: f64) {
        let Some((kick_t, prev, deadline)) = self.pending_drop else { return };
        if !matches!(self.out.section, Section::Break | Section::Buildup) || t > deadline {
            self.pending_drop = None;
            return;
        }
        let now = to_db(self.low_fast);
        let rise = now - prev;
        if rise < DROP_RISE_DB || self.low_ref - now > DROP_NEAR_REF_DB {
            return;
        }
        self.pending_drop = None;
        let mut at = kick_t;
        if matches!(tempo.state, DetectState::Locked | DetectState::Coasting) && tempo.bpm > 0.0 && tempo.next_beat > 0.0 {
            let pt = 60.0 / tempo.bpm as f64;
            let grid = tempo.next_beat + ((at - tempo.next_beat) / pt).round() * pt;
            if (grid - at).abs() <= SNAP_BEATS * pt {
                at = grid;
            }
        }
        self.out.drop += 1;
        self.out.last_drop_t = at;
        self.set(Section::Drop, at, 0.5 + 0.5 * ramp(rise, DROP_RISE_DB, 2.0 * DROP_RISE_DB + 4.0));
    }

    /// Once per trend frame.
    fn update(&mut self, t: f64, p: f64) {
        let nb = self.frames_per(1.0, p);
        let nl = self.frames_per(LONG_BEATS, p);
        let db_mean = |d: &Self, back, len, field: fn(&Frame) -> f32| d.window(back, len, false, field).map(to_db);
        let (Some(low_beat), Some(rest_beat), Some(high_s), Some(cent_s), Some(ons_s)) = (
            db_mean(self, 0, nb, |f| f.low),
            db_mean(self, 0, nb, |f| f.rest),
            db_mean(self, 0, nb, |f| f.high),
            self.window(0, nb, false, |f| f.cent),
            self.window(0, nb, true, |f| f.onsets),
        ) else {
            return;
        };
        let high_l = db_mean(self, nb, nl, |f| f.high).unwrap_or(high_s);
        let cent_l = self.window(nb, nl, false, |f| f.cent).unwrap_or(cent_s);
        let long_frames = self.frames.min(self.ring.len()).saturating_sub(nb).min(nl);
        let ons_l = if long_frames > 0 { self.window(nb, nl, true, |f| f.onsets).unwrap_or(0.0) * nb as f32 / long_frames as f32 } else { ons_s };
        let kp = self.kick_period.unwrap_or(p);
        let kicks_2 = self.window(0, self.frames_per(2.25, kp), true, |f| f.kicks).unwrap_or(0.0);

        let section = self.out.section;
        let settling = section == Section::Drop && t - self.out.start_t < MIN_SECTION_BEATS * p;
        if section == Section::Normal || (section == Section::Drop && !settling) {
            self.ref_n = self.ref_n.saturating_add(1);
            let k = (self.frame_s / (REF_TAU_BEATS * p)).max(1.0 / self.ref_n as f64) as f32;
            self.low_ref += (low_beat - self.low_ref) * k;
            self.rest_ref += (rest_beat - self.rest_ref) * k;
            self.ref_s += self.frame_s;
        }
        if self.pending_drop.is_some() {
            // A kick being checked for a drop: no other change meanwhile.
            return;
        }
        let warm = self.ref_s >= WARMUP_BEATS * p;
        // The low band's fall beyond the whole mix's: a fader move is none.
        let rest_drop = self.rest_ref - rest_beat;
        let low_drop = self.low_ref - low_beat - rest_drop.max(0.0);
        let rest_held = rest_drop <= HELD_REST_DB;
        // The newest frame far under the groove: a fade to silence (the
        // blackout before a drop) is starting, not a section change.
        let fading_out = self.window(0, 1, false, |f| f.rest).is_some_and(|r| self.rest_ref - to_db(r) > FADING_OUT_DB);
        let (dh, dc, dons) = (high_s - high_l, cent_s - cent_l, ons_s - ons_l);
        let rising_now = (dc >= RISE_CENTROID_OCT && dh >= RISE_HIGH_WITH_CENTROID_DB) || (dh >= RISE_HIGH_DB && dc >= -0.05) || (dons >= RISE_ONSETS && dh >= 0.0);
        self.rising_n = if rising_now { self.rising_n.saturating_add(1) } else { 0 };
        let rising = self.rising_n >= RISING_FRAMES;
        let kick_gap = t - self.last_kick_t;
        let kicks_back = kick_gap < KICK_GAP_BEATS * kp && kicks_2 >= 2.0 && low_drop < BUILD_LOW_DB;

        match section {
            Section::Normal | Section::Drop => {
                if section == Section::Drop && t - self.out.start_t >= DROP_HOLD_BEATS * p {
                    self.set(Section::Normal, t, 1.0);
                }
                if section == Section::Normal {
                    self.out.confidence = ramp(self.ref_s as f32, 0.0, (WARMUP_BEATS * p) as f32);
                }
                if !warm || !rest_held || settling {
                    return;
                }
                if low_drop >= BREAK_LOW_DB && kick_gap >= KICK_GAP_BEATS * kp {
                    let start = (self.last_kick_t + kp).min(t).max(self.out.start_t.min(t));
                    self.set(Section::Break, start, 0.5 + 0.5 * ramp(low_drop, BREAK_LOW_DB, 2.0 * BREAK_LOW_DB));
                } else if low_drop >= BUILD_LOW_DB && rising {
                    self.enter_buildup(t, high_l, cent_l);
                }
            }
            Section::Break => {
                if rising {
                    self.enter_buildup(t, high_l, cent_l);
                } else if kicks_back {
                    self.set(Section::Normal, t, 0.5);
                } else {
                    self.out.confidence = 0.5 + 0.5 * ramp(low_drop, BREAK_LOW_DB, 2.0 * BREAK_LOW_DB);
                }
            }
            Section::Buildup => {
                self.high_max = self.high_max.max(high_s);
                if kicks_back {
                    self.set(Section::Normal, t, 0.5);
                } else if high_s <= self.high_max - BUILD_FALL_DB && dons < RISE_ONSETS && rest_held && !fading_out {
                    self.set(Section::Break, t, 0.5);
                } else {
                    let raw = (ramp(high_s - self.entry_high, 0.0, BUILD_FULL_HIGH_DB) + ramp(cent_s - self.entry_cent, 0.0, BUILD_FULL_OCT) + ramp(ons_s, 0.0, BUILD_FULL_ONSETS)) / 3.0;
                    self.out.buildup = self.out.buildup.max(raw);
                    let evidence = ramp(dh, 0.0, RISE_HIGH_DB).max(ramp(dc, 0.0, RISE_CENTROID_OCT)).max(ramp(dons, 0.0, RISE_ONSETS));
                    self.out.confidence = 0.5 + 0.25 * evidence + 0.25 * self.out.buildup;
                }
            }
            Section::Silence => {}
        }
    }

    fn enter_buildup(&mut self, t: f64, high_l: f32, cent_l: f32) {
        self.entry_high = high_l;
        self.entry_cent = cent_l;
        self.high_max = high_l;
        self.set(Section::Buildup, t, 0.5);
    }
}

#[cfg(test)]
mod tests {
    use super::super::analysis::{Analyzer, HOP};
    use super::super::onsets::tests::{noise, render_at, Hit};
    use super::*;
    use std::f32::consts::PI;

    const RATE: u32 = 48_000;
    thread_local! {
        /// The test tempo (each test thread its own).
        static BPM: std::cell::Cell<f64> = const { std::cell::Cell::new(128.0) };
    }

    /// The beat period of the test tempo.
    fn p() -> f64 {
        60.0 / BPM.with(|b| b.get())
    }

    /// Runs `test` at 90, 128 and 174 BPM (90 reads 180 on the tempo
    /// detector with its eighth hats: the kick spacing is the beat there).
    fn at_tempos(test: fn()) {
        for bpm in [90.0, 128.0, 174.0] {
            BPM.with(|b| b.set(bpm));
            eprintln!("at {bpm} BPM"); // shown with a failure
            test();
        }
    }
    /// The first beat.
    const T0: f64 = 0.1;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Part {
        /// Kick, snare on 2 and 4, eighth hats, a 55 Hz bass, the pad; `gain`.
        Groove(f32),
        /// The pad alone.
        Break,
        /// The pad, a riser (noise high-passed at 1 kHz, low-passed from
        /// 1.5 to 12 kHz, louder and louder) and a snare roll (quarters,
        /// eighths, sixteenths).
        Buildup,
        /// Digital silence.
        Silence,
    }

    /// Four notes of a held chord (A, C#, E, A), 0.06 each.
    fn pad(i: usize) -> f32 {
        let t = i as f32 / RATE as f32;
        [220.0, 277.18, 329.63, 440.0].iter().map(|f| 0.06 * (2.0 * PI * f * t).sin()).sum()
    }

    /// Renders the parts (length in beats) back to back from `T0`, then 1 s
    /// of the pad alone (a break, for the detector); returns the signal and
    /// each part's start time, then the end of the last part.
    fn arrange(parts: &[(Part, usize)]) -> (Vec<f32>, Vec<f64>) {
        let beats: usize = parts.iter().map(|p| p.1).sum();
        let seconds = (T0 + beats as f64 * p() + 1.0) as f32;
        let mut starts = Vec::new();
        let mut hits = Vec::new();
        let mut b0 = 0;
        for &(part, n) in parts {
            let s = T0 + b0 as f64 * p();
            starts.push(s);
            for k in 0..n {
                let t = s + k as f64 * p();
                match part {
                    Part::Groove(_) => {
                        hits.push((t, Hit::Kick));
                        if (b0 + k) % 2 == 1 {
                            hits.push((t, Hit::Snare));
                        }
                        hits.push((t, Hit::Hat));
                        hits.push((t + p() / 2.0, Hit::Hat));
                    }
                    Part::Buildup => {
                        let per = if k < n / 2 { 1 } else if k < 3 * n / 4 { 2 } else { 4 };
                        for j in 0..per {
                            hits.push((t + j as f64 * p() / per as f64, Hit::Snare));
                        }
                    }
                    _ => {}
                }
            }
            b0 += n;
        }
        starts.push(T0 + b0 as f64 * p());
        let part_at = |i: usize| -> Option<(Part, f64, f64)> {
            let t = i as f64 / RATE as f64;
            let mut b0 = 0;
            for &(part, n) in parts {
                let (s, e) = (T0 + b0 as f64 * p(), T0 + (b0 + n) as f64 * p());
                if t < e {
                    return Some((part, s, e));
                }
                b0 += n;
            }
            None
        };
        let mut signal = render_at(RATE, seconds, &hits, |i| {
            let bed = 0.003 * noise(i, 7);
            match part_at(i) {
                Some((Part::Groove(_), ..)) => bed + pad(i) + 0.12 * (2.0 * PI * 55.0 * i as f32 / RATE as f32).sin(),
                Some((Part::Silence, ..)) => 0.0,
                _ => bed + pad(i),
            }
        });
        // The riser (stateful filters) and the gains.
        let (mut hp_prev, mut hp, mut lp) = (0.0f32, 0.0f32, 0.0f32);
        let a_hp = (-2.0 * PI * 1_000.0 / RATE as f32).exp();
        for (i, x) in signal.iter_mut().enumerate() {
            match part_at(i) {
                Some((Part::Buildup, s, e)) => {
                    let frac = ((i as f64 / RATE as f64 - s) / (e - s)) as f32;
                    let n = noise(i, 11);
                    hp = a_hp * (hp + n - hp_prev);
                    hp_prev = n;
                    let fc = 1_500.0 * 8f32.powf(frac);
                    lp += (hp - lp) * (1.0 - (-2.0 * PI * fc / RATE as f32).exp());
                    *x += (0.05 + 0.5 * frac) * lp;
                }
                Some((Part::Groove(g), ..)) => *x *= g,
                Some((Part::Silence, ..)) => *x = 0.0,
                _ => {}
            }
        }
        (signal, starts)
    }

    fn detect(signal: &[f32]) -> Vec<(f64, SectionState)> {
        let mut a = Analyzer::new(RATE);
        signal
            .as_chunks::<HOP>()
            .0
            .iter()
            .enumerate()
            .map(|(h, hop)| {
                let t = ((h + 1) * HOP) as f64 / RATE as f64;
                let m = a.process(hop, t);
                // The engine gets the same (T-237's fields).
                assert_eq!((m.features.section, m.features.buildup, m.features.drop), (m.sections.section, m.sections.buildup, m.sections.drop));
                (t, m.sections)
            })
            .collect()
    }

    /// The first time `section` is reported at or after `from`.
    fn first(states: &[(f64, SectionState)], section: Section, from: f64) -> Option<f64> {
        states.iter().find(|(t, s)| *t >= from && s.section == section).map(|e| e.0)
    }

    fn sections_between(states: &[(f64, SectionState)], from: f64, to: f64) -> Vec<Section> {
        let mut v: Vec<Section> = Vec::new();
        for (_, s) in states.iter().filter(|(t, _)| *t >= from && *t < to) {
            if v.last() != Some(&s.section) {
                v.push(s.section);
            }
        }
        v
    }

    /// A little slack over a beat: the end of the hop and of the trend frame.
    fn late() -> f64 {
        p() + 0.03
    }

    /// Acceptance: 16 beats of groove → 8 of break → 8 of build-up → drop:
    /// each section within a beat of its start, the drop within a beat.
    #[test]
    fn the_arrangement_is_followed_beat_by_beat() {
        at_tempos(the_arrangement_is_followed_beat_by_beat_at);
    }

    fn the_arrangement_is_followed_beat_by_beat_at() {
        let (signal, starts) = arrange(&[(Part::Groove(1.0), 16), (Part::Break, 8), (Part::Buildup, 8), (Part::Groove(1.0), 16)]);
        let states = detect(&signal);
        let (brk, bu, drop) = (starts[1], starts[2], starts[3]);
        assert_eq!(sections_between(&states, T0 + 8.0 * p(), brk), [Section::Normal], "the groove is normal once warmed up");
        let t_break = first(&states, Section::Break, 0.0).expect("a break");
        assert!(t_break >= brk && t_break - brk <= late(), "break at {t_break:.3}, starts at {brk:.3}");
        let t_bu = first(&states, Section::Buildup, 0.0).expect("a build-up");
        assert!(t_bu >= bu && t_bu - bu <= late(), "build-up at {t_bu:.3}, starts at {bu:.3}");
        assert_eq!(sections_between(&states, t_break, bu), [Section::Break], "the break holds");
        assert_eq!(sections_between(&states, t_bu, drop), [Section::Buildup], "the build-up holds");
        let t_drop = first(&states, Section::Drop, 0.0).expect("a drop");
        assert!(t_drop >= drop && t_drop - drop <= late(), "drop reported at {t_drop:.3}, true {drop:.3}");
        let s = states.iter().rev().find(|e| e.0 < drop + 8.0 * p()).unwrap().1;
        assert_eq!(s.drop, 1, "one drop: {s:?}");
        assert!((s.last_drop_t - drop).abs() <= 0.03, "drop placed at {:.3}, true {drop:.3}", s.last_drop_t);
        assert_eq!(sections_between(&states, t_drop, starts[3] + 15.0 * p()), [Section::Drop]);
        let kinds: Vec<Section> = s.history.as_slice().iter().rev().map(|c| c.section).collect();
        assert_eq!(kinds, [Section::Normal, Section::Break, Section::Buildup, Section::Drop], "{:?}", s.history);
        assert!(s.history.as_slice()[2].t >= brk - 0.05 && s.history.as_slice()[2].t <= brk + 0.05, "break start {:?}", s.history);
    }

    /// Acceptance: `buildup` climbs (within 0.02) through the build-up.
    #[test]
    fn buildup_climbs_through_the_riser() {
        at_tempos(buildup_climbs_through_the_riser_at);
    }

    fn buildup_climbs_through_the_riser_at() {
        let (signal, starts) = arrange(&[(Part::Groove(1.0), 16), (Part::Break, 8), (Part::Buildup, 8), (Part::Groove(1.0), 4)]);
        let states = detect(&signal);
        let during: Vec<f32> = states.iter().filter(|(t, s)| *t >= starts[2] && *t < starts[3] && s.section == Section::Buildup).map(|e| e.1.buildup).collect();
        assert!(!during.is_empty());
        for w in during.windows(2) {
            assert!(w[1] >= w[0] - 0.02, "buildup fell from {} to {}", w[0], w[1]);
        }
        let (lo, hi) = (during[0], *during.last().unwrap());
        assert!(hi - lo >= 0.4 && hi > 0.6, "from {lo} to {hi}");
        assert!(states.iter().filter(|(t, _)| *t < starts[2]).all(|e| e.1.buildup == 0.0));
        assert_eq!(states.last().unwrap().1.buildup, 0.0, "back to 0 after the drop");
    }

    /// A breakdown without drums straight into the groove: break, then drop.
    #[test]
    fn a_break_without_a_build_up_ends_in_a_drop() {
        at_tempos(a_break_without_a_build_up_ends_in_a_drop_at);
    }

    fn a_break_without_a_build_up_ends_in_a_drop_at() {
        let (signal, starts) = arrange(&[(Part::Groove(1.0), 16), (Part::Break, 12), (Part::Groove(1.0), 8)]);
        let states = detect(&signal);
        assert_eq!(sections_between(&states, T0 + 8.0 * p(), starts[2] + 2.0 * p()), [Section::Normal, Section::Break, Section::Drop]);
        let s = states.last().unwrap().1;
        assert_eq!(s.drop, 1);
        assert!((s.last_drop_t - starts[2]).abs() <= p(), "{} vs {}", s.last_drop_t, starts[2]);
    }

    /// The blackout on the beat(s) before the drop: silence, then the drop
    /// still counts.
    #[test]
    fn a_blackout_before_the_drop_keeps_the_build_up() {
        at_tempos(a_blackout_before_the_drop_keeps_the_build_up_at);
    }

    fn a_blackout_before_the_drop_keeps_the_build_up_at() {
        let (signal, starts) = arrange(&[(Part::Groove(1.0), 16), (Part::Break, 8), (Part::Buildup, 6), (Part::Silence, 2), (Part::Groove(1.0), 8)]);
        let states = detect(&signal);
        assert_eq!(sections_between(&states, starts[2] + p(), starts[4] + 2.0 * p()), [Section::Buildup, Section::Silence, Section::Buildup, Section::Drop]);
        let s = states.last().unwrap().1;
        assert_eq!(s.drop, 1);
        assert!((s.last_drop_t - starts[4]).abs() <= p(), "{} vs {}", s.last_drop_t, starts[4]);
    }

    /// Silence is reported 300 ms in; a groove after it is normal, no drop.
    #[test]
    fn silence_is_its_own_section_and_no_drop_follows() {
        let (signal, starts) = arrange(&[(Part::Groove(1.0), 16), (Part::Silence, 8), (Part::Groove(1.0), 16)]);
        let states = detect(&signal);
        let t = first(&states, Section::Silence, starts[1]).expect("silence");
        assert!(t - starts[1] <= 0.35, "{t} vs {}", starts[1]);
        assert_eq!(sections_between(&states, T0 + 8.0 * p(), starts[3]), [Section::Normal, Section::Silence, Section::Normal]);
        assert_eq!(states.last().unwrap().1.drop, 0);
        assert_eq!(SectionDetector::new(RATE, HOP).state().section, Section::Silence, "nothing heard yet");
    }

    fn assert_steady(states: &[(f64, SectionState)], end: f64, what: &str) {
        let from = T0 + 8.0 * p();
        assert_eq!(sections_between(states, from, end), [Section::Normal], "{what}");
        let bad = states.iter().find(|e| e.1.drop != 0 || e.1.buildup != 0.0);
        assert!(bad.is_none(), "{what}: {bad:?}");
    }

    /// No false section on steady grooves: the full groove, and kicks over
    /// pink-ish noise (three one-pole low-passed noises).
    #[test]
    fn steady_grooves_stay_normal() {
        at_tempos(steady_grooves_stay_normal_at);
    }

    fn steady_grooves_stay_normal_at() {
        let (signal, starts) = arrange(&[(Part::Groove(1.0), 64)]);
        assert_steady(&detect(&signal), starts[1], "groove");
        let kicks: Vec<(f64, Hit)> = (0..64).map(|k| (T0 + k as f64 * p(), Hit::Kick)).collect();
        let mut state = [0.0f32; 3];
        let coef = [0.02f32, 0.2, 0.8];
        let pinkish: Vec<f32> = (0..((T0 + 64.0 * p() + 1.0) * RATE as f64) as usize + 1)
            .map(|i| {
                let n = noise(i, 5);
                state.iter_mut().zip(coef).map(|(s, c)| {
                    *s += (n - *s) * c;
                    *s
                }).sum::<f32>() * 0.05
            })
            .collect();
        let signal = render_at(RATE, (T0 + 64.0 * p() + 1.0) as f32, &kicks, |i| pinkish[i]);
        assert_steady(&detect(&signal), T0 + 64.0 * p(), "kicks over pink noise");
    }

    /// Acceptance: a global volume change (−12 dB for 8 beats, then back)
    /// triggers neither a break nor a drop.
    #[test]
    fn a_volume_change_is_not_a_break() {
        at_tempos(a_volume_change_is_not_a_break_at);
    }

    fn a_volume_change_is_not_a_break_at() {
        let (signal, starts) = arrange(&[(Part::Groove(1.0), 16), (Part::Groove(0.25), 8), (Part::Groove(1.0), 16), (Part::Groove(0.1), 8), (Part::Groove(1.0), 8)]);
        assert_steady(&detect(&signal), starts[5], "volume changes");
    }

    /// Kicks back without the jump (a break whose low end fades back in):
    /// normal, not a drop.
    #[test]
    fn kicks_fading_back_in_are_not_a_drop() {
        at_tempos(kicks_fading_back_in_are_not_a_drop_at);
    }

    fn kicks_fading_back_in_are_not_a_drop_at() {
        let (mut signal, starts) = arrange(&[(Part::Groove(1.0), 16), (Part::Break, 8), (Part::Groove(1.0), 16)]);
        // The groove after the break fades in over 8 beats from −30 dB,
        // the pad staying where it was.
        let (s, e) = ((starts[2] * RATE as f64) as usize, ((starts[2] + 8.0 * p()) * RATE as f64) as usize);
        for (i, x) in signal.iter_mut().enumerate().take(e).skip(s) {
            let g = 0.0316 * 31.6f32.powf((i - s) as f32 / (e - s) as f32);
            let p = pad(i);
            *x = p + (*x - p) * g;
        }
        let states = detect(&signal);
        let at_end = states.iter().rev().find(|e| e.0 < starts[3]).unwrap().1;
        assert_eq!((at_end.drop, at_end.section), (0, Section::Normal), "{:?}", sections_between(&states, starts[1], starts[3]));
    }

    #[test]
    fn the_state_serialises_with_its_history() {
        let mut h = History::default();
        for k in 0..7 {
            h.push(SectionChange { t: k as f64, section: if k % 2 == 0 { Section::Break } else { Section::Drop } });
        }
        assert_eq!(h.as_slice().len(), HISTORY_LEN);
        assert_eq!(h.as_slice()[0].t, 6.0, "newest first");
        let s = SectionState { section: Section::Buildup, history: h, ..Default::default() };
        let v = serde_json::to_value(s).unwrap();
        assert_eq!(v["section"], "buildup");
        assert_eq!(v["history"].as_array().unwrap().len(), 5);
        assert_eq!(v["history"][0]["section"], "break");
    }

    #[test]
    fn the_drop_counter_is_carried_and_forget_keeps_it() {
        let mut d = SectionDetector::new(RATE, HOP);
        let mut h = History::default();
        h.push(SectionChange { t: 3.0, section: Section::Drop });
        d.carry(&SectionState { drop: 4, last_drop_t: 3.0, history: h, ..Default::default() });
        d.forget();
        let s = d.process(&SpectralFrame::default(), &Onsets::default(), &TempoEstimate::default(), 5.0);
        assert_eq!((s.drop, s.last_drop_t, s.history.as_slice().len()), (4, 3.0, 1));
        assert_eq!(s.section, Section::Silence);
    }

    #[test]
    fn odd_input_is_harmless() {
        let mut d = SectionDetector::new(RATE, HOP);
        let mut o = Onsets::default();
        for h in 0..20_000u64 {
            let v = if h % 7 == 0 { f32::NAN } else if h % 11 == 0 { f32::INFINITY } else { -300.0 };
            let s = SpectralFrame { bands_db: [v; 5], centroid_hz: v, silent: h % 500 < 3, ..Default::default() };
            o.kick += h % 3;
            o.last_kick_t = if h % 5 == 0 { f64::NAN } else { h as f64 / 187.5 };
            let tempo = TempoEstimate { bpm: if h % 2 == 0 { f32::NAN } else { 1e9 }, ..Default::default() };
            let s = d.process(&s, &o, &tempo, h as f64 / 187.5);
            assert!(s.confidence.is_finite() && s.buildup.is_finite() && s.since_s.is_finite() && s.start_t.is_finite());
        }
    }

    #[test]
    fn a_hop_does_not_allocate() {
        let (signal, _) = arrange(&[(Part::Groove(1.0), 16), (Part::Break, 8), (Part::Buildup, 8), (Part::Groove(1.0), 8)]);
        let hops: Vec<&[f32; HOP]> = signal.as_chunks::<HOP>().0.iter().collect();
        let mut a = Analyzer::new(RATE);
        a.process(hops[0], 0.0);
        let n = crate::audio::capture::tests::allocations_during(|| {
            for (h, hop) in hops.iter().enumerate().skip(1) {
                std::hint::black_box(a.process(&hop[..], ((h + 1) * HOP) as f64 / RATE as f64));
            }
        });
        assert_eq!(n, 0, "the ring is made once, in new()");
        assert_eq!(a.sections().drop, 1, "the loop did detect: {:?}", a.sections());
    }
}
