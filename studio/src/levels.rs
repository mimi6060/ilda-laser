//! Live modifiers per cue and per layer (T-144), and the optional
//! « Lissage » of live moves.
//!
//! Each level has the same `LiveModifiers` as the master (size, position,
//! rotation, colour, strobe and the other effects of T-142; never
//! « Figer », « Noir » nor the scan speed, which stay global). They are
//! applied in a chain, closest to the content first:
//!
//! cue → layer → (mixer, budget) → master → zones → calibration → safety.
//!
//! The FX grid (T-146) is not a level of its own: with *Maître* as its
//! destination its effects move the master copy (with the LFOs and the
//! audio routes); with *Cue sélectionné* they move the newest cue's own
//! stored master (T-405), applied just before that cue's level here. So
//! the full chain of one cue is: its stored master (+ FX on the cue) →
//! its cue level → its layer → master (+ LFOs, FX, audio routes).
//!
//! Each stage is `live::apply`, so sizes multiply (cue 0.5 × master 2 =
//! 1), angles and rotations add, brightness only ever multiplies down
//! (every stage clamps it to 0..1) and the strobe, visible points and
//! dots only remove light. Colour: the lowest level that is not *Normal*
//! wins over the layer, and the master, applied last, can still force
//! its own.
//!
//! - **Layer** modifiers live in the mixer (`Layer::modifiers`, saved
//!   with `layers.json` and the project), and act on everything the
//!   layer draws: cues, cue-list steps and timeline events.
//! - **Cue** modifiers are a working copy per cue on show
//!   (`Shared::cue_mods`, memory only), taken from the cell's stored
//!   ones (`CueSlot::modifiers`) when the cue starts and dropped when it
//!   stops. « Enregistrer dans le cue » copies the working copy (and, on
//!   request, the master composed on top) into the cell: the next start
//!   plays with it. Storing is an edit (history, refused in show mode);
//!   moving the controls is a live move (allowed in show mode).
//!
//! Controls: `layer.<n>.<param>` and `cue.<id>.<param>`, `<param>` being
//! any of `PARAMS` (the master's own ids without `master.`). They go
//! through `/api/control`, MIDI and OSC like the master's.
//!
//! Smoothing (« Lissage », off by default): a mass-spring filter on every
//! continuous value of every level, so a fader jump glides. Brightness
//! never overshoots its target upwards.

use crate::live::{ColorOverride, LiveModifiers, LiveState, Mirror, Palette};
use crate::patterns::Point;
use crate::Shared;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The master params a level may set (control ids without `master.`).
/// `scan` (an output setting) and the transport's « Figer » / « Noir »
/// are master only.
pub const PARAMS: &[&str] = &[
    "brightness",
    "size",
    "size_x",
    "size_y",
    "pos_x",
    "pos_y",
    "rot_x.angle",
    "rot_y.angle",
    "rot_z.angle",
    "rot_x.speed",
    "rot_y.speed",
    "rot_z.speed",
    "rot.preset",
    "rot.sync",
    "rot.reverse",
    "perspective",
    "speed",
    "reset",
    "strobe.on",
    "strobe.hold",
    "strobe.rate",
    "strobe.sync",
    "strobe.duty",
    "trace",
    "dots",
    "mirror",
    "prism",
    "color.mode",
    "color.hue",
    "color.palette",
    "color.palette_mode",
    "color.offset",
    "color.rate",
    "color.rate_hz",
    "color.spread",
    "color.chase_spread",
    "color.red",
    "color.green",
    "color.blue",
];

/// Whether `m` changes nothing (cells and layers without modifiers
/// don't write any).
pub fn is_neutral(m: &LiveModifiers) -> bool {
    *m == LiveModifiers::default()
}

/// The level a control id targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level<'a> {
    /// Layer 1..=4.
    Layer(u8),
    /// A cue (preset id).
    Cue(&'a str),
}

/// `layer.<n>.<param>` / `cue.<id>.<param>` → (level, param). Cue ids may
/// hold dots: the longest known param at the end wins. The layers' own
/// controls (`layer.<n>.dimmer`…) and the grid's `cue.*` are not levels.
pub fn parse(id: &str) -> Option<(Level<'_>, &'static str)> {
    if let Some(rest) = id.strip_prefix("layer.") {
        let (n, param) = rest.split_once('.')?;
        let n: u8 = n.parse().ok().filter(|n| (1..=crate::layers::LAYER_COUNT as u8).contains(n))?;
        let param = PARAMS.iter().find(|p| **p == param)?;
        return Some((Level::Layer(n), param));
    }
    let rest = id.strip_prefix("cue.")?;
    PARAMS
        .iter()
        .filter(|p| rest.len() > p.len() + 1 && rest.ends_with(*p) && rest.as_bytes()[rest.len() - p.len() - 1] == b'.')
        .max_by_key(|p| p.len())
        .map(|p| (Level::Cue(&rest[..rest.len() - p.len() - 1]), *p))
}

