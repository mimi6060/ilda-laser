//! Audio signal conditioning (T-238): what stands between a raw analysis
//! value and a laser parameter, so a route follows the music instead of
//! trembling with it.
//!
//! One `Shaper` per route, evaluated by the engine once per frame with the
//! real frame `dt` (docs/research/audio-analysis.md §6.2):
//!
//! ```text
//! continuous: x → gate (hysteresis) → gain → curve → attack/release ─┐
//! event:      trigger(strength) → gain → AD envelope → curve ─────────┴ max → min..max
//! ```
//!
//! - **Gate**: opens when the input reaches `gate`, closes only once it
//!   falls under `gate − hysteresis`, so a signal hovering at the threshold
//!   doesn't chatter. Closed = 0, open = the input.
//! - **Curve**: linear, square (punchier), square root (more sensitive),
//!   S (smoothstep), all 0..1 → 0..1.
//! - **Envelope follower**: `y += (x − y)(1 − e^(−dt/τ))`, τ = attack when
//!   rising, release when falling. Exact for any `dt`, so 30 and 60 fps
//!   give the same curve.
//! - **Event envelope** (kick, snare, hat, onset, drop, beat): `trigger`
//!   starts an AD envelope: a linear rise over the attack, then an
//!   exponential fall that is under 5 % (e^−3) `decay` after the peak.
//! - Attack, release and decay are each `Span::Ms` or `Span::Beats`; beats
//!   are converted with the beat length of the one tempo clock
//!   (`TempoClock`) passed to every `process`, so a BPM change retimes a
//!   running envelope at once and nothing keeps its own tempo.
//! - **Range**: the output is `min + v × (max − min)`, in −1..1 fractions
//!   of the target control's range (`min > max` inverts; a negative range
//!   pulls the control down). `Target::apply` turns it into the control's
//!   units.
//!
//! **Safety.** A shaped signal only reaches a control through
//! `Target`, which is bound through the LFO allow-list (`lfo::modulatable`)
//! and applied with `lfo::offset`: the engine's per-frame copies are moved,
//! never the stored values; transport (arm, blackout), tempo, cues, grid,
//! calibration and safety settings can't be targets; brightness can only
//! dim below the operator's fader. Everything is then rendered and goes
//! through calibration, the strobe limiter and the beam horizon (T-101,
//! safety.rs) like any other frame, so an audio flash can't get past them.
//!
//! Nothing here allocates once a `Target` is bound: `Shaper::process`,
//! `Shaper::feed`, `Source::read` and `Target::apply` are plain arithmetic
//! and `match`es (tested with the counting allocator).

use crate::controls::ControlRegistry;
use crate::engine::{AudioFeatures, Settings, AUDIO_EVENTS, AUDIO_VALUES};
use crate::lfo;
use crate::live::LiveModifiers;
use serde::{Deserialize, Serialize};

/// An envelope reaching `e^−3` (< 5 %) is considered finished: `decay` is
/// that length.
const DECAY_TAUS: f32 = 3.0;
/// Under this an envelope is zero (no denormals, a clean "off").
const ENV_FLOOR: f32 = 1e-4;
/// Beat length when the clock gives nonsense (120 BPM).
const DEFAULT_BEAT_S: f32 = 0.5;
/// A frame longer than this (a stall, a paused thread) counts as this long.
const MAX_DT_S: f32 = 1.0;

/// Transfer curve applied to the 0..1 signal (`SCurve` is the task's name).
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    #[default]
    Linear,
    /// x²: quiet parts quieter, peaks stand out (punchier).
    Square,
    /// √x: small signals show more (more sensitive).
    Sqrt,
    /// Smoothstep 3x² − 2x³: soft at both ends.
    #[serde(rename = "s_curve")]
    SCurve,
}

impl Curve {
    pub fn apply(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Linear => x,
            Curve::Square => x * x,
            Curve::Sqrt => x.sqrt(),
            Curve::SCurve => x * x * (3.0 - 2.0 * x),
        }
    }
}

/// A length in milliseconds, or in beats of the tempo clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Span {
    Ms(f32),
    Beats(f32),
}

/// The task's name for the event envelope's length.
pub type Decay = Span;

impl Span {
    /// Seconds, with `beat_len_s` the length of one beat right now.
    pub fn seconds(self, beat_len_s: f32) -> f32 {
        let s = match self {
            Span::Ms(ms) => ms / 1000.0,
            Span::Beats(b) => b * beat_len_s,
        };
        if s.is_finite() { s.max(0.0) } else { 0.0 }
    }

