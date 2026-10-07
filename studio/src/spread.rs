//! Phase, delay and speed spread across projection zones (T-421).
//!
//! The same look on several zones, shifted from one zone to the next: a
//! wave across the stage, a movement that starts at the centre. A spread
//! names a zone **group** (T-170 / T-420, `ProjectionConfig::groups`), not
//! a list of zones: adding a zone to the group changes the result without
//! touching the cue, modulator or event that holds the spread.
//!
//! Each zone of the group gets a value from its rank in the group
//! (`value`): *Phase* (degrees, cyclic: rank i of n gets `from + (to -
//! from) × i / n`), *Délai* (seconds or beats, linear: `i / (n - 1)`) or
//! *Vitesse* (%, linear). *Ailes* mirror the ranks around the centre,
//! *Blocs* group neighbours, *Mélange* permutes them with a fixed seed.
//!
//! Where it acts:
//! - on a modulator (lfo.rs): its phase per zone, on the master controls
//!   (`master.*`, applied per zone with the master stage); on a look
//!   parameter the look is drawn once, so the spread has no effect there;
//! - on a grid cell or a timeline event: the look is drawn once per zone of
//!   the group it plays on. *Délai* and *Phase* read a short memory of the
//!   look's content (`DelayRing`, at most `MAX_DELAY_S`); a phase of 360°
//!   is one bar at the current tempo. *Vitesse* runs one animator per zone.
//!
//! Safety: a spread only changes **when** a zone shows the content, never
//! how bright. The memory holds the look's raw content, before the cue
//! level, the transitions, the executors, the group masters, the master
//! stage, the geometry, the calibration and the whole safety stage, which
//! all run on every zone's copy of every frame, on real time. The memory
//! is emptied on an e-stop, a blackout, a « Noir » and a disarm, and
//! dropped with its cue: a stop is never delayed, an old lit frame never
//! comes back after one. A zone whose delay reaches back before the start
//! of the memory is dark. The exports (ILDA, SD card) draw the look
//! without spread.
//!
//! The notions come from the public documentation of lighting consoles;
//! names, formulas and code are ours.

