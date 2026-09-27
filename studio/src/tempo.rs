//! The one tempo clock of the app. Everything "on the beat" (synced
//! rotation, colour chases, strobes, evolving cues, the timeline) reads
//! this clock, so effects can never drift apart the way per-effect speed
//! accumulators do.
//!
//! Beats are a pure function of time: `beat_at(t) = (t - origin) * bpm / 60`.
//! Changing the tempo moves `origin` so the beat position never jumps.

use serde::Serialize;
use std::collections::VecDeque;

pub const MIN_BPM: f64 = 40.0;
pub const MAX_BPM: f64 = 250.0;
const MAX_TAPS: usize = 8;
/// A pause longer than this starts a new tap sequence.
const TAP_RESET_S: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TempoSource {
    Manual,
    Tap,
}

#[derive(Clone, Debug)]
pub struct TempoClock {
    pub bpm: f64,
    pub beats_per_bar: u8,
    pub source: TempoSource,
    origin_s: f64,
    taps: VecDeque<f64>,
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
}

impl Default for TempoClock {
    fn default() -> Self {
        Self { bpm: 120.0, beats_per_bar: 4, source: TempoSource::Manual, origin_s: 0.0, taps: VecDeque::new() }
    }
}

impl TempoClock {
    pub fn beat_at(&self, t: f64) -> f64 {
        (t - self.origin_s) * self.bpm / 60.0
    }

    pub fn state(&self, t: f64) -> TempoState {
        let beat = self.beat_at(t);
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
        }
    }

    /// Change tempo without moving the current beat position.
    pub fn set_bpm(&mut self, bpm: f64, t: f64) {
        let beat = self.beat_at(t);
        self.bpm = bpm.clamp(MIN_BPM, MAX_BPM);
        self.origin_s = t - beat * 60.0 / self.bpm;
    }

    pub fn set_bpm_manual(&mut self, bpm: f64, t: f64) {
        self.set_bpm(bpm, t);
        self.source = TempoSource::Manual;
    }

    /// Tap tempo: from the third tap, BPM = 60 / median interval, and the
    /// last tap lands exactly on a whole beat.
    pub fn tap(&mut self, t: f64) {
        if self.taps.back().is_some_and(|&last| t - last > TAP_RESET_S || t < last) {
            self.taps.clear();
        }
        self.taps.push_back(t);
        while self.taps.len() > MAX_TAPS {
            self.taps.pop_front();
        }
        if self.taps.len() < 3 {
            return;
        }
        let mut intervals: Vec<f64> = self.taps.iter().zip(self.taps.iter().skip(1)).map(|(a, b)| b - a).collect();
        intervals.sort_by(|a, b| a.total_cmp(b));
        let median = intervals[intervals.len() / 2];
        if median <= 0.0 {
            return;
        }
        self.set_bpm(60.0 / median, t);
        self.source = TempoSource::Tap;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn state_reports_bar_and_beat_in_bar() {
        let c = TempoClock::default(); // 120 bpm: 2 beats per second
        let s = c.state(2.75); // beat 5.5
        assert_eq!((s.bar, s.beat_in_bar), (1, 1));
        assert!((s.phase - 0.5).abs() < 1e-9);
    }
}
