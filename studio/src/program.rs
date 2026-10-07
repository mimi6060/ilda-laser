//! Cue programs (T-157): a cue as a mini-timeline. On top of its content
//! (any `Content`, an evolving cue's keys included, T-111) a program adds
//! up to four inner layers drawn one after the other, curves (« courbes »)
//! on the cue's own modifiers - size, position, rotation, colour, strobe -
//! and tempo-synced LFOs of its own.
//!
//! Everything is a pure function of the cue's local beat (the same
//! `beat_pos` and quantized launch as T-111), so the content keys, the
//! curves and the LFOs stay in phase, and the same local beat always gives
//! the same frame. The cue's modifiers apply to the cue's own frame inside
//! the animator: the master live controls, the layer mixer, calibration
//! and the whole safety stage still come after, and a program has no way
//! to arm anything.

use crate::engine::Content;
use crate::evolving::{self, KeyEase};
use crate::lfo::Modulator;
use crate::live::{self, ColorOverride, LiveModifiers, MAX_PRISM};
use crate::patterns::Point;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Inner layers on top of the cue's main content.
pub const MAX_LAYERS: usize = 4;
pub const MAX_LANES: usize = 16;
pub const MAX_KEYS: usize = 64;
pub const MAX_LFOS: usize = 8;
/// A program's length when neither it nor its content gives one.
pub const DEFAULT_LENGTH: f32 = 16.0;
const MIN_LENGTH: f32 = 1.0 / 16.0;
pub const MAX_LENGTH: f32 = 1024.0;

/// What happens at the end of the program's length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    /// Back to the start (« Boucle »).
    #[default]
    Loop,
    /// Stays on the last values (« Une fois »).
    Once,
    /// Plays backwards to the start, then forwards again (« Aller-retour »).
    PingPong,
}

/// One point of a curve: `value` at `beat`, then eased towards the next key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Key {
    pub beat: f32,
    pub value: f32,
    /// T-111's easing (« Palier, Linéaire, Douce, Accélère, Ralentit »).
    pub ease: KeyEase,
}

/// A curve on one of the cue's modifiers (an id of `TARGETS`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lane {
    pub target: String,
    /// In any order.
    pub keys: Vec<Key>,
}

/// `Settings::program`: the mini-timeline of a cue.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CueProgram {
    /// `None`: the evolving content's length, else `DEFAULT_LENGTH`.
    pub length_beats: Option<f32>,
    pub play: PlayMode,
    /// Drawn after the main content, in order, joined by blanked travel.
    pub layers: Vec<Content>,
    pub lanes: Vec<Lane>,
    /// The cue's own LFOs (T-151's modulators, on `TARGETS` ids). Their
    /// beat is the cue's local beat.
    pub lfos: Vec<Modulator>,
}

/// What curves and LFOs can move: (id, min, max, value without a curve),
/// in the units of the matching master control (`master.<id>`), plus
/// `strobe.div` (flashes per beat, 0 = off).
pub const TARGETS: &[(&str, f32, f32, f32)] = &[
    ("brightness", 0.0, 1.0, 1.0),
    ("size", 0.0, 2.0, 1.0),
    ("size_x", -2.0, 2.0, 1.0),
    ("size_y", -2.0, 2.0, 1.0),
    ("pos_x", -1.0, 1.0, 0.0),
    ("pos_y", -1.0, 1.0, 0.0),
    ("rot_x.angle", -180.0, 180.0, 0.0),
    ("rot_y.angle", -180.0, 180.0, 0.0),
    ("rot_z.angle", -180.0, 180.0, 0.0),
    // Degrees per second, like the master rotation (integrated over the
    // curve, so a spin that speeds up never jumps).
    ("rot_x.speed", -720.0, 720.0, 0.0),
    ("rot_y.speed", -720.0, 720.0, 0.0),
    ("rot_z.speed", -720.0, 720.0, 0.0),
    ("perspective", 0.0, 1.0, 0.3),
    // A curve or LFO here switches the cue to « Teinte » colouring.
    ("color.hue", 0.0, 360.0, 0.0),
    ("strobe.div", 0.0, evolving::MAX_STROBE_DIV, 0.0),
    // Live effects (T-142) on the cue's own frame: visible points and
    // dotted lines only remove light; the prism's copies (rounded to a
    // whole count) are joined by blanked travel.
    ("trace", 0.0, 1.0, 1.0),
    ("dots", 0.0, 1.0, 0.0),
    ("prism", 1.0, MAX_PRISM as f32, 1.0),
];

