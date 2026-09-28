//! Evolving cues (T-111): a cue whose look changes by itself over N beats,
//! through keyframes - a fan that rises and speeds up into the drop, a
//! tunnel that shrinks then pumps. Each key sets a generator, its
//! parameters, a colour, a size and a brightness at a beat; between keys
//! the numbers are eased and the rest switches exactly on the key's beat.
//!
//! Everything here is a pure function of the cue's local beat position
//! (beats since its quantized launch, read from the one tempo clock), so
//! a cue plays the same at any BPM, loops exactly, and is testable without
//! a clock. `engine::Animator` turns a sample into a frame with the usual
//! generator path. Timelines (T-160) chain these on a bar grid.

use crate::beat;
use crate::generators::GenParams;
use serde::{Deserialize, Serialize};

/// How a key's numeric values move towards the next key's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyEase {
    /// Held until the next key, then a jump (« Palier »).
    Step,
    /// Constant rate (« Linéaire »).
    #[default]
    Linear,
    /// Slow start, fast end (« Accélère »).
    EaseIn,
    /// Fast start, slow end (« Ralentit »).
    EaseOut,
    /// Slow at both ends (« Douce »).
    Smooth,
}

impl KeyEase {
    /// Progress 0..1 through a segment → how far the values have moved, 0..1.
    pub fn apply(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            KeyEase::Step => 0.0,
            KeyEase::Linear => u,
            KeyEase::EaseIn => beat::ease_in(u),
            KeyEase::EaseOut => 1.0 - beat::ease_in(1.0 - u),
            KeyEase::Smooth => beat::smoothstep(u),
        }
    }
}

/// When a launched evolving cue starts counting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Launch {
    /// On the next beat (or at once if pressed on the beat).
    #[default]
    Beat,
    /// On the next bar's « one ».
    Bar,
}

/// Launch presses this close after a beat (in beats) count as on it.
const ON_THE_BEAT: f64 = 1e-3;

impl Launch {
    /// The tempo-clock beat a cue pressed at `beat` starts on.
    pub fn quantize(self, beat: f64, beats_per_bar: u8) -> f64 {
        let grid = match self {
            Launch::Beat => 1.0,
            Launch::Bar => beats_per_bar.max(1) as f64,
        };
        let before = (beat / grid).floor() * grid;
        if beat - before < ON_THE_BEAT { before } else { before + grid }
    }
}

/// One keyframe. Missing fields take the defaults, so a key only needs what
/// it changes from them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EvolvingKey {
    /// Beat of the cue (0 = its launch) where this key is reached.
    pub at_beats: f32,
    /// A generator name from `generators::GENERATOR_NAMES`.
    pub generator: String,
    pub params: GenParams,
    pub color: [u8; 3],
    /// The look's half-extent, 0..1.
    pub scale: f32,
    /// 0..1, multiplied by the operator's look brightness.
    pub brightness: f32,
    /// Lit for this many beats after each beat (0 = no gate).
    pub gate_beats: f32,
    /// Strobe: this many flashes per beat, half on / half off (0 = none).
    pub strobe_div: f32,
    /// How the values move from this key to the next one.
    pub ease: KeyEase,
}

impl Default for EvolvingKey {
    fn default() -> Self {
        Self {
            at_beats: 0.0,
            generator: "fan".to_string(),
            params: GenParams::default(),
            color: [0, 255, 0],
            scale: 0.7,
            brightness: 1.0,
            gate_beats: 0.0,
            strobe_div: 0.0,
            ease: KeyEase::Linear,
        }
    }
}

/// Most strobe flashes per beat (the T-101 limiter still applies on top).
pub const MAX_STROBE_DIV: f32 = 8.0;
/// Shortest cue and shortest motion period, in beats.
const MIN_BEATS: f32 = 1.0 / 16.0;

/// `Content::Evolving`: keys over `length_beats`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EvolvingCue {
    pub length_beats: f32,
    /// At the end: back to key 0 (`true`), or stay on the last key.
    #[serde(rename = "loop")]
    pub looped: bool,
    pub launch: Launch,
    /// In any order; a key at `length_beats` is the value the cue ends on.
    pub keys: Vec<EvolvingKey>,
}