    /// Ms 0..10 000, beats 0..16; NaN → `fallback`.
    fn sanitized(self, fallback: Span) -> Span {
        match self {
            Span::Ms(ms) if ms.is_finite() => Span::Ms(ms.clamp(0.0, 10_000.0)),
            Span::Beats(b) if b.is_finite() => Span::Beats(b.clamp(0.0, 16.0)),
            _ => fallback,
        }
    }
}

/// Running state, never saved and never compared (two shapers with the
/// same settings are equal whatever they are doing).
#[derive(Clone, Copy, Debug, Default)]
struct State {
    open: bool,
    /// Envelope follower output, 0..1 (after the curve).
    follow: f32,
    /// Event envelope, 0..1 (before the curve).
    env: f32,
    /// Its target while rising.
    peak: f32,
    rising: bool,
    /// Triggered since the last `process`: shown as is this frame.
    fresh: bool,
    /// Last event counter seen by `feed` (None until the first frame).
    last_count: Option<u64>,
    out: f32,
}

impl PartialEq for State {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// The conditioning chain of one route. Settings are serialized with
/// defaults for anything missing; the running state is not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shaper {
    /// 0..1: under this the gate is closed (noise).
    pub gate: f32,
    /// 0..0.5: once open, the gate closes only under `gate − hysteresis`.
    pub hysteresis: f32,
    /// 0..8, before the curve (the result is clamped to 1).
    pub gain: f32,
    pub curve: Curve,
    /// Follower rise time constant (continuous), rise time (events).
    pub attack: Span,
    /// Follower fall time constant (continuous sources).
    pub release: Span,
    /// Event envelope: time to fall under 5 %.
    pub decay: Decay,
    /// Output range, −1..1 of the target control's range.
    pub min: f32,
    pub max: f32,
    #[serde(skip)]
    state: State,
}

impl Default for Shaper {
    fn default() -> Self {
        Self {
            gate: 0.05,
            hysteresis: 0.02,
            gain: 1.0,
            curve: Curve::Linear,
            attack: Span::Ms(10.0),
            release: Span::Ms(150.0),
            decay: Span::Ms(120.0),
            min: 0.0,
            max: 1.0,
            state: State::default(),
        }
    }
}

/// `1 − e^(−dt/τ)`: the share of the way a one-pole covers in `dt`
/// (τ = 0: all of it).
fn approach(dt: f32, tau: f32) -> f32 {
    if tau <= 0.0 { 1.0 } else { 1.0 - (-dt / tau).exp() }
}

fn unit(x: f32) -> f32 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 }
}