/// A level's modifiers fit for the stage: in range (non-finite → neutral),
/// never « Figer » nor « Noir » (global), scan speed neutral. A held
/// strobe flash or reverse stays: the hand is still on it.
pub fn for_stage(m: &LiveModifiers) -> LiveModifiers {
    let mut c = m.clone();
    let (hold, reverse) = (c.strobe.hold, c.rot_reverse);
    crate::timeline::sanitize_modifiers(&mut c);
    c.strobe.hold = hold;
    c.rot_reverse = reverse;
    c.scan = 1.0;
    c
}

/// A level's modifiers fit to be saved (a cell, `layers.json`): also
/// without the momentary ones.
pub fn for_storage(m: &LiveModifiers) -> LiveModifiers {
    let mut c = for_stage(m);
    c.release_momentary();
    c.rot_reverse = false;
    c
}

/// One set equivalent to `inner` then `outer` for the values that compose
/// simply (what « Enregistrer dans le cue » with the master stores):
/// sizes multiplied, positions, angles and rotation speeds added,
/// brightness and animation speed multiplied, colour from `outer` unless
/// *Normal*, effects from `outer` when it has them on.
pub fn compose(inner: &LiveModifiers, outer: &LiveModifiers) -> LiveModifiers {
    let (i, o) = (for_storage(inner), for_storage(outer));
    let mut c = i.clone();
    c.brightness = (i.brightness * o.brightness).clamp(0.0, 1.0);
    c.size = i.size * o.size;
    c.size_x = i.size_x * o.size_x;
    c.size_y = i.size_y * o.size_y;
    c.pos_x = i.pos_x + o.pos_x;
    c.pos_y = i.pos_y + o.pos_y;
    for a in 0..3 {
        c.rot_angle[a] = i.rot_angle[a] + o.rot_angle[a];
    }
    // Speeds in the same unit add up; otherwise the one that turns wins
    // (the inner one if both do: degrees and turns per bar don't add).
    let still = |m: &LiveModifiers| m.rot_speed == [0.0; 3];
    if i.rot_sync == o.rot_sync {
        for a in 0..3 {
            c.rot_speed[a] = i.rot_speed[a] + o.rot_speed[a];
        }
    } else if still(&i) {
        (c.rot_speed, c.rot_sync) = (o.rot_speed, o.rot_sync);
    }
    c.perspective = i.perspective.max(o.perspective);
    c.speed = (i.speed * o.speed).clamp(0.0, 4.0);
    if o.color != ColorOverride::Normal {
        (c.color, c.color_params) = (o.color.clone(), o.color_params.clone());
    }
    if o.strobe.on {
        c.strobe = o.strobe;
    }
    c.trace = i.trace * o.trace;
    c.dots = i.dots.max(o.dots);
    if o.mirror != Mirror::None {
        c.mirror = o.mirror;
    }
    c.prism = i.prism.max(o.prism);
    // Back within the controls' ranges (a size of 2 × 2 is 2).
    crate::timeline::sanitize_modifiers(&mut c);
    c
}

// ---------- cue working copies ----------

/// Whether cue `id` is playing, fading out or waiting for its start.
pub fn on_show(s: &Shared, id: &str) -> bool {
    s.deck.active.iter().any(|a| a.cue == id) || s.deck.ending.iter().any(|e| e.cue.cue == id) || s.deck.pending.iter().any(|p| p.cue == id)
}

/// The modifiers stored in cue `id`'s cell (neutral if none).
pub fn stored(s: &Shared, id: &str) -> LiveModifiers {
    s.deck.slots.get(id).map(|slot| slot.modifiers.clone()).unwrap_or_default()
}

/// The working copy of a cue on show, made from its cell's at the first
/// need. `None` if the cue is not on show: there is nothing to move.
pub fn cue_mods_mut<'a>(s: &'a mut Shared, id: &str) -> Option<&'a mut LiveModifiers> {
    if !on_show(s, id) {
        return None;
    }
    let from_cell = stored(s, id);
    Some(s.cue_mods.entry(id.to_string()).or_insert(from_cell))
}

