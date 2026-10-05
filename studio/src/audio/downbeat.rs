//! Downbeat and phrase detection (T-235): which beat of the bar is the
//! "1", and where the 8/16-bar phrases start. A proposal only, like the
//! tempo estimate: the tempo clock decides what to do with it (tempo.rs).
//!
//! For every tracked beat (the tempo detector's, `bpm.rs`) two features:
//! - the low-band onset strength around the beat (the kick is usually
//!   heavier on the 1);
//! - the harmonic change: the chroma flux (12 pitch classes summed from the
//!   FFT power, steady part of each beat only) between this beat and the
//!   one before (chords tend to change on the 1).
//!
//! Each feature is averaged per position modulo 4 (sliding, ~8 bars),
//! normalised over the four positions, and summed; the "1" is the strongest
//! position, and the confidence the gap to the second one (relative to the
//! best), ramped in over the first 4 bars. Section boundaries (a drop, the
//! end of a break: `sections.rs`, T-236) start a phrase; with a confident
//! downbeat the phrase start snaps onto the nearest detected bar line.
//!
//! Written from the published ideas (beat-synchronous features voted per
//! bar position, as in the downbeat literature summarised in
//! docs/research/audio-analysis.md §2.5); no third-party code.

use super::analysis::HOP;
use super::bpm::{DetectState, TempoEstimate};
use super::sections::SectionState;
use crate::engine::Section;
use serde::Serialize;
use std::collections::VecDeque;

/// Positions per bar (4/4 only).
pub const BEATS_PER_BAR: usize = 4;
/// Downbeat confidence from which the "1" is shown as a suggestion...
pub const SHOW_CONFIDENCE: f32 = 0.3;
/// ... and from which it is trusted (phrase snapping, automatic realign).
pub const HIGH_CONFIDENCE: f32 = 0.5;

/// Sliding average over about this many bars per position.
const AVG_BARS: f32 = 8.0;
/// No confidence before this many bars per position; full weight from
/// `FULL_BARS`.
const MIN_BARS: u32 = 2;
const FULL_BARS: u32 = 4;
/// The chroma flux sum (over the 4 positions) never normalises by less
/// than this: a steady chord's tiny fluctuations stay tiny.
const FLUX_FLOOR: f32 = 0.2;
/// The kick is looked for from this before the beat to this after (s),
/// at most `LOW_WINDOW_BEATS` beats after.
const LOW_BEFORE_S: f64 = 0.05;
const LOW_AFTER_S: f64 = 0.10;
const LOW_WINDOW_BEATS: f64 = 0.4;
/// The chroma of a beat skips this share of it (the drums' transients).
const CHROMA_SKIP: f64 = 0.3;
/// Chroma from these frequencies only (Hz).
const CHROMA_LO_HZ: f32 = 200.0;
const CHROMA_HI_HZ: f32 = 4_000.0;
/// Hops kept for the per-beat features (s).
const RING_S: f64 = 4.0;
/// A silence longer than this (s) forgets everything (a new track).
const FORGET_SILENCE_S: f64 = 3.0;
/// Phrase boundaries closer than this (bars) are one boundary.
const MIN_PHRASE_BARS: f64 = 4.0;
/// A boundary snaps to a tracked beat this close (beats), and with a
/// confident downbeat to a bar line this close (beats).
const SNAP_BEAT: f64 = 0.3;
const SNAP_BAR_BEATS: f64 = 2.0;
/// Beats remembered for the snapping.
const RECENT_BEATS: usize = 32;
/// Tracked beats waiting for their span's end, at most.
const MAX_PENDING: usize = 8;
/// Spare room in the hop ring (every buffer is allocated in `new`: a hop
/// never allocates).
const RING_SPARE: usize = 64;