const BRIGHTNESS: usize = 0;
const ROT_ANGLE: usize = 6;
const ROT_SPEED: usize = 9;
const HUE: usize = 13;
const STROBE: usize = 14;
const TRACE: usize = 15;
const DOTS: usize = 16;
const PRISM: usize = 17;

fn target_index(id: &str) -> Option<usize> {
    TARGETS.iter().position(|t| t.0 == id)
}

/// Area under an ease from 0 to `u` (0..1): ∫ `ease.apply`.
fn ease_area(ease: KeyEase, u: f32) -> f64 {
    let u = u.clamp(0.0, 1.0) as f64;
    match ease {
        KeyEase::Step => 0.0,
        KeyEase::Linear => u * u / 2.0,
        KeyEase::EaseIn => u * u * u / 3.0,
        KeyEase::EaseOut => u - (1.0 - (1.0 - u).powi(3)) / 3.0,
        KeyEase::Smooth => u * u * u - u.powi(4) / 2.0,
    }
}

/// A keyed curve at `x`: `keys` are (position, value, ease), finite and
/// sorted by position. The first key's value before it, the last's after
/// it, eased towards the next key in between. `None` without keys. The one
/// curve evaluation of the studio: cue programs (T-157) and the timeline's
/// envelopes (T-163) both go through it.
pub fn curve_at(keys: &[(f64, f32, KeyEase)], x: f64) -> Option<f32> {
    let (first, last) = (keys.first()?, keys.last()?);
    if x <= first.0 {
        return Some(first.1);
    }
    if x >= last.0 {
        return Some(last.1);
    }
    let i = keys.iter().rposition(|k| k.0 <= x)?;
    let (k, next) = (keys[i], keys[i + 1]);
    let u = ((x - k.0) / (next.0 - k.0)) as f32;
    Some(k.1 + (next.1 - k.1) * k.2.apply(u))
}

impl Lane {
    /// Finite keys by beat (on a tie, list order).
    fn sorted(&self) -> Vec<&Key> {
        let mut keys: Vec<&Key> = self.keys.iter().filter(|k| k.beat.is_finite() && k.value.is_finite()).collect();
        keys.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        keys
    }

    /// The curve at beat `b`: see `curve_at`. `None` without keys.
    pub fn value_at(&self, b: f64) -> Option<f32> {
        let keys: Vec<(f64, f32, KeyEase)> = self.sorted().iter().map(|k| (k.beat as f64, k.value, k.ease)).collect();
        curve_at(&keys, b)
    }

    /// ∫ of the curve from beat 0 to `x` (value × beats), exact for every ease.
    fn integral(&self, x: f64) -> f64 {
        let keys = self.sorted();
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return 0.0 };
        // F(b) = ∫ from the first key to b.
        let f = |b: f64| -> f64 {
            if b <= first.beat as f64 {
                return (b - first.beat as f64) * first.value as f64;
            }
            let mut area = 0.0;
            for w in keys.windows(2) {
                let (k, next) = (w[0], w[1]);
                let (k0, k1) = (k.beat as f64, next.beat as f64);
                if k1 <= k0 {
                    continue;
                }
                let span = k1 - k0;
                let u = ((b - k0) / span).clamp(0.0, 1.0) as f32;
                let (v0, dv) = (k.value as f64, (next.value - k.value) as f64);
                area += span * (v0 * u as f64 + dv * ease_area(k.ease, u));
                if b <= k1 {
                    return area;
                }
            }
            area + (b - last.beat as f64) * last.value as f64
        };
        f(x) - f(0.0)
    }
}

/// The cue's own modifiers at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Mods {
    pub live: LiveModifiers,
    /// Strobe flashes per beat, 0 = none.
    pub strobe_div: f32,
}

impl CueProgram {
    /// Length in beats, for a cue whose main content is `content`.
    pub fn length(&self, content: &Content) -> f64 {
        let own = self.length_beats.filter(|l| l.is_finite());
        let from_content = match content {
            Content::Evolving(cue) => cue.length() as f32,
            _ => DEFAULT_LENGTH,
        };
        own.unwrap_or(from_content).clamp(MIN_LENGTH, MAX_LENGTH) as f64
    }

