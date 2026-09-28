//! Beat maths shared by the generators: phases, envelopes, easing, steps,
//! seeded randomness and virtual beam groups. Everything here is a pure
//! function of a beat position read from the one tempo clock
//! (`tempo.rs`), so a look lands on the downbeat at any BPM and stays
//! testable without a clock.

// The helpers are the toolkit of the festival generators (T-102..T-108);
// until those land, most are only called from tests.
#![cfg_attr(not(test), allow(dead_code))]

use serde::{Deserialize, Serialize};

/// Position in a cycle of `period_beats`, 0..1. A period of 0 or less
/// means "no cycle" and gives 0.
pub fn phase(beat_pos: f64, period_beats: f32) -> f32 {
    if period_beats <= 0.0 {
        return 0.0;
    }
    (beat_pos / period_beats as f64).rem_euclid(1.0) as f32
}

/// A stab envelope over one beat: instant attack at `phase_in_beat` 0,
/// held for `gate_beats`, then cut (`decay_beats` = 0) or decaying
/// exponentially with time constant `decay_beats`. Both 0 = no envelope
/// (always 1).
pub fn env_stab(phase_in_beat: f32, gate_beats: f32, decay_beats: f32) -> f32 {
    let x = phase_in_beat.rem_euclid(1.0);
    let gate = gate_beats.max(0.0);
    if x < gate || (gate == 0.0 && decay_beats <= 0.0) {
        return 1.0;
    }
    if decay_beats <= 0.0 {
        return 0.0;
    }
    (-(x - gate) / decay_beats).exp()
}

/// Sine ease-in-out on 0..1 (clamped).
pub fn ease_sine(x: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * x.clamp(0.0, 1.0)).cos()
}

/// Quadratic ease-in on 0..1 (clamped): slow start, fast end.
pub fn ease_in(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x
}

/// Cubic smoothstep on 0..1 (clamped).
pub fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Which step of a chase we are on: `steps_per_beat` steps per beat,
/// counted from the cue's start (0 before it, or when steps are off).
pub fn step_index(beat_pos: f64, steps_per_beat: f32) -> u64 {
    if steps_per_beat <= 0.0 || beat_pos <= 0.0 {
        return 0;
    }
    // A hair of tolerance so a step lands exactly on its beat despite
    // floating-point noise in the clock.
    (beat_pos * steps_per_beat as f64 + 1e-9).floor() as u64
}

/// Deterministic pseudo-random number in 0..1 for (`seed`, `i`) - for
/// star positions, random chases... Same inputs, same output, so frames
/// and tests are reproducible. (SplitMix64 finaliser.)
pub fn seeded_rand(seed: u64, i: u64) -> f32 {
    let mut z = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ i.wrapping_add(0x632b_e59b_d9b4_e019);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// The first beat of the bar containing `beat` (the "one").
pub fn bar_start(beat: f64, beats_per_bar: u8) -> f64 {
    let bpb = beats_per_bar.max(1) as f64;
    // Tolerance: a launch quantized to a bar may land a hair before it.
    ((beat + 1e-6) / bpb).floor() * bpb
}

/// How a travelling motion (a scanner line) starts its next pass: jump
/// back to the start, or come back the other way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    /// Start again from the beginning (the jump back is a move, not a flash).
    Wrap,
    /// Go back and forth.
    #[default]
    PingPong,
}

impl LoopMode {
    /// Position along the path, 0..1, after `passes` passes (one pass =
    /// once from start to end).
    pub fn travel(self, passes: f64) -> f32 {
        match self {
            LoopMode::Wrap => passes.rem_euclid(1.0) as f32,
            LoopMode::PingPong => {
                let x = passes.rem_euclid(2.0) as f32;
                if x <= 1.0 { x } else { 2.0 - x }
            }
        }
    }
}

/// Shape of a back-and-forth motion (a fan sweep): how the move eases
/// into its extremes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Easing {
    /// Slows down at the ends, like a moving head.
    #[default]
    Sine,
    /// Constant speed, sharp turn at the ends.
    Triangle,
    /// Constant speed, then held still at each end for a quarter of the
    /// cycle.
    Trapezoid,
}

impl Easing {
    /// One back-and-forth swing per cycle, in -1..1: 0 at phase 0, +1 at a
    /// quarter, 0 at half, -1 at three quarters (all three shapes agree on
    /// those points, so switching easing keeps the timing).
    pub fn swing(self, phase: f32) -> f32 {
        let x = phase.rem_euclid(1.0);
        let tri = if x < 0.25 {
            4.0 * x
        } else if x < 0.75 {
            2.0 - 4.0 * x
        } else {
            4.0 * x - 4.0
        };
        match self {
            Easing::Sine => (std::f32::consts::TAU * x).sin(),
            Easing::Triangle => tri,
            // |tri| >= 0.5 for half the cycle: a quarter held at each end.
            Easing::Trapezoid => (2.0 * tri).clamp(-1.0, 1.0),
        }
    }
}

