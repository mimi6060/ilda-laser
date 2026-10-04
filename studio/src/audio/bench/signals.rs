//! Synthetic test signals with their ground truth (T-244). Everything is
//! a pure function of the sample index and a fixed seed (the riser's two
//! one-pole filters are the only state, run in order), so a case renders
//! the same samples on every machine and every run.
//!
//! Our own simple voices, not samples: a kick is a sine gliding
//! 120 → 45 Hz with a short noise click, a snare a noise burst over a
//! 185 Hz tone, a hi-hat differentiated noise (+12 dB/octave), a click
//! one full-scale sample. A bass line, a held pad chord, a riser
//! (high-passed noise whose low-pass cutoff climbs 1.5 → 12 kHz) and white
//! noise are the continuous layers.

use crate::engine::Section;
use std::f64::consts::PI;

/// Deterministic white noise in -1..1 (SplitMix64 of index and seed).
pub fn noise(i: usize, seed: u64) -> f32 {
    let mut z = (i as u64).wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_add(0x632B_E59B_D9B4_E019);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Voice {
    Kick,
    Snare,
    Hat,
    Click,
}

/// How long a voice rings (s); the render stops there.
const VOICE_S: f64 = 0.4;

/// `voice` `dt` seconds after its start, at sample `i` (for the noise).
fn voice(v: Voice, dt: f64, i: usize) -> f32 {
    match v {
        Voice::Kick => {
            let tau = 0.04;
            let phase = 2.0 * PI * (45.0 * dt + 75.0 * tau * (1.0 - (-dt / tau).exp()));
            (0.7 * (-dt / 0.18).exp() * phase.sin()) as f32 + 0.15 * (-dt / 0.002).exp() as f32 * noise(i, 31)
        }
        Voice::Snare if dt < 0.3 => 0.25 * (-dt / 0.06).exp() as f32 * noise(i, 32) + (0.25 * (-dt / 0.07).exp() * (2.0 * PI * 185.0 * dt).sin()) as f32,
        Voice::Hat if dt < 0.12 => {
            let d2 = noise(i, 33) - 2.0 * noise(i.wrapping_sub(1), 33) + noise(i.wrapping_sub(2), 33);
            0.35 * (-dt / 0.025).exp() as f32 * d2 / 4.0
        }
        Voice::Click if dt == 0.0 => 0.9,
        _ => 0.0,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f64,
    pub voice: Voice,
}

/// A continuous layer over `from..to` (s).
#[derive(Clone, Copy, Debug)]
pub enum Layer {
    /// White noise.
    Noise { from: f64, to: f64, amp: f32, seed: u64 },
    /// A held A major chord.
    Pad { from: f64, to: f64, amp: f32 },
    /// A plucked bass, a new note every `period` s from `from + offset`
    /// (a bar's worth of notes: A, A, C, G), 10 ms release before the next.
    Bass { from: f64, to: f64, amp: f32, period: f64, offset: f64 },
    /// Filtered noise climbing in brightness and level (a build-up).
    Riser { from: f64, to: f64 },
    /// Everything (layers and hits) times `gain`; 0 is digital silence.
    Gain { from: f64, to: f64, gain: f32 },
}

const BASS_NOTES: [f64; 4] = [55.0, 55.0, 65.41, 49.0];

impl Layer {
    /// The stateless layers' sample at `t` (sample `i`).
    fn sample(&self, t: f64, i: usize) -> f32 {
        match *self {
            Layer::Noise { from, to, amp, seed } if t >= from && t < to => amp * noise(i, seed),
            Layer::Pad { from, to, amp } if t >= from && t < to => [220.0, 277.18, 329.63, 440.0].iter().map(|f| (amp as f64 * (2.0 * PI * f * t).sin()) as f32).sum(),
            Layer::Bass { from, to, amp, period, offset } if t >= from + offset && t < to => {
                let k = ((t - from - offset) / period).floor();
                let start = from + offset + k * period;
                let dt = t - start;
                let left = (start + period).min(to) - t;
                let env = (dt / 0.005).min(1.0) * (0.5 + 0.5 * (-dt / 0.1).exp()) * (left / 0.01).min(1.0);
                let f = BASS_NOTES[k as usize % BASS_NOTES.len()];
                (amp as f64 * env * (2.0 * PI * f * dt).sin()) as f32
            }
            _ => 0.0,
        }
    }
}

/// When the tempo is `bpm` and audible (drums or clicks), and how long the
/// detector gets before it is judged (s from `from`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoSpan {
    pub from: f64,
    pub to: f64,
    pub bpm: f64,
    pub settle: f64,
}

/// What a case contains. Times in seconds from the first sample.
#[derive(Clone, Debug, Default)]
pub struct Truth {
    /// Every transient (hits closer than 30 ms merged).
    pub onsets: Vec<f64>,
    pub kicks: Vec<f64>,
    pub snares: Vec<f64>,
    pub hats: Vec<f64>,
    /// Whether onsets / kick-snare-hat are annotated (F-measures computed).
    pub onsets_labelled: bool,
    pub drums_labelled: bool,
    /// No kick at all: every kick reported is false (and every onset,
    /// with `no_onsets`); no pulse: the tempo must not lock.
    pub no_kicks: bool,
    pub no_onsets: bool,
    pub no_pulse: bool,
    /// Beats where the pulse is audible.
    pub beats: Vec<f64>,
    pub tempo: Vec<TempoSpan>,
    /// Section changes `(start, section)`, judged over `sections_eval`.
    pub sections: Vec<(f64, Section)>,
    pub sections_eval: Option<(f64, f64)>,
    pub drops: Vec<f64>,
}

impl Truth {
    pub fn section_at(&self, t: f64) -> Option<Section> {
        self.sections.iter().rev().find(|s| s.0 <= t).map(|s| s.1)
    }
}

/// A rendered case.
pub struct Case {
    pub name: String,
    pub rate: u32,
    pub signal: Vec<f32>,
    pub truth: Truth,
}

/// What the previous part was (a groove after a break or build-up drops).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    None,
    Groove,
    Quiet,
    Silence,
}