    /// Where the curves are, `raw` beats after the launch (0 before it).
    pub fn local(&self, raw: f64, len: f64) -> f64 {
        let raw = raw.max(0.0);
        match self.play {
            PlayMode::Loop => raw.rem_euclid(len),
            PlayMode::Once => raw.min(len),
            PlayMode::PingPong => {
                let p = raw.rem_euclid(2.0 * len);
                if p <= len { p } else { 2.0 * len - p }
            }
        }
    }

    /// Beats the curves have moved through by `raw` beats after the launch
    /// times their value: ∫ of `lane` over the played path (loops and
    /// returns included), for speeds.
    fn travelled(&self, lane: &Lane, raw: f64, len: f64) -> f64 {
        let raw = raw.max(0.0);
        let i = |x: f64| lane.integral(x);
        match self.play {
            PlayMode::Loop => {
                let n = (raw / len).floor();
                n * i(len) + i(raw - n * len)
            }
            PlayMode::Once if raw <= len => i(raw),
            PlayMode::Once => i(len) + (raw - len) * lane.value_at(len).unwrap_or(0.0) as f64,
            PlayMode::PingPong => {
                let n = (raw / (2.0 * len)).floor();
                let rem = raw - n * 2.0 * len;
                let part = if rem <= len { i(rem) } else { 2.0 * i(len) - i(2.0 * len - rem) };
                n * 2.0 * i(len) + part
            }
        }
    }

    /// The cue's modifiers `raw` beats after its launch at `bpm` (which
    /// turns degrees per second into degrees per beat). `None` when the
    /// program moves nothing.
    pub fn modifiers(&self, raw: f64, len: f64, bpm: f64) -> Option<Mods> {
        if self.lanes.is_empty() && self.lfos.iter().all(|m| !m.enabled || m.depth <= 0.0) {
            return None;
        }
        let raw = raw.max(0.0);
        let b = self.local(raw, len);
        let mut values: Vec<f32> = TARGETS.iter().map(|t| t.3).collect();
        let mut touched = [false; TARGETS.len()];
        let mut spin = [0.0f64; 3];
        let sec_per_beat = 60.0 / bpm.max(1.0);
        for lane in &self.lanes {
            let Some(i) = target_index(&lane.target) else { continue };
            let Some(v) = lane.value_at(b) else { continue };
            let (_, min, max, _) = TARGETS[i];
            values[i] = v.clamp(min, max);
            touched[i] = true;
            if (ROT_SPEED..ROT_SPEED + 3).contains(&i) {
                // Clamped like the value: a key past the range spins no faster.
                let clamped = Lane {
                    target: String::new(),
                    keys: lane.keys.iter().map(|k| Key { value: k.value.clamp(min, max), ..k.clone() }).collect(),
                };
                spin[i - ROT_SPEED] = self.travelled(&clamped, raw, len) * sec_per_beat;
            }
        }
        // LFOs move around the curve (or the default); several on one
        // target add up. Like the master LFOs: a brightness only dims.
        let t = raw * sec_per_beat;
        let mut deltas = [0.0f32; TARGETS.len()];
        for m in self.lfos.iter().filter(|m| m.enabled && m.depth > 0.0 && m.part.is_none()) {
            let Some(i) = target_index(&m.target).filter(|i| !(ROT_SPEED..ROT_SPEED + 3).contains(i)) else { continue };
            let wave = m.wave_at(t, raw);
            let amount = m.depth.clamp(0.0, 1.0) * (wave + m.offset.clamp(-1.0, 1.0));
            if amount.is_finite() {
                let (_, min, max, _) = TARGETS[i];
                deltas[i] += amount * (max - min) / 2.0;
                touched[i] = true;
            }
        }
        for (i, d) in deltas.iter().enumerate().filter(|(_, d)| **d != 0.0) {
            let (_, min, max, _) = TARGETS[i];
            let base = values[i];
            values[i] = (base + d).clamp(min, max);
            if i == BRIGHTNESS {
                values[i] = values[i].min(base);
            }
        }
        let v = |i: usize| values[i];
        let mut live = LiveModifiers {
            brightness: v(0),
            size: v(1),
            size_x: v(2),
            size_y: v(3),
            pos_x: v(4),
            pos_y: v(5),
            rot_angle: [0, 1, 2].map(|a| ((v(ROT_ANGLE + a) as f64 + spin[a]).rem_euclid(360.0)) as f32),
            perspective: v(12),
            trace: v(TRACE),
            dots: v(DOTS),
            prism: v(PRISM).round().clamp(1.0, MAX_PRISM as f32) as u8,
            ..LiveModifiers::default()
        };
        if touched[HUE] {
            live.color = ColorOverride::Hue { hue: v(HUE) };
        }
        Some(Mods { live, strobe_div: v(STROBE) })
    }