use crate::engine::Animator;
use crate::patterns::Point;
use crate::projection::ProjectionConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Longest delay a zone can be given, in seconds (beats are converted at
/// the current tempo, then clamped).
pub const MAX_DELAY_S: f32 = 4.0;

/// *Ailes* and *Blocs*: 1..=MAX_SHAPE.
pub const MAX_SHAPE: u8 = 16;

/// *Vitesse*: a zone plays between 0 and this many % of the look's speed.
pub const MAX_SPEED_PCT: f32 = 400.0;

/// Frames kept in a delay memory, whatever the frame rate: 4 s at 75 fps.
/// Faster engines store one frame every `MAX_DELAY_S / MAX_FRAMES`.
pub const MAX_FRAMES: usize = 300;

/// What the spread moves from zone to zone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpreadParam {
    /// Degrees: a modulator's phase, or a look's position in its bar.
    #[default]
    Phase,
    /// Seconds (or beats with `delay_in_beats`): each zone plays later.
    Delay,
    /// % of the look's speed.
    Speed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spread {
    /// Name of a zone group (`ZoneGroup::name`); empty = inactive.
    pub group: String,
    pub param: SpreadParam,
    /// Value of the first zone (degrees, seconds/beats or %).
    pub from: f32,
    /// Value of the last zone (phase: what a zone after the last would get).
    pub to: f32,
    /// *En temps*: `from` / `to` of a delay are beats, not seconds.
    pub delay_in_beats: bool,
    /// *Ailes*: 1 = none, 2 = mirrored around the centre, more = repeated
    /// (every other wing mirrored).
    pub wings: u8,
    /// *Blocs*: neighbouring zones share a value, by packs of this many.
    pub blocks: u8,
    /// *Mélange*: the ranks permuted by this seed (same seed, same order).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shuffle: Option<u64>,
}

impl Default for Spread {
    fn default() -> Self {
        Self { group: String::new(), param: SpreadParam::Phase, from: 0.0, to: 360.0, delay_in_beats: false, wings: 1, blocks: 1, shuffle: None }
    }
}

fn fin(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

impl Spread {
    /// Every number within its range (NaN and infinities → defaults): the
    /// group name trimmed and bounded, wings and blocks 1..=16, a delay
    /// within 0..MAX_DELAY_S seconds (or 0..64 beats), a speed within
    /// 0..MAX_SPEED_PCT %, a phase within -3600..3600°.
    pub fn sanitized(mut self) -> Self {
        self.group = self.group.trim().chars().take(64).collect();
        self.wings = self.wings.clamp(1, MAX_SHAPE);
        self.blocks = self.blocks.clamp(1, MAX_SHAPE);
        let (lo, hi) = match (self.param, self.delay_in_beats) {
            (SpreadParam::Phase, _) => (-3600.0, 3600.0),
            (SpreadParam::Delay, false) => (0.0, MAX_DELAY_S),
            (SpreadParam::Delay, true) => (0.0, 64.0),
            (SpreadParam::Speed, _) => (0.0, MAX_SPEED_PCT),
        };
        let d = Spread::default();
        let default_to = if self.param == SpreadParam::Phase { d.to } else { lo };
        self.from = fin(self.from, 0.0f32.clamp(lo, hi)).clamp(lo, hi);
        self.to = fin(self.to, default_to).clamp(lo, hi);
        self
    }

    /// Names a group: something to spread over (the group may still be
    /// missing or hold a single zone, then nothing changes).
    pub fn is_set(&self) -> bool {
        !self.group.trim().is_empty()
    }

    /// The seconds a zone with `value` is behind, for *Délai* and *Phase*
    /// (one bar = 360°), at `bpm` with `beats_per_bar`: within
    /// 0..=MAX_DELAY_S, 0 for anything not finite; 0 for *Vitesse*.
    pub fn delay_s(&self, value: f32, bpm: f64, beats_per_bar: u8) -> f32 {
        let beat_s = 60.0 / bpm.clamp(1.0, 1000.0) as f32;
        let s = match self.param {
            SpreadParam::Delay if self.delay_in_beats => value * beat_s,
            SpreadParam::Delay => value,
            SpreadParam::Phase => fin(value, 0.0).rem_euclid(360.0) / 360.0 * beats_per_bar.max(1) as f32 * beat_s,
            SpreadParam::Speed => 0.0,
        };
        fin(s, 0.0).clamp(0.0, MAX_DELAY_S)
    }

    /// The speed factor of a zone with `value` for *Vitesse*, 0..=4; 1
    /// otherwise.
    pub fn speed(&self, value: f32) -> f32 {
        match self.param {
            SpreadParam::Speed => fin(value, 100.0).clamp(0.0, MAX_SPEED_PCT) / 100.0,
            _ => 1.0,
        }
    }

    /// The zones of the group (in the group's order) and each one's value:
    /// empty when no group of that name exists or it holds fewer than two
    /// existing zones (then the spread has no effect). Zones that no
    /// longer exist are skipped and don't count.
    pub fn zones(&self, cfg: &ProjectionConfig) -> Vec<(u8, f32)> {
        if !self.is_set() {
            return Vec::new();
        }
        let name = self.group.trim();
        let Some(group) = cfg.groups.iter().find(|g| g.name.trim() == name) else { return Vec::new() };
        let mut members: Vec<u8> = Vec::new();
        for id in &group.zones {
            if cfg.zone(*id).is_some() && !members.contains(id) {
                members.push(*id);
            }
        }
        if members.len() < 2 {
            return Vec::new();
        }
        let n = members.len();
        members.into_iter().enumerate().map(|(i, id)| (id, value(self, i, n))).collect()
    }
}

/// splitmix64.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// 0..count permuted by `seed` (Fisher–Yates on splitmix64): the same for
/// the same seed, on every run and every machine.
pub fn permutation(seed: u64, count: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).collect();
    let mut state = seed;
    for i in (1..count).rev() {
        state = mix(state);
        let j = (state % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

/// The value of the zone at rank `index` among `count` (a pure,
/// deterministic function). `count` 0 or 1: `from`.
pub fn value(s: &Spread, index: usize, count: usize) -> f32 {
    let (from, to) = (fin(s.from, 0.0), fin(s.to, 0.0));
    if count < 2 {
        return from;
    }
    let index = index.min(count - 1);
    // Mélange: the rank this zone takes.
    let pos = match s.shuffle {
        Some(seed) => permutation(seed, count)[index],
        None => index,
    };
    // Ailes: each wing restarts the ranks, every other one mirrored, so
    // two wings are symmetric around the centre.
    let wings = (s.wings.clamp(1, MAX_SHAPE) as usize).min(count);
    let wing_len = count.div_ceil(wings);
    let wing = pos / wing_len;
    let start = wing * wing_len;
    let end = (start + wing_len).min(count);
    let local = if wing % 2 == 1 { end - 1 - pos } else { pos - start };
    // Blocs: neighbours share a value.
    let blocks = s.blocks.clamp(1, MAX_SHAPE) as usize;
    let slot = local / blocks;
    let slots = wing_len.div_ceil(blocks);
    let frac = match s.param {
        SpreadParam::Phase => slot as f32 / slots as f32,
        _ if slots < 2 => 0.0,
        _ => slot as f32 / (slots - 1) as f32,
    };
    from + (to - from) * frac
}

/// A look's recent content, for the zones that play it late (*Délai*,
/// *Phase*): (time in seconds, points), oldest first, never saved. Bounded
/// in time (what the longest delay needs, at most `MAX_DELAY_S`) and in
/// frames (`MAX_FRAMES`).
#[derive(Default)]
pub struct DelayRing {
    frames: VecDeque<(f64, Vec<Point>)>,
}

impl DelayRing {
    /// Stores this frame's content at `t`, keeping what a delay of `keep_s`
    /// (clamped to `MAX_DELAY_S`) needs. A frame closer than
    /// `MAX_DELAY_S / MAX_FRAMES` to the previous one replaces it, so the
    /// memory never holds more than `MAX_FRAMES` (+1) frames.
    pub fn push(&mut self, t: f64, points: &[Point], keep_s: f32) {
        if !t.is_finite() {
            return;
        }
        // A clock that went back (a new session): start over.
        if self.frames.back().is_some_and(|(last, _)| *last > t) {
            self.frames.clear();
        }
        let step = MAX_DELAY_S as f64 / MAX_FRAMES as f64;
        let replace = self.frames.len() >= 2 && self.frames.iter().rev().nth(1).is_some_and(|(prev, _)| t - prev < step);
        if replace {
            self.frames.pop_back();
        }
        self.frames.push_back((t, points.to_vec()));
        let keep = fin(keep_s, 0.0).clamp(0.0, MAX_DELAY_S) as f64;
        // Keep the newest frame at or before `t - keep`: it is what that
        // delay shows.
        while self.frames.len() > 1 && self.frames.get(1).is_some_and(|(t1, _)| *t1 <= t - keep) {
            self.frames.pop_front();
        }
        while self.frames.len() > MAX_FRAMES + 1 {
            self.frames.pop_front();
        }
    }

    /// The content as it was `delay_s` before `t` (the newest frame at or
    /// before that time), or `None` when the memory doesn't reach that far
    /// back (the zone stays dark).
    pub fn at(&self, t: f64, delay_s: f32) -> Option<&[Point]> {
        let want = t - fin(delay_s, 0.0).clamp(0.0, MAX_DELAY_S) as f64;
        // A frame stored just after `want` by float rounding still counts.
        self.frames.iter().rev().find(|(ft, _)| *ft <= want + 1e-6).map(|(_, p)| p.as_slice())
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    #[cfg(test)]
    /// Seconds between the oldest and the newest frame kept.
    pub fn span_s(&self) -> f64 {
        match (self.frames.front(), self.frames.back()) {
            (Some((a, _)), Some((b, _))) => b - a,
            _ => 0.0,
        }
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.frames.clear();
    }
}

/// The zones of `spread`'s group a look routed to `routed` (its zones
/// after the zone chase; empty = the default zones) plays on, each with its
/// value. Empty: no spread this frame, the look is drawn as usual.
pub fn plan(spread: Option<&Spread>, cfg: &ProjectionConfig, routed: &[u8]) -> Vec<(u8, f32)> {
    let Some(s) = spread else { return Vec::new() };
    let on = cfg.resolve(routed);
    s.zones(cfg).into_iter().filter(|(z, _)| on.contains(z)).collect()
}

/// Where the look itself still goes when `plan` draws some of its zones
/// on their own: its other zones, or none (`NO_ZONE`).
pub fn rest(cfg: &ProjectionConfig, routed: &[u8], plan: &[(u8, f32)]) -> Vec<u8> {
    if plan.is_empty() {
        return routed.to_vec();
    }
    let rest: Vec<u8> = cfg.resolve(routed).iter().copied().filter(|z| !plan.iter().any(|(p, _)| p == z)).collect();
    if rest.is_empty() {
        vec![crate::projection::NO_ZONE]
    } else {
        rest
    }
}

/// The engine's spread state (never saved): each spread look's delay
/// memory, and the per-zone animators of a *Vitesse* spread with the beat
/// each started on.
#[derive(Default)]
pub struct SpreadState {
    rings: HashMap<u64, DelayRing>,
    animators: HashMap<(u64, u8), (Animator, f64)>,
    was_armed: bool,
    /// A stop holds this frame: nothing is stored.
    stopped: bool,
}

impl SpreadState {
    /// Once a frame, before the looks: an e-stop, a blackout, a « Noir » or
    /// a disarm since the last frame (`stop`, `armed`) empties every delay
    /// memory, so no frame lit before a stop is ever shown after it; while
    /// the stop holds, nothing is stored. Looks no longer playing
    /// (`playing` false) lose their state.
    pub fn frame(&mut self, stop: bool, armed: bool, playing: impl Fn(u64) -> bool) {
        self.stopped = stop || (self.was_armed && !armed);
        if self.stopped {
            self.rings.clear();
        }
        self.was_armed = armed;
        self.rings.retain(|id, _| playing(*id));
        self.animators.retain(|(id, _), _| playing(*id));
    }

    /// The raw content of each zone of `plan` for the look `id` whose own
    /// content this frame is `points`: from the delay memory (*Délai*,
    /// *Phase*; dark while the memory doesn't reach back far enough, and
    /// during a stop), or
    /// from the zone's own animator (*Vitesse*: `render(animator, start
    /// beat, speed factor)`, a new animator starting at `beat`).
    #[allow(clippy::too_many_arguments)]
    pub fn variants(
        &mut self,
        id: u64,
        spread: &Spread,
        plan: &[(u8, f32)],
        points: &[Point],
        (t, beat, bpm, beats_per_bar): (f64, f64, f64, u8),
        mut render: impl FnMut(&mut Animator, f64, f32) -> Vec<Point>,
    ) -> Vec<(u8, Vec<Point>)> {
        if plan.is_empty() {
            return Vec::new();
        }
        if spread.param == SpreadParam::Speed {
            return plan
                .iter()
                .map(|&(zone, v)| {
                    let (animator, start) = self.animators.entry((id, zone)).or_insert_with(|| (Animator::starting_at(beat), beat));
                    (zone, render(animator, *start, spread.speed(v)))
                })
                .collect();
        }
        if self.stopped {
            return plan.iter().map(|&(zone, _)| (zone, Vec::new())).collect();
        }
        let delays: Vec<f32> = plan.iter().map(|(_, v)| spread.delay_s(*v, bpm, beats_per_bar)).collect();
        let keep = delays.iter().copied().fold(0.0, f32::max);
        let ring = self.rings.entry(id).or_default();
        ring.push(t, points, keep);
        plan.iter().zip(delays).map(|(&(zone, _), d)| (zone, ring.at(t, d).map(<[Point]>::to_vec).unwrap_or_default())).collect()
    }

    /// Frames held in every delay memory (the fuzz checks they are dropped
    /// by a stop).
    #[cfg(test)]
    pub fn held_frames(&self) -> usize {
        self.rings.values().map(DelayRing::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{ProjectionZone, ZoneGroup};

    fn sp(param: SpreadParam, from: f32, to: f32) -> Spread {
        Spread { group: "G".into(), param, from, to, ..Default::default() }
    }

    fn values(s: &Spread, n: usize) -> Vec<f32> {
        (0..n).map(|i| value(s, i, n)).collect()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn phase_is_cyclic_and_delay_linear() {
        let got = values(&sp(SpreadParam::Phase, 0.0, 360.0), 6);
        assert!(close(&got, &[0.0, 60.0, 120.0, 180.0, 240.0, 300.0]), "{got:?}");
        let got = values(&sp(SpreadParam::Delay, 0.0, 1.0), 3);
        assert!(close(&got, &[0.0, 0.5, 1.0]), "{got:?}");
        let got = values(&sp(SpreadParam::Speed, 100.0, 200.0), 5);
        assert!(close(&got, &[100.0, 125.0, 150.0, 175.0, 200.0]), "{got:?}");
    }

    #[test]
    fn two_wings_mirror_around_the_centre() {
        for param in [SpreadParam::Phase, SpreadParam::Delay] {
            let s = Spread { wings: 2, ..sp(param, 0.0, 360.0) };
            let v = values(&s, 6);
            assert_eq!((v[0], v[1], v[2]), (v[5], v[4], v[3]), "{param:?} {v:?}");
            assert!(v[0] != v[1] && v[1] != v[2]);
            // An odd count stays symmetric too.
            let v = values(&s, 5);
            assert_eq!((v[0], v[1]), (v[4], v[3]), "{v:?}");
        }
        // Three wings: the pattern repeats.
        let v = values(&Spread { wings: 3, ..sp(SpreadParam::Delay, 0.0, 1.0) }, 6);
        assert!(close(&v, &[0.0, 1.0, 1.0, 0.0, 0.0, 1.0]), "{v:?}");
    }

    #[test]
    fn blocks_pair_neighbours() {
        let v = values(&Spread { blocks: 2, ..sp(SpreadParam::Delay, 0.0, 1.0) }, 6);
        assert!(close(&v, &[0.0, 0.0, 0.5, 0.5, 1.0, 1.0]), "{v:?}");
        let v = values(&Spread { blocks: 2, ..sp(SpreadParam::Phase, 0.0, 360.0) }, 4);
        assert!(close(&v, &[0.0, 0.0, 180.0, 180.0]), "{v:?}");
    }

    #[test]
    fn shuffle_is_deterministic_per_seed() {
        let s = |seed| Spread { shuffle: Some(seed), ..sp(SpreadParam::Delay, 0.0, 5.0) };
        let a = values(&s(7), 8);
        assert_eq!(a, values(&s(7), 8), "same seed, same order");
        // After a restart: the same, from the saved form.
        let back: Spread = serde_json::from_str(&serde_json::to_string(&s(7)).unwrap()).unwrap();
        assert_eq!(a, values(&back, 8));
        assert_ne!(a, values(&s(8), 8), "another seed, another order");
        // Still a permutation of the plain ranks.
        let mut sorted = a.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(close(&sorted, &values(&sp(SpreadParam::Delay, 0.0, 5.0), 8)));
        assert_eq!(permutation(3, 0), Vec::<usize>::new());
        assert_eq!(permutation(3, 1), vec![0]);
    }

    #[test]
    fn no_division_by_zero_and_bounds() {
        for param in [SpreadParam::Phase, SpreadParam::Delay, SpreadParam::Speed] {
            let s = sp(param, 1.0, 3.0);
            assert_eq!(value(&s, 0, 0), 1.0);
            assert_eq!(value(&s, 5, 1), 1.0);
            assert!(value(&s, 99, 4).is_finite(), "an index past the end is clamped");
            let wild = Spread { wings: 0, blocks: 200, from: f32::NAN, to: f32::INFINITY, ..s }.sanitized();
            assert_eq!((wild.wings, wild.blocks), (1, MAX_SHAPE));
            assert!(wild.from.is_finite() && wild.to.is_finite());
            for n in 0..20 {
                for i in 0..n {
                    assert!(value(&wild, i, n).is_finite());
                }
            }
        }
        let d = Spread { from: -2.0, to: 99.0, ..sp(SpreadParam::Delay, 0.0, 0.0) }.sanitized();
        assert_eq!((d.from, d.to), (0.0, MAX_DELAY_S));
        assert_eq!(d.delay_s(1e9, 120.0, 4), MAX_DELAY_S);
        assert_eq!(d.delay_s(f32::NAN, 120.0, 4), 0.0);
        let beats = Spread { delay_in_beats: true, ..d.clone() };
        assert!((beats.delay_s(1.0, 120.0, 4) - 0.5).abs() < 1e-6, "one beat at 120 BPM");
        assert_eq!(beats.delay_s(64.0, 30.0, 4), MAX_DELAY_S);
        // A phase of 90° is a quarter of a 4-beat bar: one beat.
        let p = sp(SpreadParam::Phase, 0.0, 360.0);
        assert!((p.delay_s(90.0, 120.0, 4) - 0.5).abs() < 1e-6);
        assert!((p.delay_s(-270.0, 120.0, 4) - 0.5).abs() < 1e-6, "cyclic");
        assert_eq!(p.delay_s(360.0, 120.0, 4), 0.0);
        let speed = Spread { from: -5.0, to: 1e6, ..sp(SpreadParam::Speed, 0.0, 0.0) }.sanitized();
        assert_eq!((speed.from, speed.to), (0.0, MAX_SPEED_PCT));
        assert_eq!(speed.speed(150.0), 1.5);
        assert_eq!(p.speed(150.0), 1.0);
    }

    #[test]
    fn serde_defaults() {
        let s: Spread = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Spread::default());
        assert!(!s.is_set());
        let s: Spread = serde_json::from_str(r#"{"group":"Côtés","param":"delay","to":1}"#).unwrap();
        assert_eq!((s.param, s.from, s.to, s.wings), (SpreadParam::Delay, 0.0, 1.0, 1));
        assert!(serde_json::to_value(&s).unwrap().get("shuffle").is_none());
    }

    fn cfg(groups: Vec<ZoneGroup>, zones: u8) -> ProjectionConfig {
        ProjectionConfig { zones: (1..=zones).map(|id| ProjectionZone { id, name: format!("Z{id}"), ..Default::default() }).collect(), groups, ..Default::default() }
    }

    #[test]
    fn the_group_is_a_reference() {
        let s = sp(SpreadParam::Phase, 0.0, 360.0);
        let mut c = cfg(vec![ZoneGroup { name: "G".into(), zones: vec![1, 2, 3, 4, 5, 6], master: 1.0 }], 8);
        let six = s.zones(&c);
        assert_eq!(six.len(), 6);
        assert!(close(&six.iter().map(|z| z.1).collect::<Vec<_>>(), &[0.0, 60.0, 120.0, 180.0, 240.0, 300.0]));
        // A seventh zone added to the group: seven phases, same spread.
        c.groups[0].zones.push(7);
        let seven = s.zones(&c);
        assert_eq!(seven.len(), 7);
        assert!((seven[1].1 - 360.0 / 7.0).abs() < 1e-4);
        // A zone that no longer exists is skipped; one zone, no group or
        // no name: no effect.
        c.groups[0].zones = vec![2, 9];
        assert!(s.zones(&c).is_empty());
        assert!(Spread { group: "Autre".into(), ..s.clone() }.zones(&c).is_empty());
        assert!(Spread { group: String::new(), ..s }.zones(&c).is_empty());
    }

    fn frame(v: f32) -> Vec<Point> {
        vec![Point { x: v, y: 0.0, r: 1.0, g: 1.0, b: 1.0 }]
    }

    #[test]
    fn the_ring_returns_the_frame_from_the_delay_ago_and_stays_bounded() {
        let mut ring = DelayRing::default();
        assert!(ring.at(0.0, 0.0).is_none());
        let fps = 60.0;
        for i in 0..600 {
            let t = i as f64 / fps;
            ring.push(t, &frame(i as f32), 1.0);
            // Zone 2 of 0→1 s on three zones: 0.5 s ago (±1 frame).
            if t >= 1.0 {
                let half = ring.at(t, 0.5).unwrap()[0].x;
                assert!((half - (i as f32 - 30.0)).abs() <= 1.0, "{t}: {half}");
                let one = ring.at(t, 1.0).unwrap()[0].x;
                assert!((one - (i as f32 - 60.0)).abs() <= 1.0, "{t}: {one}");
                assert_eq!(ring.at(t, 0.0).unwrap()[0].x, i as f32);
            }
        }
        assert!(ring.span_s() <= 1.0 + 2.0 / fps, "only what a 1 s delay needs: {}", ring.span_s());
        // Asking further back than kept: dark, never something else.
        let mut short = DelayRing::default();
        short.push(10.0, &frame(1.0), 4.0);
        assert!(short.at(10.0, 0.5).is_none(), "nothing that old yet");
        // At most MAX_FRAMES frames, at any frame rate, over 4 s.
        let mut fast = DelayRing::default();
        for i in 0..5000 {
            fast.push(i as f64 / 1000.0, &frame(i as f32), 10.0);
        }
        assert!(fast.len() <= MAX_FRAMES + 2, "{}", fast.len());
        assert!(fast.span_s() <= MAX_DELAY_S as f64 + 0.05);
        let x = fast.at(4.999, 2.0).unwrap()[0].x;
        assert!((x - 2999.0).abs() <= 1000.0 * MAX_DELAY_S / MAX_FRAMES as f32 + 1.0, "{x}");
        fast.clear();
        assert_eq!(fast.len(), 0);
        // Time going back: start over.
        ring.push(0.0, &frame(-1.0), 1.0);
        assert_eq!(ring.len(), 1);
    }
    /// Acceptance: *Délai* 0 → 1 s on three zones: zone 2 shows the image
    /// of 0.5 s ago, zone 3 that of 1 s ago (± 1 frame); a stop or a
    /// disarm empties the memory and the late zones go dark.
    #[test]
    fn three_zones_delayed_and_a_stop_empties_the_memory() {
        let spread = sp(SpreadParam::Delay, 0.0, 1.0);
        let c = cfg(vec![ZoneGroup { name: "G".into(), zones: vec![1, 2, 3], master: 1.0 }], 3);
        let plan = plan(Some(&spread), &c, &[1, 2, 3]);
        assert_eq!(plan.len(), 3);
        let mut st = SpreadState::default();
        let no_render = |_: &mut Animator, _: f64, _: f32| -> Vec<Point> { unreachable!("no animator for a delay") };
        let mut last = Vec::new();
        for i in 0..120 {
            st.frame(false, true, |_| true);
            let t = i as f64 / 60.0;
            last = st.variants(7, &spread, &plan, &frame(i as f32), (t, 0.0, 120.0, 4), no_render);
            if i < 30 {
                assert!(last[1].1.is_empty() && last[2].1.is_empty(), "late zones dark until their delay has passed");
            }
        }
        assert_eq!(last[0].1[0].x, 119.0);
        assert!((last[1].1[0].x - 89.0).abs() <= 1.0, "{}", last[1].1[0].x);
        assert!((last[2].1[0].x - 59.0).abs() <= 1.0, "{}", last[2].1[0].x);
        assert!(st.held_frames() <= 62);
        // E-stop / blackout / « Noir »: nothing kept, nothing stored, dark.
        st.frame(true, true, |_| true);
        assert_eq!(st.held_frames(), 0);
        let v = st.variants(7, &spread, &plan, &frame(120.0), (2.0, 0.0, 120.0, 4), no_render);
        assert!(v.iter().all(|(_, p)| p.is_empty()) && st.held_frames() == 0);
        // Afterwards: zone 1 at once, the late ones start over from dark.
        st.frame(false, true, |_| true);
        let v = st.variants(7, &spread, &plan, &frame(121.0), (2.1, 0.0, 120.0, 4), no_render);
        assert_eq!(v[0].1[0].x, 121.0);
        assert!(v[1].1.is_empty() && v[2].1.is_empty());
        // A disarm: same.
        st.frame(false, false, |_| true);
        assert_eq!(st.held_frames(), 0);
        // A cue no longer playing loses its memory.
        st.frame(false, false, |_| true);
        st.variants(7, &spread, &plan, &frame(1.0), (3.0, 0.0, 120.0, 4), no_render);
        assert_eq!(st.held_frames(), 1);
        st.frame(false, false, |id| id != 7);
        assert_eq!(st.held_frames(), 0);
    }

    #[test]
    fn a_speed_spread_runs_one_animator_per_zone() {
        let spread = sp(SpreadParam::Speed, 100.0, 200.0);
        let c = cfg(vec![ZoneGroup { name: "G".into(), zones: vec![1, 2], master: 1.0 }], 2);
        let default_only = plan(Some(&spread), &c, &[]);
        assert_eq!(default_only.len(), 1, "only the zones the look plays on (the default zones: 1)");
        let both = plan(Some(&spread), &c, &[1, 2]);
        let mut st = SpreadState::default();
        let mut speeds = Vec::new();
        let v = st.variants(1, &spread, &both, &[], (0.0, 3.0, 120.0, 4), |_, start, k| {
            speeds.push((start, k));
            frame(k)
        });
        assert_eq!(speeds, vec![(3.0, 1.0), (3.0, 2.0)]);
        assert_eq!(v.iter().map(|(z, _)| *z).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(st.held_frames(), 0, "no memory for a speed spread");
        // Where the look itself still goes.
        assert_eq!(rest(&c, &[1, 2], &both), vec![crate::projection::NO_ZONE]);
        assert_eq!(rest(&c, &[1, 2], &both[..1]), vec![2]);
        assert_eq!(rest(&c, &[2], &[]), vec![2]);
    }
}