/// Builds a case part by part, keeping the truth as it goes.
pub struct Builder {
    name: String,
    rate: u32,
    t: f64,
    bpm: f64,
    /// Beats of groove so far (snare on 2 and 4 across parts).
    beat_no: usize,
    last: Part,
    last_pulse_bpm: Option<f64>,
    first_groove: Option<f64>,
    bed: f32,
    hits: Vec<Hit>,
    layers: Vec<Layer>,
    truth: Truth,
}

/// The −54 dBFS noise floor of every sounding part.
const FLOOR: f32 = 0.002;
/// A drop is *drop* for this many beats, then *normal* (sections.rs).
const DROP_BEATS: f64 = 16.0;
/// Sections are only judged after this many beats of groove.
const WARMUP_BEATS: f64 = 8.0;

impl Builder {
    pub fn new(name: &str, rate: u32, bpm: f64) -> Self {
        Self {
            name: name.into(),
            rate,
            t: 0.1,
            bpm,
            beat_no: 0,
            last: Part::None,
            last_pulse_bpm: None,
            first_groove: None,
            bed: 0.0,
            hits: Vec::new(),
            layers: Vec::new(),
            truth: Truth { onsets_labelled: true, drums_labelled: true, ..Default::default() },
        }
    }

    fn p(&self) -> f64 {
        60.0 / self.bpm
    }

    pub fn tempo(mut self, bpm: f64) -> Self {
        self.bpm = bpm;
        self
    }

    fn pulse(&mut self, from: f64, to: f64) {
        // Detected beats keep their tempo through a break: a span at the
        // same tempo needs no settling again.
        let settle = if self.last_pulse_bpm == Some(self.bpm) && self.last != Part::Silence { 0.0 } else { 8.0 };
        self.truth.tempo.push(TempoSpan { from, to, bpm: self.bpm, settle });
        self.last_pulse_bpm = Some(self.bpm);
    }