    /// Checks a program sent by a client: sizes, known targets, finite numbers.
    pub fn validate(&self) -> Result<()> {
        if let Some(l) = self.length_beats {
            if !l.is_finite() || !(MIN_LENGTH..=MAX_LENGTH).contains(&l) {
                bail!("longueur du programme hors limites (1/16 à {MAX_LENGTH} temps)");
            }
        }
        if self.layers.len() > MAX_LAYERS {
            bail!("{MAX_LAYERS} couches internes au maximum");
        }
        if self.lanes.len() > MAX_LANES {
            bail!("{MAX_LANES} courbes au maximum");
        }
        for (n, lane) in self.lanes.iter().enumerate() {
            if target_index(&lane.target).is_none() {
                bail!("courbe : « {} » n'est pas un paramètre du cue", lane.target);
            }
            if self.lanes[..n].iter().any(|l| l.target == lane.target) {
                bail!("deux courbes sur « {} »", lane.target);
            }
            if lane.keys.len() > MAX_KEYS {
                bail!("{MAX_KEYS} clés par courbe au maximum");
            }
            if lane.keys.iter().any(|k| !k.beat.is_finite() || !k.value.is_finite() || k.beat < 0.0) {
                bail!("courbe « {} » : clé invalide", lane.target);
            }
        }
        if self.lfos.len() > MAX_LFOS {
            bail!("{MAX_LFOS} LFO par cue au maximum");
        }
        for m in &self.lfos {
            if m.part.is_some() {
                bail!("LFO : une partie de figure se module dans le panneau Modulateurs, pas dans un cue");
            }
            match target_index(&m.target) {
                Some(i) if !(ROT_SPEED..ROT_SPEED + 3).contains(&i) => {}
                _ => bail!("LFO : « {} » ne peut pas être modulé dans un cue", m.target),
            }
            if !(m.depth.is_finite() && m.phase.is_finite() && m.offset.is_finite()) {
                bail!("LFO « {} » : valeur invalide", m.target);
            }
        }
        Ok(())
    }
}

