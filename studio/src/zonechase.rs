//! Zone chasers (T-170): the content passes from one group of projection
//! zones to the next on the beat (left, centre, right…), the way a
//! multi-projector show moves a look across the rig. Our own model, from
//! the idea described in public docs of laser and lighting software.
//!
//! A *chaser* is an ordered list of zone groups (`ZoneGroup`, the groups
//! of T-420 in `ProjectionConfig.groups`), a step length in beats (1/4 to
//! 4) and a mode (*Avant*, *Aller-retour*, *Aléatoire*). It plays either
//! **free** (`enabled`, the control `chaser.<n>.enabled`): every look
//! without a chaser of its own is narrowed to the active group; or **tied
//! to a cue** (`CueSlot::zone_chaser`): that cue's look only, while it
//! plays (in the grid and as a timeline event of that cue). Several free
//! chasers at once: a zone shows if it is in the active group of any of
//! them. `chaser.rate` multiplies every chaser's speed.
//!
//! Where it sits: it is **routing only**. The cue → zones routing of T-012
//! stays the base; at each step the chase keeps the part of a look's
//! zones that is in the active group, and a look left with none is drawn
//! on no zone (no points, like a cue routed to no zone of a projector).
//! It never adds light, never moves a point, never arms; everything after
//! it (mixer, group masters, geometry, calibration, the whole safety stage)
//! is unchanged.
//!
//! Strobe bound: a zone switched on and off by a chase flashes. Every step
//! lasts at least `min_step_s` (half a period of the strobe limiter's
//! `strobe_max_hz`): a step that would be shorter at this tempo is doubled
//! until it is not. Step lengths and rates are powers of two and every
//! chaser counts from beat 0 of the same tempo clock, so all chase step
//! boundaries fall on one grid at least `min_step_s` apart: whatever the
//! chasers and looks combined, a zone's light changes at most once per
//! cell, so it can't flash faster than the limiter allows. The limiter
//! still has the last word on the output.
//!
//! Exports: a chase is a playing move, like the live master or the layer
//! faders, and a file carries a look, not the moves of the moment. The
//! look, timeline and SD-card exports draw every routed zone, as with no
//! chase (`ExportSafety::place`): at most the union of what live shows
//! over a cycle, never more. A live recording (T-442) records the output
//! as it is, the chase included.

use crate::projection::{ProjectionConfig, NO_ZONE};
use serde::{Deserialize, Serialize};

/// Chasers in a configuration (and `chaser.<n>.enabled` controls).
pub const MAX_CHASERS: usize = 16;
/// Steps of one chaser.
pub const MAX_STEPS: usize = 32;
/// Step lengths in beats (« Pas »).
pub const STEP_BEATS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
pub const STEP_LABELS: [&str; 5] = ["1/4 temps", "1/2 temps", "1 temps", "2 temps", "4 temps"];
/// `chaser.rate`: speed multipliers of every chaser.
pub const RATES: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
pub const RATE_LABELS: [&str; 5] = ["×1/4", "×1/2", "×1", "×2", "×4"];
/// Index of ×1 in `RATES`.
pub const DEFAULT_RATE: usize = 2;
/// The tempo assumed when the clock's can't be read.
const FAST_BPM: f64 = 4.0 * crate::tempo::MAX_BPM;
/// Seed of the random mode: the same step picks the same group on every
/// run, so frames and tests are reproducible.
const RANDOM_SEED: u64 = 0x0170_c4a5;

/// The order a chaser visits its steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChaseMode {
    /// « Avant »: first to last, then round again.
    #[default]
    Forward,
    /// « Aller-retour »: there and back, the end steps not repeated.
    Bounce,
    /// « Aléatoire »: a step drawn per step (seeded by the step number).
    Random,
}

/// One zone chaser.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneChaser {
    pub name: String,
    /// Zone groups (indices into `ProjectionConfig.groups`), in order. A
    /// group may come back (Centre, Côtés, Centre…).
    pub steps: Vec<usize>,
    /// Beats per step, one of `STEP_BEATS`.
    pub step_beats: f32,
    pub mode: ChaseMode,
    /// Free-running (`chaser.<n>.enabled`): applies to every look without
    /// a chaser of its own. A playing value, like a group master.
    pub enabled: bool,
}

