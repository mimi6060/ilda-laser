//! Speed groups and the rate master (T-419): the console's speed masters
//! and rate master, our own way.
//!
//! Four groups, *A* to *D*, besides the default group (the master tempo
//! clock, group 0). A group is either a **multiple** of the master clock
//! (×¼, ×½, ×1, ×2, ×4) or a **free BPM**. A cue cell, a layer or a
//! modulator picks its group (cell > layer > default); the look is then
//! rendered with that group's `BeatClock`.
//!
//! No drift: a group's beat is a pure function of the master clock,
//! `beat = g0 + (master_beat - x0) × mult`, or of the engine's seconds for
//! a free BPM, `beat = g0 + (t - x0) × bpm / 60`. Nothing is accumulated
//! frame by frame: the anchor (x0, g0) only moves when the operator
//! changes the group (so the beat carries on without a jump, the way
//! `TempoClock::set_bpm` moves its origin) or resyncs (`tempo.resync`
//! brings every multiple group's bar back on the master's "one").
//!
//! *Appliquer au début de la mesure* (on by default): a new multiple waits
//! for the next master bar where the group is also on its own "one", so
//! group bars and master bars stay aligned. A free BPM applies at once.
//!
//! The rate master (*Cadence*, 50–200 %) divides the length of every fade
//! and transition played live (cue transitions, cue-list fades); the
//! fades of a timeline are not touched, so a programmed show stays
//! reproducible.
//!
//! Safety: this only moves *what* is computed for a frame (beats and fade
//! lengths), never *when* frames go out. The strobe and flash limiters
//! run on real output time after it and keep the last word; nothing here
//! arms or touches the gate.

use crate::engine::BeatClock;
use crate::tempo::{MAX_BPM, MIN_BPM};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SPEED_GROUPS: usize = 4;
pub const SPEED_MULTS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
pub const MULT_LABELS: [&str; 5] = ["×¼", "×½", "×1", "×2", "×4"];
/// Group 0 is the master clock; 1..=4 are A..D.
pub const GROUP_NAMES: [&str; SPEED_GROUPS + 1] = ["Par défaut", "A", "B", "C", "D"];
pub const MIN_RATE: f32 = 0.5;
pub const MAX_RATE: f32 = 2.0;
/// How far ahead a deferred multiple looks for an aligned bar (bars).
const MAX_WAIT_BARS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpeedMode {
    Multiple { mult: f32 },
    FreeBpm { bpm: f64 },
}

impl Default for SpeedMode {
    fn default() -> Self {
        SpeedMode::Multiple { mult: 1.0 }
    }
}

impl SpeedMode {
    /// A mode from the outside (API, controllers): a multiple must be one
    /// of `SPEED_MULTS`; a free BPM must be a positive number, then is
    /// clamped to the tempo range. NaN, infinities and negatives refused.
    pub fn checked(self) -> Result<Self, &'static str> {
        match self {
            SpeedMode::Multiple { mult } => match SPEED_MULTS.iter().find(|&&m| (m - mult).abs() < 1e-6) {
                Some(&m) => Ok(SpeedMode::Multiple { mult: m }),
                None => Err("multiple inconnu (×¼, ×½, ×1, ×2 ou ×4)"),
            },
            SpeedMode::FreeBpm { bpm } if bpm.is_finite() && bpm > 0.0 => Ok(SpeedMode::FreeBpm { bpm: bpm.clamp(MIN_BPM, MAX_BPM) }),
            SpeedMode::FreeBpm { .. } => Err("BPM invalide"),
        }
    }

    /// The same, for a saved file: never refused, the nearest valid mode.
    fn sanitized(self) -> Self {
        match self {
            SpeedMode::Multiple { mult } if mult.is_finite() => {
                let nearest = SPEED_MULTS.iter().copied().min_by(|a, b| (a - mult).abs().total_cmp(&(b - mult).abs())).unwrap_or(1.0);
                SpeedMode::Multiple { mult: nearest }
            }
            SpeedMode::FreeBpm { bpm } if bpm.is_finite() && bpm > 0.0 => SpeedMode::FreeBpm { bpm: bpm.clamp(MIN_BPM, MAX_BPM) },
            _ => SpeedMode::default(),
        }
    }

    /// Short label for the top bar (« ×½ », « 98 BPM »).
    pub fn label(self) -> String {
        match self {
            SpeedMode::Multiple { mult } => SPEED_MULTS.iter().position(|&m| m == mult).map_or_else(|| format!("×{mult}"), |i| MULT_LABELS[i].to_string()),
            SpeedMode::FreeBpm { bpm } => format!("{} BPM", bpm.round()),
        }
    }
}