/// Once a frame: a cue that starts takes its cell's modifiers, a cue that
/// stopped forgets its working copy (the next start is the stored one).
pub fn frame(s: &mut Shared) {
    let shown: Vec<String> = s.deck.active.iter().map(|a| a.cue.clone()).chain(s.deck.ending.iter().map(|e| e.cue.cue.clone())).collect();
    for id in &shown {
        if !s.cue_mods.contains_key(id) {
            let m = stored(s, id);
            if m != LiveModifiers::default() {
                s.cue_mods.insert(id.clone(), m);
            }
        }
    }
    let pending: Vec<String> = s.deck.pending.iter().map(|p| p.cue.clone()).collect();
    s.cue_mods.retain(|id, _| shown.contains(id) || pending.contains(id));
}

/// The cue level's modifiers of every cue on show (working copy, else
/// the cell's), for the page's « Cue » tab.
pub fn view(s: &Shared) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for id in s.deck.active.iter().map(|a| &a.cue).chain(s.deck.ending.iter().map(|e| &e.cue.cue)) {
        let m = s.cue_mods.get(id).cloned().unwrap_or_else(|| stored(s, id));
        out.insert(id.clone(), serde_json::json!(m));
    }
    serde_json::Value::Object(out)
}

/// « Enregistrer dans le cue »: the working copy of cue `id` (composed
/// with the master when `with_master`) into its cell. The cue must be on
/// show. Returns an HTTP status and a French message on refusal.
pub fn store(s: &mut Shared, id: &str, with_master: bool) -> Result<(), (u16, String)> {
    if !s.presets.iter().any(|p| p.id == id) {
        return Err((404, "cue inconnu".into()));
    }
    let Some(working) = cue_mods_mut(s, id).map(|m| m.clone()) else {
        return Err((409, "Ce cue ne joue pas : lancez-le pour régler et enregistrer ses modificateurs.".into()));
    };
    let m = if with_master { compose(&working, &s.live) } else { for_storage(&working) };
    let mut slot = s.deck.slot(id);
    slot.modifiers = m;
    s.deck.set_slot(id, slot);
    s.deck.save();
    Ok(())
}

// ---------- smoothing ----------

/// « Lissage »: a mass-spring filter on the continuous live values (all
/// levels). Off by default. With the defaults a jump from 0 to 1 is at
/// 0.9 after about 0.3 s, without overshoot (damping ≈ 0.95).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Smoothing {
    pub enabled: bool,
    /// « Inertie »: heavier glides slower.
    pub mass: f32,
    pub spring: f32,
    pub friction: f32,
}

pub const DEFAULT_MASS: f32 = 1.0;
pub const DEFAULT_SPRING: f32 = 160.0;
pub const DEFAULT_FRICTION: f32 = 24.0;
pub const MIN_INERTIA: f32 = 0.25;
pub const MAX_INERTIA: f32 = 4.0;

impl Default for Smoothing {
    fn default() -> Self {
        Self { enabled: false, mass: DEFAULT_MASS, spring: DEFAULT_SPRING, friction: DEFAULT_FRICTION }
    }
}

impl Smoothing {
    pub fn sanitize(&mut self) {
        let d = Self::default();
        let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        self.mass = fix(self.mass, MIN_INERTIA, MAX_INERTIA, d.mass);
        self.spring = fix(self.spring, 1.0, 2000.0, d.spring);
        self.friction = fix(self.friction, 0.0, 500.0, d.friction);
    }

    /// « Inertie »: the mass, with the friction that keeps the same
    /// damping (no overshoot at any inertia).
    pub fn set_inertia(&mut self, mass: f32) {
        self.mass = if mass.is_finite() { mass.clamp(MIN_INERTIA, MAX_INERTIA) } else { DEFAULT_MASS };
        self.friction = DEFAULT_FRICTION * (self.mass * self.spring / (DEFAULT_MASS * DEFAULT_SPRING)).sqrt();
    }
}

/// Continuous values smoothed, in this order.
const FIELDS: usize = 16;
const BRIGHTNESS: usize = 0;
const ROT_SPEED: usize = 9;

fn fields(m: &LiveModifiers) -> [f32; FIELDS] {
    [
        m.brightness,
        m.size,
        m.size_x,
        m.size_y,
        m.pos_x,
        m.pos_y,
        m.rot_angle[0],
        m.rot_angle[1],
        m.rot_angle[2],
        m.rot_speed[0],
        m.rot_speed[1],
        m.rot_speed[2],
        m.perspective,
        m.speed,
        m.trace,
        m.dots,
    ]
}