impl Shaper {
    /// Every number in its range, NaN → default (settings come from JSON).
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        let fin = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        self.gate = fin(self.gate, d.gate).clamp(0.0, 1.0);
        self.hysteresis = fin(self.hysteresis, d.hysteresis).clamp(0.0, 0.5);
        self.gain = fin(self.gain, d.gain).clamp(0.0, 8.0);
        self.attack = self.attack.sanitized(d.attack);
        self.release = self.release.sanitized(d.release);
        self.decay = self.decay.sanitized(d.decay);
        self.min = fin(self.min, d.min).clamp(-1.0, 1.0);
        self.max = fin(self.max, d.max).clamp(-1.0, 1.0);
        self
    }

    /// New settings, running state kept: a route edited while it plays
    /// (a slider dragged) goes on from where it is instead of restarting.
    pub fn retune(&mut self, settings: &Shaper) {
        *self = Shaper { state: self.state, ..settings.clone() };
    }

    /// Back to rest (gate closed, envelopes at 0), settings kept.
    pub fn reset(&mut self) {
        self.state = State::default();
    }

    /// Back to rest like `reset`, but the event counter kept (no event
    /// is lost or invented when the envelopes are cleared, T-245).
    pub fn rest(&mut self) {
        self.state = State { last_count: self.state.last_count, ..State::default() };
    }

    /// Forgets the last event counter: the next `feed` only takes note of
    /// it (another source's counters are not this one's events, T-245).
    pub fn forget_events(&mut self) {
        self.state.last_count = None;
    }

    /// The last output of `process`.
    pub fn value(&self) -> f32 {
        self.state.out
    }

    /// Whether the gate is open right now.
    pub fn gate_open(&self) -> bool {
        self.state.open
    }

    /// An event with `strength` 0..1 happened since the last frame: starts
    /// the AD envelope from where it is (a hit during a decay rises again,
    /// never drops). Events weaker than the gate are ignored.
    pub fn trigger(&mut self, strength: f32) {
        let strength = unit(strength);
        if strength <= 0.0 || strength < self.gate {
            return;
        }
        let st = &mut self.state;
        st.peak = unit(strength * self.gain).max(st.env);
        st.rising = st.env < st.peak;
        st.fresh = true;
    }

    /// One frame: `x` is the continuous input (0..1; pass 0 for an event
    /// source), `dt` the real time since the last frame (s), `beat_len_s`
    /// the current beat length of the tempo clock (60 / BPM). Returns
    /// `min + v × (max − min)`.
    pub fn process(&mut self, x: f32, dt: f32, beat_len_s: f32) -> f32 {
        let x = unit(x);
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_DT_S) } else { 0.0 };
        let beat = if beat_len_s.is_finite() && beat_len_s > 0.0 { beat_len_s } else { DEFAULT_BEAT_S };
        let (attack, release, decay) = (self.attack.seconds(beat), self.release.seconds(beat), self.decay.seconds(beat));
        let st = &mut self.state;

        // Gate with hysteresis.
        if st.open {
            st.open = x >= self.gate - self.hysteresis && x > 0.0;
        } else {
            st.open = x >= self.gate && x > 0.0;
        }
        let k = self.curve.apply(if st.open { x * self.gain } else { 0.0 });

        // Envelope follower.
        let tau = if k > st.follow { attack } else { release };
        st.follow += (k - st.follow) * approach(dt, tau);

        // Event envelope. A fresh trigger is shown at its own frame; its
        // time starts counting from there.
        if st.fresh {
            st.fresh = false;
            if st.rising && attack <= 0.0 {
                st.env = st.peak;
                st.rising = false;
            }
        } else {
            let mut rest = dt;
            if st.rising {
                let rate = st.peak / attack.max(1e-6);
                let need = (st.peak - st.env) / rate;
                if need <= rest {
                    st.env = st.peak;
                    st.rising = false;
                    rest -= need;
                } else {
                    st.env += rate * rest;
                    rest = 0.0;
                }
            }
            if !st.rising && rest > 0.0 {
                st.env *= (-rest * DECAY_TAUS / decay.max(1e-6)).exp();
            }
            if st.env < ENV_FLOOR && !st.rising {
                st.env = 0.0;
            }
        }

        let v = st.follow.max(self.curve.apply(st.env)).clamp(0.0, 1.0);
        st.out = self.min + v * (self.max - self.min);
        st.out
    }

    /// One frame from the analysis snapshot: reads `source` from
    /// `features`, triggers on a new event (the counter grew since the last
    /// frame; the first frame only takes note of it), then `process`.
    pub fn feed(&mut self, source: Source, features: &AudioFeatures, dt: f32, beat_len_s: f32) -> f32 {
        match source.read(features) {
            Reading::Level(x) => self.process(x, dt, beat_len_s),
            Reading::Count(count, strength) => {
                let last = self.state.last_count.replace(count);
                if last.is_some_and(|l| count > l) {
                    self.trigger(strength);
                }
                self.process(0.0, dt, beat_len_s)
            }
        }
    }
}

/// What a route listens to: a continuous `AUDIO_VALUES` signal or an
/// `AUDIO_EVENTS` counter. Parsed once, read every frame without a string
/// comparison chain on anything but a `&'static str`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Value(&'static str),
    Event(&'static str),
}

/// One frame's reading of a `Source`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading {
    Level(f32),
    /// Counter and the strength of its last event (1 when the source
    /// doesn't measure one, as the browser's beat).
    Count(u64, f32),
}

impl Source {
    /// `bass`, `kick`… (the ids of `AUDIO_VALUES` / `AUDIO_EVENTS`).
    pub fn parse(id: &str) -> Option<Source> {
        AUDIO_VALUES
            .iter()
            .find(|v| **v == id)
            .map(|v| Source::Value(v))
            .or_else(|| AUDIO_EVENTS.iter().find(|e| **e == id).map(|e| Source::Event(e)))
    }

    pub fn id(self) -> &'static str {
        match self {
            Source::Value(id) | Source::Event(id) => id,
        }
    }

    pub fn is_event(self) -> bool {
        matches!(self, Source::Event(_))
    }

    pub fn read(self, f: &AudioFeatures) -> Reading {
        match self {
            Source::Value(id) => Reading::Level(f.value(id).unwrap_or(0.0)),
            Source::Event(id) => {
                let strength = match id {
                    "kick" => f.kick_strength,
                    "snare" => f.snare_strength,
                    "hat" => f.hat_strength,
                    _ => 1.0,
                };
                let strength = if strength > 0.0 { unit(strength) } else { 1.0 };
                Reading::Count(f.counter(id).unwrap_or(0), strength)
            }
        }
    }
}