impl Default for ZoneChaser {
    fn default() -> Self {
        Self { name: String::new(), steps: Vec::new(), step_beats: 1.0, mode: ChaseMode::Forward, enabled: false }
    }
}

/// The value of `options` nearest to `v` (not finite: `fallback`).
fn snap(v: f32, options: &[f32], fallback: f32) -> f32 {
    if !v.is_finite() {
        return fallback;
    }
    options.iter().copied().min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs())).unwrap_or(fallback)
}

/// A step length as the chase uses it (one of `STEP_BEATS`).
pub fn snap_step(v: f32) -> f32 {
    snap(v, &STEP_BEATS, 1.0)
}

/// A rate as the chase uses it (one of `RATES`).
pub fn snap_rate(v: f32) -> f32 {
    snap(v, &RATES, 1.0)
}

/// Shortest step, in seconds, for a strobe limit of `max_hz`: a zone lit
/// on one step and dark on the next flashes at most `max_hz`. A limit
/// that can't be read takes the slowest one the settings allow.
pub fn min_step_s(max_hz: f32) -> f64 {
    let hz = if max_hz.is_finite() { max_hz.clamp(0.5, crate::safety::DEFAULT_MAX_HZ) } else { 0.5 };
    0.5 / hz as f64
}

/// Beats per step of a chaser stepping every `step_beats` at `rate`,
/// doubled until a step lasts at least `min_step_s(max_hz)` at `bpm`.
/// Always a power of two times 1/16 beat.
pub fn effective_beats(step_beats: f32, rate: f32, bpm: f64, max_hz: f32) -> f64 {
    let mut beats = snap_step(step_beats) as f64 / snap_rate(rate) as f64;
    // A tempo that can't be read counts as a fast one (more doubling).
    let bpm = if bpm.is_finite() && bpm > 0.0 { bpm.max(crate::tempo::MIN_BPM) } else { FAST_BPM };
    let min_s = min_step_s(max_hz);
    while beats * 60.0 / bpm < min_s && beats < 1024.0 {
        beats *= 2.0;
    }
    beats
}

/// The step number at `beat` for steps of `beats` (counted from beat 0).
pub fn step_at(beat: f64, beats: f64) -> i64 {
    if !beat.is_finite() || beats <= 0.0 {
        return 0;
    }
    (beat / beats).floor() as i64
}

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl ZoneChaser {
    /// The index into `steps` at step number `k` (None without steps).
    pub fn step_index(&self, k: i64) -> Option<usize> {
        let n = self.steps.len();
        if n == 0 {
            return None;
        }
        let n64 = n as i64;
        Some(match self.mode {
            ChaseMode::Forward => k.rem_euclid(n64) as usize,
            ChaseMode::Bounce if n == 1 => 0,
            ChaseMode::Bounce => {
                let period = 2 * n64 - 2;
                let j = k.rem_euclid(period);
                (if j < n64 { j } else { period - j }) as usize
            }
            ChaseMode::Random => (mix(k as u64 ^ RANDOM_SEED) % n as u64) as usize,
        })
    }
}

/// Where every chaser stands this frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ChaseNow {
    /// Per chaser (config order): the step index and the active group.
    pub steps: Vec<Option<(usize, usize)>>,
    /// Zones the free chasers allow (union of their active groups); None:
    /// no free chaser runs, looks keep their zones.
    pub free: Option<Vec<u8>>,
}