/// What the detector says (`/api/state.audio.downbeat`). Times are audio
/// times on the studio clock (s).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DownbeatDetection {
    /// The last tracked beat taken for a "1"; 0 when there is none yet.
    pub downbeat_t: f64,
    /// Gap between the best and the second-best position, 0..1.
    pub confidence: f32,
    /// The tempo the beats were tracked at (the clock compares it with its own).
    pub bpm: f32,
    /// Bars heard (the fewest observations of any position).
    pub bars: u32,
    /// Start of the current phrase (a drop or the end of a break, on the
    /// beat grid; on the bar grid when the downbeat is confident).
    pub phrase_start: Option<f64>,
    /// Phrase length in bars: 8, or 16 when the boundaries are 16 apart.
    pub phrase_bars: u8,
    /// Phrase boundaries seen (a counter: an event is a change).
    pub phrases: u64,
    /// Whether the last boundary was a drop.
    pub phrase_from_drop: bool,
}

impl Default for DownbeatDetection {
    fn default() -> Self {
        Self { downbeat_t: 0.0, confidence: 0.0, bpm: 0.0, bars: 0, phrase_start: None, phrase_bars: 8, phrases: 0, phrase_from_drop: false }
    }
}

/// One analysed hop.
#[derive(Clone, Copy)]
struct Hop {
    t: f64,
    low: f32,
    chroma: [f32; 12],
}

/// Per-position sliding averages.
#[derive(Clone, Copy, Default)]
struct Slot {
    low: f32,
    flux: f32,
    low_n: u32,
    flux_n: u32,
}

fn average(avg: &mut f32, n: &mut u32, x: f32) {
    *n += 1;
    let alpha = (1.0 / *n as f32).max(1.0 / AVG_BARS);
    *avg += alpha * (x - *avg);
}

pub struct DownbeatDetector {
    /// (FFT bin, pitch class) for the chroma.
    chroma_bins: Vec<(usize, usize)>,
    hops: VecDeque<Hop>,
    /// Tracked beats waiting for the next one (their span's end).
    pending: VecDeque<f64>,
    last_beat_t: f64,
    /// The last finalised beat: time, index, chroma (none if silent).
    prev: Option<(f64, i64, Option<[f32; 12]>)>,
    /// Recent finalised beats (time, index).
    recent: VecDeque<(f64, i64)>,
    slots: [Slot; BEATS_PER_BAR],
    best: Option<usize>,
    spb: f64,
    silent_since: Option<f64>,
    /// Section counters already seen (None until the first hop).
    seen_drop: Option<u64>,
    seen_change: Option<(f64, Section)>,
    out: DownbeatDetection,
}

/// Pitch class (C = 0) of `f` Hz.
fn pitch_class(f: f32) -> usize {
    ((12.0 * (f / 440.0).log2()).round() as i64 + 9).rem_euclid(12) as usize
}

impl DownbeatDetector {
    pub fn new(sample_rate: u32, fft_size: usize) -> Self {
        let bin_hz = sample_rate.max(1) as f32 / fft_size.max(1) as f32;
        let chroma_bins = (1..=fft_size / 2)
            .map(|k| (k, k as f32 * bin_hz))
            .filter(|&(_, f)| (CHROMA_LO_HZ..=CHROMA_HI_HZ).contains(&f))
            .map(|(k, f)| (k, pitch_class(f)))
            .collect();
        let hop_cap = (RING_S * sample_rate.max(1) as f64 / HOP as f64).ceil() as usize + RING_SPARE;
        Self {
            chroma_bins,
            hops: VecDeque::with_capacity(hop_cap),
            pending: VecDeque::with_capacity(MAX_PENDING + 1),
            last_beat_t: f64::NEG_INFINITY,
            prev: None,
            recent: VecDeque::with_capacity(RECENT_BEATS + 1),
            slots: [Slot::default(); BEATS_PER_BAR],
            best: None,
            spb: 0.5,
            silent_since: None,
            seen_drop: None,
            seen_change: None,
            out: DownbeatDetection::default(),
        }
    }