    fn section(&mut self, t: f64, s: Section) {
        if self.truth.sections.last().map(|c| c.1) != Some(s) {
            self.truth.sections.push((t, s));
        }
    }

    /// Kick on every beat, snare on 2 and 4, hats on the eighths, a bass
    /// note on each off-beat, the pad; all times `gain`.
    pub fn groove(self, beats: usize, gain: f32) -> Self {
        self.drums(beats, gain, false)
    }

    /// A broken beat: per bar, kicks on 1 and on the "and" of 2 and of 3
    /// (16th steps 0, 6, 10), snare on 2 and 4, hats on the eighths, the pad.
    pub fn breakbeat(self, beats: usize) -> Self {
        self.drums(beats, 1.0, true)
    }

    fn drums(mut self, beats: usize, gain: f32, broken: bool) -> Self {
        let (s, p) = (self.t, self.p());
        let e = s + beats as f64 * p;
        for k in 0..beats {
            let t = s + k as f64 * p;
            if !broken {
                self.hits.push(Hit { t, voice: Voice::Kick });
            } else {
                for step in [0, 6, 10].into_iter().filter(|st| st / 4 == self.beat_no % 4) {
                    self.hits.push(Hit { t: t + (step % 4) as f64 * p / 4.0, voice: Voice::Kick });
                }
            }
            if self.beat_no % 2 == 1 {
                self.hits.push(Hit { t, voice: Voice::Snare });
            }
            self.hits.push(Hit { t, voice: Voice::Hat });
            self.hits.push(Hit { t: t + p / 2.0, voice: Voice::Hat });
            self.truth.beats.push(t);
            self.beat_no += 1;
        }
        self.layers.push(Layer::Noise { from: s, to: e, amp: FLOOR, seed: 7 });
        self.layers.push(Layer::Pad { from: s, to: e, amp: 0.04 });
        if !broken {
            self.layers.push(Layer::Bass { from: s, to: e, amp: 0.15, period: p, offset: p / 2.0 });
        }
        if gain != 1.0 {
            self.layers.push(Layer::Gain { from: s, to: e, gain });
        }
        self.pulse(s, e);
        if self.last == Part::Quiet {
            self.truth.drops.push(s);
            self.section(s, Section::Drop);
            if beats as f64 > DROP_BEATS {
                self.section(s + DROP_BEATS * p, Section::Normal);
            }
        } else {
            self.section(s, Section::Normal);
        }
        self.first_groove.get_or_insert(s);
        self.last = Part::Groove;
        self.t = e;
        self
    }

    /// White noise at `amp` under everything from the start (a loud room,
    /// a noisy line): it changes no annotation.
    pub fn bed(mut self, amp: f32) -> Self {
        self.bed = amp;
        self
    }

    /// One click per beat over the noise floor.
    pub fn clicks(mut self, beats: usize) -> Self {
        let (s, p) = (self.t, self.p());
        let e = s + beats as f64 * p;
        for k in 0..beats {
            let t = s + k as f64 * p;
            self.hits.push(Hit { t, voice: Voice::Click });
            self.truth.beats.push(t);
        }
        self.layers.push(Layer::Noise { from: s, to: e, amp: FLOOR, seed: 7 });
        self.pulse(s, e);
        self.truth.drums_labelled = false;
        self.last = Part::Groove;
        self.t = e;
        self
    }

    /// The pad alone (drums out).
    pub fn breakdown(mut self, beats: usize) -> Self {
        let (s, e) = (self.t, self.t + beats as f64 * self.p());
        self.layers.push(Layer::Noise { from: s, to: e, amp: FLOOR, seed: 7 });
        self.layers.push(Layer::Pad { from: s, to: e, amp: 0.04 });
        self.section(s, Section::Break);
        self.last = Part::Quiet;
        self.t = e;
        self
    }