impl ChaseNow {
    /// The chasers of `cfg` at `beat` of the tempo clock (`bpm`), with
    /// steps bounded by the strobe limit `max_hz`.
    pub fn at(cfg: &ProjectionConfig, beat: f64, bpm: f64, max_hz: f32) -> Self {
        let steps: Vec<Option<(usize, usize)>> = cfg
            .chasers
            .iter()
            .map(|c| {
                let k = step_at(beat, effective_beats(c.step_beats, cfg.chase_rate, bpm, max_hz));
                let i = c.step_index(k)?;
                let g = c.steps[i];
                (g < cfg.groups.len()).then_some((i, g))
            })
            .collect();
        let mut free: Option<Vec<u8>> = None;
        for (c, step) in cfg.chasers.iter().zip(&steps) {
            if let (true, Some((_, g))) = (c.enabled, step) {
                free.get_or_insert_with(Vec::new).extend(&cfg.groups[*g].zones);
            }
        }
        if let Some(z) = &mut free {
            z.sort_unstable();
            z.dedup();
        }
        Self { steps, free }
    }

    /// The zones of chaser `c` now, if it runs with an active group.
    fn zones_of<'a>(&self, cfg: &'a ProjectionConfig, c: usize) -> Option<&'a [u8]> {
        let (_, g) = (*self.steps.get(c)?)?;
        Some(&cfg.groups.get(g)?.zones)
    }

    /// A look's zones (`zones`: its routing, empty = the default zones;
    /// `own`: the chaser tied to its cue) through the chase: unchanged
    /// when nothing chases it, else the part of its zones in the active
    /// group(s), `[NO_ZONE]` when none is (drawn nowhere this step).
    /// A tied chaser that no longer exists leaves the look to the free ones.
    pub fn route(&self, cfg: &ProjectionConfig, zones: &[u8], own: Option<usize>) -> Vec<u8> {
        let allowed = match own.and_then(|c| self.zones_of(cfg, c)) {
            Some(z) => z,
            None => match &self.free {
                Some(z) => z.as_slice(),
                None => return zones.to_vec(),
            },
        };
        let kept: Vec<u8> = cfg.resolve(zones).iter().copied().filter(|id| allowed.contains(id)).collect();
        if kept.is_empty() {
            vec![NO_ZONE]
        } else {
            kept
        }
    }
}