impl Default for EvolvingCue {
    fn default() -> Self {
        Self { length_beats: 16.0, looped: false, launch: Launch::Beat, keys: Vec::new() }
    }
}

/// The look of an evolving cue at one beat.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Index (in `keys`) of the key the cue is on.
    pub key: usize,
    pub generator: String,
    /// Interpolated parameters. `period_beats` is the *effective* period:
    /// with a period that changes over time, it is chosen so that the
    /// motion's phase (`beat_pos / period_beats`) keeps moving smoothly
    /// instead of jumping (see `EvolvingCue::cycles`).
    pub params: GenParams,
    pub color: [u8; 3],
    pub scale: f32,
    pub brightness: f32,
    pub gate_beats: f32,
    pub strobe_div: f32,
}

/// Where a cue is in its keys at one beat: the key it is on, the key it
/// moves towards (if any) and how far it has moved (eased, 0..1).
struct Segment<'a> {
    index: usize,
    cur: &'a EvolvingKey,
    next: Option<&'a EvolvingKey>,
    e: f32,
}

impl EvolvingCue {
    pub fn length(&self) -> f64 {
        self.length_beats.max(MIN_BEATS) as f64
    }

    /// Keys inside the cue (a key past its end is never reached).
    fn keys_in(&self) -> impl Iterator<Item = (usize, &EvolvingKey)> {
        let len = self.length() as f32;
        self.keys.iter().enumerate().filter(move |(_, k)| k.at_beats.is_finite() && k.at_beats <= len)
    }

    /// The cue's position at `raw` beats after its launch: wrapped for a
    /// loop, as-is otherwise (past the end, the last key holds while its
    /// generator keeps moving). Before the launch: 0.
    pub fn local(&self, raw: f64) -> f64 {
        let raw = raw.max(0.0);
        if self.looped { raw.rem_euclid(self.length()) } else { raw }
    }

    /// Whether a one-shot cue has played to its end.
    pub fn ended(&self, raw: f64) -> bool {
        !self.looped && raw >= self.length()
    }