    /// The pad, a riser and a snare roll (quarters, eighths, sixteenths).
    pub fn buildup(mut self, beats: usize) -> Self {
        let (s, p) = (self.t, self.p());
        let e = s + beats as f64 * p;
        for k in 0..beats {
            let per = if k < beats / 2 { 1 } else if k < 3 * beats / 4 { 2 } else { 4 };
            for j in 0..per {
                self.hits.push(Hit { t: s + k as f64 * p + j as f64 * p / per as f64, voice: Voice::Snare });
            }
        }
        self.layers.push(Layer::Noise { from: s, to: e, amp: FLOOR, seed: 7 });
        self.layers.push(Layer::Pad { from: s, to: e, amp: 0.04 });
        self.layers.push(Layer::Riser { from: s, to: e });
        self.section(s, Section::Buildup);
        self.last = Part::Quiet;
        self.t = e;
        self
    }

    /// Digital silence.
    pub fn silence(mut self, seconds: f64) -> Self {
        let (s, e) = (self.t, self.t + seconds);
        self.layers.push(Layer::Gain { from: s, to: e, gain: 0.0 });
        self.section(s, Section::Silence);
        self.last = Part::Silence;
        self.t = e;
        self
    }

    /// White noise at `amp`: no pulse, no onsets.
    pub fn noise(mut self, seconds: f64, amp: f32) -> Self {
        let (s, e) = (self.t, self.t + seconds);
        self.layers.push(Layer::Noise { from: s, to: e, amp, seed: 41 });
        let t = &mut self.truth;
        (t.no_kicks, t.no_onsets, t.no_pulse, t.onsets_labelled, t.drums_labelled) = (true, true, true, false, false);
        self.t = e;
        self
    }

    /// The bass line and the pad, no drums: no kick may fire. Its notes
    /// are a pulse (a tempo may lock) and onsets, neither annotated.
    pub fn bass_line(mut self, beats: usize) -> Self {
        let (s, p) = (self.t, self.p());
        let e = s + beats as f64 * p;
        self.layers.push(Layer::Noise { from: s, to: e, amp: FLOOR, seed: 7 });
        self.layers.push(Layer::Pad { from: s, to: e, amp: 0.04 });
        self.layers.push(Layer::Bass { from: s, to: e, amp: 0.15, period: p, offset: p / 2.0 });
        let t = &mut self.truth;
        (t.no_kicks, t.onsets_labelled, t.drums_labelled) = (true, false, false);
        self.t = e;
        self
    }

    pub fn build(mut self) -> Case {
        let end = self.t;
        if self.bed > 0.0 {
            self.layers.push(Layer::Noise { from: 0.0, to: end, amp: self.bed, seed: 43 });
        }
        let seconds = end + 0.5;
        let rate = self.rate as f64;
        let n = (seconds * rate) as usize;
        let mut out: Vec<f32> = (0..n).map(|i| self.layers.iter().map(|l| l.sample(i as f64 / rate, i)).sum()).collect();
        for h in &self.hits {
            let start = (h.t * rate).round() as usize;
            let stop = ((h.t + VOICE_S) * rate) as usize;
            for (i, x) in out.iter_mut().enumerate().take(stop.min(n)).skip(start) {
                *x += voice(h.voice, (i - start) as f64 / rate, i);
            }
        }
        for l in &self.layers {
            match *l {
                Layer::Riser { from, to } => riser(&mut out, rate, from, to),
                Layer::Gain { from, to, gain } => out[((from * rate) as usize).min(n)..((to * rate) as usize).min(n)].iter_mut().for_each(|x| *x *= gain),
                _ => {}
            }
        }

        let t = &mut self.truth;
        let of = |v: Voice| self.hits.iter().filter(|h| h.voice == v).map(|h| h.t).collect::<Vec<f64>>();
        t.kicks = of(Voice::Kick);
        t.snares = of(Voice::Snare);
        t.hats = of(Voice::Hat);
        let mut all: Vec<f64> = self.hits.iter().map(|h| h.t).collect();
        all.sort_by(f64::total_cmp);
        for x in all {
            if t.onsets.last().is_none_or(|&l| x - l > 0.03) {
                t.onsets.push(x);
            }
        }
        if let Some(g) = self.first_groove {
            let warm = g + WARMUP_BEATS * 60.0 / t.tempo.first().map_or(self.bpm, |s| s.bpm);
            t.sections_eval = Some((warm, end));
        }
        Case { name: self.name, rate: self.rate, signal: out, truth: self.truth }
    }
}