fn put(m: &mut LiveModifiers, f: [f32; FIELDS]) {
    m.brightness = f[0];
    (m.size, m.size_x, m.size_y, m.pos_x, m.pos_y) = (f[1], f[2], f[3], f[4], f[5]);
    m.rot_angle = [f[6], f[7], f[8]];
    m.rot_speed = [f[9], f[10], f[11]];
    (m.perspective, m.speed, m.trace, m.dots) = (f[12], f[13], f[14], f[15]);
}

/// Integration step: small enough to stay stable at the lightest inertia.
const MAX_STEP_S: f32 = 1.0 / 480.0;

/// One level's filter state.
#[derive(Clone, Debug, Default)]
pub struct Smoother {
    x: [f32; FIELDS],
    v: [f32; FIELDS],
    /// The rotation unit followed (degrees per second or turns per bar):
    /// a change of unit jumps, it doesn't glide from one to the other.
    sync: bool,
    primed: bool,
}

impl Smoother {
    /// `target` as shown after `dt` seconds. Off: `target` itself, and
    /// the next start of smoothing begins where the values are.
    pub fn step(&mut self, target: &LiveModifiers, sm: &Smoothing, dt: f32) -> LiveModifiers {
        let goal = fields(target);
        if !sm.enabled || !goal.iter().all(|v| v.is_finite()) {
            self.primed = false;
            return target.clone();
        }
        if !self.primed || !self.x.iter().chain(&self.v).all(|v| v.is_finite()) {
            (self.x, self.v, self.sync, self.primed) = (goal, [0.0; FIELDS], target.rot_sync, true);
        }
        if self.sync != target.rot_sync {
            self.sync = target.rot_sync;
            self.x[ROT_SPEED..ROT_SPEED + 3].copy_from_slice(&goal[ROT_SPEED..ROT_SPEED + 3]);
            self.v[ROT_SPEED..ROT_SPEED + 3].fill(0.0);
        }
        let mut sm = *sm;
        sm.sanitize();
        let before = self.x[BRIGHTNESS];
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        let steps = (dt / MAX_STEP_S).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            for ((x, v), g) in self.x.iter_mut().zip(&mut self.v).zip(&goal) {
                let a = (sm.spring * (g - *x) - sm.friction * *v) / sm.mass;
                *v += a * h;
                *x += *v * h;
            }
        }
        // Brightness only ever glides towards its target from where it
        // was: never above both, never above 1.
        let cap = goal[BRIGHTNESS].max(before).clamp(0.0, 1.0);
        if self.x[BRIGHTNESS] > cap {
            (self.x[BRIGHTNESS], self.v[BRIGHTNESS]) = (cap, 0.0);
        }
        self.x[BRIGHTNESS] = self.x[BRIGHTNESS].max(0.0);
        let mut out = target.clone();
        put(&mut out, self.x);
        out
    }
}

/// The engine's smoothing state for every level.
#[derive(Default)]
pub struct Smoothers {
    pub master: Smoother,
    layers: [Smoother; crate::layers::LAYER_COUNT],
    cues: HashMap<String, Smoother>,
}

/// What the cue and layer stages apply this frame.
#[derive(Clone, Debug, Default)]
pub struct FrameLevels {
    /// Per layer (index = layer - 1); `None` = neutral.
    pub layers: [Option<LiveModifiers>; crate::layers::LAYER_COUNT],
    /// Per cue instance (`ActiveCue::id`).
    pub cues: HashMap<u64, LiveModifiers>,
}

impl Smoothers {
    /// The levels of this frame, smoothed over `dt` seconds.
    pub fn frame(&mut self, s: &Shared, dt: f32) -> FrameLevels {
        let sm = s.mixer.smoothing;
        let neutral = LiveModifiers::default();
        let mut out = FrameLevels::default();
        for (i, layer) in s.mixer.layers.iter().enumerate() {
            let m = self.layers[i].step(&for_stage(&layer.modifiers), &sm, dt);
            out.layers[i] = (m != neutral).then_some(m);
        }
        self.cues.retain(|id, _| s.cue_mods.contains_key(id));
        let mut done: HashMap<&str, Option<LiveModifiers>> = HashMap::new();
        for (instance, cue) in s.deck.active.iter().map(|a| (a.id, &a.cue)).chain(s.deck.ending.iter().map(|e| (e.cue.id, &e.cue.cue))) {
            let Some(working) = s.cue_mods.get(cue) else { continue };
            let m = done
                .entry(cue.as_str())
                .or_insert_with(|| {
                    let m = self.cues.entry(cue.clone()).or_default().step(&for_stage(working), &sm, dt);
                    (m != neutral).then_some(m)
                })
                .clone();
            if let Some(m) = m {
                out.cues.insert(instance, m);
            }
        }
        out
    }
}