/// Where a group's beat line is anchored: beat = g0 + (x - x0) × slope,
/// x being the master beat (multiple) or the engine's seconds (free BPM).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Anchor {
    x0: f64,
    g0: f64,
}

/// A multiple waiting for its bar (*Appliquer au début de la mesure*).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    mode: SpeedMode,
    /// The master beat it applies on (a master bar start).
    at_beat: f64,
    /// Its anchor, when the group's beat at `at_beat` is known in advance
    /// (from a multiple: a function of the master beat). From a free BPM
    /// it is taken when the bar comes (`frame`).
    anchor: Option<Anchor>,
    beats_per_bar: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeedGroups {
    pub groups: [SpeedMode; SPEED_GROUPS],
    /// *Cadence*, 0.5..=2.0: live fades last `length / rate`.
    pub rate: f32,
    /// *Appliquer au début de la mesure*.
    pub change_on_bar: bool,
    #[serde(skip)]
    anchors: [Anchor; SPEED_GROUPS],
    #[serde(skip)]
    pending: [Option<Pending>; SPEED_GROUPS],
    /// The multiple a group had before going free (the toggle brings it back).
    #[serde(skip)]
    last_mult: [Option<f32>; SPEED_GROUPS],
}

impl Default for SpeedGroups {
    fn default() -> Self {
        Self {
            groups: [SpeedMode::default(); SPEED_GROUPS],
            rate: 1.0,
            change_on_bar: true,
            anchors: [Anchor::default(); SPEED_GROUPS],
            pending: [None; SPEED_GROUPS],
            last_mult: [None; SPEED_GROUPS],
        }
    }
}

/// A group number (0 = default, 1..=4 = A..D) from outside: refused beyond D.
pub fn check_group(group: u8) -> Result<u8, &'static str> {
    if group as usize <= SPEED_GROUPS {
        Ok(group)
    } else {
        Err("groupe de vitesse inconnu (0 à 4)")
    }
}

/// The group a look plays in: its cell's, else its layer's, else default.
pub fn resolve(slot: Option<u8>, layer: u8) -> u8 {
    let ok = |g: u8| (g as usize <= SPEED_GROUPS).then_some(g);
    slot.and_then(ok).or_else(|| ok(layer)).unwrap_or(0)
}

fn near_multiple(x: f64, step: f64) -> bool {
    let q = x / step;
    (q - q.round()).abs() < 1e-6
}

fn bar_len(beats_per_bar: u8) -> f64 {
    beats_per_bar.max(1) as f64
}

impl SpeedGroups {
    /// Every number back in range (a file edited by hand, an old file).
    pub fn sanitized(mut self) -> Self {
        for g in &mut self.groups {
            *g = g.sanitized();
        }
        self.rate = if self.rate.is_finite() { self.rate.clamp(MIN_RATE, MAX_RATE) } else { 1.0 };
        self
    }

    /// Index 0..4 of group 1..=4; `None` for the default group or beyond D.
    fn index(group: u8) -> Option<usize> {
        (1..=SPEED_GROUPS).contains(&(group as usize)).then(|| group as usize - 1)
    }

    /// The beat of group `group` (0 = the master clock) when the master
    /// clock is at `master_beat` and the engine at `t` seconds. A pure
    /// function of those two: no state moves while frames go by.
    pub fn beat(&self, group: u8, master_beat: f64, t: f64) -> f64 {
        let Some(i) = Self::index(group) else { return master_beat };
        if let Some(Pending { mode: SpeedMode::Multiple { mult }, at_beat, anchor: Some(a), .. }) = self.pending[i] {
            if master_beat >= at_beat {
                return a.g0 + (master_beat - a.x0) * mult as f64;
            }
        }
        self.mode_beat(self.groups[i], self.anchors[i], master_beat, t)
    }