    pub fn state(&self) -> DownbeatDetection {
        self.out
    }

    /// Continues the phrase counter of an earlier stream (a reopened input
    /// must not look like a new boundary).
    pub fn carry(&mut self, d: &DownbeatDetection) {
        self.out.phrases = d.phrases;
    }

    /// Forgets the bar statistics and the phrase (a new track); the phrase
    /// counter stays.
    pub fn forget(&mut self) {
        let phrases = self.out.phrases;
        self.hops.clear();
        self.pending.clear();
        self.last_beat_t = f64::NEG_INFINITY;
        self.prev = None;
        self.recent.clear();
        self.slots = [Slot::default(); BEATS_PER_BAR];
        self.best = None;
        self.out = DownbeatDetection { phrases, ..Default::default() };
    }

    /// One hop ending at `t`: the low-band flux (`OnsetDetector::band_flux`),
    /// the FFT power, the tempo estimate and the sections of this hop.
    pub fn process(&mut self, low_flux: f32, power: &[f32], tempo: &TempoEstimate, sections: &SectionState, silent: bool, t: f64) -> DownbeatDetection {
        if silent {
            let since = *self.silent_since.get_or_insert(t);
            if t - since > FORGET_SILENCE_S && (self.out.bars > 0 || self.prev.is_some()) {
                self.forget();
            }
            // Nothing heard: no beat of silence takes part.
            self.pending.clear();
            self.watch_sections(sections);
            return self.out;
        }
        self.silent_since = None;

        let mut chroma = [0.0f32; 12];
        for &(k, c) in &self.chroma_bins {
            if let Some(&p) = power.get(k) {
                if p.is_finite() && p > 0.0 {
                    chroma[c] += p.sqrt();
                }
            }
        }
        let low = if low_flux.is_finite() { low_flux.max(0.0) } else { 0.0 };
        if self.hops.len() >= self.hops.capacity() {
            self.hops.pop_front();
        }
        self.hops.push_back(Hop { t, low, chroma });
        while self.hops.front().is_some_and(|h| t - h.t > RING_S) {
            self.hops.pop_front();
        }

        let bpm = tempo.bpm as f64;
        if tempo.state == DetectState::Locked && bpm.is_finite() && bpm > 0.0 {
            self.spb = 60.0 / bpm;
            self.out.bpm = tempo.bpm;
            if tempo.beat_time > self.last_beat_t + 0.5 * self.spb && tempo.beat_time <= t {
                self.last_beat_t = tempo.beat_time;
                if self.pending.len() >= MAX_PENDING {
                    self.pending.pop_front();
                }
                self.pending.push_back(tempo.beat_time);
            }
        } else {
            // Not following a pulse: the beats in flight are dropped, the
            // statistics kept (a break).
            self.pending.clear();
        }
        while self.pending.len() >= 2 && t >= self.pending[1] {
            let (b, next) = (self.pending[0], self.pending[1]);
            self.pending.pop_front();
            self.finalise(b, next);
        }
        self.watch_sections(sections);
        self.out
    }