impl Mods {
    /// The cue's frame through its modifiers (geometry, colour, dimmer),
    /// then its strobe at local beat `raw`. Only ever dims: brightness is
    /// 0..1 and the strobe cuts.
    pub fn apply(&self, points: Vec<Point>, raw: f64) -> Vec<Point> {
        let mut out = if self.live == LiveModifiers::default() {
            points
        } else {
            live::apply(&points, &self.live, &live::LiveState::default(), &[])
        };
        if evolving::strobe(raw.max(0.0), self.strobe_div) < 1.0 {
            for p in &mut out {
                (p.r, p.g, p.b) = (0.0, 0.0, 0.0);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(target: &str, keys: &[(f32, f32, KeyEase)]) -> Lane {
        Lane { target: target.into(), keys: keys.iter().map(|&(beat, value, ease)| Key { beat, value, ease }).collect() }
    }

    fn program(play: PlayMode, lanes: Vec<Lane>) -> CueProgram {
        CueProgram { length_beats: Some(16.0), play, lanes, ..Default::default() }
    }

    fn size_at(p: &CueProgram, raw: f64) -> f32 {
        p.modifiers(raw, p.length(&Content::Wave), 120.0).unwrap().live.size
    }

    #[test]
    fn a_linear_size_curve_is_half_way_at_half_time() {
        let p = program(PlayMode::Loop, vec![lane("size", &[(0.0, 0.2, KeyEase::Linear), (16.0, 1.0, KeyEase::Linear)])]);
        assert!((size_at(&p, 8.0) - 0.6).abs() < 1e-3, "{}", size_at(&p, 8.0));
        assert!((size_at(&p, 0.0) - 0.2).abs() < 1e-6);
        // Looped: the same 8 beats into the next pass.
        assert!((size_at(&p, 24.0) - 0.6).abs() < 1e-3);
    }

    #[test]
    fn eases_shape_the_curve() {
        let at = |ease| size_at(&program(PlayMode::Once, vec![lane("size", &[(0.0, 0.0, ease), (16.0, 1.0, KeyEase::Linear)])]), 4.0);
        assert_eq!(at(KeyEase::Step), 0.0);
        assert!((at(KeyEase::Linear) - 0.25).abs() < 1e-6);
        assert!(at(KeyEase::EaseIn) < 0.25 && at(KeyEase::EaseOut) > 0.25);
    }

    #[test]
    fn once_holds_the_last_value_and_ping_pong_comes_back() {
        let keys = [(0.0, 0.2, KeyEase::Linear), (16.0, 1.0, KeyEase::Linear)];
        let once = program(PlayMode::Once, vec![lane("size", &keys)]);
        assert!((size_at(&once, 16.0) - 1.0).abs() < 1e-6);
        assert!((size_at(&once, 40.0) - 1.0).abs() < 1e-6, "stays on the last value");
        let pp = program(PlayMode::PingPong, vec![lane("size", &keys)]);
        assert!((size_at(&pp, 20.0) - 0.8).abs() < 1e-3, "4 beats back from the end");
        assert!((size_at(&pp, 32.0) - 0.2).abs() < 1e-3, "back at the start");
        assert!((size_at(&pp, 40.0) - 0.6).abs() < 1e-3, "forwards again");
        let looped = program(PlayMode::Loop, vec![lane("size", &keys)]);
        assert!((size_at(&looped, 20.0) - 0.4).abs() < 1e-3, "a loop starts over");
    }

    #[test]
    fn integrals_are_exact() {
        for ease in [KeyEase::Step, KeyEase::Linear, KeyEase::EaseIn, KeyEase::EaseOut, KeyEase::Smooth] {
            let l = lane("rot_z.speed", &[(2.0, 1.0, ease), (6.0, 3.0, KeyEase::Linear)]);
            // Riemann sum, fine enough to compare to 1e-3.
            let n = 200_000;
            let x = 9.0;
            let num: f64 = (0..n).map(|k| l.value_at((k as f64 + 0.5) * x / n as f64).unwrap() as f64 * x / n as f64).sum();
            assert!((l.integral(x) - num).abs() < 1e-3, "{ease:?}: {} vs {num}", l.integral(x));
        }
    }

    #[test]
    fn a_spin_curve_turns_without_jumps_through_loops_and_returns() {
        // 90 °/s at 60 BPM = 90 ° per beat.
        for play in [PlayMode::Loop, PlayMode::Once, PlayMode::PingPong] {
            let p = CueProgram {
                length_beats: Some(4.0),
                play,
                lanes: vec![lane("rot_z.speed", &[(0.0, 0.0, KeyEase::Linear), (4.0, 180.0, KeyEase::Linear)])],
                ..Default::default()
            };
            let angle = |raw: f64| p.modifiers(raw, 4.0, 60.0).unwrap().live.rot_angle[2];
            let mut prev = angle(0.0);
            for k in 1..=400 {
                let a = angle(k as f64 * 0.05);
                let step = (a - prev).rem_euclid(360.0);
                assert!(step < 10.0, "{play:?}: jump of {step}° at {}", k as f64 * 0.05);
                prev = a;
            }
        }
        let constant = CueProgram { lanes: vec![lane("rot_z.speed", &[(0.0, 90.0, KeyEase::Linear)])], ..Default::default() };
        let a = constant.modifiers(1.5, 16.0, 60.0).unwrap().live.rot_angle[2];
        assert!((a - 135.0).abs() < 1e-3, "{a}");
    }

    #[test]
    fn local_lfos_move_around_the_curve_and_only_dim() {
        let lfo = |target: &str, offset| Modulator { target: target.into(), depth: 0.5, offset, rate: live::Rate::Beats(4.0), ..Default::default() };
        let p = CueProgram { lfos: vec![lfo("size", 0.0), lfo("brightness", 1.0)], ..Default::default() };
        // Sine at a quarter cycle (+1): size 1 + 0.5 × 1 × (2 - 0) / 2.
        let m = p.modifiers(1.0, 16.0, 120.0).unwrap();
        assert!((m.live.size - 1.5).abs() < 1e-4, "{}", m.live.size);
        assert_eq!(m.live.brightness, 1.0, "an LFO never brightens");
        // Same local beat → same values.
        assert_eq!(p.modifiers(1.0, 16.0, 120.0), Some(m));
    }

    #[test]
    fn hue_curves_recolour_and_strobe_cuts() {
        let p = program(PlayMode::Loop, vec![lane("color.hue", &[(0.0, 240.0, KeyEase::Step)]), lane("strobe.div", &[(0.0, 2.0, KeyEase::Step)])]);
        let m = p.modifiers(0.1, 16.0, 120.0).unwrap();
        assert_eq!(m.live.color, ColorOverride::Hue { hue: 240.0 });
        let pts = vec![Point::lit(0.0, 0.0, 1.0, 0.0, 0.0), Point::lit(0.1, 0.0, 1.0, 0.0, 0.0)];
        let on = m.apply(pts.clone(), 0.1);
        assert!(on[0].b > 0.9 && on[0].r < 0.1, "blue: {:?}", on[0]);
        let off = m.apply(pts, 0.3);
        assert!(off.iter().all(|p| !p.is_lit()), "second half of the 1/2-beat slot is dark");
    }

    #[test]
    fn out_of_range_and_bad_numbers_are_clamped_at_render() {
        let p = program(PlayMode::Loop, vec![lane("brightness", &[(0.0, 7.0, KeyEase::Linear)]), lane("size", &[(f32::NAN, 1.0, KeyEase::Linear), (0.0, 99.0, KeyEase::Linear)])]);
        let m = p.modifiers(3.0, 16.0, 120.0).unwrap();
        assert_eq!((m.live.brightness, m.live.size), (1.0, 2.0));
        assert!(p.validate().is_err(), "NaN key refused by the API");
    }

    #[test]
    fn validation_names_the_problem() {
        assert!(CueProgram::default().validate().is_ok());
        let bad = |p: CueProgram| p.validate().unwrap_err().to_string();
        assert!(bad(CueProgram { layers: vec![Content::Wave; 5], ..Default::default() }).contains("4 couches"));
        assert!(bad(program(PlayMode::Loop, vec![lane("mirror", &[])])).contains("mirror"));
        // The live effects of T-142 are cue parameters now.
        assert!(program(PlayMode::Loop, vec![lane("trace", &[]), lane("prism", &[]), lane("dots", &[])]).validate().is_ok());
        assert!(bad(program(PlayMode::Loop, vec![lane("size", &[]), lane("size", &[])])).contains("deux courbes"));
        assert!(bad(CueProgram { length_beats: Some(0.0), ..Default::default() }).contains("longueur"));
        let spin_lfo = Modulator { target: "rot_z.speed".into(), ..Default::default() };
        assert!(bad(CueProgram { lfos: vec![spin_lfo], ..Default::default() }).contains("rot_z.speed"));
    }

    #[test]
    fn trace_and_prism_curves_drive_the_cues_own_effects() {
        let p = program(PlayMode::Loop, vec![lane("prism", &[(0.0, 3.6, KeyEase::Step)]), lane("trace", &[(0.0, 0.5, KeyEase::Step)])]);
        let m = p.modifiers(1.0, 16.0, 120.0).unwrap();
        assert_eq!((m.live.prism, m.live.trace), (4, 0.5));
        let line: Vec<Point> = (0..40).map(|i| Point::lit(i as f32 / 40.0, 0.1, 1.0, 1.0, 1.0)).collect();
        let out = m.apply(line, 1.0);
        let runs = (0..out.len()).filter(|&i| out[i].is_lit() && (i == 0 || !out[i - 1].is_lit())).count();
        assert_eq!(runs, 2, "half of four copies stays lit");
    }

    #[test]
    fn length_comes_from_the_program_then_the_evolving_content() {
        let evolving = Content::Evolving(evolving::EvolvingCue { length_beats: 32.0, ..Default::default() });
        assert_eq!(CueProgram::default().length(&evolving), 32.0);
        assert_eq!(CueProgram::default().length(&Content::Wave), 16.0);
        assert_eq!(CueProgram { length_beats: Some(8.0), ..Default::default() }.length(&evolving), 8.0);
    }

    #[test]
    fn a_program_without_curves_or_lfos_moves_nothing() {
        assert_eq!(CueProgram { layers: vec![Content::Wave], ..Default::default() }.modifiers(3.0, 16.0, 120.0), None);
    }

    #[test]
    fn json_is_small_and_defaults_fill_in() {
        let p: CueProgram = serde_json::from_str(r#"{"play":"ping_pong","lanes":[{"target":"size","keys":[{"beat":4,"value":0.5}]}]}"#).unwrap();
        assert_eq!(p.play, PlayMode::PingPong);
        assert_eq!(p.lanes[0].keys[0].ease, KeyEase::Linear);
        assert_eq!(p.length_beats, None);
        let back: CueProgram = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }
}