    fn mode_beat(&self, mode: SpeedMode, a: Anchor, master_beat: f64, t: f64) -> f64 {
        match mode {
            SpeedMode::Multiple { mult } => a.g0 + (master_beat - a.x0) * mult as f64,
            SpeedMode::FreeBpm { bpm } => a.g0 + (t - a.x0) * bpm / 60.0,
        }
    }

    /// The group's tempo: the master's × its multiple, or its own BPM.
    pub fn bpm(&self, group: u8, master_bpm: f64) -> f64 {
        match Self::index(group).map(|i| self.groups[i]) {
            Some(SpeedMode::Multiple { mult }) => master_bpm * mult as f64,
            Some(SpeedMode::FreeBpm { bpm }) => bpm,
            None => master_bpm,
        }
    }

    /// Where an animator for a look launched at master beat `master_beat`
    /// starts on group `group`'s beat line; `None` for a free BPM (it
    /// starts on its first frame).
    pub fn start_beat(&self, group: u8, master_beat: f64) -> Option<f64> {
        match Self::index(group).map(|i| self.groups[i]) {
            Some(SpeedMode::FreeBpm { .. }) => None,
            _ => Some(self.beat(group, master_beat, 0.0)),
        }
    }

    /// One frame's clock for every group (0 = `master` itself); `t` is
    /// the effects' seconds (`TempoClock::effect_t`).
    pub fn clocks(&self, master: &BeatClock, t: f64) -> [BeatClock; SPEED_GROUPS + 1] {
        std::array::from_fn(|g| BeatClock { beat: self.beat(g as u8, master.beat, t), bpm: self.bpm(g as u8, master.bpm), beats_per_bar: master.beats_per_bar })
    }

    /// The beats of the default group and A..D, at once.
    pub fn beats(&self, master_beat: f64, t: f64) -> [f64; SPEED_GROUPS + 1] {
        std::array::from_fn(|g| self.beat(g as u8, master_beat, t))
    }

    /// The *Cadence*, always in 0.5..=2.0.
    pub fn rate(&self) -> f64 {
        if self.rate.is_finite() {
            self.rate.clamp(MIN_RATE, MAX_RATE) as f64
        } else {
            1.0
        }
    }

    /// A live fade's length under the *Cadence*: `duration / rate`
    /// (2 s at 200 % = 1 s). Negative or not a number: 0.
    pub fn scaled(&self, duration_s: f32) -> f32 {
        self.scaled_s(duration_s as f64) as f32
    }

    pub fn scaled_s(&self, duration_s: f64) -> f64 {
        if duration_s.is_finite() && duration_s > 0.0 {
            duration_s / self.rate()
        } else {
            0.0
        }
    }