    /// The features of the beat at `b` (the next one at `next`).
    fn finalise(&mut self, b: f64, next: f64) {
        let spb = self.spb;
        let span = (next - b).min(spb);
        let (lo, hi) = (b - LOW_BEFORE_S, b + LOW_AFTER_S.min(LOW_WINDOW_BEATS * spb));
        let low = self.hops.iter().filter(|h| h.t >= lo && h.t <= hi).map(|h| h.low).fold(0.0f32, f32::max);
        let (c_lo, c_hi) = (b + CHROMA_SKIP * span, b + span);
        let mut chroma = [0.0f32; 12];
        for h in self.hops.iter().filter(|h| h.t >= c_lo && h.t < c_hi) {
            for (c, x) in chroma.iter_mut().zip(h.chroma) {
                *c += x;
            }
        }
        let sum: f32 = chroma.iter().sum();
        let chroma = (sum > 1e-9).then(|| chroma.map(|c| c / sum));

        let (index, flux) = match self.prev {
            Some((pt, pi, pc)) => {
                let steps = ((b - pt) / spb).round().max(1.0) as i64;
                let flux = match (steps, pc, chroma) {
                    (1, Some(a), Some(c)) => Some(0.5 * a.iter().zip(c).map(|(x, y)| (x - y).abs()).sum::<f32>()),
                    _ => None,
                };
                (pi + steps, flux)
            }
            None => (0, None),
        };
        self.prev = Some((b, index, chroma));
        self.recent.push_back((b, index));
        while self.recent.len() > RECENT_BEATS {
            self.recent.pop_front();
        }
        let slot = &mut self.slots[index.rem_euclid(BEATS_PER_BAR as i64) as usize];
        average(&mut slot.low, &mut slot.low_n, low);
        if let Some(f) = flux {
            average(&mut slot.flux, &mut slot.flux_n, f);
        }
        self.estimate();
    }

    /// The best position and the confidence, from the slots.
    fn estimate(&mut self) {
        let bars = self.slots.iter().map(|s| s.low_n).min().unwrap_or(0);
        self.out.bars = bars;
        let low_sum: f32 = self.slots.iter().map(|s| s.low).sum();
        let flux_sum: f32 = self.slots.iter().map(|s| s.flux).sum::<f32>().max(FLUX_FLOOR);
        let score = |s: &Slot| if low_sum > 1e-12 { s.low / low_sum } else { 0.0 } + s.flux / flux_sum;
        let mut order: [(usize, f32); BEATS_PER_BAR] = std::array::from_fn(|i| (i, score(&self.slots[i])));
        order.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        let (best, s1, s2) = (order[0].0, order[0].1, order[1].1);
        if bars < MIN_BARS || s1.is_nan() || s1 <= 0.0 {
            self.best = None;
            self.out.confidence = 0.0;
            self.out.downbeat_t = 0.0;
            return;
        }
        let ramp = (bars.min(FULL_BARS) as f32 / FULL_BARS as f32).min(1.0);
        self.best = Some(best);
        self.out.confidence = (((s1 - s2) / s1) * ramp).clamp(0.0, 1.0);
        self.out.downbeat_t = self.recent.iter().rev().find(|(_, i)| i.rem_euclid(BEATS_PER_BAR as i64) as usize == best).map_or(0.0, |r| r.0);
    }

    /// Section boundaries become phrase starts: each new drop, and a
    /// break ending into the groove.
    fn watch_sections(&mut self, s: &SectionState) {
        let seen_drop = self.seen_drop.replace(s.drop);
        if seen_drop.is_some_and(|d| d != s.drop) {
            self.boundary(s.last_drop_t, true);
        }
        let h = s.history.as_slice();
        let newest = h.first().map(|c| (c.t, c.section));
        let seen_change = std::mem::replace(&mut self.seen_change, newest);
        if let (Some(_), Some(new)) = (seen_change, newest) {
            if seen_change != Some(new) && new.1 == Section::Normal && h.get(1).is_some_and(|c| c.section == Section::Break) {
                self.boundary(new.0, false);
            }
        }
    }

