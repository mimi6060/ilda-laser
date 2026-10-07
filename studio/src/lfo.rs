//! Tempo-synced LFO modulators: a waveform that moves any continuous
//! control (size that breathes, rotation that swings, hue that turns)
//! around the value the operator set.
//!
//! The phase is a pure function of the tempo clock (`Rate::cycles`, i.e.
//! `beat_at(t) / period`), never accumulated frame by frame, so two
//! modulators with the same period stay in phase forever and a BPM change
//! alters the speed without a jump.
//!
//! Modulation is applied each frame to the engine's *copies* of the look
//! and the live modifiers: the stored values (what the faders, MIDI and
//! saved scenes show) are the base and are never written.

use crate::controls::{ControlKind, ControlRegistry};
use crate::engine::Settings;
use crate::live::{LiveModifiers, Rate};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Master-level modulators allowed at once.
pub const MAX_MODULATORS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wave {
    #[default]
    Sine,
    Triangle,
    Square,
    SawUp,
    SawDown,
    /// Sample and hold: a new value each cycle, from a stable seed.
    Random,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Modulator {
    /// Control id (controls.rs) of a continuous control, see `modulatable`.
    pub target: String,
    pub wave: Wave,
    /// `Beats(n)`: one cycle every n beats of the tempo clock; `Hz`: free.
    pub rate: Rate,
    /// 0..1: 1 swings over the control's whole half-range each way.
    pub depth: f32,
    /// 0..1 of a cycle.
    pub phase: f32,
    /// -1..1, added to the wave before `depth` (-1 = only below the base).
    pub offset: f32,
    pub enabled: bool,
    /// Speed group (T-419, speed.rs) whose beats a `Beats` rate counts:
    /// 0 = the master clock, 1..=4 = A..D. 0 in older files.
    pub speed_group: u8,
    /// A part of a figure cue (T-441) instead of `target`: the modulator
    /// moves that part's parameter only. `None` in older files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<PartTarget>,
    /// *Largeur* (T-421): the part of each cycle the modulator is active,
    /// 0..=1; outside it the wave rests at -1 (before depth). 1 in older
    /// files: the wave as before.
    pub width: f32,
    /// *Attaque* / *Déclin*: the parts of the active part spent rising
    /// from rest and falling back to it, 0..=1 each, at most 1 together.
    /// 0 in older files.
    pub attack: f32,
    pub decay: f32,
    /// *Étalement* (T-421, spread.rs): the phase shifted from zone to zone
    /// of a group, on a master control (applied per zone with the master
    /// stage). `None` in older files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spread: Option<crate::spread::Spread>,
}

/// What a part modulator moves (T-441): one parameter of one part of a
/// figure cue. A cue or a part that no longer exists (the figure deleted
/// or imported again without it) is kept, and does nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartTarget {
    /// The figure cue's id, `figure:<name>`.
    pub cue: String,
    /// 1..=`figures::MAX_PARTS`.
    pub part: u8,
    pub param: PartParam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartParam {
    Brightness,
    Size,
    PosX,
    PosY,
    AngleZ,
}

impl PartParam {
    /// (min, max) the parameter is moved within, like the master control
    /// of the same name.
    pub fn range(self) -> (f32, f32) {
        match self {
            PartParam::Brightness => (0.0, 1.0),
            PartParam::Size => (0.0, 2.0),
            PartParam::PosX | PartParam::PosY => (-1.0, 1.0),
            PartParam::AngleZ => (-180.0, 180.0),
        }
    }

    fn slot(self, m: &mut LiveModifiers) -> &mut f32 {
        match self {
            PartParam::Brightness => &mut m.brightness,
            PartParam::Size => &mut m.size,
            PartParam::PosX => &mut m.pos_x,
            PartParam::PosY => &mut m.pos_y,
            PartParam::AngleZ => &mut m.rot_angle[2],
        }
    }
}

impl Default for Modulator {
    fn default() -> Self {
        Self {
            target: "master.size".into(),
            wave: Wave::Sine,
            rate: Rate::Beats(4.0),
            depth: 0.5,
            phase: 0.0,
            offset: 0.0,
            enabled: true,
            speed_group: 0,
            part: None,
            width: 1.0,
            attack: 0.0,
            decay: 0.0,
            spread: None,
        }
    }
}

impl Modulator {
    /// (cycle index, position in the cycle 0..1) at `t` seconds, `beat` beats.
    pub fn position(&self, t: f64, beat: f64) -> (i64, f64) {
        let x = self.rate.cycles(t, beat) + self.phase as f64;
        let cycle = x.floor();
        (cycle as i64, x - cycle)
    }

    /// The wave, -1..1, at `t` seconds / `beat` beats.
    pub fn wave_at(&self, t: f64, beat: f64) -> f32 {
        self.wave_shifted(t, beat, 0.0)
    }

    /// The wave with its phase moved by `shift` cycles (a zone of a spread,
    /// T-421). The whole wave plays within the first `width` of the cycle,
    /// shaped by the attack and decay ramps; the rest of the cycle rests at
    /// -1. Width 1 without ramps: exactly the plain wave.
    pub fn wave_shifted(&self, t: f64, beat: f64, shift: f64) -> f32 {
        let (cycle, p) = self.position(t, beat);
        let p = if shift.is_finite() && shift != 0.0 { (p + shift).rem_euclid(1.0) } else { p };
        let (width, attack, decay) = self.envelope_parts();
        if width >= 1.0 && attack <= 0.0 && decay <= 0.0 {
            return match self.wave {
                Wave::Random => random(cycle, &self.target),
                wave => shape(wave, p) as f32,
            };
        }
        if p >= width as f64 {
            return -1.0;
        }
        let q = p / width as f64;
        let w = match self.wave {
            Wave::Random => random(cycle, &self.target) as f64,
            wave => shape(wave, q),
        };
        let env = if attack > 0.0 && q < attack as f64 {
            q / attack as f64
        } else if decay > 0.0 && q > 1.0 - decay as f64 {
            (1.0 - q) / decay as f64
        } else {
            1.0
        };
        (-1.0 + (w + 1.0) * env.clamp(0.0, 1.0)) as f32
    }

    /// Whether the wave is in the active part of its cycle (*Largeur*).
    #[cfg(test)]
    pub fn active_at(&self, t: f64, beat: f64) -> bool {
        (self.position(t, beat).1) < self.envelope_parts().0 as f64
    }

    /// (width, attack, decay) as the wave uses them: within range whatever
    /// the struct holds.
    fn envelope_parts(&self) -> (f32, f32, f32) {
        let width = if self.width.is_finite() { self.width.clamp(0.0, 1.0) } else { 1.0 };
        let attack = if self.attack.is_finite() { self.attack.clamp(0.0, 1.0) } else { 0.0 };
        let decay = if self.decay.is_finite() { self.decay.clamp(0.0, 1.0) } else { 0.0 };
        let sum = attack + decay;
        if sum > 1.0 {
            (width, attack / sum, decay / sum)
        } else {
            (width, attack, decay)
        }
    }

    /// Spreads per zone (T-421): a spread naming a group, on a master
    /// control (a look parameter is drawn once for every zone).
    pub fn spreads(&self) -> bool {
        self.part.is_none() && self.target.starts_with("master.") && self.spread.as_ref().is_some_and(crate::spread::Spread::is_set)
    }

    /// Clamp every number to its range (NaN and infinities → defaults).
    pub(crate) fn sanitized(mut self) -> Self {
        let d = Self::default();
        let fin = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        self.depth = fin(self.depth, d.depth).clamp(0.0, 1.0);
        self.phase = fin(self.phase, 0.0).rem_euclid(1.0);
        self.offset = fin(self.offset, 0.0).clamp(-1.0, 1.0);
        if crate::speed::check_group(self.speed_group).is_err() {
            self.speed_group = 0;
        }
        (self.width, self.attack, self.decay) = self.envelope_parts();
        // A phase spread only: a modulator moves its phase, nothing else.
        self.spread = self.spread.take().map(|s| crate::spread::Spread { param: crate::spread::SpreadParam::Phase, ..s }.sanitized()).filter(crate::spread::Spread::is_set);
        self.rate = match self.rate {
            Rate::Beats(b) => Rate::Beats(fin(b, 4.0).clamp(1.0 / 16.0, 64.0)),
            Rate::Hz(hz) => Rate::Hz(fin(hz, 1.0).clamp(0.0, 20.0)),
        };
        // The cue id of a part target, bounded (a part out of range or an
        // unknown cue is kept, inactive: `modulate_parts`).
        if let Some(p) = &mut self.part {
            p.cue = p.cue.trim().chars().take(256).collect();
        }
        self
    }

    /// Whether this modulator can move something: a master control in the
    /// allow-list, or a part of a figure cue (an unknown one is accepted,
    /// inactive).
    fn valid(&self, reg: &ControlRegistry) -> bool {
        match &self.part {
            Some(p) => p.cue.starts_with(crate::figures::ID_PREFIX) && (1..=crate::figures::MAX_PARTS).contains(&p.part),
            None => modulatable(reg, &self.target).is_some(),
        }
    }
}

/// Periodic shapes at position `p` (0..1), -1..1. Sine and triangle start
/// at 0 going up, so every shape is in step at the same phase.
pub(crate) fn shape(wave: Wave, p: f64) -> f64 {
    match wave {
        Wave::Sine => (std::f64::consts::TAU * p).sin(),
        Wave::Triangle if p < 0.25 => 4.0 * p,
        Wave::Triangle if p < 0.75 => 2.0 - 4.0 * p,
        Wave::Triangle => 4.0 * p - 4.0,
        Wave::Square if p < 0.5 => 1.0,
        Wave::Square => -1.0,
        Wave::SawUp => 2.0 * p - 1.0,
        Wave::SawDown => 1.0 - 2.0 * p,
        Wave::Random => 0.0,
    }
}

/// Sample-and-hold value for `cycle`, -1..1: a hash of the cycle index and
/// the target (so the same modulator gives the same sequence every run).
fn random(cycle: i64, target: &str) -> f32 {
    let seed = target.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    // splitmix64
    let mut z = (cycle as u64 ^ seed).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    ((z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
}

/// One modulatable value, whatever its storage type.
enum Slot<'a> {
    F32(&'a mut f32),
    /// 0..255 stored, 0..1 as a control.
    Byte(&'a mut u8),
    /// A whole step (palette offset).
    Step(&'a mut usize),
}

impl Slot<'_> {
    fn get(&self) -> f32 {
        match self {
            Slot::F32(v) => **v,
            Slot::Byte(v) => **v as f32 / 255.0,
            Slot::Step(v) => **v as f32,
        }
    }

    fn set(self, value: f32) {
        match self {
            Slot::F32(v) => *v = value,
            Slot::Byte(v) => *v = (value * 255.0).round().clamp(0.0, 255.0) as u8,
            Slot::Step(v) => *v = value.round().max(0.0) as usize,
        }
    }
}

/// Where each modulatable control lives. This is also the allow-list:
/// transport, tempo, cue and grid controls, and anything calibration or
/// safety related, have no slot and can't be modulated.
fn slot<'a>(id: &str, settings: &'a mut Settings, live: &'a mut LiveModifiers) -> Option<Slot<'a>> {
    let c = &mut live.color_params;
    Some(match id {
        "look.size" => Slot::F32(&mut settings.scale),
        "look.brightness" => Slot::F32(&mut settings.brightness),
        "look.rotation_speed" => Slot::F32(&mut settings.rotation_speed),
        "audio.size" => Slot::F32(&mut settings.audio.size),
        "audio.rotate" => Slot::F32(&mut settings.audio.rotate),
        "audio.flash" => Slot::F32(&mut settings.audio.flash),
        "master.brightness" => Slot::F32(&mut live.brightness),
        "master.size" => Slot::F32(&mut live.size),
        "master.size_x" => Slot::F32(&mut live.size_x),
        "master.size_y" => Slot::F32(&mut live.size_y),
        "master.pos_x" => Slot::F32(&mut live.pos_x),
        "master.pos_y" => Slot::F32(&mut live.pos_y),
        "master.rot_x.angle" => Slot::F32(&mut live.rot_angle[0]),
        "master.rot_y.angle" => Slot::F32(&mut live.rot_angle[1]),
        "master.rot_z.angle" => Slot::F32(&mut live.rot_angle[2]),
        "master.rot_x.speed" => Slot::F32(&mut live.rot_speed[0]),
        "master.rot_y.speed" => Slot::F32(&mut live.rot_speed[1]),
        "master.rot_z.speed" => Slot::F32(&mut live.rot_speed[2]),
        "master.perspective" => Slot::F32(&mut live.perspective),
        "master.speed" => Slot::F32(&mut live.speed),
        // Live effects that only remove light (T-142). The strobe, mirror,
        // prism and the momentary ones are left to the operator's hand.
        "master.trace" => Slot::F32(&mut live.trace),
        "master.dots" => Slot::F32(&mut live.dots),
        "master.color.hue" => Slot::F32(&mut c.hue),
        "master.color.spread" => Slot::F32(&mut c.spread),
        "master.color.offset" => Slot::Step(&mut c.offset),
        "master.color.red" => Slot::Byte(&mut c.rgb[0]),
        "master.color.green" => Slot::Byte(&mut c.rgb[1]),
        "master.color.blue" => Slot::Byte(&mut c.rgb[2]),
        _ => return None,
    })
}

/// The current values of the master controls `ids` (`master.*` ids of the
/// allow-list), read from the live modifiers; `None` for any other id.
/// For the live take (take.rs, T-169), which reads, never writes.
pub fn master_values(ids: &[&str], live: &LiveModifiers) -> Vec<Option<f32>> {
    let (mut settings, mut live) = (Settings::default(), live.clone());
    ids.iter().map(|id| if id.starts_with("master.") { slot(id, &mut settings, &mut live).map(|s| s.get()) } else { None }).collect()
}

/// Brightness controls: the fader is a ceiling, a modulator can only dim.
fn is_brightness(id: &str) -> bool {
    matches!(id, "look.brightness" | "master.brightness")
}

/// (min, max) of a control a modulator may drive: continuous, allowed from
/// outside (never `transport.arm` & co) and in the allow-list above.
pub fn modulatable(reg: &ControlRegistry, id: &str) -> Option<(f32, f32)> {
    let desc = reg.get(id)?;
    let ControlKind::Continuous { min, max, .. } = desc.kind else { return None };
    let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
    (desc.external && desc.id == id && slot(id, &mut settings, &mut live).is_some()).then_some((min, max))
}

/// Move the targets of the enabled modulators on the engine's copies of
/// the look and the live modifiers. Several modulators on one control add
/// up. Value = base + depth × (wave + offset) × (max - min) / 2, clamped.
/// (The engine calls `modulate_scaled` with the crossfader's share.)
#[cfg(test)]
pub fn modulate(mods: &[Modulator], reg: &ControlRegistry, settings: &mut Settings, live: &mut LiveModifiers, t: f64, beat: f64) {
    modulate_scaled(mods, reg, settings, live, t, beat, 1.0);
}

/// `modulate` with every depth scaled by `share` (0..1): the time side of
/// the *Temps ↔ Audio* crossfader (audio/routes.rs). Every modulator on
/// `beat` (the engine uses `modulate_grouped`).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn modulate_scaled(mods: &[Modulator], reg: &ControlRegistry, settings: &mut Settings, live: &mut LiveModifiers, t: f64, beat: f64, share: f32) {
    modulate_grouped(mods, reg, settings, live, t, &|_| beat, share);
}

/// `modulate_scaled` with each modulator on its speed group's beat
/// (T-419): `beat_of(group)` is that group's beat (0 = the master's).
#[allow(clippy::too_many_arguments)]
pub fn modulate_grouped(mods: &[Modulator], reg: &ControlRegistry, settings: &mut Settings, live: &mut LiveModifiers, t: f64, beat_of: &dyn Fn(u8) -> f64, share: f32) {
    modulate_shifted(mods, reg, settings, live, t, beat_of, share, &|_| 0.0);
}

/// Splits the modulators for the engine (T-421): (those that spread per
/// zone over a group of `cfg` with at least two zones, the others). The
/// first are applied by `modulate_zone` on each zone's copy of the master
/// modifiers, the others by `modulate_grouped` once for every zone.
pub fn split_zoned(mods: &[Modulator], cfg: &crate::projection::ProjectionConfig) -> (Vec<Modulator>, Vec<Modulator>) {
    mods.iter().cloned().partition(|m| m.spreads() && m.spread.as_ref().is_some_and(|s| !s.zones(cfg).is_empty()))
}

/// The zoned modulators (`split_zoned`) on the master modifiers of `zone`:
/// each at its phase plus the zone's spread value (degrees → cycles); a
/// zone outside a modulator's group gets its plain phase. Same rules as
/// `modulate_grouped` (clamped, brightness only dims).
#[allow(clippy::too_many_arguments)]
pub fn modulate_zone(mods: &[Modulator], zone: u8, cfg: &crate::projection::ProjectionConfig, reg: &ControlRegistry, live: &mut LiveModifiers, t: f64, beat_of: &dyn Fn(u8) -> f64, share: f32) {
    let shift = |m: &Modulator| {
        let Some(s) = &m.spread else { return 0.0 };
        s.zones(cfg).iter().find(|(id, _)| *id == zone).map_or(0.0, |(_, deg)| *deg as f64 / 360.0)
    };
    // Master targets only: the look's copy is not drawn per zone.
    modulate_shifted(mods, reg, &mut Settings::default(), live, t, beat_of, share, &shift);
}

#[allow(clippy::too_many_arguments)]
fn modulate_shifted(mods: &[Modulator], reg: &ControlRegistry, settings: &mut Settings, live: &mut LiveModifiers, t: f64, beat_of: &dyn Fn(u8) -> f64, share: f32, shift: &dyn Fn(&Modulator) -> f64) {
    if share <= 0.0 {
        return;
    }
    let mut sums: Vec<(&str, f32)> = Vec::new();
    // Part modulators (T-441) move their part's modifiers, not a control.
    for m in mods.iter().filter(|m| m.enabled && m.depth > 0.0 && m.part.is_none()) {
        let amount = share.min(1.0) * m.depth * (m.wave_shifted(t, beat_of(m.speed_group), shift(m)) + m.offset);
        match sums.iter_mut().find(|(id, _)| *id == m.target) {
            Some((_, sum)) => *sum += amount,
            None => sums.push((&m.target, amount)),
        }
    }
    let mut recolor = false;
    for (id, sum) in sums {
        let Some(range) = modulatable(reg, id) else { continue };
        recolor |= offset(id, range, sum * (range.1 - range.0) / 2.0, settings, live);
    }
    if recolor {
        recolor_live(live);
    }
}

/// Moves one allowed control (`range` = what `modulatable` returned for
/// it) by `delta`, in the control's units, on the engine's copies: value =
/// base + delta, clamped to the range, and never above the fader on a
/// brightness control. The one place every modulation source (LFOs, shaped
/// audio signals: audio/shape.rs) goes through, so they all obey the same
/// rules. Allocates nothing; an id outside the allow-list does nothing.
/// Returns true for a colour target: call `recolor_live` once after the
/// last one.
pub fn offset(id: &str, (min, max): (f32, f32), delta: f32, settings: &mut Settings, live: &mut LiveModifiers) -> bool {
    let Some(slot) = slot(id, settings, live) else { return false };
    if !delta.is_finite() {
        return false;
    }
    let base = slot.get();
    let mut value = (base + delta).clamp(min, max);
    if is_brightness(id) {
        value = value.min(base);
    }
    slot.set(value);
    id.starts_with("master.color.")
}

/// Sets one allowed control from a timeline envelope (T-163) on the
/// engine's copies: `value` replaces the base (`relative` false) or
/// multiplies it (`relative` true), clamped to `range` (what `modulatable`
/// returned) and, like every modulation source, never above the base on a
/// brightness control: an envelope only dims. An id outside the allow-list
/// or a non-finite value does nothing. Returns true for a colour target:
/// call `recolor_live` once after the last one.
pub fn envelope(id: &str, (min, max): (f32, f32), relative: bool, value: f32, settings: &mut Settings, live: &mut LiveModifiers) -> bool {
    let Some(slot) = slot(id, settings, live) else { return false };
    if !value.is_finite() {
        return false;
    }
    let base = slot.get();
    let mut v = if relative { base * value } else { value };
    if !v.is_finite() {
        return false;
    }
    v = v.clamp(min, max);
    if is_brightness(id) {
        v = v.min(base);
    }
    slot.set(v);
    id.starts_with("master.color.")
}

/// Per-part modifiers of figure cues (T-441): cue id → part → modifiers.
pub type PartMods = std::collections::HashMap<String, std::collections::BTreeMap<u8, LiveModifiers>>;

/// The part modulators (T-441) on `parts` (each cue's stored per-part
/// modifiers, copies): value = base + depth × (wave + offset) × half the
/// parameter's range, clamped, several on one parameter adding up; a
/// brightness only dims. `share` and `beat_of` as for `modulate_grouped`.
/// A modulator whose cue is not in `is_figure` (no such figure cue) does
/// nothing; one on a part the figure doesn't use moves nothing visible.
pub fn modulate_parts(mods: &[Modulator], parts: &mut PartMods, is_figure: &dyn Fn(&str) -> bool, t: f64, beat_of: &dyn Fn(u8) -> f64, share: f32) {
    if share <= 0.0 {
        return;
    }
    let mut sums: Vec<(&PartTarget, f32)> = Vec::new();
    for m in mods.iter().filter(|m| m.enabled && m.depth > 0.0) {
        let Some(p) = &m.part else { continue };
        if !(1..=crate::figures::MAX_PARTS).contains(&p.part) || !is_figure(&p.cue) {
            continue;
        }
        let amount = share.min(1.0) * m.depth * (m.wave_at(t, beat_of(m.speed_group)) + m.offset);
        match sums.iter_mut().find(|(q, _)| q.cue == p.cue && q.part == p.part && q.param == p.param) {
            Some((_, sum)) => *sum += amount,
            None => sums.push((p, amount)),
        }
    }
    for (p, sum) in sums {
        let (min, max) = p.param.range();
        let delta = sum * (max - min) / 2.0;
        if !delta.is_finite() {
            continue;
        }
        let m = parts.entry(p.cue.clone()).or_default().entry(p.part).or_default();
        let slot = p.param.slot(m);
        let base = if slot.is_finite() { *slot } else { 0.0 };
        let mut value = (base + delta).clamp(min, max);
        if p.param == PartParam::Brightness {
            value = value.min(base.clamp(0.0, 1.0));
        }
        *slot = value;
    }
}

/// Rebuilds the active colour override from the (modulated) colour params.
pub fn recolor_live(live: &mut LiveModifiers) {
    live.color = live.color_params.build(live.color.mode_index());
}

/// At most `MAX_MODULATORS`, each on a control that can be modulated.
pub fn validate(list: &[Modulator], reg: &ControlRegistry) -> Result<()> {
    if list.len() > MAX_MODULATORS {
        bail!("{MAX_MODULATORS} modulateurs au maximum");
    }
    if let Some(m) = list.iter().find(|m| !m.valid(reg)) {
        match &m.part {
            Some(p) => bail!("partie {} de « {} » : cible de modulateur invalide", p.part, p.cue),
            None => bail!("« {} » ne peut pas être modulé", m.target),
        }
    }
    Ok(())
}

/// The master modulators, saved to `lfos.json` on every change.
pub struct LfoStore {
    path: PathBuf,
    list: Vec<Modulator>,
}

impl LfoStore {
    /// A missing or damaged file (quarantined, T-402) starts empty; entries that aren't valid
    /// any more (unknown target) are dropped.
    pub fn load_or_create(path: PathBuf, reg: &ControlRegistry) -> Self {
        let list: Vec<Modulator> = crate::load_json(&path);
        let list = list
            .into_iter()
            .filter(|m| m.valid(reg))
            .take(MAX_MODULATORS)
            .map(Modulator::sanitized)
            .collect();
        Self { path, list }
    }

    pub fn list(&self) -> &[Modulator] {
        &self.list
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Replaces every modulator in memory only (checked with `validate`,
    /// numbers clamped); the caller saves the file (project open, T-286).
    pub fn replace_in_memory(&mut self, list: Vec<Modulator>) {
        self.list = list.into_iter().map(Modulator::sanitized).collect();
    }

    /// Replaces every modulator (numbers clamped to their ranges) and saves.
    pub fn set(&mut self, list: Vec<Modulator>, reg: &ControlRegistry) -> Result<()> {
        validate(&list, reg)?;
        let list: Vec<Modulator> = list.into_iter().map(Modulator::sanitized).collect();
        let json = serde_json::to_string_pretty(&list).context("failed to serialize modulators")?;
        crate::datafile::replace(&self.path, json.as_bytes()).with_context(|| format!("failed to write {}", self.path.display()))?;
        self.list = list;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets;
    use crate::tempo::TempoClock;

    fn reg() -> ControlRegistry {
        ControlRegistry::build(&presets::catalog())
    }

    fn lfo(target: &str, wave: Wave, rate: Rate) -> Modulator {
        Modulator { target: target.into(), wave, rate, ..Default::default() }
    }

    /// master.size after modulation at `t` seconds on `clock`.
    fn size_at(mods: &[Modulator], clock: &TempoClock, t: f64) -> f32 {
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        modulate(mods, &reg(), &mut settings, &mut live, t, clock.beat_at(t));
        live.size
    }

    fn part_lfo(cue: &str, part: u8, param: PartParam) -> Modulator {
        Modulator { wave: Wave::Square, rate: Rate::Beats(1.0), depth: 0.5, part: Some(PartTarget { cue: cue.into(), part, param }), ..Default::default() }
    }

    #[test]
    fn part_modulators_move_their_part_only() {
        let figure = |c: &str| c == "figure:Visage";
        let mods = [part_lfo("figure:Visage", 2, PartParam::Brightness), part_lfo("figure:Visage", 1, PartParam::AngleZ)];
        let mut parts = PartMods::new();
        // Square at +1 in the first half of the beat: brightness can't go up.
        modulate_parts(&mods, &mut parts, &figure, 0.0, &|_| 0.1, 1.0);
        assert_eq!(parts["figure:Visage"][&2].brightness, 1.0, "a brightness only dims");
        assert_eq!(parts["figure:Visage"][&1].rot_angle[2], 90.0, "half the -180..180 range × 0.5");
        parts.clear();
        modulate_parts(&mods, &mut parts, &figure, 0.0, &|_| 0.6, 1.0);
        assert_eq!(parts["figure:Visage"][&2].brightness, 0.75, "-1 × 0.5 × half of 0..1");
        assert_eq!(parts["figure:Visage"][&2].size, 1.0, "nothing else moves");
        // A stored base is moved, not replaced; the crossfader scales it.
        let mut parts = PartMods::from([("figure:Visage".to_string(), std::collections::BTreeMap::from([(2, LiveModifiers { brightness: 0.8, ..Default::default() })]))]);
        modulate_parts(&mods, &mut parts, &figure, 0.0, &|_| 0.6, 0.5);
        assert!((parts["figure:Visage"][&2].brightness - 0.675).abs() < 1e-6);
        // The master ignores them.
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        modulate(&mods, &reg(), &mut settings, &mut live, 0.0, 0.6);
        assert_eq!(live, LiveModifiers::default());
    }

    #[test]
    fn a_part_modulator_on_a_missing_figure_or_part_is_inactive_without_error() {
        let mut parts = PartMods::new();
        let gone = [part_lfo("figure:Disparue", 1, PartParam::Size), Modulator { part: Some(PartTarget { cue: "figure:Visage".into(), part: 12, param: PartParam::Size }), ..part_lfo("", 1, PartParam::Size) }];
        modulate_parts(&gone, &mut parts, &|c| c == "figure:Visage", 0.0, &|_| 0.1, 1.0);
        assert!(parts.is_empty());
        // Accepted and kept when saved and reloaded (the figure may come back).
        validate(&gone[..1], &reg()).unwrap();
        assert!(validate(&gone[1..], &reg()).is_err(), "part out of range");
        assert!(validate(&[part_lfo("tunnels-001", 1, PartParam::Size)], &reg()).is_err(), "not a figure cue");
        let dir = std::env::temp_dir().join(format!("laser-studio-lfo-parts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lfos.json");
        std::fs::write(&path, serde_json::to_string(&gone[..1]).unwrap()).unwrap();
        assert_eq!(LfoStore::load_or_create(path.clone(), &reg()).list(), &gone[..1]);
        // Older files: no part.
        std::fs::write(&path, r#"[{"target":"master.size"}]"#).unwrap();
        assert_eq!(LfoStore::load_or_create(path, &reg()).list()[0].part, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn waves_at_known_phases() {
        let at = |wave, p: f32| Modulator { wave, phase: p, rate: Rate::Hz(0.0), ..Default::default() }.wave_at(0.0, 0.0);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        for (wave, expected) in [
            (Wave::Sine, [0.0, 1.0, 0.0, -1.0]),
            (Wave::Triangle, [0.0, 1.0, 0.0, -1.0]),
            (Wave::Square, [1.0, 1.0, -1.0, -1.0]),
            (Wave::SawUp, [-1.0, -0.5, 0.0, 0.5]),
            (Wave::SawDown, [1.0, 0.5, 0.0, -0.5]),
        ] {
            for (i, e) in expected.iter().enumerate() {
                let v = at(wave, i as f32 * 0.25);
                assert!(close(v, *e), "{wave:?} at {}: {v} != {e}", i as f32 * 0.25);
            }
        }
        assert!(close(at(Wave::Triangle, 0.125), 0.5));
    }

    #[test]
    fn random_holds_within_a_cycle_and_is_stable() {
        let m = lfo("master.size", Wave::Random, Rate::Beats(1.0));
        let values: Vec<f32> = (0..64).map(|beat| m.wave_at(0.0, beat as f64 + 0.1)).collect();
        assert!(values.iter().all(|v| (-1.0..=1.0).contains(v)));
        assert_eq!(m.wave_at(0.0, 3.1), m.wave_at(0.0, 3.9), "held for the whole cycle");
        assert_eq!(values, (0..64).map(|beat| m.clone().wave_at(0.0, beat as f64 + 0.1)).collect::<Vec<_>>());
        let distinct = values.iter().filter(|v| (**v - values[0]).abs() > 1e-3).count();
        assert!(distinct > 50, "a new value each cycle");
        let mean = values.iter().sum::<f32>() / 64.0;
        assert!(mean.abs() < 0.3, "spread around 0: {mean}");
    }

    #[test]
    fn sine_on_size_repeats_every_four_beats() {
        let clock = TempoClock::default(); // 120 BPM: 4 beats = 2 s
        let mods = [lfo("master.size", Wave::Sine, Rate::Beats(4.0))];
        for i in 0..50 {
            let t = 0.37 + i as f64 * 0.113;
            let (a, b, c) = (size_at(&mods, &clock, t), size_at(&mods, &clock, t + 2.0), size_at(&mods, &clock, t + 200.0));
            assert!((a - b).abs() < 1e-4 && (a - c).abs() < 1e-4, "{a} {b} {c}");
        }
        // Base 1, range 0..2, depth 0.5: peak 1.5 at beat 1, trough 0.5 at beat 3.
        assert!((size_at(&mods, &clock, 0.5) - 1.5).abs() < 1e-4);
        assert!((size_at(&mods, &clock, 1.5) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn modulators_added_ten_seconds_apart_are_in_phase() {
        // Nothing is accumulated: a modulator created later reads the same
        // clock and lands on the same value.
        let mut clock = TempoClock::default();
        clock.set_bpm(128.0, 3.0);
        let first = lfo("master.size", Wave::Triangle, Rate::Beats(2.0));
        let later = first.clone();
        for i in 0..100 {
            let t = 10.0 + i as f64 * 0.0167;
            assert_eq!(first.wave_at(t, clock.beat_at(t)), later.wave_at(t, clock.beat_at(t)));
        }
    }

    #[test]
    fn bpm_change_changes_speed_without_a_jump() {
        let mut clock = TempoClock::default();
        let mods = [lfo("master.size", Wave::Sine, Rate::Beats(4.0))];
        let t = 7.3;
        let before = size_at(&mods, &clock, t);
        clock.set_bpm(174.0, t);
        assert!((size_at(&mods, &clock, t) - before).abs() < 1e-4, "no jump at the change");
        // Faster: at 174 BPM a 4-beat cycle lasts 60/174*4 s.
        let period = 4.0 * 60.0 / 174.0;
        assert!((size_at(&mods, &clock, t + period) - before).abs() < 1e-4);
    }

    #[test]
    fn zero_depth_or_disabled_leaves_the_control_alone() {
        let clock = TempoClock::default();
        let zero = Modulator { depth: 0.0, offset: 0.7, ..Default::default() };
        let off = Modulator { enabled: false, ..Default::default() };
        for i in 0..20 {
            let t = i as f64 * 0.21;
            assert_eq!(size_at(std::slice::from_ref(&zero), &clock, t), 1.0);
            assert_eq!(size_at(std::slice::from_ref(&off), &clock, t), 1.0);
        }
    }

    #[test]
    fn values_are_clamped_and_modulators_add_up() {
        let clock = TempoClock::default();
        let full = Modulator { wave: Wave::Square, depth: 1.0, rate: Rate::Beats(4.0), ..Default::default() };
        // Square high: base 1 + 1 × (2 - 0) / 2 = 2 = max; two of them still clamp to 2.
        assert_eq!(size_at(&[full.clone(), full.clone()], &clock, 0.1), 2.0);
        assert_eq!(size_at(&[full.clone(), full], &clock, 1.1), 0.0);
        let half = Modulator { wave: Wave::Square, depth: 0.25, ..Default::default() };
        assert!((size_at(&[half.clone(), half], &clock, 0.1) - 1.5).abs() < 1e-6);
        // Offset -1 with depth 0.5: only ever below the base.
        let below = Modulator { offset: -1.0, ..Default::default() };
        for i in 0..40 {
            assert!(size_at(std::slice::from_ref(&below), &clock, i as f64 * 0.05) <= 1.0 + 1e-6);
        }
    }

    #[test]
    fn the_time_share_scales_every_modulator() {
        let reg = reg();
        let m = Modulator { wave: Wave::Square, depth: 0.5, ..Default::default() };
        let size = |share: f32| {
            let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
            modulate_scaled(std::slice::from_ref(&m), &reg, &mut settings, &mut live, 0.0, 0.1, share);
            live.size
        };
        // Square high, depth 0.5, range 0..2: +0.5 in full, +0.25 at half, nothing at 0.
        assert_eq!((size(1.0), size(0.5), size(0.0)), (1.5, 1.25, 1.0));
        assert_eq!(size(3.0), 1.5, "never more than in full");
    }

    #[test]
    fn brightness_can_only_dim() {
        let reg = reg();
        let m = Modulator { target: "master.brightness".into(), depth: 1.0, ..Default::default() };
        let mut max_seen: f32 = 0.0;
        let mut min_seen: f32 = 1.0;
        for i in 0..80 {
            let (mut settings, mut live) = (Settings::default(), LiveModifiers { brightness: 0.6, ..Default::default() });
            modulate(std::slice::from_ref(&m), &reg, &mut settings, &mut live, 0.0, i as f64 * 0.05);
            max_seen = max_seen.max(live.brightness);
            min_seen = min_seen.min(live.brightness);
        }
        assert!(max_seen <= 0.6 + 1e-6, "never above the fader: {max_seen}");
        assert!((min_seen - 0.1).abs() < 1e-4, "dims to 0.6 - 1 × 1 × (1 - 0) / 2: {min_seen}");
    }

    #[test]
    fn only_safe_continuous_controls_can_be_targets() {
        let reg = reg();
        for id in ["transport.arm", "transport.blackout", "tempo.bpm", "cue.max_active", "master.rot.sync", "grid.1.1.1", "nope", "master.strobe.rate", "master.prism", "transport.black_hold"] {
            assert!(modulatable(&reg, id).is_none(), "{id} must not be modulatable");
        }
        for id in ["master.size", "master.color.hue", "look.rotation_speed", "audio.flash", "master.brightness"] {
            assert!(modulatable(&reg, id).is_some(), "{id}");
        }
        // Every allowed target is a real continuous, external registry control
        // and actually moves when modulated.
        let targets: Vec<&str> = reg.list().iter().map(|d| d.id.as_str()).filter(|id| modulatable(&reg, id).is_some()).collect();
        assert_eq!(targets.len(), 28);
        for id in targets {
            // Pushed fully up, then fully down: at least one must move it
            // (a base at the bottom of its range can only go up, brightness only down).
            let moved = [(1.0, 1.0), (3.0, -1.0)].iter().any(|&(beat, offset)| {
                let m = Modulator { target: id.into(), wave: Wave::Square, depth: 1.0, offset, ..Default::default() };
                let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
                live.color_params.rgb = [128, 128, 128];
                live.color_params.offset = 5;
                live.color_params.hue = 180.0;
                let before = (serde_json::to_value(&settings).unwrap(), serde_json::to_value(&live).unwrap());
                modulate(&[m], &reg, &mut settings, &mut live, 0.0, beat);
                before != (serde_json::to_value(&settings).unwrap(), serde_json::to_value(&live).unwrap())
            });
            assert!(moved, "{id} did not move");
        }
    }

    #[test]
    fn colour_targets_update_the_active_override() {
        let reg = reg();
        let mut live = LiveModifiers::default();
        live.color = live.color_params.build(2); // Teinte
        let m = Modulator { target: "master.color.hue".into(), wave: Wave::Square, depth: 0.5, ..Default::default() };
        modulate(&[m], &reg, &mut Settings::default(), &mut live, 0.0, 0.5);
        assert_eq!(live.color, crate::live::ColorOverride::Hue { hue: 90.0 }); // 0 + 0.5 × 1 × 360 / 2
    }

    #[test]
    fn store_validates_sanitizes_and_reloads() {
        let reg = reg();
        let dir = std::env::temp_dir().join(format!("laser-studio-lfo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lfos.json");
        let _ = std::fs::remove_file(&path);
        let mut store = LfoStore::load_or_create(path.clone(), &reg);
        assert!(store.list().is_empty());
        assert!(store.set(vec![lfo("transport.arm", Wave::Square, Rate::Hz(1.0))], &reg).is_err());
        assert!(store.set(vec![Modulator::default(); MAX_MODULATORS + 1], &reg).is_err());
        let wild = Modulator { depth: 3.0, phase: 1.25, offset: f32::NAN, rate: Rate::Hz(500.0), ..Default::default() };
        store.set(vec![wild, lfo("master.color.hue", Wave::SawUp, Rate::Beats(0.25))], &reg).unwrap();
        let first = &store.list()[0];
        assert_eq!((first.depth, first.phase, first.offset, first.rate), (1.0, 0.25, 0.0, Rate::Hz(20.0)));
        let reloaded = LfoStore::load_or_create(path, &reg);
        assert_eq!(reloaded.list(), store.list());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn serde_defaults_and_format() {
        let m: Modulator = serde_json::from_str(r#"{"target":"master.pos_x","rate":{"hz":0.5}}"#).unwrap();
        assert_eq!((m.wave, m.rate, m.depth, m.enabled), (Wave::Sine, Rate::Hz(0.5), 0.5, true));
        let json = serde_json::to_value(Modulator::default()).unwrap();
        assert_eq!(json["rate"], serde_json::json!({ "beats": 4.0 }));
        assert_eq!(json["wave"], "sine");
    }

    #[test]
    fn a_damaged_file_is_quarantined_and_never_overwritten() {
        let reg = ControlRegistry::build(&crate::presets::catalog());
        let (dir, path) = crate::datafile::tests::with_damaged("lfos", "lfos.json");
        let mut store = LfoStore::load_or_create(path, &reg);
        assert!(store.list().is_empty());
        store.set(Vec::new(), &reg).unwrap();
        crate::datafile::tests::check_kept(&dir, "lfos.json", false);
    }
    /// T-419: a beat-synced modulator in a ×2 speed group makes two cycles
    /// where it made one; older files load in the default group.
    #[test]
    fn a_modulator_in_a_double_speed_group_makes_two_cycles() {
        let mut groups = crate::speed::SpeedGroups::default();
        groups.change_on_bar = false;
        groups.set_mode(2, crate::speed::SpeedMode::Multiple { mult: 2.0 }, 0.0, 0.0, 4).unwrap();
        let m = Modulator { rate: Rate::Beats(4.0), ..Default::default() };
        let fast = Modulator { speed_group: 2, ..m.clone() };
        let cycles = |m: &Modulator| m.position(0.0, groups.beat(m.speed_group, 4.0, 0.0)).0;
        assert_eq!((cycles(&m), cycles(&fast)), (1, 2));
        let sq = |g| Modulator { target: "master.size".into(), wave: Wave::Square, rate: Rate::Beats(4.0), depth: 0.5, speed_group: g, ..Default::default() };
        // At master beat 1: the default group is in its first half (+),
        // the ×2 group (beat 2) in its second half (-).
        let base = LiveModifiers::default().size;
        let size = |g| {
            let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
            modulate_grouped(&[sq(g)], &reg(), &mut settings, &mut live, 0.0, &|g| groups.beat(g, 1.0, 0.0), 1.0);
            live.size
        };
        assert!(size(0) > base && size(2) < base);
        let old: Modulator = serde_json::from_str(r#"{"target":"master.size"}"#).unwrap();
        assert_eq!(old.speed_group, 0);
        assert_eq!(Modulator { speed_group: 9, ..old }.sanitized().speed_group, 0);
    }
    /// T-421: *Largeur* 25 %: active a quarter of the cycle, at rest (-1)
    /// the rest of it.
    #[test]
    fn a_quarter_width_is_active_a_quarter_of_the_cycle() {
        let m = Modulator { wave: Wave::Sine, rate: Rate::Beats(1.0), width: 0.25, ..Default::default() };
        let n = 1000;
        let active = (0..n).filter(|i| m.active_at(0.0, *i as f64 / n as f64)).count();
        assert_eq!(active, n / 4);
        let moving = (0..n).filter(|i| m.wave_at(0.0, *i as f64 / n as f64) > -1.0 + 1e-4).count();
        assert!((240..=250).contains(&moving), "{moving}");
        for i in 250..1000 {
            assert_eq!(m.wave_at(0.0, i as f64 / n as f64), -1.0);
        }
        // The whole wave plays within the active part: its peak at 1/16.
        assert!((m.wave_at(0.0, 1.0 / 16.0) - 1.0).abs() < 1e-5);
        // Width 0: always at rest.
        let zero = Modulator { width: 0.0, ..m };
        assert!((0..100).all(|i| zero.wave_at(0.0, i as f64 / 100.0) == -1.0));
    }

    #[test]
    fn attack_and_decay_ramp_up_then_down_continuously() {
        let m = Modulator { wave: Wave::Square, rate: Rate::Beats(1.0), attack: 0.25, decay: 0.25, ..Default::default() };
        let at = |p: f64| m.wave_at(0.0, p);
        assert_eq!(at(0.0), -1.0, "starts from rest");
        assert!((at(0.125) - 0.0).abs() < 1e-5, "half way up the attack");
        assert_eq!(at(0.3), 1.0, "full between the ramps (square high)");
        assert!(at(0.9) < -1.0 + 1e-5, "square low stays at -1");
        // A sine: continuous around the cycle (no jump larger than the
        // steepest slope allows).
        let sine = Modulator { wave: Wave::Sine, attack: 0.3, decay: 0.3, width: 0.5, ..m };
        let n = 2000;
        let values: Vec<f32> = (0..=n).map(|i| sine.wave_at(0.0, i as f64 / n as f64)).collect();
        for w in values.windows(2) {
            assert!((w[1] - w[0]).abs() < 0.03, "{w:?}");
        }
        assert!(values.iter().all(|v| (-1.0..=1.0).contains(v)));
        // Attack + decay over 1 is scaled back.
        let over = Modulator { attack: 0.8, decay: 0.8, ..Default::default() }.sanitized();
        assert!((over.attack + over.decay - 1.0).abs() < 1e-6);
        let wild = Modulator { width: f32::NAN, attack: -1.0, decay: 3.0, ..Default::default() }.sanitized();
        assert_eq!((wild.width, wild.attack, wild.decay), (1.0, 0.0, 1.0));
    }

    #[test]
    fn full_width_without_ramps_is_the_plain_wave() {
        for wave in [Wave::Sine, Wave::Triangle, Wave::Square, Wave::SawUp, Wave::SawDown, Wave::Random] {
            let m = Modulator { wave, rate: Rate::Beats(2.0), phase: 0.3, ..Default::default() };
            for i in 0..200 {
                let beat = i as f64 * 0.037;
                let (cycle, p) = m.position(0.0, beat);
                let plain = match wave {
                    Wave::Random => random(cycle, &m.target),
                    w => shape(w, p) as f32,
                };
                assert_eq!(m.wave_at(0.0, beat), plain);
            }
        }
        // Older files: the new fields at their defaults.
        let old: Modulator = serde_json::from_str(r#"{"target":"master.size","wave":"triangle"}"#).unwrap();
        assert_eq!((old.width, old.attack, old.decay, old.spread.clone()), (1.0, 0.0, 0.0, None));
        assert!(serde_json::to_value(&old).unwrap().get("spread").is_none());
    }

    /// T-421: six zones, phase 0→360°: six copies evenly shifted; two
    /// wings: symmetric around the centre.
    #[test]
    fn a_phase_spread_shifts_each_zone() {
        use crate::projection::{ProjectionConfig, ProjectionZone, ZoneGroup};
        use crate::spread::{Spread, SpreadParam};
        let cfg = ProjectionConfig {
            zones: (1..=7).map(|id| ProjectionZone { id, ..Default::default() }).collect(),
            groups: vec![ZoneGroup { name: "Six".into(), zones: vec![1, 2, 3, 4, 5, 6], master: 1.0 }],
            ..Default::default()
        };
        let spread = Spread { group: "Six".into(), param: SpreadParam::Phase, from: 0.0, to: 360.0, ..Default::default() };
        let m = Modulator { wave: Wave::Sine, rate: Rate::Beats(1.0), depth: 0.5, spread: Some(spread.clone()), ..Default::default() };
        let size = |m: &Modulator, cfg: &ProjectionConfig, zone: u8, beat: f64| {
            let (zoned, global) = split_zoned(std::slice::from_ref(m), cfg);
            let mut live = LiveModifiers::default();
            modulate_grouped(&global, &reg(), &mut Settings::default(), &mut live, 0.0, &|_| beat, 1.0);
            modulate_zone(&zoned, zone, cfg, &reg(), &mut live, 0.0, &|_| beat, 1.0);
            live.size
        };
        // Zone k at beat b reads the wave at b + k/6.
        for k in 0..6u8 {
            let want = 1.0 + 0.5 * (std::f64::consts::TAU * (0.1 + k as f64 / 6.0)).sin() as f32;
            assert!((size(&m, &cfg, k + 1, 0.1) - want).abs() < 1e-4, "zone {}", k + 1);
        }
        // Zone 7 is not in the group: the plain wave.
        assert!((size(&m, &cfg, 7, 0.25) - 1.5).abs() < 1e-4);
        // Two wings: zone 1 = zone 6, 2 = 5, 3 = 4.
        let wings = Modulator { spread: Some(Spread { wings: 2, ..spread.clone() }), ..m.clone() };
        for (a, b) in [(1, 6), (2, 5), (3, 4)] {
            assert!((size(&wings, &cfg, a, 0.13) - size(&wings, &cfg, b, 0.13)).abs() < 1e-5);
        }
        assert!((size(&wings, &cfg, 1, 0.13) - size(&wings, &cfg, 2, 0.13)).abs() > 1e-3);
        // A seventh zone added to the group: the shifts follow, the
        // modulator untouched.
        let mut seven = cfg.clone();
        seven.groups[0].zones.push(7);
        let want = 1.0 + 0.5 * (std::f64::consts::TAU * (0.1 + 1.0 / 7.0)).sin() as f32;
        assert!((size(&m, &seven, 2, 0.1) - want).abs() < 1e-4);
        // A look parameter or a group of one: modulated once, unshifted.
        let look = Modulator { target: "look.size".into(), ..m.clone() };
        assert!(!look.spreads());
        let mut one = cfg.clone();
        one.groups[0].zones = vec![1];
        assert!(split_zoned(std::slice::from_ref(&m), &one).0.is_empty());
        // Sanitised: a phase spread whatever the file says.
        let delay = Modulator { spread: Some(Spread { param: SpreadParam::Delay, ..spread }), ..m }.sanitized();
        assert_eq!(delay.spread.unwrap().param, SpreadParam::Phase);
    }
}