    fn segment(&self, b: f32) -> Option<Segment<'_>> {
        let len = self.length() as f32;
        let first = self.keys_in().min_by(|a, b| a.1.at_beats.total_cmp(&b.1.at_beats))?;
        let last = self.keys_in().max_by(|a, b| a.1.at_beats.total_cmp(&b.1.at_beats))?;
        // The key reached last at or before b (on a tie, the later one in the list).
        let reached = self.keys_in().filter(|(_, k)| k.at_beats <= b).max_by(|a, b| a.1.at_beats.total_cmp(&b.1.at_beats));
        let ahead = self.keys_in().filter(|(_, k)| k.at_beats > b).min_by(|a, b| a.1.at_beats.total_cmp(&b.1.at_beats));
        let ((index, cur), cur_at) = match reached {
            Some(r) => (r, r.1.at_beats),
            // Before the first key: a loop comes from its last key, else hold the first.
            None if self.looped => (last, last.1.at_beats - len),
            None => return Some(Segment { index: first.0, cur: first.1, next: None, e: 0.0 }),
        };
        let (next, next_at) = match ahead {
            Some((_, k)) => (k, k.at_beats),
            None if self.looped => (first.1, first.1.at_beats + len),
            None => return Some(Segment { index, cur, next: None, e: 0.0 }),
        };
        let span = next_at - cur_at;
        let e = if span > 0.0 { cur.ease.apply((b - cur_at) / span) } else { 0.0 };
        Some(Segment { index, cur, next: Some(next), e })
    }

    /// The look at local beat `b` (see `local`); `None` without keys.
    pub fn sample(&self, b: f64) -> Option<Sample> {
        let seg = self.segment(b as f32)?;
        let (cur, e) = (seg.cur, seg.e);
        let lerp = |a: f32, b: f32| a + (b - a) * e;
        let lerp_rgb = |a: [u8; 3], b: [u8; 3]| std::array::from_fn(|i| lerp(a[i] as f32, b[i] as f32).round().clamp(0.0, 255.0) as u8);
        let mut params = cur.params.clone();
        let (mut color, mut scale, mut brightness, mut gate) = (cur.color, cur.scale, cur.brightness, cur.gate_beats);
        if let Some(next) = seg.next {
            color = lerp_rgb(cur.color, next.color);
            scale = lerp(cur.scale, next.scale);
            brightness = lerp(cur.brightness, next.brightness);
            gate = lerp(cur.gate_beats, next.gate_beats);
            // Parameters only mean the same thing within one generator.
            if next.generator == cur.generator {
                let (p, q) = (&cur.params, &next.params);
                params.a = lerp(p.a, q.a);
                params.b = lerp(p.b, q.b);
                params.speed = lerp(p.speed, q.speed);
                params.color2 = lerp_rgb(p.color2, q.color2);
            }
        }
        // Beat-locked: the cue plays in beats whatever the key says, and the
        // key's own gate is the one that applies.
        params.beat_sync = true;
        params.gate_beats = gate.max(0.0);
        let cycles = self.cycles(b);
        params.period_beats = if b > 0.0 && cycles > 0.0 { (b / cycles) as f32 } else { self.period_at(0.0) };
        Some(Sample {
            key: seg.index,
            generator: cur.generator.clone(),
            params,
            color,
            scale: scale.clamp(0.0, 1.0),
            brightness: brightness.clamp(0.0, 1.0),
            gate_beats: gate.max(0.0),
            strobe_div: cur.strobe_div.clamp(0.0, MAX_STROBE_DIV),
        })
    }

    /// The main motion's period at local beat `b`, eased between keys of
    /// the same generator like the other numbers.
    fn period_at(&self, b: f32) -> f32 {
        let Some(seg) = self.segment(b) else { return 4.0 };
        let p = seg.cur.params.period_beats;
        let p = match seg.next {
            Some(next) if next.generator == seg.cur.generator => p + (next.params.period_beats - p) * seg.e,
            _ => p,
        };
        p.max(MIN_BEATS)
    }

    /// Motion cycles done by local beat `b`: ∫ 1/period. A generator is
    /// handed `period_beats = b / cycles`, so its phase `b / period_beats`
    /// runs on smoothly when the period eases (a rotation that speeds up)
    /// or steps, instead of jumping. With a constant period it is `b / p`.
    fn cycles(&self, b: f64) -> f64 {
        if b <= 0.0 {
            return 0.0;
        }
        // Break at every key: the period is smooth between two keys.
        let mut cuts: Vec<f64> = self.keys_in().map(|(_, k)| k.at_beats as f64).filter(|&t| t > 0.0 && t < b).collect();
        cuts.push(0.0);
        cuts.push(b);
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        let inv = |t: f64| 1.0 / self.period_at(t as f32) as f64;
        cuts.windows(2)
            .map(|w| {
                let (x0, x1) = (w[0], w[1]);
                // Sample just inside the piece: the period jumps at a key.
                let (lo, hi) = (x0 + (x1 - x0) * 1e-6, x1 - (x1 - x0) * 1e-6);
                let (a, z) = (inv(lo), inv(hi));
                if (a - z).abs() < 1e-12 && (a - inv((lo + hi) / 2.0)).abs() < 1e-12 {
                    return (x1 - x0) * a;
                }
                // Simpson's rule, 32 intervals: plenty for an eased period.
                const N: usize = 32;
                let h = (hi - lo) / N as f64;
                let sum: f64 = (0..=N)
                    .map(|i| {
                        let w = if i == 0 || i == N { 1.0 } else if i % 2 == 1 { 4.0 } else { 2.0 };
                        w * inv(lo + h * i as f64)
                    })
                    .sum();
                (x1 - x0) * sum * h / 3.0 / (hi - lo)
            })
            .sum()
    }
}

/// Strobe gain at local beat `b`: `div` flashes per beat, each lit for the
/// first half of its slot (1 = no strobe).
pub fn strobe(b: f64, div: f32) -> f32 {
    if div <= 0.0 {
        return 1.0;
    }
    if (b * div as f64 + 1e-9).rem_euclid(1.0) < 0.5 { 1.0 } else { 0.0 }
}