    fn boundary(&mut self, t: f64, drop: bool) {
        let spb = self.spb;
        let bar = BEATS_PER_BAR as f64 * spb;
        if self.out.phrase_start.is_some_and(|p| (t - p).abs() < MIN_PHRASE_BARS * bar) {
            return;
        }
        // Onto the nearest tracked beat...
        let beats = self.recent.iter().map(|r| r.0).chain(self.pending.iter().copied());
        let mut start = beats.min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).filter(|b| (b - t).abs() < SNAP_BEAT * spb).unwrap_or(t);
        // ... and, the downbeat being trusted, onto the nearest bar line.
        if self.out.confidence >= HIGH_CONFIDENCE && self.out.downbeat_t > 0.0 {
            let line = self.out.downbeat_t + ((start - self.out.downbeat_t) / bar).round() * bar;
            if (line - start).abs() <= SNAP_BAR_BEATS * spb {
                start = line;
            }
        }
        if let Some(prev) = self.out.phrase_start {
            let bars = ((start - prev) / bar).round() as i64;
            if bars > 0 && bars % 16 == 0 {
                self.out.phrase_bars = 16;
            } else if bars > 0 && bars % 8 == 0 {
                self.out.phrase_bars = 8;
            }
        }
        self.out.phrase_start = Some(start);
        self.out.phrases += 1;
        self.out.phrase_from_drop = drop;
    }
}

#[cfg(test)]
mod tests {
    use super::super::analysis::Analyzer;
    use super::super::onsets::tests::{noise, render_at, Hit};
    use super::super::sections::{History, SectionChange};
    use super::super::spectrum::FFT_SIZE;
    use super::*;
    use std::f32::consts::PI;

    const RATE: u32 = 48_000;

    /// A chord progression, one chord per bar (C, Am, F, G), as frequencies.
    const CHORDS: [[f32; 3]; 4] = [[261.63, 329.63, 392.0], [220.0, 261.63, 329.63], [349.23, 440.0, 523.25], [392.0, 493.88, 587.33]];

    /// The groove: kick on every beat (`accent` × louder on beat `one` of
    /// the bar), hats on the eighths, and chords changing at beat `one`
    /// when `changes`, else a held chord. Beats from `t0` at `bpm`, `bars`
    /// bars. Returns the signal and the beat times.
    fn song(bpm: f64, t0: f64, bars: usize, one: usize, accent: f32, changes: bool) -> (Vec<f32>, Vec<f64>) {
        let p = 60.0 / bpm;
        let n = bars * BEATS_PER_BAR;
        let beats: Vec<f64> = (0..n).map(|k| t0 + k as f64 * p).collect();
        let seconds = (t0 + n as f64 * p + 0.5) as f32;
        let mut plain = Vec::new();
        let mut accented = Vec::new();
        for (k, &b) in beats.iter().enumerate() {
            if k % BEATS_PER_BAR == one { accented.push((b, Hit::Kick)) } else { plain.push((b, Hit::Kick)) }
            plain.push((b, Hit::Hat));
            plain.push((b + p / 2.0, Hit::Hat));
        }
        // The chord sounding at sample i: the bar that began at the last "1".
        let chord_at = |i: usize| -> usize {
            let t = i as f64 / RATE as f64;
            if !changes {
                return 0;
            }
            let first_one = t0 + one as f64 * p;
            if t < first_one {
                return 3;
            }
            (((t - first_one) / (4.0 * p)).floor() as usize) % CHORDS.len()
        };
        let mut signal = render_at(RATE, seconds, &plain, |i| {
            let t = i as f32 / RATE as f32;
            0.003 * noise(i, 7) + CHORDS[chord_at(i)].iter().map(|f| 0.05 * (2.0 * PI * f * t).sin()).sum::<f32>()
        });
        let acc = render_at(RATE, seconds, &accented, |_| 0.0);
        for (x, a) in signal.iter_mut().zip(acc) {
            *x += a * accent;
        }
        (signal, beats)
    }

    /// Every hop's tempo and downbeat detections, with the hop's end time.
    fn detect(signal: &[f32]) -> Vec<(f64, TempoEstimate, DownbeatDetection)> {
        let mut a = Analyzer::new(RATE);
        signal
            .as_chunks::<HOP>()
            .0
            .iter()
            .enumerate()
            .map(|(h, hop)| {
                let t = ((h + 1) * HOP) as f64 / RATE as f64;
                let m = a.process(hop, t);
                (t, m.tempo, m.downbeat)
            })
            .collect()
    }