/// How the virtual groups of a generator's beams move relative to each
/// other ("virtual heads").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupMode {
    /// Every group moves the same way at the same time.
    #[default]
    Unison,
    /// Odd groups move the opposite way (a mirror image around the centre).
    Mirror,
    /// Group g runs g/groups of a cycle later: a travelling wave.
    Offset,
}

/// Group of beam `i` out of `n` when split into `groups` contiguous groups
/// (as even as possible). `groups` is clamped to 1..=4.
pub fn group_of(i: usize, n: usize, groups: u32) -> usize {
    let g = groups.clamp(1, 4) as usize;
    if n == 0 {
        return 0;
    }
    (i.min(n - 1) * g / n).min(g - 1)
}

impl GroupMode {
    /// The motion of group `g`: its phase in the cycle (0..1) and the sign
    /// to apply to its movement (1 or -1). A generator moves group g by
    /// `sign * amplitude * shape(phase)`.
    pub fn motion(self, g: usize, groups: u32, phase: f32) -> (f32, f32) {
        let groups = groups.clamp(1, 4) as usize;
        match self {
            GroupMode::Unison => (phase, 1.0),
            GroupMode::Mirror => (phase, if g % 2 == 1 { -1.0 } else { 1.0 }),
            GroupMode::Offset => ((phase + g as f32 / groups as f32).rem_euclid(1.0), 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tempo-clock beat at `t` seconds for a clock started at 0.
    fn beat_at(bpm: f64, t: f64) -> f64 {
        t * bpm / 60.0
    }

    #[test]
    fn phase_wraps_every_period_at_any_tempo() {
        for bpm in [128.0, 150.0] {
            let bar = 4.0 * 60.0 / bpm; // seconds per 4 beats
            let a = phase(beat_at(bpm, 0.7), 4.0);
            let b = phase(beat_at(bpm, 0.7 + bar), 4.0);
            assert!((a - b).abs() < 1e-5, "{bpm}: {a} vs {b}");
            assert!((phase(beat_at(bpm, bar / 2.0), 4.0) - 0.5).abs() < 1e-6);
        }
        assert_eq!(phase(5.0, 0.0), 0.0);
        assert!((phase(-1.0, 4.0) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn env_stab_gates_and_decays() {
        // Hard gate: on for 0.2 beat, then off.
        assert_eq!(env_stab(0.0, 0.2, 0.0), 1.0);
        assert_eq!(env_stab(0.19, 0.2, 0.0), 1.0);
        assert_eq!(env_stab(0.21, 0.2, 0.0), 0.0);
        // Decay: instant attack, e^-1 after one time constant.
        assert_eq!(env_stab(0.0, 0.0, 0.12), 1.0);
        assert!((env_stab(0.12, 0.0, 0.12) - (-1.0f32).exp()).abs() < 1e-6);
        assert!(env_stab(0.9, 0.0, 0.12) < 0.01);
        // Nothing set: no envelope.
        assert_eq!(env_stab(0.7, 0.0, 0.0), 1.0);
        // At 128 and 150 BPM the gate follows the beat, not the seconds.
        for bpm in [128.0, 150.0] {
            let beat = beat_at(bpm, 10.0 * 60.0 / bpm + 0.1 * 60.0 / bpm); // 0.1 beat after a beat
            assert_eq!(env_stab(beat.fract() as f32, 0.2, 0.0), 1.0);
            let beat = beat_at(bpm, 10.5 * 60.0 / bpm);
            assert_eq!(env_stab(beat.fract() as f32, 0.2, 0.0), 0.0);
        }
    }

    #[test]
    fn easing_curves_start_at_0_and_end_at_1() {
        for f in [ease_sine, ease_in, smoothstep] {
            assert!(f(0.0).abs() < 1e-6 && (f(1.0) - 1.0).abs() < 1e-6);
            assert!(f(-3.0).abs() < 1e-6 && (f(3.0) - 1.0).abs() < 1e-6);
        }
        assert!((ease_sine(0.5) - 0.5).abs() < 1e-6);
        assert!(ease_in(0.5) < 0.5);
    }

    #[test]
    fn step_index_counts_steps_at_128_and_150_bpm() {
        for bpm in [128.0, 150.0] {
            let beat_s = 60.0 / bpm;
            assert_eq!(step_index(beat_at(bpm, 0.0), 1.0), 0);
            assert_eq!(step_index(beat_at(bpm, 3.0 * beat_s), 1.0), 3, "exactly on a beat");
            assert_eq!(step_index(beat_at(bpm, 3.5 * beat_s), 2.0), 7);
            assert_eq!(step_index(beat_at(bpm, 3.9 * beat_s), 0.5), 1);
        }
        assert_eq!(step_index(12.0, 0.0), 0);
        assert_eq!(step_index(-2.0, 4.0), 0);
    }

    #[test]
    fn seeded_rand_is_deterministic_and_spread() {
        assert_eq!(seeded_rand(7, 3), seeded_rand(7, 3));
        assert_ne!(seeded_rand(7, 3), seeded_rand(7, 4));
        assert_ne!(seeded_rand(7, 3), seeded_rand(8, 3));
        let values: Vec<f32> = (0..1000).map(|i| seeded_rand(42, i)).collect();
        assert!(values.iter().all(|v| (0.0..1.0).contains(v)));
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!((mean - 0.5).abs() < 0.05, "mean {mean}");
    }

    #[test]
    fn bar_start_finds_the_one() {
        assert_eq!(bar_start(0.0, 4), 0.0);
        assert_eq!(bar_start(6.3, 4), 4.0);
        assert_eq!(bar_start(7.9999999, 4), 8.0, "a hair before the bar counts as on it");
        assert_eq!(bar_start(5.0, 3), 3.0);
    }

    #[test]
    fn groups_split_beams_contiguously() {
        let groups: Vec<usize> = (0..8).map(|i| group_of(i, 8, 2)).collect();
        assert_eq!(groups, [0, 0, 0, 0, 1, 1, 1, 1]);
        let groups: Vec<usize> = (0..6).map(|i| group_of(i, 6, 4)).collect();
        assert_eq!(groups, [0, 0, 1, 2, 2, 3]);
        assert_eq!(group_of(3, 5, 1), 0);
        assert_eq!(group_of(9, 5, 9), 3, "clamped to 4 groups and to the last beam");
    }

    #[test]
    fn group_modes_move_groups_together_mirrored_or_offset() {
        // A sweep: group g sits at sign * 0.3 * sin(2π phase).
        let x = |mode: GroupMode, g: usize, groups: u32, p: f32| {
            let (ph, sign) = mode.motion(g, groups, p);
            sign * 0.3 * (std::f32::consts::TAU * ph).sin()
        };
        let p = 0.2;
        assert_eq!(x(GroupMode::Unison, 0, 2, p), x(GroupMode::Unison, 1, 2, p));
        let (a, b) = (x(GroupMode::Mirror, 0, 2, p), x(GroupMode::Mirror, 1, 2, p));
        assert!(a > 0.1 && (a + b).abs() < 1e-6, "mirror groups must have opposite x offsets: {a} {b}");
        let (ph, _) = GroupMode::Offset.motion(1, 4, 0.9);
        assert!((ph - 0.15).abs() < 1e-6);
        assert_eq!(GroupMode::Offset.motion(0, 4, 0.9).0, 0.9);
    }

    #[test]
    fn easings_swing_through_the_same_key_points() {
        for e in [Easing::Sine, Easing::Triangle, Easing::Trapezoid] {
            assert!(e.swing(0.0).abs() < 1e-6, "{e:?}");
            assert!((e.swing(0.25) - 1.0).abs() < 1e-6, "{e:?}");
            assert!(e.swing(0.5).abs() < 1e-5, "{e:?}");
            assert!((e.swing(0.75) + 1.0).abs() < 1e-6, "{e:?}");
            assert!((e.swing(1.3) - e.swing(0.3)).abs() < 1e-5, "{e:?} wraps");
            assert!((0..100).all(|i| e.swing(i as f32 / 100.0).abs() <= 1.0 + 1e-6));
        }
        // Trapezoid: held at the top from 1/8 to 3/8 of the cycle.
        assert_eq!(Easing::Trapezoid.swing(0.13), 1.0);
        assert_eq!(Easing::Trapezoid.swing(0.37), 1.0);
        assert!(Easing::Trapezoid.swing(0.1) < 1.0);
        assert!((Easing::Triangle.swing(0.125) - 0.5).abs() < 1e-6);
        assert_eq!(serde_json::to_string(&Easing::Trapezoid).unwrap(), "\"trapezoid\"");
    }

    #[test]
    fn group_mode_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&GroupMode::Mirror).unwrap(), "\"mirror\"");
        assert_eq!(serde_json::from_str::<GroupMode>("\"offset\"").unwrap(), GroupMode::Offset);
    }
}