/// Repairs a configuration's chasers: at most `MAX_CHASERS`, names
/// trimmed, steps on existing groups (others dropped), at most
/// `MAX_STEPS`, step lengths and the rate snapped to their lists.
pub fn sanitize(chasers: &[ZoneChaser], groups: usize) -> Vec<ZoneChaser> {
    chasers
        .iter()
        .take(MAX_CHASERS)
        .enumerate()
        .map(|(i, c)| {
            let mut name: String = c.name.trim().chars().take(40).collect();
            if name.is_empty() {
                name = format!("Chenillard {}", i + 1);
            }
            let steps = c.steps.iter().copied().filter(|g| *g < groups).take(MAX_STEPS).collect();
            ZoneChaser { name, steps, step_beats: snap_step(c.step_beats), mode: c.mode, enabled: c.enabled }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{ProjectionZone, ZoneGroup};

    fn cfg(chasers: Vec<ZoneChaser>) -> ProjectionConfig {
        let zone = |id: u8| ProjectionZone { id, name: format!("Z{id}"), ..Default::default() };
        let group = |name: &str, zones: Vec<u8>| ZoneGroup { name: name.into(), zones, master: 1.0 };
        ProjectionConfig {
            zones: vec![zone(1), zone(2), zone(3)],
            default_zones: vec![1, 2, 3],
            groups: vec![group("Gauche", vec![1]), group("Centre", vec![2]), group("Droite", vec![3]), group("Côtés", vec![1, 3])],
            chasers,
            ..Default::default()
        }
    }

    fn chaser(steps: Vec<usize>, mode: ChaseMode, enabled: bool) -> ZoneChaser {
        ZoneChaser { name: "C".into(), steps, step_beats: 1.0, mode, enabled }
    }

    /// Acceptance: three groups one beat apart, on a simulated clock: the
    /// active group changes exactly on every beat, and only there.
    #[test]
    fn three_groups_one_beat_apart_change_exactly_on_each_beat() {
        let cfg = cfg(vec![chaser(vec![0, 1, 2], ChaseMode::Forward, true)]);
        let mut last = None;
        let mut changes = Vec::new();
        // 120 BPM, 60 fps: 30 frames a beat, 8 beats.
        for f in 0..240 {
            let beat = f as f64 / 30.0;
            let now = ChaseNow::at(&cfg, beat, 120.0, 4.0);
            let g = now.steps[0].unwrap().1;
            if last.is_some_and(|l| l != g) {
                changes.push(f);
            }
            last = Some(g);
            assert_eq!(g, (beat.floor() as usize) % 3, "frame {f}");
            assert_eq!(now.free.as_deref(), Some(&[g as u8 + 1][..]));
        }
        assert_eq!(changes, (1..8).map(|b| b * 30).collect::<Vec<_>>());
    }

    #[test]
    fn modes_visit_their_steps() {
        let order = |mode, n: usize| -> Vec<usize> {
            let c = chaser((0..n).collect(), mode, true);
            (0..10).map(|k| c.step_index(k).unwrap()).collect()
        };
        assert_eq!(order(ChaseMode::Forward, 3), vec![0, 1, 2, 0, 1, 2, 0, 1, 2, 0]);
        assert_eq!(order(ChaseMode::Bounce, 3), vec![0, 1, 2, 1, 0, 1, 2, 1, 0, 1]);
        assert_eq!(order(ChaseMode::Bounce, 1), vec![0; 10]);
        let random = order(ChaseMode::Random, 4);
        assert_eq!(random, order(ChaseMode::Random, 4), "reproducible");
        assert!(random.iter().all(|i| *i < 4));
        let seen: std::collections::BTreeSet<usize> = (0..200).map(|k| chaser(vec![0, 1, 2, 3], ChaseMode::Random, true).step_index(k).unwrap()).collect();
        assert_eq!(seen.len(), 4, "every step comes up");
        assert_eq!(chaser(vec![], ChaseMode::Forward, true).step_index(3), None);
        // Before beat 0 (a negative beat) still a valid step.
        assert_eq!(chaser(vec![0, 1, 2], ChaseMode::Forward, true).step_index(-1), Some(2));
    }

    /// Acceptance: a disabled chaser routes as before; a zone outside the
    /// active group gets nothing.
    #[test]
    fn routing_keeps_only_the_active_group() {
        let off = cfg(vec![chaser(vec![0, 1, 2], ChaseMode::Forward, false)]);
        let now = ChaseNow::at(&off, 0.5, 120.0, 4.0);
        assert_eq!(now.free, None);
        assert_eq!(now.route(&off, &[], None), Vec::<u8>::new(), "default zones kept as they were");
        assert_eq!(now.route(&off, &[2, 3], None), vec![2, 3]);
        let on = cfg(vec![chaser(vec![0, 1, 2], ChaseMode::Forward, true)]);
        let now = ChaseNow::at(&on, 1.5, 120.0, 4.0); // step 1: Centre
        assert_eq!(now.route(&on, &[], None), vec![2], "the default zones, narrowed");
        assert_eq!(now.route(&on, &[1, 2], None), vec![2]);
        assert_eq!(now.route(&on, &[1, 3], None), vec![NO_ZONE], "nothing of it in the group: nowhere");
        // A chaser tied to the cue wins over the free one, even disabled.
        let tied = cfg(vec![chaser(vec![0, 1, 2], ChaseMode::Forward, true), chaser(vec![3], ChaseMode::Forward, false)]);
        let now = ChaseNow::at(&tied, 1.5, 120.0, 4.0);
        assert_eq!(now.route(&tied, &[], Some(1)), vec![1, 3]);
        assert_eq!(now.route(&tied, &[], Some(0)), vec![2]);
        assert_eq!(now.route(&tied, &[], Some(9)), vec![2], "a tied chaser gone: the free one");
        // Two free chasers: the union of their groups.
        let two = cfg(vec![chaser(vec![0], ChaseMode::Forward, true), chaser(vec![2], ChaseMode::Forward, true)]);
        assert_eq!(ChaseNow::at(&two, 0.0, 120.0, 4.0).route(&two, &[], None), vec![1, 3]);
    }

    /// The chase never routes a look to a zone it was not routed to.
    #[test]
    fn the_chase_only_ever_narrows_a_looks_zones() {
        let mut rng = 0x1234_5678u64;
        let mut next = || {
            rng = mix(rng);
            rng
        };
        for _ in 0..500 {
            let steps: Vec<usize> = (0..1 + next() % 5).map(|_| (next() % 6) as usize).collect();
            let mode = [ChaseMode::Forward, ChaseMode::Bounce, ChaseMode::Random][(next() % 3) as usize];
            let c = cfg(vec![ZoneChaser { steps, mode, enabled: next() % 2 == 0, step_beats: STEP_BEATS[(next() % 5) as usize], ..Default::default() }]).sanitized();
            let zones: Vec<u8> = (0..next() % 3).map(|_| 1 + (next() % 4) as u8).collect();
            let own = (next() % 3 == 0).then_some((next() % 2) as usize);
            let beat = (next() % 10_000) as f64 / 37.0;
            let routed = ChaseNow::at(&c, beat, 60.0 + (next() % 180) as f64, 4.0).route(&c, &zones, own);
            let base = c.resolve(&zones);
            assert!(routed == zones || routed == [NO_ZONE] || routed.iter().all(|z| base.contains(z)), "{zones:?} → {routed:?}");
        }
    }

    /// The step bound: never shorter than half a strobe period, whatever
    /// the step, rate and tempo; powers of two, so every chaser shares one
    /// grid of step boundaries.
    #[test]
    fn steps_never_beat_the_strobe_limit() {
        for &step in &STEP_BEATS {
            for &rate in &RATES {
                for bpm in [40.0, 90.0, 120.0, 174.0, 250.0, 999.0, f64::NAN] {
                    for hz in [0.5, 2.0, 4.0, 40.0, f32::NAN] {
                        let beats = effective_beats(step, rate, bpm, hz);
                        let real_bpm = if bpm.is_finite() { bpm.max(40.0) } else { FAST_BPM };
                        assert!(beats * 60.0 / real_bpm >= min_step_s(hz) - 1e-12, "{step} {rate} {bpm} {hz} → {beats}");
                        assert!(min_step_s(hz) >= 0.125 - 1e-12, "no limit is looser than 4 Hz");
                        let sixteenths = beats * 16.0;
                        assert!(sixteenths.fract() == 0.0 && (sixteenths as u64).is_power_of_two(), "{beats}");
                    }
                }
            }
        }
        // Slow enough already: unchanged.
        assert_eq!(effective_beats(1.0, 1.0, 120.0, 4.0), 1.0);
        // 1/4 beat ×4 at 250 BPM = 15 ms: doubled up to 1 beat (½ beat is
        // 120 ms, still under 125 ms); at a 2 Hz limit, 2 beats.
        assert_eq!(effective_beats(0.25, 4.0, 250.0, 4.0), 1.0);
        assert_eq!(effective_beats(0.25, 4.0, 250.0, 2.0), 2.0);
    }

    #[test]
    fn sanitize_drops_missing_groups_and_snaps() {
        let raw = vec![ZoneChaser { name: "  ".into(), steps: vec![0, 7, 3, 1], step_beats: 0.3, mode: ChaseMode::Bounce, enabled: true }];
        let c = sanitize(&raw, 4);
        assert_eq!(c[0].name, "Chenillard 1");
        assert_eq!(c[0].steps, vec![0, 3, 1]);
        assert_eq!(c[0].step_beats, 0.25);
        assert_eq!(snap_step(f32::NAN), 1.0);
        assert_eq!(snap_rate(3.1), 4.0);
        assert_eq!(sanitize(&vec![ZoneChaser::default(); 40], 1).len(), MAX_CHASERS);
        // Old files: no chasers, a missing field takes its default.
        let c: ZoneChaser = serde_json::from_str(r#"{"name":"A","steps":[1]}"#).unwrap();
        assert_eq!((c.step_beats, c.mode, c.enabled), (1.0, ChaseMode::Forward, false));
        let m: ChaseMode = serde_json::from_str(r#""bounce""#).unwrap();
        assert_eq!(m, ChaseMode::Bounce);
    }
}