    /// Which beat of the bar (0 = the true "1") `t` is, from `beats`.
    fn position(beats: &[f64], one: usize, t: f64) -> Option<usize> {
        let (k, b) = beats.iter().enumerate().min_by(|a, b| (a.1 - t).abs().total_cmp(&(b.1 - t).abs()))?;
        ((b - t).abs() < 0.05).then(|| (k + BEATS_PER_BAR - one) % BEATS_PER_BAR)
    }

    /// When the tempo first locked.
    fn locked_at(d: &[(f64, TempoEstimate, DownbeatDetection)]) -> f64 {
        d.iter().find(|e| e.1.state == DetectState::Locked).expect("the tempo locks").0
    }

    /// From `from` on, the share of hops whose suggestion (downbeat_t) is
    /// a true "1", and the lowest confidence.
    fn accuracy(d: &[(f64, TempoEstimate, DownbeatDetection)], beats: &[f64], one: usize, from: f64) -> (f64, f32) {
        let after: Vec<_> = d.iter().filter(|e| e.0 >= from).collect();
        let right = after.iter().filter(|e| e.2.downbeat_t > 0.0 && position(beats, one, e.2.downbeat_t) == Some(0)).count();
        (right as f64 / after.len().max(1) as f64, after.iter().map(|e| e.2.confidence).fold(1.0, f32::min))
    }

    /// Bars after the lock from which every suggestion is a true "1".
    fn settled_bars(d: &[(f64, TempoEstimate, DownbeatDetection)], beats: &[f64], one: usize, bpm: f64) -> f64 {
        let lock = locked_at(d);
        let last_wrong = d.iter().filter(|e| e.0 >= lock && !(e.2.downbeat_t > 0.0 && position(beats, one, e.2.downbeat_t) == Some(0))).map(|e| e.0).fold(lock, f64::max);
        (last_wrong - lock) / (4.0 * 60.0 / bpm)
    }

    #[test]
    fn accent_and_chord_on_the_one_are_found_within_8_bars() {
        // Several tempos and bar phases (where the music's "1" falls among
        // the tracker's beats).
        let bar = |bpm: f64| 4.0 * 60.0 / bpm;
        for (bpm, one) in [(120.0, 0), (120.0, 1), (128.0, 2), (128.0, 3), (100.0, 1), (140.0, 2)] {
            let (signal, beats) = song(bpm, 0.1, 24, one, 1.0, true);
            let d = detect(&signal);
            let from = locked_at(&d) + 8.0 * bar(bpm);
            let (acc, min_conf) = accuracy(&d, &beats, one, from);
            let settled = settled_bars(&d, &beats, one, bpm);
            eprintln!("{bpm} BPM, 1 on beat {}: right after {settled:.1} bars, then {:.0} % right, confidence ≥ {min_conf:.2}", one + 1, 100.0 * acc);
            assert!(settled <= 8.0, "{bpm} BPM, one {one}: {settled} bars");
            assert!(acc > 0.99, "{bpm} BPM, one {one}: {acc}");
            assert!(min_conf >= HIGH_CONFIDENCE, "{bpm} BPM, one {one}: {min_conf}");
        }
    }

    #[test]
    fn accent_on_three_is_found_there() {
        // The music's "1" on the tracker's third beat: the suggestion is
        // that beat, never the first.
        let (signal, beats) = song(124.0, 0.1, 24, 2, 1.0, true);
        let d = detect(&signal);
        let from = locked_at(&d) + 8.0 * 4.0 * 60.0 / 124.0;
        let (acc, _) = accuracy(&d, &beats, 2, from);
        assert!(acc > 0.99, "{acc}");
    }