/// The riser over `from..to`: noise high-passed at 1 kHz, low-passed
/// from 1.5 to 12 kHz, its level climbing from 0.05 to 0.55.
fn riser(out: &mut [f32], rate: f64, from: f64, to: f64) {
    let (mut hp_prev, mut hp, mut lp) = (0.0f64, 0.0f64, 0.0f64);
    let a_hp = (-2.0 * PI * 1_000.0 / rate).exp();
    let (a, b) = ((from * rate) as usize, ((to * rate) as usize).min(out.len()));
    for (i, x) in out.iter_mut().enumerate().take(b).skip(a) {
        let frac = (i - a) as f64 / (b - a).max(1) as f64;
        let n = noise(i, 11) as f64;
        hp = a_hp * (hp + n - hp_prev);
        hp_prev = n;
        let fc = 1_500.0 * 8f64.powf(frac);
        lp += (hp - lp) * (1.0 - (-2.0 * PI * fc / rate).exp());
        *x += ((0.05 + 0.5 * frac) * lp) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_white_and_bounded() {
        let v: Vec<f32> = (0..48_000).map(|i| noise(i, 3)).collect();
        assert_eq!(v, (0..48_000).map(|i| noise(i, 3)).collect::<Vec<_>>());
        assert!(v.iter().all(|x| (-1.0..1.0).contains(x)));
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let rms = (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        assert!(mean.abs() < 0.01 && (rms - 0.577).abs() < 0.01, "mean {mean}, rms {rms}");
        assert_ne!(noise(5, 3), noise(5, 4));
    }

    #[test]
    fn a_breakbeat_has_three_kicks_a_bar() {
        let c = Builder::new("b", 48_000, 120.0).breakbeat(8).build();
        let k: Vec<f64> = c.truth.kicks.iter().map(|t| ((t - 0.1) / 0.125 * 1e6).round() / 1e6).collect();
        assert_eq!(k, [0.0, 6.0, 10.0, 16.0, 22.0, 26.0]);
        assert_eq!(c.truth.beats.len(), 8);
    }

    #[test]
    fn the_builder_keeps_the_truth() {
        let c = Builder::new("t", 48_000, 120.0).groove(16, 1.0).breakdown(4).buildup(4).groove(20, 1.0).silence(1.0).build();
        let t = &c.truth;
        assert_eq!(t.kicks.len(), 36);
        assert_eq!(t.snares.len(), 8 + 8 + 10);
        assert_eq!(t.hats.len(), 72);
        assert_eq!(t.beats.len(), 36);
        // Kick, snare and hat on a beat are one onset, the off-beat hat
        // another; the roll's 8 snares are on their own.
        assert_eq!(t.onsets.len(), 72 + 8);
        assert_eq!(t.drops, [0.1 + 24.0 * 0.5]);
        let kinds: Vec<Section> = t.sections.iter().map(|s| s.1).collect();
        assert_eq!(kinds, [Section::Normal, Section::Break, Section::Buildup, Section::Drop, Section::Normal, Section::Silence]);
        assert_eq!(t.tempo.len(), 2);
        assert_eq!((t.tempo[0].settle, t.tempo[1].settle), (8.0, 0.0));
        assert_eq!(t.sections_eval, Some((0.1 + 4.0, 0.1 + 44.0 * 0.5 + 1.0)));
        assert!((c.signal.len() as f64 / 48_000.0 - (0.1 + 22.0 + 1.0 + 0.5)).abs() < 1e-3);
        // Silence is digital silence; the rest sounds.
        let at = |s: f64| c.signal[(s * 48_000.0) as usize];
        assert_eq!(at(22.5), 0.0);
        assert!(c.signal.iter().all(|x| x.is_finite() && x.abs() < 2.0));
    }
}