/// A control a shaped signal may drive, bound once (when the route is
/// set up) through the LFO allow-list, so applying it every frame is a
/// `match` and a clamp.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    id: String,
    range: (f32, f32),
}

impl Target {
    /// None for anything an LFO can't modulate either (`transport.arm`,
    /// `tempo.bpm`, cues, grid, toggles, unknown ids…).
    pub fn bind(reg: &ControlRegistry, id: &str) -> Option<Target> {
        lfo::modulatable(reg, id).map(|range| Target { id: id.to_string(), range })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// (min, max) of the control, in its units.
    pub fn range(&self) -> (f32, f32) {
        self.range
    }

    /// Moves the engine's copy of the control by `amount` (−1..1) of its
    /// range, with the LFO rules (clamped, brightness only dims). Returns
    /// true for a colour target: call `lfo::recolor_live` once after the
    /// last one.
    pub fn apply(&self, amount: f32, settings: &mut Settings, live: &mut LiveModifiers) -> bool {
        let (min, max) = self.range;
        lfo::offset(&self.id, self.range, amount.clamp(-1.0, 1.0) * (max - min), settings, live)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, BeatClock, Settings};
    use crate::patterns::Point;
    use crate::presets;
    use crate::safety::{self, SafetySettings, StrobeLimiter};
    use crate::tempo::TempoClock;

    const FPS60: f32 = 1.0 / 60.0;

    fn reg() -> ControlRegistry {
        ControlRegistry::build(&presets::catalog())
    }

    /// Runs `shaper` for `seconds` at `fps` on `input`, returning (t,
    /// output) per frame. Frame 0 is at t = 0 with dt = 0; each later frame
    /// gets the input at the middle of the interval it covers, so two frame
    /// rates see the same signal wherever it changes on a shared frame
    /// boundary.
    fn run(shaper: &mut Shaper, fps: f32, seconds: f32, beat_s: f32, input: impl Fn(f32) -> f32) -> Vec<(f32, f32)> {
        let dt = 1.0 / fps;
        let n = (seconds * fps).round() as usize;
        (0..=n)
            .map(|i| {
                let t = i as f32 * dt;
                let (step, at) = if i == 0 { (0.0, 0.0) } else { (dt, t - dt / 2.0) };
                (t, shaper.process(input(at), step, beat_s))
            })
            .collect()
    }

    /// The first frame time from which `out` stays below `level`.
    fn under_from(out: &[(f32, f32)], level: f32) -> f32 {
        let last_above = out.iter().rposition(|(_, v)| *v >= level).expect("was above");
        out[last_above + 1].0
    }

    #[test]
    fn curves_map_zero_to_zero_and_one_to_one() {
        for c in [Curve::Linear, Curve::Square, Curve::Sqrt, Curve::SCurve] {
            assert_eq!((c.apply(0.0), c.apply(1.0)), (0.0, 1.0), "{c:?}");
            assert_eq!((c.apply(-3.0), c.apply(7.0)), (0.0, 1.0), "{c:?} clamps");
        }
        assert_eq!(Curve::Square.apply(0.5), 0.25);
        assert!((Curve::Sqrt.apply(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(Curve::SCurve.apply(0.5), 0.5);
        assert!(Curve::SCurve.apply(0.1) < 0.1 && Curve::SCurve.apply(0.9) > 0.9);
    }

    #[test]
    fn step_with_10_ms_attack_reaches_63_percent_at_10_ms() {
        // At 1 kHz to see the curve, and at 60 fps: 63 % at 10 ms ± 1 frame.
        let mut fine = Shaper { gate: 0.0, ..Default::default() };
        let out = run(&mut fine, 1000.0, 0.05, 0.5, |t| if t > 0.0 { 1.0 } else { 0.0 });
        let at_10 = out.iter().find(|(t, _)| (*t - 0.010).abs() < 1e-4).unwrap().1;
        assert!((at_10 - (1.0 - (-1.0f32).exp())).abs() < 0.01, "63 % one τ after the step: {at_10}");
        let mut shaper = Shaper { gate: 0.0, ..Default::default() };
        let out = run(&mut shaper, 60.0, 0.2, 0.5, |_| 1.0);
        let reached = out.iter().find(|(_, v)| *v >= 0.632).unwrap().0;
        assert!(reached <= 0.010 + FPS60 + 1e-4, "63 % by 10 ms + 1 frame: {reached}");
        assert!(out[1].1 > 0.5 && out[1].1 < 1.0, "a 10 ms attack isn't a jump: {}", out[1].1);
    }

    #[test]
    fn release_is_slower_than_attack() {
        let mut shaper = Shaper { gate: 0.0, ..Default::default() }; // 10 / 150 ms
        let out = run(&mut shaper, 60.0, 1.0, 0.5, |t| if t < 0.5 { 1.0 } else { 0.0 });
        let at = |time: f32| out.iter().find(|(t, _)| (*t - time).abs() < FPS60 / 2.0).unwrap().1;
        assert!(at(0.49) > 0.999);
        // 150 ms after the fall: e^−1 ± a frame.
        let v = at(0.65);
        assert!((v - (-1.0f32).exp()).abs() < 0.01, "{v}");
        assert!(out.windows(2).filter(|w| w[0].0 >= 0.5).all(|w| w[1].1 <= w[0].1), "falls monotonically");
    }

    #[test]
    fn decay_of_a_quarter_beat_at_120_bpm_is_125_ms() {
        let clock = TempoClock::default();
        assert_eq!(clock.bpm, 120.0);
        let beat_s = (60.0 / clock.bpm) as f32;
        // The decay counts from the peak: after a 10 ms rise it ends at 135 ms.
        for (attack, end) in [(Span::Ms(0.0), 0.125), (Span::Ms(10.0), 0.135)] {
            let mut shaper = Shaper { decay: Span::Beats(0.25), attack, ..Default::default() };
            shaper.trigger(1.0);
            let out = run(&mut shaper, 60.0, 0.5, beat_s, |_| 0.0);
            // With a 10 ms rise, the first frame after it has already started falling.
            assert!(out.iter().any(|(_, v)| *v > 0.8), "{attack:?}: peaks");
            let under = under_from(&out, 0.05);
            assert!((under - end).abs() <= FPS60 + 1e-4, "{attack:?}: under 5 % at {under} s, not {end} s ± 1 frame");
        }
        // Same Beats(0.25) at 60 BPM: twice as long.
        let mut slow = Shaper { decay: Span::Beats(0.25), attack: Span::Ms(0.0), ..Default::default() };
        slow.trigger(1.0);
        let out = run(&mut slow, 60.0, 1.0, 1.0, |_| 0.0);
        assert!((under_from(&out, 0.05) - 0.25).abs() <= FPS60 + 1e-4);
    }

    #[test]
    fn an_instant_trigger_shows_its_peak_on_its_own_frame() {
        let mut shaper = Shaper { attack: Span::Ms(0.0), ..Default::default() };
        shaper.process(0.0, FPS60, 0.5);
        shaper.trigger(0.8);
        assert_eq!(shaper.process(0.0, FPS60, 0.5), 0.8);
        assert!(shaper.process(0.0, FPS60, 0.5) < 0.8);
        // A weaker hit during the decay never pulls it down.
        let before = shaper.value();
        shaper.trigger(0.1);
        assert!(shaper.process(0.0, 0.0, 0.5) >= before - 1e-6);
        // Under the gate: ignored.
        let mut gated = Shaper { gate: 0.5, ..Default::default() };
        gated.trigger(0.3);
        assert_eq!(gated.process(0.0, FPS60, 0.5), 0.0);
    }

    #[test]
    fn bpm_change_retimes_a_running_decay() {
        let mut a = Shaper { decay: Span::Beats(1.0), attack: Span::Ms(0.0), ..Default::default() };
        let mut b = a.clone();
        a.trigger(1.0);
        b.trigger(1.0);
        a.process(0.0, 0.0, 0.5);
        b.process(0.0, 0.0, 0.5);
        for _ in 0..6 {
            a.process(0.0, FPS60, 0.5); // 120 BPM
            b.process(0.0, FPS60, 0.25); // 240 BPM: twice as fast
        }
        assert!(b.value() < a.value() * 0.8, "{} vs {}", b.value(), a.value());
    }

    #[test]
    fn hysteresis_stops_chatter_at_the_threshold() {
        // A signal oscillating ±0.01 around the default 0.05 gate.
        let mut shaper = Shaper { attack: Span::Ms(0.0), release: Span::Ms(0.0), ..Default::default() };
        let mut toggles = 0;
        let mut was_open = false;
        for i in 0..600 {
            let x = 0.05 + if i % 2 == 0 { 0.01 } else { -0.01 };
            shaper.process(x, FPS60, 0.5);
            toggles += (shaper.gate_open() != was_open) as usize;
            was_open = shaper.gate_open();
        }
        assert_eq!(toggles, 1, "opens once, then stays open");
        // Without hysteresis it would chatter every frame.
        let mut bare = Shaper { hysteresis: 0.0, attack: Span::Ms(0.0), release: Span::Ms(0.0), ..Default::default() };
        let outs: Vec<f32> = (0..20).map(|i| bare.process(0.05 + if i % 2 == 0 { 0.01 } else { -0.01 }, FPS60, 0.5)).collect();
        assert!(outs.windows(2).filter(|w| (w[0] == 0.0) != (w[1] == 0.0)).count() > 10);
        // And it does close once the signal really goes away.
        shaper.process(0.02, FPS60, 0.5);
        assert!(!shaper.gate_open());
        assert_eq!(shaper.process(0.0, FPS60, 0.5), 0.0);
        shaper.process(0.9, FPS60, 0.5);
        shaper.trigger(1.0);
        shaper.reset();
        assert!(!shaper.gate_open());
        assert_eq!((shaper.value(), shaper.process(0.0, FPS60, 0.5)), (0.0, 0.0), "reset: back to rest");
    }

    #[test]
    fn same_result_at_30_and_60_fps() {
        let beat_s = 0.5;
        // A pulse train (continuous) and a kick every half second (events).
        let pulses = |t: f32| if (t * 2.0).fract() < 0.4 { 0.9 } else { 0.0 };
        for shaper in [
            Shaper::default(),
            Shaper { curve: Curve::SCurve, attack: Span::Ms(40.0), release: Span::Beats(0.5), ..Default::default() },
        ] {
            let (mut a, mut b) = (shaper.clone(), shaper);
            let fast = run(&mut a, 60.0, 3.0, beat_s, pulses);
            let slow = run(&mut b, 30.0, 3.0, beat_s, pulses);
            for (i, (t, v)) in slow.iter().enumerate() {
                let (t60, v60) = fast[i * 2];
                assert!((t - t60).abs() < 1e-4);
                assert!((v - v60).abs() <= 0.02, "at {t}: 30 fps {v}, 60 fps {v60}");
            }
        }
        let events = |fps: f32| {
            let mut s = Shaper { decay: Span::Beats(0.5), attack: Span::Ms(20.0), ..Default::default() };
            let mut f = AudioFeatures::default();
            let n = (3.0 * fps) as usize;
            let frames_per_kick = (fps / 2.0) as usize;
            (0..=n)
                .map(|i| {
                    if i % frames_per_kick == 0 && i > 0 {
                        f.kick += 1;
                    }
                    s.feed(Source::Event("kick"), &f, if i == 0 { 0.0 } else { 1.0 / fps }, beat_s)
                })
                .collect::<Vec<f32>>()
        };
        let (fast, slow) = (events(60.0), events(30.0));
        for (i, v) in slow.iter().enumerate() {
            assert!((v - fast[i * 2]).abs() <= 0.02, "frame {i}: {v} vs {}", fast[i * 2]);
        }
        assert!(slow.iter().any(|v| *v > 0.8));
    }

    #[test]
    fn gain_and_range() {
        let mut s = Shaper { gate: 0.0, gain: 2.0, attack: Span::Ms(0.0), min: 0.2, max: -0.6, ..Default::default() };
        assert!((s.process(0.25, FPS60, 0.5) - (0.2 - 0.8 * 0.5)).abs() < 1e-6, "gain 2 then inverted range");
        assert!((s.process(0.9, FPS60, 0.5) + 0.6).abs() < 1e-6, "gain clamps to 1 → max");
    }

    #[test]
    fn feed_reads_values_and_triggers_on_new_events_only() {
        let mut f = AudioFeatures { kick: 7, kick_strength: 0.6, ..Default::default() };
        let mut s = Shaper { attack: Span::Ms(0.0), ..Default::default() };
        assert_eq!(s.feed(Source::Event("kick"), &f, 0.0, 0.5), 0.0, "an old counter isn't an event");
        f.kick = 8;
        assert!((s.feed(Source::Event("kick"), &f, FPS60, 0.5) - 0.6).abs() < 1e-6, "strength of the kick");
        let mut beat = Shaper { attack: Span::Ms(0.0), ..Default::default() };
        beat.feed(Source::Event("beat"), &f, 0.0, 0.5);
        f.beat += 1;
        assert_eq!(beat.feed(Source::Event("beat"), &f, FPS60, 0.5), 1.0, "no strength → 1");
        // Stale audio holds the counters: no event, the envelope falls.
        let neutral = f.neutral();
        let mut v = beat.value();
        for _ in 0..30 {
            let now = beat.feed(Source::Event("beat"), &neutral, FPS60, 0.5);
            assert!(now <= v);
            v = now;
        }
        assert_eq!(v, 0.0);
        let mut bass = Shaper { gate: 0.0, attack: Span::Ms(0.0), ..Default::default() };
        f.bass = 0.7;
        assert!((bass.feed(Source::Value("bass"), &f, FPS60, 0.5) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn sources_parse_from_the_feature_ids() {
        for id in AUDIO_VALUES {
            assert_eq!(Source::parse(id), Some(Source::Value(id)));
        }
        for id in AUDIO_EVENTS {
            assert_eq!(Source::parse(id), Some(Source::Event(id)));
            assert!(Source::parse(id).unwrap().is_event());
        }
        assert_eq!(Source::parse("nope"), None);
        assert_eq!(Source::parse("kick").unwrap().id(), "kick");
    }

    #[test]
    fn nonsense_in_gives_bounded_out() {
        let mut s = Shaper::default();
        for (x, dt, beat) in [(f32::NAN, FPS60, 0.5), (5.0, f32::INFINITY, 0.5), (-1.0, -3.0, f32::NAN), (0.5, 1e9, 0.0)] {
            s.trigger(f32::NAN);
            let v = s.process(x, dt, beat);
            assert!(v.is_finite() && (0.0..=1.0).contains(&v), "{v}");
        }
        let wild = Shaper { gate: f32::NAN, gain: 100.0, hysteresis: -1.0, attack: Span::Beats(f32::INFINITY), min: -9.0, max: 9.0, ..Default::default() }.sanitized();
        assert_eq!((wild.gate, wild.gain, wild.hysteresis, wild.attack, wild.min, wild.max), (0.05, 8.0, 0.0, Span::Ms(10.0), -1.0, 1.0));
    }

    #[test]
    fn serde_defaults_and_format() {
        let s: Shaper = serde_json::from_str(r#"{"curve":"s_curve","decay":{"beats":0.25}}"#).unwrap();
        assert_eq!((s.curve, s.decay, s.attack, s.gate), (Curve::SCurve, Span::Beats(0.25), Span::Ms(10.0), 0.05));
        let json = serde_json::to_value(Shaper::default()).unwrap();
        assert_eq!(json["decay"], serde_json::json!({ "ms": 120.0 }));
        assert_eq!(json["curve"], "linear");
        assert!(json.get("state").is_none());
        // Running state doesn't make two shapers different.
        let mut busy = Shaper::default();
        busy.trigger(1.0);
        busy.process(0.5, FPS60, 0.5);
        assert_eq!(busy, Shaper::default());
    }

    // ---------- safety ----------

    #[test]
    fn targets_are_the_lfo_allow_list() {
        let reg = reg();
        for id in ["transport.arm", "transport.blackout", "tempo.bpm", "tempo.tap", "cue.max_active", "grid.1.1.1", "master.rot.sync", "calibration.x_scale", "safety.strobe_max_hz", "nope"] {
            assert!(Target::bind(&reg, id).is_none(), "{id} must not be a target");
        }
        for id in reg.list().iter().map(|d| d.id.as_str()) {
            assert_eq!(Target::bind(&reg, id).is_some(), lfo::modulatable(&reg, id).is_some(), "{id}");
        }
        let size = Target::bind(&reg, "master.size").unwrap();
        assert_eq!((size.id(), size.range()), ("master.size", (0.0, 2.0)));
    }

    #[test]
    fn shaped_signals_never_arm_and_only_move_copies() {
        let reg = reg();
        let s = crate::test_support::shared();
        let size = Target::bind(&reg, "master.size").unwrap();
        let mut shaper = Shaper { gate: 0.0, attack: Span::Ms(0.0), ..Default::default() };
        let f = AudioFeatures { bass: 1.0, ..Default::default() };
        let (mut settings, mut live) = (s.settings.clone(), s.live.clone());
        let v = shaper.feed(Source::Value("bass"), &f, FPS60, 0.5);
        size.apply(v, &mut settings, &mut live);
        assert_eq!(live.size, 2.0, "base 1 + 1 × range 2, clamped to 2");
        assert_eq!(s.live.size, 1.0, "the stored value is the base");
        assert!(!s.gate.is_armed(), "audio never arms");
    }

    #[test]
    fn brightness_is_only_ever_dimmed() {
        let reg = reg();
        for id in ["master.brightness", "look.brightness"] {
            let target = Target::bind(&reg, id).unwrap();
            let mut up = Shaper { attack: Span::Ms(0.0), ..Default::default() };
            let mut down = Shaper { attack: Span::Ms(0.0), min: 0.0, max: -1.0, ..Default::default() };
            let mut f = AudioFeatures::default();
            up.feed(Source::Event("kick"), &f, 0.0, 0.5);
            down.feed(Source::Event("kick"), &f, 0.0, 0.5);
            for i in 0..120 {
                if i % 10 == 0 {
                    f.kick += 1;
                    f.kick_strength = 1.0;
                }
                let (a, b) = (up.feed(Source::Event("kick"), &f, FPS60, 0.5), down.feed(Source::Event("kick"), &f, FPS60, 0.5));
                for (amount, can_dim) in [(a, false), (b, true)] {
                    let mut settings = Settings { brightness: 0.6, ..Default::default() };
                    let mut live = LiveModifiers { brightness: 0.6, ..Default::default() };
                    target.apply(amount, &mut settings, &mut live);
                    let got = if id == "look.brightness" { settings.brightness } else { live.brightness };
                    assert!(got <= 0.6 + 1e-6, "{id}: never above the fader ({got})");
                    if can_dim && amount < -0.5 {
                        assert!(got < 0.1, "{id}: dims ({got})");
                    }
                }
            }
        }
    }

    #[test]
    fn audio_flashes_still_go_through_the_strobe_limiter() {
        // A kick-driven flash at 12 Hz on `master.brightness` (a pulse that
        // dims to black between hits), rendered and passed through the
        // T-101 stage as the engine does: after 5 s of burst the limiter
        // holds the output steady.
        let reg = reg();
        let target = Target::bind(&reg, "master.brightness").unwrap();
        let mut shaper = Shaper { attack: Span::Ms(0.0), decay: Span::Ms(40.0), min: -1.0, max: 0.0, ..Default::default() };
        let mut f = AudioFeatures::default();
        let mut animator = Animator::default();
        let base = Settings::default();
        let mut limiter = StrobeLimiter::default();
        let cfg = SafetySettings::default();
        let dt = FPS60 as f64;
        let mut lit_after_6s = Vec::new();
        for i in 0..(8 * 60) {
            let t = i as f64 * dt;
            if i % 5 == 0 {
                f.kick += 1; // 12 kicks a second
            }
            let amount = shaper.feed(Source::Event("kick"), &f, FPS60, 0.5);
            let (settings, mut live) = (base.clone(), LiveModifiers::default());
            let mut look = settings.clone();
            target.apply(amount, &mut look, &mut live);
            let points = animator.render(&look, f, FPS60, &BeatClock::default());
            let st = crate::live::LiveState::default();
            let frame: Vec<Point> = crate::live::apply(&points, &live, &st, &[]);
            let out = safety::apply(frame, t, &cfg, &mut limiter);
            if t > 6.0 {
                lit_after_6s.push(safety::level(&out) > 0.05);
            }
        }
        assert!(limiter.status().active, "the limiter caught the audio strobe: {:?}", limiter.status());
        assert!(lit_after_6s.iter().all(|lit| *lit), "held steady, no more flashing");
    }

    #[test]
    fn a_frame_of_shaping_does_not_allocate() {
        let reg = reg();
        let (size, bright) = (Target::bind(&reg, "master.size").unwrap(), Target::bind(&reg, "master.brightness").unwrap());
        let mut shapers = [Shaper::default(), Shaper { decay: Span::Beats(0.25), ..Default::default() }];
        let mut f = AudioFeatures { bass: 0.5, ..Default::default() };
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        let n = crate::audio::capture::tests::allocations_during(|| {
            for i in 0..600 {
                f.kick += (i % 30 == 0) as u64;
                f.bass = (i as f32 * 0.05).sin().abs();
                let a = shapers[0].feed(Source::Value("bass"), &f, FPS60, 0.5);
                let b = shapers[1].feed(Source::Event("kick"), &f, FPS60, 0.5);
                size.apply(a, &mut settings, &mut live);
                bright.apply(-b, &mut settings, &mut live);
            }
        });
        assert_eq!(n, 0);
    }
}