    #[test]
    fn either_cue_alone_still_points_at_the_one() {
        // Only the kick accent, then only the chord changes.
        for (accent, changes) in [(1.6, false), (1.0, true)] {
            let (signal, beats) = song(124.0, 0.1, 24, 1, accent, changes);
            let d = detect(&signal);
            let from = locked_at(&d) + 8.0 * 4.0 * 60.0 / 124.0;
            let (acc, min_conf) = accuracy(&d, &beats, 1, from);
            eprintln!("accent ×{accent}, chord changes {changes}: {:.0} % right, confidence ≥ {min_conf:.2}", 100.0 * acc);
            assert!(acc > 0.99, "{accent} {changes}: {acc}");
        }
    }

    #[test]
    fn a_groove_without_accent_stays_below_0_3() {
        for bpm in [120.0, 128.0] {
            let (signal, _) = song(bpm, 0.1, 24, 0, 1.0, false);
            let d = detect(&signal);
            let worst = d.iter().map(|e| e.2.confidence).fold(0.0, f32::max);
            eprintln!("{bpm} BPM, no accent: confidence ≤ {worst:.2}");
            assert!(worst < SHOW_CONFIDENCE, "{bpm}: {worst}");
        }
    }

    #[test]
    fn silence_and_noise_give_no_downbeat_and_no_phrase() {
        let silence = vec![0.0f32; RATE as usize * 20];
        let noise_in: Vec<f32> = (0..RATE as usize * 20).map(|i| 0.3 * noise(i, 3)).collect();
        for signal in [silence, noise_in] {
            let d = detect(&signal);
            assert!(d.iter().all(|e| e.2.confidence < SHOW_CONFIDENCE && e.2.phrases == 0));
        }
    }

    #[test]
    fn a_long_silence_forgets_the_bar() {
        let (mut signal, _) = song(120.0, 0.1, 16, 0, 1.0, true);
        signal.extend(vec![0.0; RATE as usize * 4]);
        let d = detect(&signal);
        let end = d.last().unwrap().2;
        assert_eq!((end.bars, end.downbeat_t, end.confidence), (0, 0.0, 0.0));
    }

    fn sections(drop: u64, last_drop_t: f64, history: &[(f64, Section)]) -> SectionState {
        let mut h = History::default();
        for &(t, section) in history.iter().rev() {
            h.push_for_test(SectionChange { t, section });
        }
        SectionState { drop, last_drop_t, history: h, ..Default::default() }
    }

    /// A detector fed synthetic features: a kick (low flux) on every beat,
    /// `accent` on index ≡ `one` (mod 4), at 120 BPM from t = 0.
    fn fed(one: usize, accent: f32, until: f64, sections_at: impl Fn(f64) -> SectionState) -> DownbeatDetector {
        let mut d = DownbeatDetector::new(RATE, FFT_SIZE);
        let power = vec![1e-6f32; FFT_SIZE / 2 + 1];
        let hop = HOP as f64 / RATE as f64;
        let p = 0.5;
        let mut t = hop;
        while t < until {
            let k = (t / p).floor();
            let beat = k * p;
            let low = if t - beat < hop { if k as usize % 4 == one { accent } else { 1.0 } } else { 0.0 };
            let est = TempoEstimate { bpm: 120.0, confidence: 0.9, beat_time: if t - beat > 0.03 { beat } else { (beat - p).max(0.0) }, next_beat: beat + p, state: DetectState::Locked };
            d.process(low, &power, &est, &sections_at(t), false, t);
            t += hop;
        }
        d
    }

    #[test]
    fn a_drop_after_16_bars_starts_a_phrase_on_its_beat() {
        // No accent: the drop alone gives the phrase start, on its beat.
        let drop_t = 32.0 * 0.5 + 0.01;
        let d = fed(0, 1.0, 40.0, |t| if t > drop_t + 0.2 { sections(1, drop_t, &[(drop_t, Section::Drop)]) } else { sections(0, 0.0, &[]) });
        let s = d.state();
        assert_eq!((s.phrases, s.phrase_from_drop), (1, true));
        assert!((s.phrase_start.unwrap() - 16.0).abs() < 1e-9, "{:?}", s.phrase_start);
    }