impl FrameLevels {
    /// Animation speed of a look on `layer` (cue instance `id`, if any):
    /// the cue's and the layer's speeds multiplied.
    pub fn speed(&self, id: Option<u64>, layer: u8) -> f32 {
        let cue = id.and_then(|id| self.cues.get(&id)).map_or(1.0, |m| m.speed.clamp(0.0, 4.0));
        cue * self.layer(layer).map_or(1.0, |m| m.speed.clamp(0.0, 4.0))
    }

    pub fn layer(&self, layer: u8) -> Option<&LiveModifiers> {
        self.layers[(layer.clamp(1, crate::layers::LAYER_COUNT as u8) - 1) as usize].as_ref()
    }

    /// Advances the layers' rotations (once a frame, all their looks share
    /// it), each on its speed group's clock (T-419), « Figer » included.
    pub fn advance_layers(
        &self,
        states: &mut [LiveState; crate::layers::LAYER_COUNT],
        t: f64,
        clocks: &[crate::engine::BeatClock; crate::layers::LAYER_COUNT],
        frozen: &[Option<(f64, f64)>; crate::layers::LAYER_COUNT],
        dt: f32,
    ) {
        for (((st, m), clock), frozen) in states.iter_mut().zip(&self.layers).zip(clocks).zip(frozen) {
            st.set_clock(t, clock.beat);
            st.freeze_colours(*frozen);
            if let Some(m) = m {
                st.advance(m, dt, clock.bpm, clock.beats_per_bar);
            }
        }
    }