/// What the UI and the API show of a playing evolving cue.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Progress {
    /// Local beat (wrapped for a loop).
    pub pos: f64,
    pub length: f64,
    /// Index of the current key, and how many keys there are.
    pub key: usize,
    pub keys: usize,
    #[serde(rename = "loop")]
    pub looped: bool,
    /// Launched, waiting for its quantized start.
    pub waiting: bool,
    /// A one-shot cue past its end (holding its last key).
    pub ended: bool,
    /// Loops done so far.
    pub pass: u64,
    pub beats_per_bar: u8,
}

/// A 16-beat test cue: a fan growing from size 0.2 to 0.6 over 8 beats,
/// then a wave from beat 8 (for tests across modules).
#[cfg(test)]
pub fn test_cue(looped: bool) -> EvolvingCue {
    let key = |at: f32, generator: &str, scale: f32| EvolvingKey { at_beats: at, generator: generator.into(), scale, ..Default::default() };
    EvolvingCue { length_beats: 16.0, looped, launch: Launch::Beat, keys: vec![key(0.0, "fan", 0.2), key(8.0, "fan_wave", 0.6)] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(at: f32, generator: &str, scale: f32) -> EvolvingKey {
        EvolvingKey { at_beats: at, generator: generator.into(), scale, ..Default::default() }
    }

    fn cue(len: f32, looped: bool, keys: Vec<EvolvingKey>) -> EvolvingCue {
        EvolvingCue { length_beats: len, looped, launch: Launch::Beat, keys }
    }

    #[test]
    fn eases_start_at_0_and_end_at_1() {
        for e in [KeyEase::Linear, KeyEase::EaseIn, KeyEase::EaseOut, KeyEase::Smooth] {
            assert_eq!((e.apply(0.0), e.apply(1.0)), (0.0, 1.0), "{e:?}");
            assert!(e.apply(0.5) > 0.0 && e.apply(0.5) < 1.0);
        }
        assert_eq!(KeyEase::Step.apply(0.99), 0.0);
        assert!(KeyEase::EaseIn.apply(0.25) < 0.25 && KeyEase::EaseOut.apply(0.25) > 0.25);
        assert_eq!(KeyEase::Smooth.apply(0.5), 0.5);
    }

    #[test]
    fn scale_is_linear_half_way_through_a_16_beat_cue() {
        let c = cue(16.0, false, vec![key(0.0, "fan", 0.2), key(16.0, "fan", 1.0)]);
        let at = |b: f64| c.sample(b).unwrap().scale;
        assert!((at(0.0) - 0.2).abs() < 1e-6);
        assert!((at(8.0) - 0.6).abs() < 1e-6);
        assert!((at(4.0) - 0.4).abs() < 1e-6);
        assert!((at(16.0) - 1.0).abs() < 1e-6);
        // Past the end of a one-shot cue: the last key holds.
        assert!((at(40.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn eased_keys_follow_their_curve() {
        let mut a = key(0.0, "fan", 0.0);
        a.ease = KeyEase::EaseIn;
        let c = cue(8.0, false, vec![a, key(8.0, "fan", 1.0)]);
        assert!((c.sample(4.0).unwrap().scale - 0.25).abs() < 1e-6);
        let mut s = key(0.0, "fan", 0.0);
        s.ease = KeyEase::Step;
        let c = cue(8.0, false, vec![s, key(8.0, "fan", 1.0)]);
        assert_eq!(c.sample(7.99).unwrap().scale, 0.0);
        assert_eq!(c.sample(8.0).unwrap().scale, 1.0);
    }

    #[test]
    fn generator_switches_exactly_on_the_key_beat() {
        let c = cue(16.0, false, vec![key(0.0, "fan", 0.5), key(8.0, "fan_sweep", 0.5)]);
        assert_eq!(c.sample(7.999).unwrap().generator, "fan");
        assert_eq!(c.sample(8.0).unwrap().generator, "fan_sweep");
        assert_eq!(c.sample(8.0).unwrap().key, 1);
        // Discrete parameters too: count, colour mode, steps.
        let mut k1 = key(4.0, "fan", 0.5);
        k1.params.count = 16;
        k1.params.steps_per_beat = 4.0;
        let mut k0 = key(0.0, "fan", 0.5);
        k0.params.count = 4;
        let c = cue(8.0, false, vec![k0, k1]);
        assert_eq!(c.sample(3.99).unwrap().params.count, 4);
        assert_eq!(c.sample(3.99).unwrap().params.steps_per_beat, 1.0);
        assert_eq!(c.sample(4.0).unwrap().params.count, 16);
        assert_eq!(c.sample(4.0).unwrap().params.steps_per_beat, 4.0);
    }

    #[test]
    fn numbers_interpolate_colours_too_but_params_only_within_a_generator() {
        let mut a = key(0.0, "fan", 0.5);
        (a.color, a.params.a, a.params.color2, a.brightness) = ([0, 0, 0], 0.0, [0, 0, 0], 0.0);
        let mut b = key(4.0, "fan", 0.5);
        (b.color, b.params.a, b.params.color2, b.brightness) = ([200, 100, 0], 1.0, [0, 0, 250], 1.0);
        let c = cue(4.0, false, vec![a.clone(), b.clone()]);
        let s = c.sample(2.0).unwrap();
        assert_eq!((s.color, s.params.color2), ([100, 50, 0], [0, 0, 125]));
        assert!((s.params.a - 0.5).abs() < 1e-6 && (s.brightness - 0.5).abs() < 1e-6);
        // Towards another generator, the look (colour) still moves, its params hold.
        b.generator = "fan_wave".into();
        let s = cue(4.0, false, vec![a, b]).sample(2.0).unwrap();
        assert_eq!(s.color, [100, 50, 0]);
        assert_eq!(s.params.a, 0.0);
    }

    #[test]
    fn a_loop_comes_back_to_key_0_at_its_length() {
        let c = cue(16.0, true, vec![key(0.0, "fan", 0.2), key(8.0, "fan_sweep", 1.0)]);
        assert_eq!(c.local(16.0), 0.0);
        assert_eq!(c.local(19.0), 3.0);
        let at = |raw: f64| c.sample(c.local(raw)).unwrap();
        assert_eq!(at(16.0), at(0.0));
        assert_eq!(at(16.0 * 5.0 + 3.0), at(3.0));
        assert_eq!(at(15.99).generator, "fan_sweep");
        assert_eq!(at(16.0).generator, "fan");
        // The last key eases back towards key 0 over the rest of the loop.
        assert!((at(12.0).scale - 0.6).abs() < 1e-6);
        // A one-shot cue stays on its last key.
        let once = cue(16.0, false, c.keys.clone());
        assert_eq!(once.sample(once.local(20.0)).unwrap().generator, "fan_sweep");
        assert!((once.sample(once.local(20.0)).unwrap().scale - 1.0).abs() < 1e-6);
        assert!(once.ended(16.0) && !once.ended(15.9) && !c.ended(100.0));
    }

    #[test]
    fn keys_in_any_order_and_a_first_key_after_0() {
        let c = cue(16.0, false, vec![key(12.0, "fan_wave", 0.9), key(4.0, "fan", 0.1)]);
        assert_eq!(c.sample(0.0).unwrap().generator, "fan");
        assert_eq!(c.sample(2.0).unwrap().scale, 0.1);
        assert!((c.sample(8.0).unwrap().scale - 0.5).abs() < 1e-6);
        assert_eq!(c.sample(12.0).unwrap().key, 0);
        // Keys past the end are never reached; no keys = nothing to draw.
        let c = cue(4.0, false, vec![key(0.0, "fan", 0.3), key(9.0, "fan_wave", 0.9)]);
        assert_eq!(c.sample(3.9).unwrap().scale, 0.3);
        assert!(cue(4.0, true, Vec::new()).sample(1.0).is_none());
    }

    #[test]
    fn launch_waits_for_the_next_beat_or_bar() {
        assert_eq!(Launch::Beat.quantize(5.3, 4), 6.0);
        assert_eq!(Launch::Beat.quantize(5.0, 4), 5.0);
        assert_eq!(Launch::Beat.quantize(5.0004, 4), 5.0);
        assert_eq!(Launch::Beat.quantize(4.9999, 4), 5.0);
        assert_eq!(Launch::Bar.quantize(5.3, 4), 8.0);
        assert_eq!(Launch::Bar.quantize(8.0, 4), 8.0);
        assert_eq!(Launch::Bar.quantize(8.2, 3), 9.0);
        assert_eq!(Launch::Beat.quantize(-0.5, 4), 0.0);
    }

    #[test]
    fn constant_period_is_left_alone_and_an_eased_one_keeps_the_phase_smooth() {
        let mut k = key(0.0, "fan_sweep", 0.5);
        k.params.period_beats = 4.0;
        let c = cue(16.0, false, vec![k.clone()]);
        for b in [0.0, 0.5, 3.0, 7.25, 15.0] {
            assert!((c.sample(b).unwrap().params.period_beats - 4.0).abs() < 1e-5, "b = {b}");
        }
        // Period 8 → 1 over 16 beats: the phase b / period never goes back.
        let mut fast = k.clone();
        (fast.at_beats, fast.params.period_beats) = (16.0, 1.0);
        let c = cue(16.0, false, vec![{ let mut s = k.clone(); s.params.period_beats = 8.0; s }, fast]);
        let phase = |b: f64| b / c.sample(b).unwrap().params.period_beats as f64;
        let mut last = 0.0;
        for i in 1..=640 {
            let b = i as f64 / 40.0;
            let p = phase(b);
            assert!(p > last && p - last < 1.1 / 40.0, "phase jumped at b = {b}: {last} → {p}");
            last = p;
        }
        // ∫ dx / (8 - 7x/16) over 0..16 = (16/7)·ln 8.
        assert!((last - 16.0 / 7.0 * 8f64.ln()).abs() < 1e-3, "{last}");
        // A stepped period: cycles add up piece by piece, no jump at the key.
        let mut s0 = k.clone();
        (s0.params.period_beats, s0.ease) = (4.0, KeyEase::Step);
        let mut s1 = k;
        (s1.at_beats, s1.params.period_beats) = (2.0, 1.0);
        let c = cue(8.0, false, vec![s0, s1]);
        let phase = |b: f64| b / c.sample(b).unwrap().params.period_beats as f64;
        assert!((phase(2.0) - 0.5).abs() < 1e-4 && (phase(3.0) - 1.5).abs() < 1e-4);
    }

    #[test]
    fn strobe_flashes_div_times_per_beat() {
        assert_eq!(strobe(3.3, 0.0), 1.0);
        assert_eq!((strobe(3.0, 2.0), strobe(3.3, 2.0), strobe(3.5, 2.0), strobe(3.8, 2.0)), (1.0, 0.0, 1.0, 0.0));
        assert_eq!((strobe(3.1, 4.0), strobe(3.2, 4.0), strobe(3.25, 4.0)), (1.0, 0.0, 1.0));
        let mut k = key(0.0, "fan", 0.5);
        k.strobe_div = 99.0;
        assert_eq!(cue(4.0, false, vec![k]).sample(0.0).unwrap().strobe_div, MAX_STROBE_DIV);
    }

    #[test]
    fn json_round_trips_with_defaults() {
        let c: EvolvingCue = serde_json::from_str(
            r#"{"length_beats":8,"loop":true,"keys":[{"at_beats":0,"generator":"fan"},{"at_beats":4,"generator":"fan_tilt","ease":"ease_in","scale":0.3}]}"#,
        )
        .unwrap();
        assert!(c.looped);
        assert_eq!(c.launch, Launch::Beat);
        assert_eq!(c.keys[0].brightness, 1.0);
        assert_eq!(c.keys[1].ease, KeyEase::EaseIn);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""loop":true"#) && json.contains(r#""launch":"beat""#));
        assert_eq!(serde_json::from_str::<EvolvingCue>(&json).unwrap(), c);
    }
}