    #[test]
    fn with_a_confident_downbeat_the_phrase_snaps_to_the_bar() {
        // The "1" on index 1 (t = 0.5 + 2n); a drop reported on the next
        // beat (t = 33.0) is moved back onto the bar line at 32.5.
        let drop_t = 33.0;
        let d = fed(1, 3.0, 40.0, |t| if t > drop_t + 0.2 { sections(1, drop_t, &[(drop_t, Section::Drop)]) } else { sections(0, 0.0, &[]) });
        let s = d.state();
        assert!(s.confidence >= HIGH_CONFIDENCE, "{}", s.confidence);
        assert!((s.phrase_start.unwrap() - 32.5).abs() < 1e-6, "{:?}", s.phrase_start);
    }

    #[test]
    fn a_break_ending_starts_a_phrase_and_16_bar_phrases_are_seen() {
        // Drop at 8 s, end of a break 16 bars (32 s) later.
        let d = fed(0, 1.0, 45.0, |t| {
            if t > 40.2 {
                sections(1, 8.0, &[(40.0, Section::Normal), (36.0, Section::Break), (8.0, Section::Drop)])
            } else if t > 8.2 {
                sections(1, 8.0, &[(8.0, Section::Drop)])
            } else {
                sections(0, 0.0, &[])
            }
        });
        let s = d.state();
        assert_eq!((s.phrases, s.phrase_from_drop, s.phrase_bars), (2, false, 16));
        assert!((s.phrase_start.unwrap() - 40.0).abs() < 1e-9);
    }

    #[test]
    fn boundaries_closer_than_4_bars_are_one() {
        let d = fed(0, 1.0, 20.0, |t| {
            let n = (t / 1.0).floor() as u64; // a "drop" every second
            sections(n, n as f64, &[])
        });
        // 20 s at 120 BPM = 10 bars: at most one boundary per 4 bars.
        assert!(d.state().phrases <= 3, "{}", d.state().phrases);
    }

    #[test]
    fn carried_counters_are_not_boundaries() {
        let mut d = DownbeatDetector::new(RATE, FFT_SIZE);
        d.carry(&DownbeatDetection { phrases: 7, ..Default::default() });
        let power = vec![0.0f32; FFT_SIZE / 2 + 1];
        // The first hop sees an old drop counter: not an event.
        let s = d.process(0.0, &power, &TempoEstimate::default(), &sections(5, 1.0, &[(1.0, Section::Drop)]), false, 2.0);
        assert_eq!((s.phrases, s.phrase_start), (7, None));
    }

    #[test]
    fn a_hop_does_not_allocate_bars_included() {
        // Locked and counting bars before the measure, then 8 s more
        // (beats finalised, statistics and estimate updated) without one
        // allocation.
        let (signal, _) = song(120.0, 0.1, 16, 0, 1.0, true);
        let hops: Vec<&[f32; HOP]> = signal.as_chunks::<HOP>().0.iter().collect();
        let mut a = Analyzer::new(RATE);
        let split = hops.len() / 2;
        let t = |h: usize| ((h + 1) * HOP) as f64 / RATE as f64;
        for (h, hop) in hops.iter().enumerate().take(split) {
            a.process(&hop[..], t(h));
        }
        let before = a.downbeat().bars;
        assert!(before > 0, "counting bars already");
        let n = crate::audio::capture::tests::allocations_during(|| {
            for (h, hop) in hops.iter().enumerate().skip(split) {
                std::hint::black_box(a.process(&hop[..], t(h)));
            }
        });
        assert_eq!(n, 0, "buffers are allocated once, in new()");
        assert!(a.downbeat().bars > before);
    }

    #[test]
    fn pitch_classes() {
        assert_eq!(pitch_class(440.0), 9);
        assert_eq!(pitch_class(261.63), 0);
        assert_eq!(pitch_class(392.0), 7);
    }
}