    /// The layer stage on one look. `inner_coloured`: a lower level (the
    /// cue, an event) already recolours it, so the layer's colour stands
    /// back (the lowest non-*Normal* level wins; the master still forces).
    pub fn layer_stage(&self, layer: u8, points: Vec<Point>, inner_coloured: bool, states: &[LiveState; crate::layers::LAYER_COUNT], user: &[Palette]) -> Vec<Point> {
        let Some(m) = self.layer(layer) else { return points };
        let st = &states[(layer.clamp(1, crate::layers::LAYER_COUNT as u8) - 1) as usize];
        if inner_coloured && m.color != ColorOverride::Normal {
            let m = LiveModifiers { color: ColorOverride::Normal, ..m.clone() };
            crate::live::apply(&points, &m, st, user)
        } else {
            crate::live::apply(&points, m, st, user)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{self, ControlInput};
    use crate::live::{self, Rate, Strobe};

    #[test]
    fn control_ids_parse_to_their_level() {
        assert_eq!(parse("layer.2.size"), Some((Level::Layer(2), "size")));
        assert_eq!(parse("layer.4.color.hue"), Some((Level::Layer(4), "color.hue")));
        assert_eq!(parse("cue.tunnels-001.rot_z.angle"), Some((Level::Cue("tunnels-001"), "rot_z.angle")));
        // Ids with dots: the param is the known end.
        assert_eq!(parse("cue.ilda:a.b.ild.strobe.on"), Some((Level::Cue("ilda:a.b.ild"), "strobe.on")));
        // Not levels: the layers' own controls, the grid's, unknown params,
        // layers out of range, master only params.
        for id in ["layer.1.dimmer", "layer.1.clear", "cue.mode", "cue.transition_length", "layer.5.size", "layer.0.size", "cue..size", "cue.size", "layer.1.scan", "cue.x.scan", "master.size"] {
            assert_eq!(parse(id), None, "{id}");
        }
    }

    #[test]
    fn every_level_param_is_a_master_control() {
        let s = crate::test_support::shared();
        for p in PARAMS {
            assert!(s.controls.get(&format!("master.{p}")).is_some(), "master.{p}");
        }
    }

    #[test]
    fn cue_size_times_master_size_is_the_effective_size() {
        // Acceptance: size 0.5 on the cue × 2.0 on the master = 1.0.
        let pts = vec![Point::lit(0.4, 0.2, 1.0, 1.0, 1.0)];
        let st = LiveState::default();
        let cue = live::apply(&pts, &LiveModifiers { size: 0.5, ..Default::default() }, &st, &[]);
        let out = live::apply(&cue, &LiveModifiers { size: 2.0, ..Default::default() }, &st, &[]);
        assert!((out[0].x - 0.4).abs() < 1e-6 && (out[0].y - 0.2).abs() < 1e-6, "{:?}", out[0]);
        let c = compose(&LiveModifiers { size: 0.5, ..Default::default() }, &LiveModifiers { size: 2.0, ..Default::default() });
        assert_eq!(c.size, 1.0);
    }

    #[test]
    fn compose_adds_positions_and_angles_and_only_dims() {
        let inner = LiveModifiers { pos_x: 0.2, rot_angle: [0.0, 10.0, 30.0], brightness: 0.5, rot_speed: [0.0, 0.0, 90.0], ..Default::default() };
        let outer = LiveModifiers { pos_x: -0.5, rot_angle: [0.0, 5.0, 15.0], brightness: 0.8, rot_speed: [0.0, 0.0, 30.0], ..Default::default() };
        let c = compose(&inner, &outer);
        assert!((c.pos_x + 0.3).abs() < 1e-6);
        assert_eq!(c.rot_angle, [0.0, 15.0, 45.0]);
        assert_eq!(c.rot_speed[2], 120.0);
        assert!((c.brightness - 0.4).abs() < 1e-6);
        // Brightness never goes up, whatever comes in.
        let up = compose(&LiveModifiers { brightness: 0.7, ..Default::default() }, &LiveModifiers { brightness: 5.0, ..Default::default() });
        assert!(up.brightness <= 0.7 + 1e-6);
        let nan = compose(&LiveModifiers { brightness: f32::NAN, ..Default::default() }, &LiveModifiers { brightness: 0.5, ..Default::default() });
        assert!(nan.brightness <= 0.5 + 1e-6);
    }

    #[test]
    fn compose_takes_the_outer_colour_unless_normal() {
        let red = LiveModifiers { color: ColorOverride::Fixed { rgb: [255, 0, 0] }, ..Default::default() };
        let blue = LiveModifiers { color: ColorOverride::Fixed { rgb: [0, 0, 255] }, ..Default::default() };
        assert_eq!(compose(&red, &LiveModifiers::default()).color, red.color);
        assert_eq!(compose(&red, &blue).color, blue.color);
    }

    #[test]
    fn levels_never_keep_freeze_black_or_a_scan_speed() {
        let m = LiveModifiers { freeze: true, black: true, scan: 1.4, strobe: Strobe { hold: true, ..Default::default() }, ..Default::default() };
        let st = for_stage(&m);
        assert!(!st.freeze && !st.black && st.scan == 1.0 && st.strobe.hold, "a held flash stays while held");
        assert!(!for_storage(&m).strobe.hold, "never stored held");
    }

    #[test]
    fn layer_colour_stands_back_for_a_coloured_cue() {
        let mut levels = FrameLevels::default();
        levels.layers[0] = Some(LiveModifiers { color: ColorOverride::Fixed { rgb: [0, 0, 255] }, size: 0.5, ..Default::default() });
        let states: [LiveState; 4] = Default::default();
        let red = vec![Point::lit(0.4, 0.0, 1.0, 0.0, 0.0)];
        let plain = levels.layer_stage(1, red.clone(), false, &states, &[]);
        assert_eq!((plain[0].r, plain[0].b), (0.0, 1.0), "the layer recolours a cue in its own colours");
        let kept = levels.layer_stage(1, red.clone(), true, &states, &[]);
        assert_eq!((kept[0].r, kept[0].b), (1.0, 0.0), "the cue's own override wins");
        assert!((kept[0].x - 0.2).abs() < 1e-6, "the rest of the layer still applies");
        // Other layers are untouched.
        assert_eq!(levels.layer_stage(2, red.clone(), false, &states, &[]), red);
    }

    fn step_response(sm: &Smoothing, frames: usize) -> Vec<f32> {
        let mut f = Smoother::default();
        let zero = LiveModifiers { pos_x: 0.0, ..Default::default() };
        f.step(&zero, sm, 1.0 / 60.0);
        let one = LiveModifiers { pos_x: 1.0, ..Default::default() };
        (0..frames).map(|_| f.step(&one, sm, 1.0 / 60.0).pos_x).collect()
    }

    #[test]
    fn smoothing_reaches_ninety_percent_in_about_a_third_of_a_second_without_overshoot() {
        let sm = Smoothing { enabled: true, ..Default::default() };
        let xs = step_response(&sm, 180);
        let at = xs.iter().position(|&x| x >= 0.9).expect("reaches 0.9") as f32 / 60.0 + 1.0 / 60.0;
        assert!((0.2..=0.4).contains(&at), "0.9 after {at} s");
        let peak = xs.iter().cloned().fold(0.0, f32::max);
        assert!(peak <= 1.05, "peak {peak}");
        assert!((xs[179] - 1.0).abs() < 1e-3, "settles");
        // More inertia: slower, still no overshoot.
        let mut heavy = sm;
        heavy.set_inertia(4.0);
        let xs = step_response(&heavy, 240);
        let slow = xs.iter().position(|&x| x >= 0.9).unwrap() as f32 / 60.0;
        assert!(slow > at * 1.5, "{slow} vs {at}");
        assert!(xs.iter().all(|&x| x <= 1.05));
        let mut light = sm;
        light.set_inertia(MIN_INERTIA);
        assert!(step_response(&light, 120).iter().all(|&x| x.is_finite() && x <= 1.05), "stable at the lightest inertia");
    }

    #[test]
    fn smoothing_off_passes_values_through() {
        let mut f = Smoother::default();
        let m = LiveModifiers { size: 1.7, ..Default::default() };
        assert_eq!(f.step(&m, &Smoothing::default(), 1.0 / 60.0), m);
    }

    #[test]
    fn smoothed_brightness_never_rises_above_its_target() {
        let sm = Smoothing { enabled: true, ..Default::default() };
        let mut f = Smoother::default();
        f.step(&LiveModifiers { brightness: 0.0, ..Default::default() }, &sm, 1.0 / 60.0);
        for target in [0.6, 0.6, 0.6, 0.1, 0.9, 1.0] {
            for _ in 0..40 {
                let b = f.step(&LiveModifiers { brightness: target, ..Default::default() }, &sm, 1.0 / 60.0).brightness;
                assert!((0.0..=1.0).contains(&b));
            }
        }
        // Rising, never above the target at any frame.
        let mut f = Smoother::default();
        f.step(&LiveModifiers { brightness: 0.0, ..Default::default() }, &sm, 1.0 / 60.0);
        for _ in 0..120 {
            let b = f.step(&LiveModifiers { brightness: 0.5, ..Default::default() }, &sm, 1.0 / 60.0).brightness;
            assert!(b <= 0.5 + 1e-6, "{b}");
        }
    }

    #[test]
    fn a_rotation_unit_change_jumps_instead_of_gliding() {
        let sm = Smoothing { enabled: true, ..Default::default() };
        let mut f = Smoother::default();
        f.step(&LiveModifiers { rot_speed: [0.0, 0.0, 90.0], ..Default::default() }, &sm, 1.0 / 60.0);
        let synced = LiveModifiers { rot_speed: [0.0, 0.0, 1.0], rot_sync: true, ..Default::default() };
        assert_eq!(f.step(&synced, &sm, 1.0 / 60.0).rot_speed[2], 1.0);
    }

    fn playing(id: &str) -> Shared {
        let mut s = crate::test_support::shared();
        assert!(controls::press_cue(&mut s, id, None, true));
        s
    }

    #[test]
    fn cue_controls_move_the_working_copy_of_a_playing_cue_only() {
        let id = crate::presets::catalog()[0].id.clone();
        let mut s = crate::test_support::shared();
        let r = controls::apply(&mut s, &format!("cue.{id}.size"), ControlInput::Value(0.5), false);
        assert!(r.is_err(), "not playing: refused");
        assert!(controls::press_cue(&mut s, &id, None, true));
        controls::apply(&mut s, &format!("cue.{id}.size"), ControlInput::Value(0.5), false).unwrap();
        controls::apply(&mut s, &format!("cue.{id}.color.mode"), ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.cue_mods[&id].size, 0.5);
        assert_eq!(s.cue_mods[&id].color.mode_index(), 1);
        assert_eq!(s.live, LiveModifiers::default(), "the master is untouched");
        controls::apply(&mut s, &format!("cue.{id}.reset"), ControlInput::Value(1.0), false).unwrap();
        assert_eq!(s.cue_mods[&id], LiveModifiers::default());
    }

    #[test]
    fn layer_controls_move_the_layer_and_are_saved_with_the_mixer() {
        let mut s = crate::test_support::shared();
        controls::apply(&mut s, "layer.3.pos_x", ControlInput::Value(0.4), false).unwrap();
        controls::apply(&mut s, "layer.3.rot_z.speed", ControlInput::Value(45.0), false).unwrap();
        controls::apply(&mut s, "layer.3.strobe.on", ControlInput::Value(1.0), false).unwrap();
        let m = &s.mixer.layers[2].modifiers;
        assert_eq!((m.pos_x, m.rot_speed[2], m.strobe.on), (0.4, 45.0, true));
        assert!(s.mixer_dirty);
        // Values are clamped like the master's.
        controls::apply(&mut s, "layer.3.size", ControlInput::Value(9.0), false).unwrap();
        assert_eq!(s.mixer.layers[2].modifiers.size, 2.0);
        // Old layers.json without modifiers loads neutral.
        let old: crate::layers::Mixer = serde_json::from_str(r#"{"layers":[{"dimmer":0.5},{},{},{}]}"#).unwrap();
        assert_eq!(old.layers[0].modifiers, LiveModifiers::default());
        assert!(!old.smoothing.enabled);
    }

    #[test]
    fn stored_modifiers_come_back_at_the_next_start() {
        let id = crate::presets::catalog()[0].id.clone();
        let mut s = playing(&id);
        controls::apply(&mut s, &format!("cue.{id}.size"), ControlInput::Value(0.5), false).unwrap();
        controls::apply(&mut s, &format!("cue.{id}.rot_z.angle"), ControlInput::Value(30.0), false).unwrap();
        controls::apply(&mut s, &format!("cue.{id}.strobe.hold"), ControlInput::Value(1.0), false).unwrap();
        store(&mut s, &id, false).unwrap();
        let saved = s.deck.slot(&id).modifiers;
        assert_eq!((saved.size, saved.rot_angle[2], saved.strobe.hold), (0.5, 30.0, false), "never stored held");
        // Stop: the working copy goes; a tweak before that is not kept.
        controls::apply(&mut s, &format!("cue.{id}.size"), ControlInput::Value(1.5), false).unwrap();
        controls::stop_cue(&mut s, &id);
        s.deck.ending.clear();
        frame(&mut s);
        assert!(!s.cue_mods.contains_key(&id));
        // Start again: the stored ones.
        assert!(controls::press_cue(&mut s, &id, None, true));
        frame(&mut s);
        assert_eq!(s.cue_mods[&id], saved);
        assert_eq!(view(&s)[&id]["size"], 0.5);
    }

    #[test]
    fn storing_with_the_master_composes_it_in() {
        let id = crate::presets::catalog()[0].id.clone();
        let mut s = playing(&id);
        controls::apply(&mut s, &format!("cue.{id}.size"), ControlInput::Value(0.5), false).unwrap();
        controls::apply(&mut s, "master.size", ControlInput::Value(1.5), false).unwrap();
        controls::apply(&mut s, "master.color.mode", ControlInput::Value(2.0), false).unwrap();
        store(&mut s, &id, true).unwrap();
        let m = s.deck.slot(&id).modifiers;
        assert_eq!(m.size, 0.75);
        assert_eq!(m.color.mode_index(), 2);
        // Not playing: refused, nothing stored.
        let other = crate::presets::catalog()[1].id.clone();
        assert_eq!(store(&mut s, &other, false).unwrap_err().0, 409);
        assert_eq!(s.deck.slot(&other).modifiers, LiveModifiers::default());
        assert_eq!(store(&mut s, "nope", false).unwrap_err().0, 404);
    }

    #[test]
    fn old_cells_without_modifiers_load_neutral() {
        let slot: crate::cues::CueSlot = serde_json::from_str(r#"{"layer":2,"group":1}"#).unwrap();
        assert_eq!(slot.modifiers, LiveModifiers::default());
        // And a neutral cell writes nothing new.
        let json = serde_json::to_string(&crate::cues::CueSlot::default()).unwrap();
        assert!(!json.contains("modifiers"), "{json}");
    }

    #[test]
    fn frame_levels_follow_the_deck_and_the_mixer() {
        let id = crate::presets::catalog()[0].id.clone();
        let mut s = playing(&id);
        controls::apply(&mut s, &format!("cue.{id}.speed"), ControlInput::Value(2.0), false).unwrap();
        controls::apply(&mut s, "layer.1.speed", ControlInput::Value(0.5), false).unwrap();
        controls::apply(&mut s, "layer.2.strobe.rate", ControlInput::Value(4.0), false).unwrap();
        let mut sm = Smoothers::default();
        let lv = sm.frame(&s, 1.0 / 60.0);
        let instance = s.deck.active[0].id;
        assert_eq!(lv.cues[&instance].speed, 2.0);
        assert_eq!(lv.speed(Some(instance), 1), 1.0);
        assert!(lv.layer(2).is_some(), "a changed strobe rate is a non-neutral layer");
        assert!(lv.layer(3).is_none());
        assert_eq!(lv.layers[1].as_ref().unwrap().strobe.rate, Rate::Hz(4.0));
    }
}