    /// Sets the *Cadence*: refused when not a number, clamped to 50–200 %.
    pub fn set_rate(&mut self, rate: f32) -> Result<f32, &'static str> {
        if !rate.is_finite() {
            return Err("cadence invalide");
        }
        self.rate = rate.clamp(MIN_RATE, MAX_RATE);
        Ok(self.rate)
    }

    /// The mode the group will have (a waiting multiple counts).
    pub fn target(&self, group: u8) -> Option<SpeedMode> {
        let i = Self::index(group)?;
        Some(self.pending[i].map_or(self.groups[i], |p| p.mode))
    }

    /// Changes group `group` (1..=4) to `mode`, checked. A new multiple
    /// waits for the next aligned bar when `change_on_bar` is on (and
    /// `immediate` is false); everything else applies now, the beat
    /// carrying on from where it is (no jump).
    pub fn set_mode(&mut self, group: u8, mode: SpeedMode, master_beat: f64, t: f64, beats_per_bar: u8) -> Result<(), &'static str> {
        self.set_mode_with(group, mode, master_beat, t, beats_per_bar, false)
    }

    fn set_mode_with(&mut self, group: u8, mode: SpeedMode, master_beat: f64, t: f64, beats_per_bar: u8, immediate: bool) -> Result<(), &'static str> {
        let i = Self::index(group).ok_or("groupe de vitesse inconnu (A à D)")?;
        let mode = mode.checked()?;
        if !master_beat.is_finite() || !t.is_finite() {
            return Err("horloge invalide");
        }
        // A waiting change lands first where it would have (it may be
        // due), then is replaced.
        self.commit(i, master_beat, t);
        self.pending[i] = None;
        if self.groups[i] == mode {
            return Ok(());
        }
        if let SpeedMode::Multiple { mult } = self.groups[i] {
            self.last_mult[i] = Some(mult);
        }
        let deferred = matches!(mode, SpeedMode::Multiple { .. }) && self.change_on_bar && !immediate;
        if deferred {
            let at_beat = self.next_aligned_bar(i, master_beat, t, beats_per_bar);
            let anchor = match self.groups[i] {
                SpeedMode::Multiple { .. } => {
                    let g = self.mode_beat(self.groups[i], self.anchors[i], at_beat, t);
                    let bar = bar_len(beats_per_bar);
                    Some(Anchor { x0: at_beat, g0: if near_multiple(g, bar) { (g / bar).round() * bar } else { g } })
                }
                SpeedMode::FreeBpm { .. } => None,
            };
            self.pending[i] = Some(Pending { mode, at_beat, anchor, beats_per_bar });
            if at_beat <= master_beat {
                self.commit(i, master_beat, t);
            }
            return Ok(());
        }
        let g = self.beat(group, master_beat, t);
        let x0 = match mode {
            SpeedMode::Multiple { .. } => master_beat,
            SpeedMode::FreeBpm { .. } => t,
        };
        self.anchors[i] = Anchor { x0, g0: g };
        self.groups[i] = mode;
        Ok(())
    }

    /// The first master bar start at or after `master_beat` where the
    /// group (as it is) is also on the first beat of one of its bars.
    fn next_aligned_bar(&self, i: usize, master_beat: f64, t: f64, beats_per_bar: u8) -> f64 {
        let bar = bar_len(beats_per_bar);
        let mut first = (master_beat / bar).ceil() * bar;
        if near_multiple(master_beat, bar) {
            first = (master_beat / bar).round() * bar;
        }
        if let SpeedMode::Multiple { .. } = self.groups[i] {
            for k in 0..MAX_WAIT_BARS {
                let b = first + k as f64 * bar;
                if near_multiple(self.mode_beat(self.groups[i], self.anchors[i], b, t), bar) {
                    return b;
                }
            }
        }
        first
    }

    /// Lands group `i`'s waiting change once the master clock reached it.
    fn commit(&mut self, i: usize, master_beat: f64, t: f64) {
        let Some(p) = self.pending[i] else { return };
        if master_beat < p.at_beat {
            return;
        }
        let anchor = p.anchor.unwrap_or_else(|| {
            // From a free BPM: on the group's next "one", the master's bar.
            let bar = bar_len(p.beats_per_bar);
            let g = self.mode_beat(self.groups[i], self.anchors[i], master_beat, t);
            let g0 = if near_multiple(g, bar) { (g / bar).round() * bar } else { (g / bar).ceil() * bar };
            Anchor { x0: p.at_beat, g0 }
        });
        self.anchors[i] = anchor;
        self.groups[i] = p.mode;
        self.pending[i] = None;
    }

    /// Every engine frame: waiting multiples whose bar has come land.
    /// Returns true when one did (worth saving).
    pub fn frame(&mut self, master_beat: f64, t: f64) -> bool {
        let mut landed = false;
        for i in 0..SPEED_GROUPS {
            if self.pending[i].is_some_and(|p| master_beat >= p.at_beat) {
                self.commit(i, master_beat, t);
                landed = true;
            }
        }
        landed
    }

    /// `tempo.resync`: the master's beat `bar_beat` (now a bar start) is
    /// also a "one" for every multiple group, the nearest one, so their
    /// phase stays a function of the master clock. Free BPMs are their own.
    pub fn resync(&mut self, bar_beat: f64, t: f64, beats_per_bar: u8) {
        let bar = bar_len(beats_per_bar);
        for i in 0..SPEED_GROUPS {
            self.commit(i, bar_beat, t);
            if let SpeedMode::Multiple { .. } = self.groups[i] {
                let g = self.beat(i as u8 + 1, bar_beat, t);
                self.anchors[i] = Anchor { x0: bar_beat, g0: (g / bar).round() * bar };
            }
        }
    }

    /// The *BPM libre* toggle: on, the group keeps its present tempo as a
    /// free BPM; off, it goes back to the multiple it had (×1 if none).
    pub fn set_free(&mut self, group: u8, free: bool, master_bpm: f64, master_beat: f64, t: f64, beats_per_bar: u8) -> Result<(), &'static str> {
        let i = Self::index(group).ok_or("groupe de vitesse inconnu (A à D)")?;
        let now = self.target(group).unwrap_or_default();
        match (now, free) {
            (SpeedMode::Multiple { mult }, true) => {
                let bpm = (master_bpm * mult as f64).clamp(MIN_BPM, MAX_BPM);
                self.set_mode(group, SpeedMode::FreeBpm { bpm }, master_beat, t, beats_per_bar)
            }
            (SpeedMode::FreeBpm { .. }, false) => {
                let mult = self.last_mult[i].unwrap_or(1.0);
                self.set_mode(group, SpeedMode::Multiple { mult }, master_beat, t, beats_per_bar)
            }
            _ => Ok(()),
        }
    }

    /// Another set of groups (a project opened, `POST /api/speed-groups`
    /// with every group): each group changes at once, from where its beat
    /// is, so nothing playing jumps.
    pub fn replace(&mut self, other: &SpeedGroups, master_beat: f64, t: f64, beats_per_bar: u8) {
        let other = other.clone().sanitized();
        self.rate = other.rate;
        self.change_on_bar = other.change_on_bar;
        for (i, mode) in other.groups.iter().enumerate() {
            let _ = self.set_mode_with(i as u8 + 1, *mode, master_beat, t, beats_per_bar, true);
        }
    }

    /// For `/api/speed-groups` and `tempo.groups` in `/api/frame`: no beat
    /// positions (they move every frame; `/api/frame` adds them).
    pub fn view(&self, master_bpm: f64) -> Value {
        let groups: Vec<Value> = (0..SPEED_GROUPS)
            .map(|i| {
                let g = i as u8 + 1;
                let mode = self.groups[i];
                json!({
                    "group": g,
                    "name": GROUP_NAMES[g as usize],
                    "mode": mode,
                    "mult": match mode { SpeedMode::Multiple { mult } => Some(mult), _ => None },
                    "free": matches!(mode, SpeedMode::FreeBpm { .. }),
                    "bpm": (self.bpm(g, master_bpm) * 10.0).round() / 10.0,
                    "label": mode.label(),
                    "pending": self.pending[i].map(|p| json!({ "mode": p.mode, "label": p.mode.label(), "at_beat": p.at_beat })),
                })
            })
            .collect();
        json!({
            "groups": groups,
            "rate": self.rate(),
            "change_on_bar": self.change_on_bar,
            "names": GROUP_NAMES,
            "mults": SPEED_MULTS,
            "mult_labels": MULT_LABELS,
            "min_rate": MIN_RATE,
            "max_rate": MAX_RATE,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempo::TempoClock;

    fn mult(m: f32) -> SpeedMode {
        SpeedMode::Multiple { mult: m }
    }

    /// Groups A..D at ×¼, ×½, ×2, ×4, set at once (no bar wait).
    fn four() -> SpeedGroups {
        let mut g = SpeedGroups { change_on_bar: false, ..Default::default() };
        for (n, m) in [(1, 0.25), (2, 0.5), (3, 2.0), (4, 4.0)] {
            g.set_mode(n, mult(m), 0.0, 0.0, 4).unwrap();
        }
        g
    }

    #[test]
    fn every_multiple_is_exact_over_32_master_beats() {
        let g = four();
        for (n, m) in [(0u8, 1.0), (1, 0.25), (2, 0.5), (3, 2.0), (4, 4.0)] {
            // Thousands of frames, nothing accumulated: exact equality.
            for k in 0..=32 * 60 {
                let master = k as f64 / 60.0;
                assert_eq!(g.beat(n, master, 0.0), master * m);
            }
            assert_eq!(g.beat(n, 32.0, 0.0) - g.beat(n, 0.0, 0.0), 32.0 * m);
        }
    }

    #[test]
    fn a_master_tempo_change_keeps_the_ratio_exact() {
        let g = four();
        let mut clock = TempoClock::default();
        let b0 = clock.beat_at(0.0);
        clock.set_bpm(150.0, 3.3);
        clock.set_bpm(97.0, 7.1);
        for t in [8.0, 12.5, 30.0] {
            let master = clock.beat_at(t);
            assert_eq!(g.beat(2, master, t) - g.beat(2, b0, 0.0), (master - b0) * 0.5);
            assert_eq!(g.bpm(2, clock.bpm), 48.5);
        }
    }

    #[test]
    fn a_free_bpm_ignores_the_master_tempo() {
        let mut g = SpeedGroups::default();
        g.set_mode(1, SpeedMode::FreeBpm { bpm: 90.0 }, 0.0, 0.0, 4).unwrap();
        // Free applies at once, even with the bar option on.
        assert_eq!(g.groups[0], SpeedMode::FreeBpm { bpm: 90.0 });
        for (master, bpm) in [(10.0, 120.0), (500.0, 200.0)] {
            assert_eq!(g.beat(1, master, 4.0), 6.0);
            assert_eq!(g.bpm(1, bpm), 90.0);
        }
    }

    #[test]
    fn scaled_divides_by_the_rate_within_bounds() {
        let mut g = SpeedGroups::default();
        assert_eq!(g.scaled(2.0), 2.0);
        g.set_rate(2.0).unwrap();
        assert_eq!(g.scaled(2.0), 1.0);
        g.set_rate(0.5).unwrap();
        assert_eq!(g.scaled(2.0), 4.0);
        assert_eq!(g.set_rate(9.0), Ok(2.0));
        assert_eq!(g.set_rate(0.0), Ok(0.5));
        assert!(g.set_rate(f32::NAN).is_err() && g.set_rate(f32::INFINITY).is_err());
        assert_eq!(g.rate, 0.5);
        for bad in [f32::NAN, -1.0, f32::NEG_INFINITY] {
            assert_eq!(g.scaled(bad), 0.0);
        }
        let file: SpeedGroups = serde_json::from_str(r#"{"rate": 7}"#).unwrap();
        assert_eq!(file.sanitized().rate, 2.0);
    }

    #[test]
    fn a_new_multiple_waits_for_the_bar_and_carries_on_without_a_jump() {
        let mut g = SpeedGroups::default();
        // At master beat 5.5, ×½ waits for beat 8 (the next bar).
        g.set_mode(1, mult(0.5), 5.5, 0.0, 4).unwrap();
        assert_eq!(g.groups[0], mult(1.0));
        assert_eq!(g.target(1), Some(mult(0.5)));
        assert_eq!(g.beat(1, 7.0, 0.0), 7.0);
        // Pure: the waiting multiple is already the answer past its bar.
        assert_eq!(g.beat(1, 10.0, 0.0), 9.0);
        assert!(!g.frame(7.9, 0.0));
        assert!(g.frame(8.01, 0.0));
        assert_eq!(g.groups[0], mult(0.5));
        assert_eq!(g.beat(1, 8.0, 0.0), 8.0);
        assert_eq!(g.beat(1, 16.0, 0.0), 12.0);
        // Back to ×1: group bar every 8 master beats from 8, so not at 20
        // (group beat 14) but at 24 (group beat 16).
        g.set_mode(1, mult(1.0), 17.0, 0.0, 4).unwrap();
        g.frame(20.0, 0.0);
        assert_eq!(g.groups[0], mult(0.5));
        g.frame(24.0, 0.0);
        assert_eq!(g.groups[0], mult(1.0));
        assert_eq!(g.beat(1, 28.0, 0.0), 20.0);
    }

    #[test]
    fn without_the_bar_option_a_multiple_applies_at_once_from_where_it_is() {
        let mut g = SpeedGroups { change_on_bar: false, ..Default::default() };
        g.set_mode(1, mult(2.0), 5.0, 0.0, 4).unwrap();
        assert_eq!(g.beat(1, 5.0, 0.0), 5.0);
        assert_eq!(g.beat(1, 6.0, 0.0), 7.0);
    }

    #[test]
    fn resync_brings_multiple_groups_back_on_the_master_one() {
        let mut g = SpeedGroups { change_on_bar: false, ..Default::default() };
        g.set_mode(1, mult(0.5), 5.0, 0.0, 4).unwrap();
        // Off the bar: group beat 5 at master 5, 6.5 at master 8.
        assert_eq!(g.beat(1, 8.0, 0.0), 6.5);
        let mut clock = TempoClock::default();
        clock.resync(4.2);
        let bar_beat = clock.beat_at(4.2);
        g.resync(bar_beat, 4.2, 4);
        let at = g.beat(1, bar_beat, 4.2);
        assert_eq!(at % 4.0, 0.0);
        assert_eq!(g.beat(1, bar_beat + 8.0, 4.2), at + 4.0);
    }

    #[test]
    fn free_toggle_keeps_the_tempo_and_brings_the_multiple_back() {
        let mut g = SpeedGroups { change_on_bar: false, ..Default::default() };
        g.set_mode(2, mult(0.5), 0.0, 0.0, 4).unwrap();
        g.set_free(2, true, 128.0, 4.0, 2.0, 4).unwrap();
        assert_eq!(g.groups[1], SpeedMode::FreeBpm { bpm: 64.0 });
        assert_eq!(g.beat(2, 4.0, 2.0), 2.0);
        g.set_free(2, false, 128.0, 8.0, 4.0, 4).unwrap();
        assert_eq!(g.groups[1], mult(0.5));
    }

    #[test]
    fn bad_values_are_refused_and_files_are_clamped() {
        let mut g = SpeedGroups::default();
        for bad in [mult(3.0), mult(f32::NAN), SpeedMode::FreeBpm { bpm: f64::NAN }, SpeedMode::FreeBpm { bpm: -10.0 }, SpeedMode::FreeBpm { bpm: f64::INFINITY }] {
            assert!(g.set_mode(1, bad, 0.0, 0.0, 4).is_err(), "{bad:?}");
        }
        assert!(g.set_mode(5, mult(1.0), 0.0, 0.0, 4).is_err());
        assert!(g.set_mode(0, mult(1.0), 0.0, 0.0, 4).is_err());
        assert!(g.set_mode(1, mult(1.0), f64::NAN, 0.0, 4).is_err());
        assert_eq!(g, SpeedGroups::default());
        g.set_mode(1, SpeedMode::FreeBpm { bpm: 999.0 }, 0.0, 0.0, 4).unwrap();
        assert_eq!(g.groups[0], SpeedMode::FreeBpm { bpm: MAX_BPM });
        let file: SpeedGroups =
            serde_json::from_str(r#"{"groups":[{"type":"multiple","mult":3.1},{"type":"free_bpm","bpm":-4},{"type":"free_bpm","bpm":10},{"type":"multiple","mult":0.3}]}"#).unwrap();
        let file = file.sanitized();
        assert_eq!(file.groups, [mult(4.0), mult(1.0), SpeedMode::FreeBpm { bpm: MIN_BPM }, mult(0.25)]);
        assert_eq!(check_group(4), Ok(4));
        assert!(check_group(5).is_err());
    }

    #[test]
    fn resolution_is_cell_then_layer_then_default() {
        assert_eq!(resolve(Some(2), 3), 2);
        assert_eq!(resolve(None, 3), 3);
        assert_eq!(resolve(None, 0), 0);
        assert_eq!(resolve(Some(0), 3), 0, "a cell can force the default group");
        assert_eq!(resolve(Some(9), 3), 3);
        assert_eq!(resolve(None, 9), 0);
    }

    #[test]
    fn old_files_load_with_every_group_at_x1() {
        let g: SpeedGroups = serde_json::from_str("{}").unwrap();
        assert_eq!(g, SpeedGroups::default());
        assert_eq!(g.rate(), 1.0);
        assert!(g.change_on_bar);
        let back: SpeedGroups = serde_json::from_str(&serde_json::to_string(&four()).unwrap()).unwrap();
        assert_eq!(back.groups, four().groups);
    }
}
