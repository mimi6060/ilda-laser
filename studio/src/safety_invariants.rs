//! Safety invariants (T-264): property tests over the whole output path.
//!
//! Each case builds a studio state, then drives the real engine frame
//! (`Engine::tick`, the very function the 60 fps thread runs) on a
//! simulated clock, with random actions between frames: cues, looks and
//! live modifiers sent as an API client could send them, LFOs, audio
//! routes and features, MIDI, the timeline (with envelopes and bus
//! tracks, T-163), test patterns, zones, caps,
//! calibration, projection zones and their geometry (T-012, cue → zone
//! routing included), arm / disarm / e-stop. A recording output captures what
//! would leave for the laser. After every frame we check:
//!
//! 1. disarmed or e-stopped ⇒ no lit point sent (and the output is not
//!    armed);
//! 2. no colour above the output's caps (global × per colour);
//! 3. nothing lit in a Blank zone, nor on a segment crossing one; with
//!    the full horizon at level 0, nothing lit below it;
//! 4. (outdoor sectors / « Ciel coupé », T-262: not implemented yet, so
//!    not checked);
//! 5. coordinates always finite and within -1..1;
//! 6. no sequence of controls, MIDI, external inputs (DMX / OSC /
//!    timecode through the gate, T-408) or API calls arms, except the
//!    explicit user paths (UI / keyboard / API arm request, and the MIDI
//!    opt-in gesture when the operator allowed it).
//!
//! 7. the output's colour rendering (T-414: white balance, curves, colour
//!    delay) never lights a point that was dark, never moves or adds
//!    points, keeps every channel within 0..1, and runs before the whole
//!    safety stage: the caps (2) and zones (3) hold with it on;
//! 8. audience scanning off (the default, T-255): nothing lit in a
//!    « Public » zone nor on a segment crossing one, whatever its kind;
//! 9. audience scanning on: no lit sample in a Public zone brighter than
//!    the zone's level allows (through the minimum diode level and caps);
//! 10. audience scanning starts only from the explicit unlock (never from
//!     an action, a frame or a refused unlock), an unlock never arms, and
//!     no frame runs it with the e-stop latched.
//! 11. transitions between cues (T-158): a morph never lights blanked
//!     travel, whatever the two frames' point counts (every lit step of a
//!     morph is a blend of a lit piece of stroke of each frame), keeps a
//!     stable point count and never gets brighter than the brighter
//!     frame; fades and quantised launches (T-159) run before the safety
//!     stage (the session fuzz sets them at random), and the e-stop cuts
//!     every transition and drops every waiting launch at once.
//! 12. audio triggers (T-240): they never arm (covered by 6, their frame
//!     included), the automation is off whenever the e-stop is latched,
//!     and no rule acts in a frame that starts with the stop latched or
//!     the switch off.
//! 13. cue lists (T-410): a random list (every trigger, fades, morphs,
//!     split timing, fired controls) driven by Go / Retour / Aller à /
//!     Pause / Début and the `list.*` controls renders through the same
//!     chain (1-12 hold with it), never arms, is paused by the e-stop and
//!     never moves on by itself while paused; a fade from black never
//!     shows intermediate geometry (`a_cue_list_fade_from_black_*`).
//! 14. festival palettes and perceived balance (T-130): a palette, the
//!     « Vers le blanc » and « Chase couleur » modes and any balance gains
//!     (out of range ones included) render through the same chain, so
//!     1-13 hold with them; the balance only ever dims.
//! 15. output scan limits (T-171): random points-per-frame and scan
//!     speed ranges and the live scan speed run in the sessions (1-14
//!     hold with them); completing a short frame never draws a lit
//!     segment nor lights a spot the frame didn't, the limits run before
//!     the safety stage (no Blank zone lit after it), and no look is
//!     traced faster than the output's `pps_max`
//!     (`scan_limits_never_add_light_and_never_trace_faster_than_pps_max`).
//!
//! Plus the strobe limiter seen from the output: no fast flashing for
//! longer than the allowed burst, on random cases and with every
//! catalogue cue forced onto the default seeds (flashed over itself, and
//! strobed every way; every cue on every seed when `SAFETY_CASES` is
//! set; T-360, `SAFETY_CUE=<id>` replays one cue); and the ShowNET SD card over Art-Net
//! (T-301): what it would send is intensity 0 and pattern 0 whenever the
//! stage sends no light, and its intensity never exceeds the caps.
//!
//! The generator is a small seeded xorshift (no new dependency); every
//! failure prints its seed. `SAFETY_SEED=<n>` replays one seed,
//! `SAFETY_CASES=<n>` runs more (or fewer) cases.
//!
//! **Adding an invariant** when a new protection lands: give `Rig` a way
//! to set it up (or randomise it in `random_case`), add an action that
//! exercises it to `random_action` if it has a runtime control, and
//! assert it in `check_points` / `run_case` (every frame) or in a dedicated test like
//! `strobe_never_exceeds_the_burst_limit` (behaviour over time).

use crate::colorcal::{ChannelCurve, ColorCalibration};
use crate::controls::{self, ControlInput};
use crate::cues::{ClickMode, CueSlot, Quantize, BEAT_EVERY, QUANTIZE_MODES};
use crate::engine::{Calibration, Content, Settings};
use crate::festival::{Balance, PaletteQuantize, PALETTES};
use crate::generators::ColorMode;
use crate::evolving::KeyEase;
use crate::interlock::ArmSource;
use crate::midi::{self, decode::MidiMsg, MidiEvent};
use crate::output::Output;
use crate::patterns::Point;
use crate::power::{OutputLimits, MAIN_OUTPUT};
use crate::presence::{Presence, PresenceSettings};
use crate::projection::{Geometry, ProjectionConfig, ProjectionZone, Warp, MAX_ZONES, SQUARE};
use crate::safety::{self, SafetySettings};
use crate::zones::{self, Horizon, Zone, ZoneKind};
use crate::transition::{self, Transition, TransitionKind, TransitionLength, LENGTHS, TRANSITION_KINDS};
use crate::{audience, dwell, interlock, lfo, live, program, test_support, testpattern, timeline, Engine, Shared, FRAME_INTERVAL};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------- random numbers ----------

/// xorshift64*: small, fast, reproducible.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in 0..1.
    pub(crate) fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub(crate) fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub(crate) fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }

    pub(crate) fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.parse().ok()
}

/// The seeds of one property: `SAFETY_SEED` alone, or `n` (or
/// `SAFETY_CASES`) seeds derived from a fixed base, so a run is reproducible.
pub(crate) fn seeds(property: u64, n: usize) -> Vec<u64> {
    if let Some(seed) = env_u64("SAFETY_SEED") {
        return vec![seed];
    }
    let n = env_u64("SAFETY_CASES").map_or(n, |c| c as usize);
    (0..n as u64).map(|i| property * 1_000_003 + i).collect()
}

/// Replaces some numbers (and booleans) of a JSON value, the way a buggy
/// or hostile API client could: kept to sane magnitudes (huge sizes only
/// make huge frames, which is not what these tests are about).
fn mutate(v: &mut Value, rng: &mut Rng, p: f32) {
    match v {
        Value::Number(n) if rng.chance(p) => {
            *v = if let Some(i) = n.as_u64() {
                json!(rng.below((i as usize * 2 + 4).min(600)))
            } else {
                let x = n.as_f64().unwrap_or(0.0) as f32;
                let y = match rng.below(5) {
                    0 => x * rng.range(-3.0, 3.0),
                    1 => rng.range(-2.0, 2.0),
                    2 => 0.0,
                    3 => 1.0,
                    _ => rng.range(-20.0, 20.0),
                };
                json!(y)
            }
        }
        Value::Bool(b) if rng.chance(p) => *b = !*b,
        Value::Array(a) => a.iter_mut().for_each(|x| mutate(x, rng, p)),
        Value::Object(o) => o.values_mut().for_each(|x| mutate(x, rng, p)),
        _ => {}
    }
}

/// `base` with some fields mutated through JSON; `base` itself when the
/// mutated value no longer deserializes (the API would answer 400).
pub(crate) fn mutated<T: serde::Serialize + serde::de::DeserializeOwned + Clone>(base: &T, rng: &mut Rng, p: f32) -> T {
    let mut v = serde_json::to_value(base).unwrap();
    mutate(&mut v, rng, p);
    serde_json::from_value(v).unwrap_or_else(|_| base.clone())
}

// ---------- the rig: shared state + real engine + recording output ----------

/// What the output was told.
#[derive(Default)]
struct Wire {
    armed: bool,
    last: Vec<Point>,
    frames: u64,
    /// The last cue signal (only for an "artnet" output, T-301).
    signal: Option<crate::output::CueSignal>,
    /// Set by a test: the output reports this fault (unplugged, T-277).
    fault: Option<String>,
}

/// The recording output, with an optional frame size limit (like a Helios
/// USB DAC) so the fuzz drives the thinning path too (T-353).
struct Recorder(Arc<Mutex<Wire>>, Option<usize>);

impl Output for Recorder {
    fn name(&self) -> &str {
        "enregistreur"
    }
    fn kind(&self) -> &'static str {
        "test"
    }
    fn set_armed(&mut self, armed: bool) -> anyhow::Result<()> {
        self.0.lock().unwrap().armed = armed;
        Ok(())
    }
    fn send(&mut self, points: &[Point]) {
        let mut w = self.0.lock().unwrap();
        w.last = points.to_vec();
        w.frames += 1;
    }
    fn max_points(&self) -> Option<usize> {
        self.1
    }
    fn cue_signal(&mut self, signal: &crate::output::CueSignal) {
        self.0.lock().unwrap().signal = Some(signal.clone());
    }
    fn fault(&self) -> Option<String> {
        self.0.lock().unwrap().fault.clone()
    }
}

struct Rig {
    shared: Mutex<Shared>,
    engine: Option<Engine>,
    wire: Arc<Mutex<Wire>>,
    now: Instant,
    dir: PathBuf,
}

impl Rig {
    fn new(s: Shared, tag: &str, seed: u64) -> Self {
        Self::with_max_points(s, tag, seed, None)
    }

    /// A rig whose output takes at most `max_points` points per frame.
    fn with_max_points(s: Shared, tag: &str, seed: u64, max_points: Option<usize>) -> Self {
        let dir = std::env::temp_dir().join(format!("laser-studio-invariants-{tag}-{seed}-{}", std::process::id()));
        // One simulated clock for the whole rig, starting at the epoch: the
        // engine's frames, its first step and every `Shared::now_s` (cue
        // presses, tempo, timeline) read it, so the same session replays
        // bit for bit, whatever the machine's speed.
        let mut s = s;
        let now = Instant::now();
        s.epoch = now;
        s.sim_now = Some(now);
        // The browser's (default) audio features arrived at startup: they
        // go stale 500 ms of simulated time later, not of wall-clock time.
        s.audio_at = now;
        let shared = Mutex::new(s);
        let wire = Arc::new(Mutex::new(Wire::default()));
        let mut engine = Engine::new(&shared, Some(Box::new(Recorder(Arc::clone(&wire), max_points))), dir.join("live.json"), dir.join("layers.json"));
        engine.last = now;
        Rig { shared, engine: Some(engine), wire, now, dir }
    }

    fn s(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    /// One engine frame, 1/60 s after the previous one.
    fn tick(&mut self) {
        self.now += FRAME_INTERVAL;
        self.s().sim_now = Some(self.now);
        // The analogue clock look shows simulated time too (noon + t).
        let t = self.t();
        crate::generators::TEST_CLOCK_SECONDS.with(|c| c.set(Some(43_200.0 + t as f32)));
        self.engine.as_mut().unwrap().tick(&self.shared, self.now);
    }

    /// The simulated seconds since startup (`Shared::now_s` at `now`).
    fn t(&self) -> f64 {
        self.now.saturating_duration_since(self.s().epoch).as_secs_f64()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.shutdown(&self.shared);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------- random configuration ----------

fn random_polygon(rng: &mut Rng) -> Vec<[f32; 2]> {
    let n = 3 + rng.below(7);
    let (cx, cy, r) = (rng.range(-0.8, 0.8), rng.range(-0.8, 0.8), rng.range(0.05, 0.7));
    let mut angles: Vec<f32> = (0..n).map(|_| rng.range(0.0, std::f32::consts::TAU)).collect();
    // Mostly simple polygons (sorted angles); sometimes self-crossing ones.
    if rng.chance(0.8) {
        angles.sort_by(f32::total_cmp);
    }
    angles
        .iter()
        .map(|a| {
            let rr = r * rng.range(0.3, 1.0);
            // Up to the field's edge: content clamped onto ±1 by the
            // calibration then sits exactly on the zone edge (T-351).
            [(cx + rr * a.cos()).clamp(-1.0, 1.0), (cy + rr * a.sin()).clamp(-1.0, 1.0)]
        })
        .collect()
}

fn random_safety(rng: &mut Rng) -> SafetySettings {
    let zones = (0..rng.below(4))
        .map(|i| Zone {
            id: i as u32 + 1,
            name: format!("Z{i}"),
            kind: if rng.chance(0.6) { ZoneKind::Blank } else { ZoneKind::Dim },
            level: rng.f(),
            points: random_polygon(rng),
            audience: false,
        })
        .collect();
    let horizon = Horizon {
        y: rng.range(-1.0, 0.5),
        lines: rng.chance(0.5),
        ramp: rng.range(0.0, 0.5),
        level: if rng.chance(0.5) { 0.0 } else { rng.f() },
    };
    let unit_or_one = |rng: &mut Rng| if rng.chance(0.5) { 1.0 } else { rng.f() };
    SafetySettings {
        strobe_max_hz: rng.range(0.5, safety::DEFAULT_MAX_HZ),
        strobe_burst_s: rng.range(0.0, safety::DEFAULT_BURST_S),
        strobe_cooldown_s: rng.range(safety::DEFAULT_COOLDOWN_S, 6.0),
        zones,
        horizon,
        color_gain: [unit_or_one(rng), unit_or_one(rng), unit_or_one(rng)],
        min_diode_level: if rng.chance(0.6) { 0.0 } else { rng.range(0.0, 0.5) },
        dwell: dwell::DwellGuard { profile: *rng.pick(&[dwell::DwellProfile::Strict, dwell::DwellProfile::Beams, dwell::DwellProfile::Off]), ..Default::default() },
    }
}

fn random_limits(rng: &mut Rng) -> OutputLimits {
    let c = |rng: &mut Rng| if rng.chance(0.4) { 1.0 } else { rng.f() };
    OutputLimits { max_power: c(rng), max_color: [c(rng), c(rng), c(rng)] }
}

/// An output colour rendering (T-414) as `POST /api/outputs/color` could
/// send it: mostly in range (curves often brighter than linear, decreasing
/// five-point curves included), sometimes wild values the store refuses.
fn random_color(rng: &mut Rng) -> ColorCalibration {
    let curve = |rng: &mut Rng| match rng.below(3) {
        0 => ChannelCurve::Gamma { gamma: 1.0 },
        1 => ChannelCurve::Gamma { gamma: rng.range(0.2, 5.0) },
        _ => ChannelCurve::Points { out: [rng.f(), rng.f(), rng.f(), rng.f(), rng.f()] },
    };
    let gain = |rng: &mut Rng| if rng.chance(0.5) { 1.0 } else { rng.f() };
    let c = ColorCalibration {
        white_balance: [gain(rng), gain(rng), gain(rng)],
        curve: [curve(rng), curve(rng), curve(rng)],
        color_delay_points: rng.below(17) as i8 - 8,
    };
    if rng.chance(0.15) {
        mutated(&c, rng, 0.3)
    } else {
        c
    }
}

fn random_calibration(rng: &mut Rng) -> Calibration {
    // The ranges `POST /api/calibration` clamps to.
    Calibration {
        offset_x: rng.range(-1.0, 1.0),
        offset_y: rng.range(-1.0, 1.0),
        scale_x: rng.range(0.1, 2.0),
        scale_y: rng.range(0.1, 2.0),
        rotation_deg: rng.range(-180.0, 180.0),
    }
}

/// Projection zones (T-012) as `POST /api/projection` could send them:
/// 1 to 4 zones with any geometry the API accepts after clamping (wild
/// values included), sometimes a 4-corner warp, sometimes an invalid
/// quad (refused), an output that may not be the connected one.
fn random_projection(rng: &mut Rng) -> ProjectionConfig {
    let mut ids: Vec<u8> = (1..=MAX_ZONES).collect();
    let n = 1 + rng.below(4);
    let zones = (0..n)
        .map(|_| {
            let id = ids.remove(rng.below(ids.len()));
            let a = |rng: &mut Rng, span: f32| if rng.chance(0.5) { 0.0 } else { rng.range(-span, span) };
            let mut corners = SQUARE;
            for c in &mut corners {
                c[0] = (c[0] * rng.range(0.2, 1.0) + rng.range(-0.3, 0.3)).clamp(-1.0, 1.0);
                c[1] = (c[1] * rng.range(0.2, 1.0) + rng.range(-0.3, 0.3)).clamp(-1.0, 1.0);
            }
            if rng.chance(0.1) {
                corners.swap(1, 2); // crossed: refused by the API
            }
            let geometry = Geometry {
                size_x: rng.range(0.05, 3.0),
                size_y: rng.range(0.05, 3.0),
                pos_x: a(rng, 1.5),
                pos_y: a(rng, 1.5),
                rotation_deg: a(rng, 180.0),
                shear_x: a(rng, 1.0),
                shear_y: a(rng, 1.0),
                keystone_x: a(rng, 1.0),
                keystone_y: a(rng, 1.0),
                pincushion_x: a(rng, 0.6),
                pincushion_y: a(rng, 0.6),
                bow_x: a(rng, 0.6),
                bow_y: a(rng, 0.6),
                warp: Warp { enabled: rng.chance(0.4), corners },
            };
            let output = if rng.chance(0.2) { Some(rng.pick(&["test", "helios:autre"]).to_string()) } else { None };
            ProjectionZone { id, name: format!("Z{id}"), enabled: rng.chance(0.9), output, geometry }
        })
        .collect::<Vec<_>>();
    let default_zones = (0..1 + rng.below(2)).map(|_| zones[rng.below(zones.len())].id).collect();
    ProjectionConfig { zones, default_zones }
}

/// Projection zones, cue → zone routing and the alignment zone, changed
/// the way the API would (refusals included).
fn random_routing(s: &mut Shared, rng: &mut Rng) {
    match rng.below(3) {
        0 => {
            let _ = s.projection.set(random_projection(rng));
        }
        1 => {
            let (id, _) = random_preset(s, rng);
            let zones = (0..rng.below(3)).map(|_| rng.below(MAX_ZONES as usize + 2) as u8).collect();
            let old = s.deck.slot(&id);
            // A cue program (T-157) as `POST /api/cues/program` takes it:
            // values out of range on purpose, clamped at render.
            let program = if rng.chance(0.4) { Some(random_program(s, rng)) } else { old.program.clone() };
            let slot = CueSlot { zones, program, ..old };
            s.deck.set_slot(&id, slot);
        }
        _ => {
            let ids: Vec<u8> = s.projection.get().zones.iter().map(|z| z.id).collect();
            s.projection.align = if rng.chance(0.3) { None } else { Some(*rng.pick(&ids)) };
        }
    }
}

/// Inner layers from other cues, curves and LFOs on random targets with
/// values well past their ranges, any play mode.
fn random_program(s: &Shared, rng: &mut Rng) -> program::CueProgram {
    use crate::evolving::KeyEase;
    let eases = [KeyEase::Step, KeyEase::Linear, KeyEase::EaseIn, KeyEase::EaseOut, KeyEase::Smooth];
    let plays = [program::PlayMode::Loop, program::PlayMode::Once, program::PlayMode::PingPong];
    let targets: Vec<&str> = program::TARGETS.iter().map(|t| t.0).collect();
    let mut lanes: Vec<program::Lane> = Vec::new();
    for _ in 0..rng.below(5) {
        let target = rng.pick(&targets).to_string();
        if lanes.iter().any(|l| l.target == target) {
            continue;
        }
        let keys = (0..rng.below(5))
            .map(|_| program::Key { beat: rng.range(0.0, 20.0), value: rng.range(-1000.0, 1000.0), ease: *rng.pick(&eases) })
            .collect();
        lanes.push(program::Lane { target, keys });
    }
    let lfo_targets: Vec<&str> = targets.iter().copied().filter(|t| !t.ends_with(".speed")).collect();
    let lfos = (0..rng.below(3))
        .map(|_| lfo::Modulator {
            target: rng.pick(&lfo_targets).to_string(),
            rate: if rng.chance(0.5) { live::Rate::Hz(rng.range(0.1, 25.0)) } else { live::Rate::Beats(rng.range(0.125, 8.0)) },
            depth: rng.range(0.0, 3.0),
            offset: rng.range(-3.0, 3.0),
            ..Default::default()
        })
        .collect();
    let layers = (0..rng.below(program::MAX_LAYERS + 1)).map(|_| random_preset(s, rng).1.content).collect();
    program::CueProgram {
        length_beats: if rng.chance(0.5) { Some(rng.range(0.1, 64.0)) } else { None },
        play: *rng.pick(&plays),
        layers,
        lanes,
        lfos,
    }
}

/// A cue list of 1 to 8 random steps (T-410): every source, trigger,
/// transition, split timing and a fired control now and then.
fn random_cue_list(s: &Shared, rng: &mut Rng) -> crate::cuelist::CueList {
    use crate::cuelist::{CueList, SplitTiming, Step, StepAction, StepSource, StepTrigger};
    let fireable: Vec<String> = s
        .controls
        .list()
        .iter()
        .filter(|d| d.external && !d.id.starts_with("list.") && d.id != "safety.estop" && d.id != "transport.blackout")
        .map(|d| d.id.clone())
        .collect();
    let steps = (0..1 + rng.below(8))
        .map(|_| {
            let source = match rng.below(4) {
                0 => StepSource::Black,
                _ => StepSource::Cue { id: random_preset(s, rng).0 },
            };
            let trigger = match rng.below(6) {
                0 | 1 => StepTrigger::Go,
                2 => StepTrigger::After { s: rng.range(0.0, 2.0) },
                3 => StepTrigger::Follow,
                4 => StepTrigger::Beats { n: 1 + rng.below(4) as u32 },
                _ => StepTrigger::Audio { event: rng.pick(&crate::engine::AUDIO_EVENTS).to_string() },
            };
            let split = rng.chance(0.3).then(|| SplitTiming { geometry_s: rng.range(0.0, 2.0), color_s: rng.range(0.0, 3.0), brightness_s: rng.range(0.0, 2.0) });
            let fire = rng.chance(0.1).then(|| StepAction { control: rng.pick(&fireable).clone(), value: rng.range(-1.0, 2.0) });
            Step {
                source,
                trigger,
                delay_s: if rng.chance(0.2) { rng.range(0.0, 1.0) } else { 0.0 },
                fade_in_s: rng.range(0.0, 2.0),
                fade_out_s: rng.range(0.0, 2.0),
                transition: *rng.pick(&TRANSITION_KINDS),
                split,
                keep_color: rng.chance(0.2),
                fire,
                ..Step::default()
            }
        })
        .collect();
    CueList { name: "fuzz".into(), layer: 1 + rng.below(crate::layers::LAYER_COUNT) as u8, wrap: rng.chance(0.5), steps }
}

fn random_preset(s: &Shared, rng: &mut Rng) -> (String, Settings) {
    let p = rng.pick(&s.presets);
    (p.id.clone(), p.settings.clone())
}

fn modulatable(s: &Shared) -> Vec<String> {
    s.controls.list().iter().filter(|d| lfo::modulatable(&s.controls, &d.id).is_some()).map(|d| d.id.clone()).collect()
}

fn random_lfos(s: &Shared, rng: &mut Rng) -> Vec<lfo::Modulator> {
    let targets = modulatable(s);
    let waves = [lfo::Wave::Sine, lfo::Wave::Triangle, lfo::Wave::Square, lfo::Wave::SawUp, lfo::Wave::SawDown, lfo::Wave::Random];
    (0..rng.below(4))
        .map(|_| lfo::Modulator {
            target: rng.pick(&targets).clone(),
            wave: *rng.pick(&waves),
            rate: if rng.chance(0.5) { live::Rate::Hz(rng.range(0.1, 25.0)) } else { live::Rate::Beats(rng.range(0.125, 8.0)) },
            depth: rng.f(),
            phase: rng.f(),
            offset: rng.range(-1.0, 1.0),
            enabled: rng.chance(0.9),
        })
        .collect()
}

fn random_routes(s: &Shared, rng: &mut Rng) -> crate::audio::routes::AudioRouting {
    let targets = modulatable(s);
    let routes = (0..rng.below(4))
        .map(|_| {
            let base = crate::audio::routes::AudioRoute {
                source: rng.pick(&crate::audio::routes::SOURCES).0.to_string(),
                target: rng.pick(&targets).clone(),
                ..Default::default()
            };
            mutated(&base, rng, 0.3)
        })
        .collect();
    crate::audio::routes::AudioRouting { mix: rng.f(), routes }
}

/// Audio features as the browser source (`POST /api/audio`) sends them.
fn random_audio(s: &mut Shared, rng: &mut Rng, now: Instant, t: f64, beats: &mut u64) {
    *beats += rng.below(3) as u64;
    let body = json!({
        "level": rng.range(0.0, 6.0),
        "bass": rng.f(),
        "beat": *beats,
        "kick": *beats,
        "snare": *beats / 2,
        "drop": *beats / 16,
        "buildup": rng.f(),
    });
    if let Ok(f) = crate::audio::browser_features(&body, t) {
        s.audio = f;
        s.audio_at = now;
    }
}

/// Audio trigger rules (T-240) on random cues, any event and action, some
/// mangled.
fn random_triggers(s: &Shared, rng: &mut Rng) -> crate::audio::triggers::TriggerConfig {
    use crate::audio::triggers::{AudioEvent, AudioTrigger, TriggerAction, TriggerConfig};
    let triggers = (0..1 + rng.below(4))
        .map(|_| {
            let (cue, _) = random_preset(s, rng);
            let event = match rng.below(7) {
                0 => AudioEvent::Kick,
                1 => AudioEvent::Snare,
                2 => AudioEvent::Drop,
                3 => AudioEvent::Break,
                4 => AudioEvent::BuildupAbove(rng.f()),
                5 => AudioEvent::Silence,
                _ => AudioEvent::EveryBars(1 + rng.below(4) as u32),
            };
            let action = match rng.below(4) {
                0 => TriggerAction::LaunchCue(cue),
                1 => TriggerAction::NextCue,
                2 => TriggerAction::FlashCue { id: cue, beats: rng.range(0.25, 4.0) },
                _ => TriggerAction::BlackHold { beats: rng.range(0.25, 2.0) },
            };
            let base = AudioTrigger { event, action, min_interval_beats: rng.range(0.0, 2.0), ..Default::default() };
            mutated(&base, rng, 0.1)
        })
        .collect();
    TriggerConfig { triggers }
}

/// Random envelopes (T-163): mostly on controls LFOs may move, sometimes
/// on any control id (arm, safety, layers…), which must be ignored.
fn random_envelopes(rng: &mut Rng, targets: &[String], every: &[String]) -> Vec<timeline::Envelope> {
    let eases = [KeyEase::Step, KeyEase::Linear, KeyEase::EaseIn, KeyEase::EaseOut, KeyEase::Smooth];
    (0..rng.below(4))
        .map(|_| timeline::Envelope {
            target: if rng.chance(0.8) { rng.pick(targets).clone() } else { rng.pick(every).clone() },
            mode: if rng.chance(0.5) { timeline::EnvMode::Absolute } else { timeline::EnvMode::Relative },
            keys: (0..1 + rng.below(5))
                .map(|_| timeline::Key { at: rng.range(0.0, 10.0) as f64, value: rng.range(-400.0, 400.0) * if rng.chance(0.7) { 0.005 } else { 1.0 }, ease: *rng.pick(&eases) })
                .collect(),
        })
        .collect()
}

fn one_event_show(s: &Shared, rng: &mut Rng, cue: &str) -> timeline::Show {
    let (targets, every) = (modulatable(s), s.controls.list().iter().map(|d| d.id.clone()).collect::<Vec<_>>());
    let layer = 1 + rng.below(4) as u8;
    let event = timeline::Event {
        id: 1,
        start: rng.range(0.0, 2.0) as f64,
        len: rng.range(0.1, 30.0) as f64,
        source: timeline::EventSource::Cue { id: cue.into() },
        modifiers: if rng.chance(0.5) { mutated(&live::LiveModifiers::default(), rng, 0.2) } else { Default::default() },
        envelopes: if rng.chance(0.5) { random_envelopes(rng, &targets, &every) } else { Vec::new() },
        scan: random_scan_id(rng),
        ..Default::default()
    };
    let mut tracks = vec![timeline::Track { layer, events: vec![event], ..Default::default() }];
    if rng.chance(0.4) {
        // A bus on the same layer (or another one).
        let bus = timeline::Event { id: 2, start: rng.range(0.0, 2.0) as f64, len: rng.range(0.1, 30.0) as f64, envelopes: random_envelopes(rng, &targets, &every), ..Default::default() };
        let layer = if rng.chance(0.7) { layer } else { 1 + rng.below(4) as u8 };
        tracks.push(timeline::Track { kind: timeline::TrackKind::Bus, layer, events: vec![bus], ..Default::default() });
    }
    timeline::Show { name: "fuzz".into(), tracks, ..Default::default() }
}

/// A scan preset id (T-415): built-in, user, deleted, empty, or none.
fn random_scan_id(rng: &mut Rng) -> Option<String> {
    const IDS: [&str; 7] = ["beams", "graphics", "raster", "u-1", "u-2", "u-404", ""];
    rng.chance(0.6).then(|| rng.pick(&IDS).to_string())
}

/// Scan presets (T-415): user presets with any values (validated or not:
/// `scan::effective` must bound them either way), a preset on any cell,
/// the output's default.
fn random_scan(s: &mut Shared, rng: &mut Rng) {
    if rng.chance(0.5) {
        let base = crate::scan::ScanPreset { id: "u-1".into(), name: "Fuzz".into(), ..Default::default() };
        let mut user = vec![mutated(&base, rng, 0.5), crate::scan::ScanPreset { id: "u-2".into(), scan_pct: rng.range(-100.0, 400.0), ..mutated(&base, rng, 0.8) }];
        if rng.chance(0.5) {
            let _ = crate::scan::validate(&mut user);
        }
        s.scan_presets.replace_in_memory(user);
    }
    let (id, _) = random_preset(s, rng);
    let slot = CueSlot { scan: random_scan_id(rng), ..s.deck.slot(&id) };
    s.deck.set_slot(&id, slot);
    s.mixer.scan_default = random_scan_id(rng);
}

const PATTERNS: [testpattern::TestPattern; 7] = [
    testpattern::TestPattern::CrossFrame,
    testpattern::TestPattern::Grid,
    testpattern::TestPattern::CircleSquare,
    testpattern::TestPattern::ColorBars,
    testpattern::TestPattern::WhiteRamp,
    testpattern::TestPattern::ScanSpeed,
    testpattern::TestPattern::Blanking,
];

const ARM_SOURCES: [ArmSource; 3] = [ArmSource::Ui, ArmSource::Keyboard, ArmSource::Api];

/// A fresh studio, randomly configured: safety settings, caps,
/// calibration, a look, maybe presence with hold-to-run.
fn random_case(rng: &mut Rng) -> (Shared, bool) {
    let mut s = test_support::shared();
    s.safety.set(random_safety(rng), true).expect("valid random safety settings");
    s.outputs.set(MAIN_OUTPUT, Some(random_limits(rng)), None, false, true).expect("valid random caps");
    s.calibration = random_calibration(rng);
    let (_, look) = random_preset(&s, rng);
    s.settings = look;
    let hold_to_run = rng.chance(0.15);
    if hold_to_run || rng.chance(0.2) {
        s.presence = Presence::new(PresenceSettings { hold_to_run, ..Default::default() }, true);
        s.gate.register(interlock::UI_ALIVE, "Aucune interface ouverte", false);
    }
    (s, hold_to_run)
}

/// `POST /api/festival`, `/api/festival/palette` and a look in one of the
/// festival colour modes (T-130): any quantisation, any gains (out of
/// range ones too), any palette on the next boundary or at once.
fn random_festival(s: &mut Shared, rng: &mut Rng, t: f64) {
    let quantize = *rng.pick(&[PaletteQuantize::Now, PaletteQuantize::Bar, PaletteQuantize::Phrase]);
    let gains = [0, 1, 2].map(|_| rng.range(-0.5, 2.0));
    s.festival.configure(quantize, Balance { enabled: rng.chance(0.7), gains });
    let palette = rng.chance(0.8).then(|| rng.below(PALETTES.len() + 1));
    let (beat, bpb) = (s.tempo.beat_at(t), s.tempo.beats_per_bar);
    let _ = s.festival.request(palette, beat, bpb);
    if rng.chance(0.5) {
        let mut look = s.settings.clone();
        if let Content::Generator { params, .. } = &mut look.content {
            params.color_mode = *rng.pick(&[ColorMode::ToWhite, ColorMode::ColorChase]);
            params.period_beats = *rng.pick(&[0.0, 1.0, 4.0, 16.0]);
            params.steps_per_beat = *rng.pick(&[0.0, 0.5, 1.0, 4.0]);
        }
        controls::set_look(s, look);
    }
}

/// One random action between two frames. Returns true when it was an
/// explicit arm request (the only actions allowed to arm).
fn random_action(rig: &mut Rig, rng: &mut Rng, beats: &mut u64) -> bool {
    let (now, t) = (rig.now, rig.t());
    let mut s = rig.s();
    match rng.below(20) {
        0 => {
            let _ = s.request_arm(*rng.pick(&ARM_SOURCES));
            return true;
        }
        1 => s.gate.disarm(interlock::DisarmReason::User, ArmSource::Ui),
        2 if rng.chance(0.3) => s.emergency_stop(*rng.pick(&[ArmSource::Ui, ArmSource::Keyboard, ArmSource::Midi])),
        3 => {
            let estop = Arc::clone(&s.estop);
            s.gate.reset_estop(&estop);
        }
        4 | 5 => {
            // Any control, from a controller or the UI (the UI's arm toggle
            // is an explicit arm request).
            let ids: Vec<String> = s.controls.list().iter().map(|d| d.id.clone()).collect();
            let id = rng.pick(&ids).clone();
            let external = rng.chance(0.5);
            let input = if rng.chance(0.5) { ControlInput::Norm(rng.f()) } else { ControlInput::Value(rng.range(-3.0, 3.0)) };
            if external && rng.chance(0.3) {
                // DMX / OSC / timecode through the gate (T-408), which the
                // operator may have opened.
                if rng.chance(0.5) {
                    let _ = crate::external::set_open(&mut s, true, ArmSource::Ui);
                }
                let src = *rng.pick(&crate::external::SourceKind::ALL);
                let _ = crate::external::apply(&mut s, src, "rig", &id, input, now);
            } else {
                let _ = controls::apply(&mut s, &id, input, external);
            }
            return id == "transport.arm" && !external;
        }
        6 | 7 => {
            let (id, _) = random_preset(&s, rng);
            let mode = *rng.pick(&[None, Some(ClickMode::Flash), Some(ClickMode::Toggle), Some(ClickMode::Solo), Some(ClickMode::Restart)]);
            controls::press_cue(&mut s, &id, mode, rng.chance(0.7));
        }
        8 => {
            // `POST /api/settings` with a mangled look.
            let (_, look) = random_preset(&s, rng);
            controls::set_look(&mut s, mutated(&look, rng, 0.15));
        }
        9 => {
            // `POST /api/live`.
            s.live = mutated(&s.live.clone(), rng, 0.15);
            s.live_dirty = true;
        }
        10 => random_audio(&mut s, rng, now, t, beats),
        11 => {
            let list = random_lfos(&s, rng);
            s.lfos.replace_in_memory(list);
        }
        12 => {
            let routing = random_routes(&s, rng);
            let reg = controls::ControlRegistry::build(&s.presets);
            s.routes.replace_in_memory(routing, &reg);
        }
        13 if rng.chance(0.3) => {
            s.safety.set(random_safety(rng), true).expect("valid random safety settings");
        }
        14 if rng.chance(0.3) => {
            // Raising is refused while armed: that's the store's rule.
            let armed = s.gate.is_armed();
            let _ = s.outputs.set(MAIN_OUTPUT, Some(random_limits(rng)), None, armed, true);
        }
        15 if rng.chance(0.3) => s.calibration = random_calibration(rng),
        16 => {
            let p = if rng.chance(0.5) { Some(*rng.pick(&PATTERNS)) } else { None };
            s.test_pattern.brightness = rng.f();
            testpattern::select(&mut s, p);
        }
        17 => {
            let (id, _) = random_preset(&s, rng);
            let show = one_event_show(&s, rng, &id);
            s.timeline.load(show);
            if rng.chance(0.6) {
                let _ = controls::timeline_play(&mut s);
            } else {
                // Incoming timecode (T-168), TC-IN armed when allowed and
                // the gate open: it drives the timeline, never arming.
                use crate::timecode::{self, mtc, Rate, Tc};
                let _ = timecode::update(&mut s, &json!({ "source": "mtc", "offset_s": rng.range(-5.0, 5.0) as f64 }));
                let _ = crate::external::set_open(&mut s, true, ArmSource::Ui);
                let _ = timecode::set_armed(&mut s, true, ArmSource::Ui);
                let from = (rng.f() * 400.0) as i64;
                let events: Vec<MidiEvent> = (0..3)
                    .flat_map(|p| mtc::quarter_frames(Tc::from_frames(from + 2 * p, Rate::F25), Rate::F25))
                    .map(|data| MidiEvent { port: "MTC".into(), msg: MidiMsg::QuarterFrame { data }, at: now })
                    .collect();
                timecode::midi(&mut s, &events);
            }
        }
        18 => {
            let bpm = rng.range(40.0, 220.0) as f64;
            s.tempo.set_bpm_manual(bpm, t);
            s.mixer = mutated(&s.mixer.clone(), rng, 0.2);
            s.mixer_dirty = true;
            if rng.chance(0.5) {
                random_scan(&mut s, rng);
            }
        }
        _ => {
            if s.presence.enforced {
                s.presence.beat("page", now, true, false);
            }
        }
    }
    false
}

// ---------- the checks ----------

/// A Blank zone covering everything below the horizon, for the « full
/// horizon at level 0 » check.
fn below_horizon(h: &Horizon) -> Zone {
    let y = h.y - 1e-4;
    Zone { kind: ZoneKind::Blank, points: vec![[-3.0, -3.0], [3.0, -3.0], [3.0, y], [-3.0, y]], ..Default::default() }
}

/// Invariants 2, 3 and 5 on a frame (the one sent, or the preview's).
fn check_points(frame: &[Point], caps: &OutputLimits, cfg: &SafetySettings, what: &str) -> Result<(), String> {
    let caps = caps.sanitized();
    for (i, p) in frame.iter().enumerate() {
        if !(p.x.is_finite() && p.y.is_finite() && (-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y)) {
            return Err(format!("{what}: point {i} at ({}, {}) outside -1..1", p.x, p.y));
        }
        for (c, v) in [p.r, p.g, p.b].into_iter().enumerate() {
            if !v.is_finite() || v < 0.0 || v > caps.ceiling(c) + 1e-5 {
                return Err(format!("{what}: point {i} channel {c} = {v}, cap {}", caps.ceiling(c)));
            }
        }
    }
    let blank = zones::blank_violations(frame, &cfg.zones);
    if blank > 0 {
        return Err(format!("{what}: {blank} lit samples/segments in a Blank zone"));
    }
    if cfg.horizon.lines && cfg.horizon.level == 0.0 {
        let below = zones::blank_violations(frame, &[below_horizon(&cfg.horizon)]);
        if below > 0 {
            return Err(format!("{what}: {below} lit samples/segments below the horizon (y = {})", cfg.horizon.y));
        }
    }
    Ok(())
}

/// Invariants 8 and 9 on a frame. Audience scanning off: nothing lit in a
/// Public zone nor through one, whatever its kind. On: a lit sample in a
/// Public zone is at most the zone's level, raised by the minimum diode
/// level, under the caps' global power.
fn check_public(frame: &[Point], caps: &OutputLimits, cfg: &SafetySettings, scanning: bool, what: &str) -> Result<(), String> {
    if !scanning {
        let public: Vec<Zone> = cfg.zones.iter().filter(|z| z.audience).map(|z| Zone { kind: ZoneKind::Blank, ..z.clone() }).collect();
        let v = zones::blank_violations(frame, &public);
        if v > 0 {
            return Err(format!("{what}: {v} lit samples/segments in a Public zone, audience scanning off"));
        }
        return Ok(());
    }
    let caps = caps.sanitized();
    let min = cfg.min_diode_level.clamp(0.0, 0.5);
    for z in cfg.zones.iter().filter(|z| z.audience) {
        let area = zones::AudienceArea::new(std::slice::from_ref(z));
        let bound = (min + z.factor() * (1.0 - min)) * caps.max_power + 1e-4;
        for (i, p) in frame.iter().enumerate().filter(|(_, p)| p.is_lit() && area.covers(p.x, p.y)) {
            let peak = p.r.max(p.g).max(p.b);
            if peak > bound {
                return Err(format!("{what}: point {i} at ({}, {}) = {peak} in Public zone « {} » (level {}, bound {bound})", p.x, p.y, z.name, z.level));
            }
        }
    }
    Ok(())
}

/// Marks some of the current zones « Public » (T-255), sometimes adds a
/// Dim Public zone, as `POST /api/safety` could (Dim levels within the cap).
fn mark_public(s: &mut Shared, rng: &mut Rng) {
    let mut cfg = s.safety.get();
    if cfg.zones.len() < zones::MAX_ZONES && (cfg.zones.is_empty() || rng.chance(0.4)) {
        let id = cfg.zones.iter().map(|z| z.id).max().unwrap_or(0) + 1;
        cfg.zones.push(Zone { id, name: format!("Public {id}"), kind: ZoneKind::Dim, level: rng.range(0.0, audience::MAX_AUDIENCE_LEVEL), points: random_polygon(rng), audience: true });
    }
    for z in &mut cfg.zones {
        if rng.chance(0.5) {
            z.audience = true;
        }
        if z.audience {
            z.level = z.level.min(audience::MAX_AUDIENCE_LEVEL);
        }
    }
    s.safety.set(cfg, true).expect("valid Public zones");
}

/// Every condition of the audience scanning unlock met (T-255): a Dim
/// Public zone, the strict guard, a complete sheet with a hardware
/// scan-fail, the checklist with its audience items, hold-to-run.
fn prepare_scanning(s: &mut Shared, rng: &mut Rng) {
    mark_public(s, rng);
    let mut cfg = s.safety.get();
    if !cfg.zones.iter().any(|z| z.audience && z.kind == ZoneKind::Dim) {
        // mark_public leaves at least one zone: make one a Dim Public zone.
        let i = cfg.zones.iter().position(|z| z.audience).unwrap_or(0);
        let z = &mut cfg.zones[i];
        z.audience = true;
        z.kind = ZoneKind::Dim;
        z.level = z.level.min(audience::MAX_AUDIENCE_LEVEL);
    }
    cfg.dwell.profile = dwell::DwellProfile::Strict;
    s.safety.set(cfg, true).expect("valid settings");
    let sheet = crate::power::ProjectorInfo { name: "P".into(), power_mw: [100.0; 3], divergence_mrad: 2.0, hw_scan_fail: Some(true), ..Default::default() };
    s.outputs.set(MAIN_OUTPUT, None, Some(sheet), false, true).expect("valid sheet");
    let mut list = s.checklist.config().clone();
    list.audience_scan = true;
    let armed = s.gate.is_armed();
    if s.checklist.set_config(list, armed).is_ok() {
        let _ = s.checklist.set_operator("fuzz");
        let ids: Vec<String> = s.checklist.visible_items().map(|i| i.id.clone()).collect();
        for id in ids {
            let _ = s.checklist.check(&id, true, Some("1"));
        }
        let _ = s.checklist.complete();
    }
    s.presence.settings.hold_to_run = true;
}

/// How much light the fuzz really exercised, so a generator that stops
/// lighting anything can't make the checks pass vacuously.
#[derive(Default)]
struct Coverage {
    frames: usize,
    /// Frames that sent lit points to the output.
    sent_lit: usize,
    /// Frames with a lit preview while at least one Blank zone was set.
    lit_with_zones: usize,
    /// Frames with a lit preview below a full horizon at level 0.
    lit_with_horizon: usize,
    /// Frames with a lit preview through a non-neutral projection zone.
    lit_with_projection: usize,
    /// Frames with a lit preview while a DMX fixture instance shaped a zone (T-407).
    lit_with_remote: usize,
    /// Frames with a lit preview and a Public zone, scanning off / on.
    lit_with_public_above: usize,
    lit_with_public_scanning: usize,
    /// Frames with a lit preview while a transition between cues ran (T-158).
    lit_with_transition: usize,
    /// Audio trigger actions taken (T-240).
    trigger_actions: u64,
    /// Frames with a lit preview through a live effect (T-142): strobe,
    /// visible points, dotted lines, mirror, prism or « Figer ».
    lit_with_effects: usize,
}

/// The live effects of T-142 as a controller would drive them: latched
/// effects set, momentary ones pressed and released.
fn random_live_effect(s: &mut Shared, rng: &mut Rng) {
    let ids = [
        "master.strobe.on",
        "master.strobe.hold",
        "master.strobe.rate",
        "master.strobe.sync",
        "master.strobe.duty",
        "master.trace",
        "master.dots",
        "master.mirror",
        "master.prism",
        "transport.freeze",
        "transport.black_hold",
    ];
    let id = *rng.pick(&ids);
    let input = if rng.chance(0.8) { ControlInput::Norm(rng.f()) } else { ControlInput::Value(rng.range(-2.0, 30.0)) };
    let _ = controls::apply(s, id, input, rng.chance(0.5));
}

/// One DMX fixture instance per zone (T-407), random personalities and
/// addresses on universe 1, sACN on (in memory), the gate open on every
/// group. What the instances do comes from `random_universe`.
fn setup_fixtures(s: &mut Shared, rng: &mut Rng) {
    use crate::dmx::{fixture::DmxFixture, personality::Personality};
    let mut cfg = s.netin.hub.file();
    cfg.sacn.enabled = true;
    let mut address = 1;
    for z in s.projection.get().zones.clone() {
        let mode = *rng.pick(&Personality::ALL);
        if address + mode.width() > 513 {
            break;
        }
        cfg.dmx.fixtures.push(DmxFixture { enabled: true, zone: z.id, mode, universe: 1, address, smoothing_ms: rng.below(200) as u16 });
        address += mode.width() + rng.below(8) as u16;
    }
    s.netin.hub.store(cfg).expect("valid fixtures");
    let groups: Vec<String> = crate::external::SCOPE_SETS.iter().flat_map(|set| set.groups.iter().map(|g| g.to_string())).collect();
    let scopes = crate::external::SourceKind::ALL.iter().map(|&source| crate::external::SourceScope { source, groups: groups.clone(), require_access: true }).collect();
    s.external.set_config(crate::external::ExternalConfig { scopes });
    let _ = crate::external::set_open(s, true, ArmSource::Ui);
}

/// A universe as a console could send it: random levels, access mostly
/// raised, the function channels mostly at « rien ».
fn random_universe(s: &mut Shared, rng: &mut Rng, frame: usize) {
    let mut data: Vec<u8> = (0..512).map(|_| (rng.next() & 0xFF) as u8).collect();
    for f in s.netin.hub.file().dmx.fixtures {
        let at = |key: &str| f.mode.channels().iter().find(|c| c.key == key).map(|c| f.address as usize - 1 + c.offset as usize);
        if let Some(i) = at("access") {
            data[i] = if rng.chance(0.85) { 255 } else { 0 };
        }
        if let Some(i) = at("function").filter(|_| rng.chance(0.97)) {
            data[i] = 0;
        }
        if let Some(i) = at("intensity").filter(|_| rng.chance(0.5)) {
            data[i] = 255;
        }
    }
    let ev = crate::netin::patch::UniverseEvent { proto: crate::netin::Proto::Sacn, universe: 1, from: "127.0.0.1".parse().unwrap(), name: format!("fuzz {frame}"), data };
    let _ = s.netin.sender().try_send(crate::netin::NetEvent::Universe(ev));
}

/// Runs one fuzz case; `Err` describes the first broken invariant.
/// Transitions, quantised launch and beat mode (T-158 / T-159) as the
/// operator could set them, with a cell's own transition sometimes.
fn random_transitions(s: &mut Shared, rng: &mut Rng) {
    let random = |rng: &mut Rng| Transition {
        kind: *rng.pick(&TRANSITION_KINDS),
        length: if rng.chance(0.7) { *rng.pick(&LENGTHS) } else { TransitionLength::Seconds(rng.range(0.0, 0.6)) },
        in_flash: rng.chance(0.5),
    };
    s.deck.transition = random(rng);
    s.deck.quantize = *rng.pick(&QUANTIZE_MODES);
    s.deck.beat_every = if rng.chance(0.3) { *rng.pick(&BEAT_EVERY) } else { 0 };
    s.deck.multi = rng.chance(0.5);
    if rng.chance(0.4) {
        let (id, _) = random_preset(s, rng);
        let slot = CueSlot { transition: Some(random(rng)), ..s.deck.slot(&id) };
        s.deck.set_slot(&id, slot);
    }
}

fn run_case(seed: u64, frames: usize, cov: &mut Coverage) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let (s, hold_to_run) = random_case(&mut rng);
    // One case in three on an output with a small frame size limit, taken
    // from the seed so the case's own random stream is unchanged.
    let max_points = seed.is_multiple_of(3).then(|| 16 + (seed as usize).wrapping_mul(7919) % 600);
    let mut rig = Rig::with_max_points(s, "pipeline", seed, max_points);
    // Projection zones and routing (T-012) from their own stream, so the
    // case's main stream (and older replays) stays as it was.
    let mut zrng = Rng::new(seed ^ 0x0012_0012);
    if zrng.chance(0.7) {
        let _ = rig.s().projection.set(random_projection(&mut zrng));
    }
    // The output's colour rendering (T-414), from its own stream too;
    // brighter settings are confirmed, as the operator could (refused
    // while armed: that's the store's rule).
    let mut crng = Rng::new(seed ^ 0x0414_0414);
    if crng.chance(0.6) {
        let _ = rig.s().outputs.set_color(MAIN_OUTPUT, random_color(&mut crng), false, true);
    }
    // Public zones and audience scanning (T-255), also from their own stream.
    let mut arng = Rng::new(seed ^ 0x0255_0255);
    if arng.chance(0.6) {
        mark_public(&mut rig.s(), &mut arng);
    }
    if arng.chance(0.4) {
        let mut s = rig.s();
        prepare_scanning(&mut s, &mut arng);
        s.unlock_audience_scanning(audience::PHRASE, ArmSource::Ui).map_err(|e| format!("setup: unlock refused: {e:?}"))?;
    }
    // DMX fixture instances on the zones (T-407), from their own stream:
    // a console sending random universes, the gate opened.
    let mut drng = Rng::new(seed ^ 0x0407_0407);
    let fixtures = drng.chance(0.4);
    if fixtures {
        setup_fixtures(&mut rig.s(), &mut drng);
    }
    // Transitions and quantised launches (T-158 / T-159), from their own stream.
    let mut trng = Rng::new(seed ^ 0x0158_0158);
    if trng.chance(0.6) {
        random_transitions(&mut rig.s(), &mut trng);
    }
    // Audio triggers (T-240), from their own stream: rules on random cues,
    // the switch flipped from the UI or a controller now and then.
    let mut grng = Rng::new(seed ^ 0x0240_0240);
    if grng.chance(0.5) {
        let mut s = rig.s();
        let cfg = random_triggers(&s, &mut grng);
        crate::audio::triggers::set_config(&mut s, cfg).map_err(|e| format!("setup: triggers refused: {e:#}"))?;
        let _ = crate::audio::triggers::set_enabled(&mut s, grng.chance(0.8), ArmSource::Ui);
    }
    // A cue list (T-410), from its own stream: loaded, driven now and then.
    let mut lrng = Rng::new(seed ^ 0x0410_0410);
    let lists = lrng.chance(0.5);
    if lists {
        let mut s = rig.s();
        let list = random_cue_list(&s, &mut lrng);
        crate::cuelist::save(&mut s, list).map_err(|e| format!("setup: cue list refused: {e:#}"))?;
        crate::cuelist::load(&mut s, "fuzz").map_err(|e| format!("setup: {e}"))?;
    }
    // Festival palettes, colour modes and perceived balance (T-130), from
    // their own stream.
    let mut frng = Rng::new(seed ^ 0x0130_0130);
    // The output's scan limits and the live scan speed (T-171), from
    // their own stream: frames completed or thinned before the safety
    // stage, presets traced faster or slower; 1-14 hold with them.
    let mut srng = Rng::new(seed ^ 0x0171_0171);
    if srng.chance(0.6) {
        let mut s = rig.s();
        let _ = s.outputs.set_scan(MAIN_OUTPUT, random_output_scan(&mut srng), false);
        let _ = controls::apply(&mut s, "master.scan", ControlInput::Norm(srng.f()), srng.chance(0.5));
    }
    // Live effects (T-142), from their own stream.
    let mut erng = Rng::new(seed ^ 0x0142_0142);
    let effects = erng.chance(0.25);
    let mut beats = 0;
    for frame in 0..frames {
        {
            let mut s = rig.s();
            let armed = s.gate.is_armed();
            if effects && erng.chance(0.3) {
                random_live_effect(&mut s, &mut erng);
            }
            // Held buttons are let go of: « Noir » and « Figer » don't
            // stay down for the whole session.
            if (s.live.black || s.live.freeze || s.live.strobe.hold) && erng.chance(0.7) {
                let id = *erng.pick(&["transport.black_hold", "transport.freeze", "master.strobe.hold"]);
                let _ = controls::apply(&mut s, id, ControlInput::Value(0.0), erng.chance(0.5));
            }
            if s.gate.is_armed() != armed {
                return Err(format!("frame {frame}: a live effect changed the arming"));
            }
        }
        if lists && lrng.chance(0.08) {
            let mut s = rig.s();
            let src = *lrng.pick(&[controls::ControlSource::Ui, controls::ControlSource::Midi, controls::ControlSource::Keyboard]);
            let was_paused = s.cue_list.paused;
            let estop = s.estop.is_latched();
            let _ = match lrng.below(5) {
                0 | 1 => crate::cuelist::go(&mut s, src),
                2 => crate::cuelist::back(&mut s, src),
                3 => crate::cuelist::pause(&mut s, src),
                _ => {
                    let n = lrng.below(8);
                    crate::cuelist::goto(&mut s, n, src)
                }
            };
            if estop && was_paused && !s.cue_list.paused {
                return Err(format!("frame {frame}: the cue list resumed while the e-stop is latched"));
            }
        }
        if lists {
            let s = rig.s();
            if s.estop.is_latched() && s.cue_list.is_active() && !s.cue_list.paused {
                // The engine pauses it at the top of the next frame; it must
                // never stay running across one.
                drop(s);
                rig.tick();
                let s = rig.s();
                if s.estop.is_latched() && s.cue_list.is_active() && !s.cue_list.paused {
                    return Err(format!("frame {frame}: the cue list keeps running with the e-stop latched"));
                }
            }
        }
        if grng.chance(0.1) {
            let mut s = rig.s();
            let on = grng.chance(0.7);
            let id = "audio.triggers.enabled";
            let _ = if grng.chance(0.5) { controls::apply(&mut s, id, ControlInput::Value(on as u8 as f32), true) } else { crate::audio::triggers::set_enabled(&mut s, on, ArmSource::Ui).map_err(controls::ControlError::Refused) };
        }
        if trng.chance(0.05) {
            random_transitions(&mut rig.s(), &mut trng);
        }
        if zrng.chance(0.15) {
            random_routing(&mut rig.s(), &mut zrng);
        }
        if fixtures && drng.chance(0.5) {
            random_universe(&mut rig.s(), &mut drng, frame);
        }
        if frng.chance(0.1) {
            let t = rig.t();
            random_festival(&mut rig.s(), &mut frng, t);
        }
        if crng.chance(0.05) {
            let mut s = rig.s();
            let armed = s.gate.is_armed();
            let _ = s.outputs.set_color(MAIN_OUTPUT, random_color(&mut crng), armed, true);
        }
        if arng.chance(0.08) {
            let mut s = rig.s();
            match arng.below(4) {
                0 => mark_public(&mut s, &mut arng),
                1 => prepare_scanning(&mut s, &mut arng),
                2 => s.end_audience_scanning("user", ArmSource::Ui, &[]),
                _ => {
                    // The unlock, with the right phrase or not: refused while
                    // armed or with a condition missing, and it never arms.
                    let (was_armed, was_scanning) = (s.gate.is_armed(), s.audience.is_scanning());
                    let phrase = if arng.chance(0.7) { audience::PHRASE } else { "je prends" };
                    let r = s.unlock_audience_scanning(phrase, ArmSource::Ui);
                    if s.gate.is_armed() != was_armed {
                        return Err(format!("frame {frame}: the audience unlock changed the arming"));
                    }
                    if r.is_err() && s.audience.is_scanning() != was_scanning {
                        return Err(format!("frame {frame}: a refused unlock changed the audience mode"));
                    }
                    if r.is_ok() && was_armed {
                        return Err(format!("frame {frame}: audience scanning unlocked while armed"));
                    }
                }
            }
        }
        for _ in 0..rng.below(3) {
            let (was, was_scanning) = {
                let s = rig.s();
                (s.gate.is_armed(), s.audience.is_scanning())
            };
            let explicit = random_action(&mut rig, &mut rng, &mut beats);
            let is = rig.s().gate.is_armed();
            if is && !was && !explicit {
                return Err(format!("frame {frame}: armed by a non-arming action"));
            }
            if rig.s().audience.is_scanning() && !was_scanning {
                return Err(format!("frame {frame}: audience scanning started by an action"));
            }
        }
        // The fuzz's pages never hold the hold-to-run key; the controller
        // pad (`safety.hold`) may be pressed by a random control.
        let (may_emit, caps, held, scanning_before) = {
            let s = rig.s();
            (s.gate.is_armed() && !s.estop.is_latched(), s.outputs.active_limits(), s.presence.status().midi_hold, s.audience.is_scanning())
        };
        let armed_before = rig.s().gate.is_armed();
        let (black_held, with_effects) = {
            let s = rig.s();
            let m = &s.live;
            (m.black, m.strobe.active() || m.trace < 1.0 || m.dots > 0.0 || m.mirror != live::Mirror::None || m.prism > 1 || m.freeze)
        };
        let (fired_before, triggers_may_act) = {
            let s = rig.s();
            if s.estop.is_latched() && s.triggers.enabled() {
                return Err(format!("frame {frame}: audio automation on with the e-stop latched"));
            }
            (s.triggers.engine.fired_total(), s.triggers.enabled() && !s.estop.is_latched())
        };
        rig.tick();
        if rig.s().gate.is_armed() && !armed_before {
            return Err(format!("frame {frame}: the engine frame armed the laser"));
        }
        {
            let s = rig.s();
            let fired = s.triggers.engine.fired_total() - fired_before;
            if fired > 0 && !triggers_may_act {
                return Err(format!("frame {frame}: {fired} audio trigger action(s) with the automation off or the e-stop latched"));
            }
            if s.estop.is_latched() && s.triggers.enabled() {
                return Err(format!("frame {frame}: audio automation still on after an e-stop"));
            }
            cov.trigger_actions += fired;
        }
        // The frame ran in the mode it ends in: the engine only ever ends
        // audience scanning, at its top (invariants 8 to 10).
        let (scanning, cfg) = {
            let s = rig.s();
            if s.audience.is_scanning() && !scanning_before {
                return Err(format!("frame {frame}: the engine frame started audience scanning"));
            }
            if s.audience.is_scanning() && s.estop.is_latched() {
                return Err(format!("frame {frame}: audience scanning with the e-stop latched"));
            }
            // The saved settings: the checks don't trust `audience::effective`.
            (s.audience.is_scanning(), s.safety.get())
        };
        let limiter_holding = rig.s().strobe.active;
        let w = rig.wire.lock().unwrap();
        let lit = w.last.iter().filter(|p| p.is_lit()).count();
        if lit > 0 && !may_emit {
            return Err(format!("frame {frame}: {lit} lit points sent while disarmed or e-stopped"));
        }
        if lit > 0 && !w.armed {
            return Err(format!("frame {frame}: {lit} lit points sent to a disarmed output"));
        }
        if lit > 0 && hold_to_run && !held {
            return Err(format!("frame {frame}: {lit} lit points with hold-to-run not held"));
        }
        // « Noir » held (T-142): a dark output, whatever plays - except
        // while the strobe limiter holds steady output, which fills a dip
        // shorter than one allowed flash period with the last lit frame
        // (so « Noir » hammered on a pad can't make a strobe either).
        if lit > 0 && black_held && !limiter_holding {
            return Err(format!("frame {frame}: {lit} lit points sent while « Noir » is held"));
        }
        check_points(&w.last, &caps, &cfg, &format!("frame {frame}, sent"))?;
        check_public(&w.last, &caps, &cfg, scanning, &format!("frame {frame}, sent"))?;
        if let Some(max) = max_points.filter(|&m| w.last.len() > m) {
            return Err(format!("frame {frame}: {} points sent to an output that takes {max}", w.last.len()));
        }
        drop(w);
        let t = rig.t();
        let s = rig.s();
        check_points(&s.frame, &caps, &cfg, &format!("frame {frame}, preview"))?;
        check_public(&s.frame, &caps, &cfg, scanning, &format!("frame {frame}, preview"))?;
        let preview_lit = s.frame.iter().any(|p| p.is_lit());
        let public = cfg.zones.iter().any(|z| z.audience);
        cov.lit_with_public_above += (preview_lit && public && !scanning) as usize;
        cov.lit_with_public_scanning += (preview_lit && public && scanning) as usize;
        cov.frames += 1;
        cov.sent_lit += (lit > 0) as usize;
        cov.lit_with_zones += (preview_lit && cfg.zones.iter().any(|z| z.kind == ZoneKind::Blank)) as usize;
        cov.lit_with_horizon += (preview_lit && cfg.horizon.lines && cfg.horizon.level == 0.0) as usize;
        cov.lit_with_projection += (preview_lit && s.projection.get().zones.iter().any(|z| !z.geometry.is_identity())) as usize;
        cov.lit_with_remote += (preview_lit && !s.netin.fixtures.remotes.is_empty()) as usize;
        cov.lit_with_transition += (preview_lit && !s.deck.transition_fx(t).is_empty()) as usize;
        cov.lit_with_effects += (preview_lit && with_effects) as usize;
        if black_held && preview_lit && !limiter_holding {
            return Err(format!("frame {frame}: the preview is lit while « Noir » is held"));
        }
        if s.estop.is_latched() && (!s.deck.ending.is_empty() || !s.deck.pending.is_empty()) {
            return Err(format!("frame {frame}: a transition or a waiting launch survived the e-stop"));
        }
    }
    Ok(())
}

/// T-358: a property's time limit: 3 minutes at its default case count,
/// more when `SAFETY_CASES` asks for more cases. A hang fails fast instead
/// of running for hours.
fn property_guard(name: &str, cases: usize, default_cases: usize) -> test_support::TimeGuard {
    let runs = cases.div_ceil(default_cases.max(1)).max(1) as u32;
    test_support::time_guard(name, std::time::Duration::from_secs(180) * runs)
}

fn run_property(name: &str, property: u64, cases: usize, case: impl Fn(u64) -> Result<(), String>) {
    let seeds = seeds(property, cases);
    let _guard = property_guard(name, seeds.len(), cases);
    for seed in seeds {
        if let Err(e) = case(seed) {
            panic!("{name}: {e} (replay: SAFETY_SEED={seed} cargo test -p laser-studio {name})");
        }
    }
}

/// Invariants 1, 2, 3, 5, 6 and 8 to 10 over random sessions.
#[test]
fn random_sessions_keep_every_output_invariant() {
    let name = "random_sessions_keep_every_output_invariant";
    let mut cov = Coverage::default();
    let seeds = seeds(1, 1000);
    let _guard = property_guard(name, seeds.len(), 1000);
    for &seed in &seeds {
        if let Err(e) = run_case(seed, 30, &mut cov) {
            panic!("{name}: {e} (replay: SAFETY_SEED={seed} cargo test -p laser-studio {name})");
        }
    }
    if seeds.len() > 100 {
        let share = |n: usize| n as f64 / cov.frames as f64;
        eprintln!("coverage: Public zones lit {:.3} above, {:.3} scanning; transitions {:.3}; sent lit {:.3}, effects {:.3}", share(cov.lit_with_public_above), share(cov.lit_with_public_scanning), share(cov.lit_with_transition), share(cov.sent_lit), share(cov.lit_with_effects));
        assert!(share(cov.sent_lit) > 0.1, "only {} of {} frames sent light", cov.sent_lit, cov.frames);
        assert!(share(cov.lit_with_zones) > 0.1, "only {} of {} lit frames had a Blank zone", cov.lit_with_zones, cov.frames);
        assert!(share(cov.lit_with_horizon) > 0.05, "only {} of {} lit frames had a full horizon", cov.lit_with_horizon, cov.frames);
        assert!(share(cov.lit_with_projection) > 0.2, "only {} of {} lit frames went through a projection zone", cov.lit_with_projection, cov.frames);
        assert!(share(cov.lit_with_remote) > 0.02, "only {} of {} lit frames went through a DMX fixture instance", cov.lit_with_remote, cov.frames);
        assert!(share(cov.lit_with_public_above) > 0.1, "only {} of {} lit frames had a Public zone, scanning off", cov.lit_with_public_above, cov.frames);
        assert!(share(cov.lit_with_public_scanning) > 0.05, "only {} of {} lit frames had a Public zone while scanning", cov.lit_with_public_scanning, cov.frames);
        assert!(share(cov.lit_with_transition) > 0.01, "only {} of {} lit frames ran a transition", cov.lit_with_transition, cov.frames);
        assert!(share(cov.lit_with_effects) > 0.05, "only {} of {} lit frames went through a live effect", cov.lit_with_effects, cov.frames);
        assert!(cov.trigger_actions > seeds.len() as u64 / 10,"only {} audio trigger actions in {} cases", cov.trigger_actions, seeds.len());
    }
}

// ---------- ShowNET SD card over Art-Net (T-301) ----------

/// Random sessions with the engine believing its output is the Art-Net
/// one: the DMX values it would send (`artnet::look_for`, from what the
/// stage really sent) are dark whenever the stage is disarmed, e-stopped
/// or sends no light, never brighter than the caps allow, and pattern 0
/// always comes with intensity 0.
fn run_artnet_case(seed: u64) -> Result<(), String> {
    use crate::artnet::{look_for, DmxLook};
    let mut rng = Rng::new(seed);
    let (mut s, _) = random_case(&mut rng);
    // Half the cues in the SD bank, the rest without a number.
    let slots = s.presets.iter().step_by(2).take(sdbank_len()).enumerate().map(|(i, p)| crate::sdbank::SdSlot { number: i as u8 + 1, cue_id: p.id.clone(), seconds: 1.0 }).collect();
    s.sdbank.set(crate::sdbank::SdBank { slots }).map_err(|e| format!("setup: {e:#}"))?;
    s.output_kind = Some(crate::artnet::KIND);
    // A ShowNET output: its hardware stop item, ticked by the operator.
    s.sync_checklist();
    s.checklist.check(crate::checklist::HW_STOP_ITEM, true, None).map_err(|e| format!("setup: {e:#}"))?;
    s.sync_checklist();
    let mut rig = Rig::new(s, "artnet", seed);
    let (mut beats, mut lit_dmx) = (0, 0);
    for frame in 0..60 {
        for _ in 0..rng.below(3) {
            random_action(&mut rig, &mut rng, &mut beats);
        }
        let (may_emit, caps) = {
            let s = rig.s();
            (s.gate.is_armed() && !s.estop.is_latched(), s.outputs.active_limits())
        };
        rig.tick();
        let w = rig.wire.lock().unwrap();
        let signal = w.signal.clone().ok_or_else(|| format!("frame {frame}: no cue signal for the Art-Net output"))?;
        let lit = w.last.iter().any(|p| p.is_lit());
        let look = look_for(w.armed, lit, true, &signal, rng.chance(0.5));
        if look != DmxLook::DARK && !may_emit {
            return Err(format!("frame {frame}: Art-Net {look:?} while disarmed or e-stopped"));
        }
        if look != DmxLook::DARK && !lit {
            return Err(format!("frame {frame}: Art-Net {look:?} for a dark frame"));
        }
        if (look.intensity == 0) != (look.pattern == 0) {
            return Err(format!("frame {frame}: Art-Net {look:?}: intensity 0 must mean pattern 0"));
        }
        let l = caps.sanitized();
        let bound = 255.0 * l.max_power * l.max_color.iter().copied().fold(1.0f32, f32::min);
        if look.intensity as f32 > bound + 1e-3 {
            return Err(format!("frame {frame}: Art-Net intensity {} above the caps ({bound:.1})", look.intensity));
        }
        if look.pattern != 0 && rig.s().sdbank.get().slots.iter().all(|sl| sl.number != look.pattern) {
            return Err(format!("frame {frame}: pattern {} is not in the SD bank", look.pattern));
        }
        lit_dmx += (look != DmxLook::DARK) as usize;
    }
    LIT_DMX.fetch_add(lit_dmx, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// Frames that played a bank cue lit, over all cases (coverage).
static LIT_DMX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn sdbank_len() -> usize {
    (crate::sdbank::LAST - crate::sdbank::FIRST + 1) as usize
}

#[test]
fn artnet_dmx_is_dark_whenever_the_stage_is_and_within_the_caps() {
    let name = "artnet_dmx_is_dark_whenever_the_stage_is_and_within_the_caps";
    run_property(name, 8, 300, run_artnet_case);
    // Coverage at the default case count: bank cues really went out lit.
    if env_u64("SAFETY_SEED").is_none() && env_u64("SAFETY_CASES").is_none() {
        let lit = LIT_DMX.load(std::sync::atomic::Ordering::Relaxed);
        assert!(lit > 50, "only {lit} frames played a bank cue lit");
    }
}

// ---------- strobe ----------

/// Flash onsets in the output, measured like the limiter does
/// (`safety.rs`: mean drive level, hysteresis against a decaying peak).
struct FlashMeter {
    peak: f32,
    on: bool,
    onsets: Vec<f64>,
}

impl FlashMeter {
    fn feed(&mut self, frame: &[Point], t: f64, dt: f32) {
        let e = safety::level(frame);
        self.peak = e.max(self.peak * (-dt).exp());
        let bright = e > 1e-3 && e >= 0.6 * self.peak;
        if !self.on && bright {
            self.on = true;
            self.onsets.push(t);
        } else if self.on && (e <= 1e-3 || e <= 0.3 * self.peak) {
            self.on = false;
        }
    }

    /// Longest stretch of output flashes all closer together than one
    /// allowed period (less the limiter's timing slack), in seconds.
    fn longest_fast_run(&self, period: f64) -> f64 {
        let (mut best, mut start) = (0.0f64, None::<f64>);
        for w in self.onsets.windows(2) {
            if w[1] - w[0] < period - 0.034 {
                let s = *start.get_or_insert(w[0]);
                best = best.max(w[1] - s);
            } else {
                start = None;
            }
        }
        best
    }
}

/// One strobe case: a look flashing fast by one of the ways the studio
/// has (the look's beat gate, a square LFO on a brightness, a controller
/// fader pumped every frame, a flash cue hammered), armed, for 9 s.
fn run_strobe_case(seed: u64) -> Result<(), String> {
    strobe_case(seed, None)
}

/// A catalogue cue a strobe case is forced onto (T-360).
#[derive(Clone, Copy)]
struct Forced<'a> {
    cue: &'a str,
    /// Flashed over itself (way 3) rather than the seed's way.
    self_flash: bool,
}

/// `run_strobe_case`, or with `forced` that cue as the look (and the
/// flash cue), for the burst plus 2.5 s. The seed still draws the
/// limits, guard profile, tempo, rate and way.
fn strobe_case(seed: u64, forced: Option<Forced>) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut s = test_support::shared();
    let mut cfg = SafetySettings { strobe_max_hz: rng.range(1.0, 4.0), strobe_burst_s: rng.range(0.5, 3.0), ..Default::default() };
    if rng.chance(0.5) {
        cfg.dwell.profile = dwell::DwellProfile::Strict;
    }
    s.safety.set(cfg.clone(), true).map_err(|e| format!("{e:?}"))?;
    let presets: Vec<_> = s.presets.iter().filter(|p| p.settings.gate == crate::engine::GateMode::Off).map(|p| (p.id.clone(), p.settings.clone())).collect();
    let (mut cue, look) = rng.pick(&presets).clone();
    s.settings = look;
    let bpm = rng.range(90.0, 200.0) as f64;
    s.tempo.set_bpm_manual(bpm, 0.0);
    let hz = rng.range(6.0, 15.0);
    let mut way = rng.below(5);
    if let Some(f) = forced {
        // The look is the cue too: flashed over itself, every release
        // falls back on it and every press restarts it.
        let p = s.presets.iter().find(|p| p.id == f.cue).ok_or_else(|| format!("no cue {}", f.cue))?;
        (cue, s.settings) = (p.id.clone(), p.settings.clone());
        if f.self_flash {
            way = 3;
        }
    }
    match way {
        // The live strobe (T-142), free or synced: it runs before the
        // limiter and must not get past it.
        4 => {
            let _ = controls::apply(&mut s, "master.strobe.on", ControlInput::Value(1.0), true);
            let _ = controls::apply(&mut s, "master.strobe.rate", ControlInput::Value(hz), true);
            if rng.chance(0.5) {
                let sync = 1.0 + rng.below(5) as f32;
                let _ = controls::apply(&mut s, "master.strobe.sync", ControlInput::Value(sync), true);
            }
            let _ = controls::apply(&mut s, "master.strobe.duty", ControlInput::Value(rng.range(0.1, 0.9)), true);
        }
        0 => {
            s.settings.gate = crate::engine::GateMode::BeatAndOffbeat;
            s.settings.gate_beats = 0.1;
        }
        1 => {
            let target = *rng.pick(&["master.brightness", "look.brightness"]);
            s.lfos.replace_in_memory(vec![lfo::Modulator { target: target.into(), wave: lfo::Wave::Square, rate: live::Rate::Hz(hz), depth: 1.0, offset: 0.0, ..Default::default() }]);
        }
        _ => {}
    }
    s.request_arm(ArmSource::Ui).map_err(|e| e.join("; "))?;
    let mut rig = Rig::new(s, &format!("strobe-{}", forced.map_or("any", |f| f.cue)), seed);
    // Frames from the studio's epoch, not from whenever the rig was built
    // (T-360): the beat clock's phase against the frames, and so the
    // outcome of a seed, must not depend on how loaded the machine is.
    let epoch = rig.s().epoch;
    rig.now = epoch;
    let mut meter = FlashMeter { peak: 0.0, on: false, onsets: Vec::new() };
    // Past the burst, the hold and a little after it is where a stage
    // after the limiter would show.
    let secs = forced.map_or(9.0, |_| cfg.strobe_burst_s as f64 + 2.5);
    let frames = (secs * 60.0) as usize;
    for i in 0..frames {
        let t = i as f64 / 60.0;
        let on = (t * hz as f64).fract() < 0.5;
        {
            let mut s = rig.s();
            match way {
                2 => {
                    let _ = controls::apply(&mut s, "master.brightness", ControlInput::Norm(if on { 1.0 } else { 0.0 }), true);
                }
                3 => {
                    controls::press_cue(&mut s, &cue, Some(ClickMode::Flash), on);
                }
                _ => {}
            }
        }
        rig.tick();
        let w = rig.wire.lock().unwrap();
        meter.feed(&w.last, rig.now.saturating_duration_since(rig.s().epoch).as_secs_f64(), 1.0 / 60.0);
    }
    let period = 1.0 / cfg.strobe_max_hz as f64;
    let run = meter.longest_fast_run(period);
    // The burst, plus a frame or two of the limiter's own timing.
    if run > cfg.strobe_burst_s as f64 + 0.1 {
        return Err(format!(
            "way {way}, cue {cue}: {:.2} s of output flashing faster than {:.2} Hz (burst limit {:.2} s, {} flashes in {secs:.1} s)",
            run,
            cfg.strobe_max_hz,
            cfg.strobe_burst_s,
            meter.onsets.len()
        ));
    }
    Ok(())
}

/// The strobe limiter, measured on what leaves for the laser.
#[test]
fn strobe_never_exceeds_the_burst_limit() {
    run_property("strobe_never_exceeds_the_burst_limit", 2, STROBE_SEEDS, run_strobe_case);
}

/// T-360: every catalogue cue strobed through the real engine, on the
/// default seeds of `strobe_never_exceeds_the_burst_limit` (limits, guard
/// profile, tempo, rate). Whatever a stage after the strobe limiter does
/// with the cue (the static-beam guard blanking a lone beam, the caps,
/// the hold itself), the output must not flash faster than the limit for
/// longer than the burst.
///
/// A plain `cargo test` runs each cue on `per_cue` of the seeds, in turn,
/// so the catalogue covers all of them; a deep run (`SAFETY_CASES` set)
/// runs every cue on every default seed. `SAFETY_SEED` replays one seed
/// and `SAFETY_CUE` one cue.
fn sweep_every_cue(name: &str, self_flash: bool, per_cue: usize) {
    let seeds: Vec<u64> = match env_u64("SAFETY_SEED") {
        Some(seed) => vec![seed],
        None => (0..STROBE_SEEDS as u64).map(|i| 2 * 1_000_003 + i).collect(),
    };
    let only = std::env::var("SAFETY_CUE").ok();
    let cues: Vec<String> = test_support::shared().presets.iter().map(|p| p.id.clone()).filter(|c| only.as_ref().is_none_or(|o| o == c)).collect();
    assert!(!cues.is_empty(), "{name}: no cue {only:?}");
    let deep = env_u64("SAFETY_CASES").is_some();
    let n = if deep { seeds.len() } else { per_cue.min(seeds.len()) };
    let mut jobs: Vec<(u64, &str)> = Vec::new();
    for (i, c) in cues.iter().enumerate() {
        jobs.extend((0..n).map(|k| (seeds[(i * n + k) % seeds.len()], c.as_str())));
    }
    let _guard = property_guard(name, if deep { 4 } else { 1 }, 1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    // Half the machine: the other tests (some of them timed) run meanwhile.
    let threads = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&(seed, cue)) = jobs.get(i) else { break };
                if let Err(e) = strobe_case(seed, Some(Forced { cue, self_flash })) {
                    failures.lock().unwrap().push(format!("{e} (replay: SAFETY_SEED={seed} SAFETY_CUE={cue} cargo test -p laser-studio {name})"));
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "{name}: {} of {} runs failed:\n{}", failures.len(), jobs.len(), failures.join("\n"));
}

/// Each cue flashed over itself, fast (way 3 with the cue as the look):
/// every press restarts it, every release falls back on the look.
#[test]
fn every_cue_flashed_over_itself_stays_within_the_burst_limit() {
    sweep_every_cue("every_cue_flashed_over_itself_stays_within_the_burst_limit", true, 8);
}

/// Each cue as the look, strobed the seed's way (beat gate, square LFO,
/// pumped fader, its own flash, the live strobe).
#[test]
fn every_cue_strobed_every_way_stays_within_the_burst_limit() {
    sweep_every_cue("every_cue_strobed_every_way_stays_within_the_burst_limit", false, 4);
}

/// T-360: « Strobe 1/2 temps » under the live strobe, at 2.56 Hz with a
/// 1.61 s burst. The hold began in a dark gap, filling it with a flash of
/// its own; the gap outlasted a period so the hold let the look go dark,
/// and the look came back a third of a second after that flash: 1.95 s
/// of fast flashing out. The output check refuses that flash.
#[test]
fn strobe_seed_2000035_holds_within_the_burst_on_a_strobed_beam_fan() {
    let cue = test_support::shared().presets.iter().find(|p| p.name.starts_with("Strobe 1/2 temps")).map(|p| p.id.clone()).expect("the cue");
    strobe_case(2000035, Some(Forced { cue: &cue, self_flash: false })).unwrap();
}

/// T-360: an evolving look flashed over, fast, carries on where it was
/// at each release; it used to start over (a fresh animator) and sit on
/// its first frame waiting for the next beat, never moving on.
#[test]
fn a_flash_released_goes_back_to_an_evolving_look_where_it_was() {
    let mut s = test_support::shared();
    s.settings = Settings { content: Content::Evolving(crate::evolving::test_cue(true)), brightness: 1.0, ..Default::default() };
    s.tempo.set_bpm_manual(120.0, 0.0);
    let cue = s.presets[0].id.clone();
    let mut rig = Rig::new(s, "evolving-flash", 0);
    let mut furthest = 0.0f64;
    for i in 0..8 * 60 {
        let down = (i / 5) % 2 == 0;
        controls::press_cue(&mut rig.s(), &cue, Some(ClickMode::Flash), down);
        rig.tick();
        if !down {
            let s = rig.s();
            let (_, p) = s.evolving.iter().find(|(id, _)| *id == 0).expect("the look is on show between flashes");
            furthest = furthest.max(p.pos);
        }
    }
    assert!(furthest > 8.0, "the look moves on between flashes (reached beat {furthest:.2} of 16 beats played)");
}

/// Default seed count of the strobe property (and of the cue sweeps).
const STROBE_SEEDS: usize = 40;

/// T-354: a burst limit (0.52 s) shorter than two allowed periods
/// (1.13 Hz); the limiter used to wait for a third fast flash before
/// starting the burst clock, so 0.63 s of fast flashing went out.
#[test]
fn strobe_seed_2002967_is_cut_within_the_burst() {
    run_strobe_case(2002967).unwrap();
}

// ---------- arming: MIDI and controls ----------

const PORT: &str = "APC40 mkII";

fn random_midi(rng: &mut Rng) -> MidiMsg {
    // Biased to the notes and CCs the APC40 profile maps.
    let number = if rng.chance(0.6) { *rng.pick(&[0x51u8, 0x62, 0x20, 0x21, 0x30, 0x07, 0x0E, 0x2F, 0x52, 0x5E]) } else { rng.below(128) as u8 };
    let channel = if rng.chance(0.7) { 0 } else { rng.below(16) as u8 };
    match rng.below(5) {
        0 | 1 => MidiMsg::NoteOn { channel, note: number, velocity: 1 + rng.below(127) as u8 },
        2 => MidiMsg::NoteOff { channel, note: number },
        3 => MidiMsg::Cc { channel, number, value: rng.below(128) as u8 },
        _ => MidiMsg::PitchBend { channel, value: rng.below(16384) as u16 },
    }
}

/// Invariant 6 for controllers: random MIDI (and every control, from a
/// controller) never arms; with the operator's opt-in, only the Shift +
/// hold gesture does, and only after the hold time.
fn run_midi_case(seed: u64) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let mut s = test_support::shared();
    let allow = rng.chance(0.5);
    s.midi.store.devices.safety.allow_arm = allow;
    let t0 = Instant::now();
    {
        let d = s.midi.device_mut(PORT);
        d.profile = "apc40-mk2".into();
        d.connected = true;
        d.connected_at = Some(t0 - Duration::from_secs(10));
    }
    let ids: Vec<String> = s.controls.list().iter().map(|d| d.id.clone()).collect();
    let mut rig = Rig::new(s, "midi", seed);
    // When the arm gesture (Shift + Stop All) started, if it is held.
    let mut gesture: Option<Instant> = None;
    let mut shift = false;
    let mut stop = false;
    for frame in 0..120 {
        let now = rig.now;
        let was = rig.s().gate.is_armed();
        let mut batch = Vec::new();
        for _ in 0..rng.below(6) {
            let msg = if rng.chance(0.15) {
                // Deliberately aim at the gesture's two buttons.
                let note = *rng.pick(&[0x62u8, 0x51]);
                if rng.chance(0.6) {
                    MidiMsg::NoteOn { channel: 0, note, velocity: 127 }
                } else {
                    MidiMsg::NoteOff { channel: 0, note }
                }
            } else {
                random_midi(&mut rng)
            };
            match msg {
                MidiMsg::NoteOn { note: 0x62, .. } => shift = true,
                MidiMsg::NoteOff { note: 0x62, .. } => shift = false,
                MidiMsg::NoteOn { note: 0x51, .. } => stop = true,
                MidiMsg::NoteOff { note: 0x51, .. } => stop = false,
                _ => {}
            }
            batch.push(MidiEvent { port: PORT.into(), msg, at: now });
        }
        {
            let mut s = rig.s();
            midi::engine::handle_batch(&mut s, &batch);
            if rng.chance(0.3) {
                let id = rng.pick(&ids).clone();
                let _ = controls::apply(&mut s, &id, ControlInput::Norm(rng.f()), true);
            }
            if rng.chance(0.05) {
                let estop = Arc::clone(&s.estop);
                s.gate.reset_estop(&estop);
            }
        }
        if shift && stop {
            gesture.get_or_insert(now);
        } else {
            gesture = None;
        }
        if rig.s().gate.is_armed() && !was {
            return Err(format!("frame {frame}: a MIDI batch or controller control armed directly"));
        }
        rig.tick();
        let s = rig.s();
        if s.gate.is_armed() && !was {
            let held = gesture.map_or(Duration::ZERO, |g| rig.now.saturating_duration_since(g));
            if !allow {
                return Err(format!("frame {frame}: MIDI armed without the opt-in"));
            }
            if held < midi::safety::ARM_HOLD {
                return Err(format!("frame {frame}: MIDI armed after a {held:?} hold"));
            }
            if s.gate.status(&s.estop).source != Some(ArmSource::Midi) {
                return Err(format!("frame {frame}: armed by the engine frame, not by the MIDI gesture"));
            }
        }
        let lit = rig.wire.lock().unwrap().last.iter().filter(|p| p.is_lit()).count();
        if lit > 0 && !(s.gate.is_armed() && !s.estop.is_latched()) {
            return Err(format!("frame {frame}: {lit} lit points while disarmed"));
        }
    }
    Ok(())
}

#[test]
fn midi_never_arms_except_the_opt_in_gesture() {
    run_property("midi_never_arms_except_the_opt_in_gesture", 3, 500, run_midi_case);
}

/// The gate itself: a controller or the system can never arm, whatever
/// the interlocks say, and nothing arms while the e-stop is latched.
#[test]
fn the_gate_refuses_controllers_the_system_and_a_latched_estop() {
    let seeds = seeds(4, 1000);
    let _guard = property_guard("the_gate_refuses_controllers_the_system_and_a_latched_estop", seeds.len(), 1000);
    for seed in seeds {
        let mut rng = Rng::new(seed);
        let mut s = test_support::shared();
        let mut latched = false;
        for _ in 0..30 {
            let was = s.gate.is_armed();
            match rng.below(6) {
                0 => assert!(s.request_arm(*rng.pick(&[ArmSource::Midi, ArmSource::System])).is_err(), "seed {seed}"),
                1 => {
                    s.emergency_stop(ArmSource::Keyboard);
                    latched = true;
                }
                2 => {
                    let estop = Arc::clone(&s.estop);
                    s.gate.reset_estop(&estop);
                    latched = false;
                }
                3 => {
                    let r = s.request_arm(*rng.pick(&ARM_SOURCES));
                    assert_eq!(r.is_ok(), !latched, "seed {seed}: an arm request must fail exactly while the e-stop is latched");
                    continue;
                }
                4 => s.gate.disarm(interlock::DisarmReason::User, ArmSource::Ui),
                _ => {
                    // The MIDI opt-in path, refused while MIDI arming is off.
                    assert!(s.request_arm_midi_opt_in().is_err(), "seed {seed}: MIDI arming is off by default");
                }
            }
            assert!(!s.gate.is_armed() || was, "seed {seed}: armed by a non-arming call");
            if latched {
                assert!(!s.gate.is_armed(), "seed {seed}: armed while the e-stop is latched");
            }
        }
    }
}

// ---------- DAC thinning ----------

/// DACs with a small frame buffer (T-303, T-353): the engine thins the
/// content *before* the safety stage, then the output stage's last-resort
/// `fit_frame_dark` may only remove light. Neither may light a straight
/// shortcut through a Blank zone. `fit_frame_dark` is also checked
/// directly on an already shaped frame (the case `fit_frame` failed at
/// seed 5000020: a lit path that went around a zone became a lit chord
/// through it).
fn run_thinning_case(seed: u64) -> Result<(), String> {
    use crate::output::{content_budget, fit_frame, fit_frame_dark};
    let mut rng = Rng::new(seed);
    let cfg = SafetySettings { zones: random_safety(&mut rng).zones, horizon: Horizon { y: -1.0, ..Default::default() }, ..Default::default() };
    if cfg.zones.iter().all(|z| z.kind != ZoneKind::Blank) {
        return Ok(());
    }
    // A lit random walk, densified like the engine does.
    let mut raw = Vec::new();
    let (mut x, mut y) = (rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
    for _ in 0..rng.below(300) + 50 {
        x = (x + rng.range(-0.05, 0.05)).clamp(-1.0, 1.0);
        y = (y + rng.range(-0.05, 0.05)).clamp(-1.0, 1.0);
        raw.push(if rng.chance(0.95) { Point::lit(x, y, 1.0, 1.0, 1.0) } else { Point::blanked(x, y) });
    }
    let lit = |f: &[Point]| f.iter().filter(|p| p.is_lit()).count();
    let shaped = safety::apply(raw.clone(), 0.0, &cfg, &mut safety::StrobeLimiter::default());
    if zones::blank_violations(&shaped, &cfg.zones) > 0 {
        return Err("the safety stage itself left light in a Blank zone".into());
    }
    let max = 2 + rng.below(shaped.len());
    // The output's guard alone, on a frame that already passed the stage.
    let thin = fit_frame_dark(&shaped, max);
    let v = zones::blank_violations(&thin, &cfg.zones);
    if v > 0 {
        return Err(format!("dark thinning {} → {max} points lit {v} segments through a Blank zone", shaped.len()));
    }
    if thin.len() > max || lit(&thin) > lit(&shaped) {
        return Err(format!("dark thinning to {max} sent {} points, {} lit (from {})", thin.len(), lit(&thin), lit(&shaped)));
    }
    // The engine's order: fit the content, safety stage, output guard.
    let budget = content_budget(max);
    let fitted = if raw.len() > budget { fit_frame(&raw, budget).into_owned() } else { raw };
    let shaped = safety::apply(fitted, 0.0, &cfg, &mut safety::StrobeLimiter::default());
    let sent = fit_frame_dark(&shaped, max);
    let v = zones::blank_violations(&sent, &cfg.zones);
    if v > 0 {
        return Err(format!("pipeline to {max} points lit {v} segments through a Blank zone"));
    }
    if sent.len() > max {
        return Err(format!("pipeline sent {} points to an output that takes {max}", sent.len()));
    }
    Ok(())
}

#[test]
fn dac_thinning_never_lights_a_blank_zone() {
    run_property("dac_thinning_never_lights_a_blank_zone", 5, 1000, run_thinning_case);
}

/// Found by `random_sessions_keep_every_output_invariant`: a Blank zone
/// drawn to the edge of the field (x = 1) and content clamped onto that
/// edge by the calibration. The segment from a dark sample inside the zone
/// to a lit sample exactly on its edge is not split (`Poly::crossings`
/// only counts crossings strictly inside the segment, `t < 1`), so a DAC
/// that colours a segment with its end sample draws it lit through the
/// zone.
#[test]
fn a_lit_point_on_a_blank_zone_edge_stays_outside() {
    let zone = Zone { kind: ZoneKind::Blank, points: vec![[0.5, -0.5], [1.0, -0.5], [1.0, 0.5], [0.5, 0.5]], ..Default::default() };
    let cfg = SafetySettings { zones: vec![zone], horizon: Horizon { y: -1.0, ..Default::default() }, ..Default::default() };
    let frame = vec![Point::lit(0.9, 0.0, 1.0, 1.0, 1.0), Point::lit(1.0, 0.1, 1.0, 1.0, 1.0)];
    let out = safety::apply(frame, 0.0, &cfg, &mut safety::StrobeLimiter::default());
    assert_eq!(zones::blank_violations(&out, &cfg.zones), 0, "{out:?}");
}

// ---------- output colour rendering (T-414) ----------

/// Invariant 7 on one frame: `after` is `before` through the colour
/// rendering. Same points at the same places, nothing dark lit, every
/// channel finite within 0..1.
fn check_colour(before: &[Point], after: &[Point]) -> Result<(), String> {
    if before.len() != after.len() {
        return Err(format!("{} points became {}", before.len(), after.len()));
    }
    for (i, (b, a)) in before.iter().zip(after).enumerate() {
        if (a.x.to_bits(), a.y.to_bits()) != (b.x.to_bits(), b.y.to_bits()) {
            return Err(format!("point {i} moved: ({}, {}) → ({}, {})", b.x, b.y, a.x, a.y));
        }
        if !b.is_lit() && a.is_lit() {
            return Err(format!("point {i} was dark and is lit: {a:?}"));
        }
        if [a.r, a.g, a.b].iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return Err(format!("point {i} out of 0..1: {a:?}"));
        }
    }
    Ok(())
}

/// A random frame: strokes of random colours (odd values included)
/// joined by blanked travel, sometimes dark points inside a stroke.
fn random_strokes(rng: &mut Rng) -> Vec<Point> {
    let mut f = Vec::new();
    let (mut x, mut y) = (rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
    for _ in 0..1 + rng.below(8) {
        for _ in 0..rng.below(6) {
            f.push(Point::blanked(x, y));
        }
        for _ in 0..1 + rng.below(40) {
            x = (x + rng.range(-0.08, 0.08)).clamp(-1.0, 1.0);
            y = (y + rng.range(-0.08, 0.08)).clamp(-1.0, 1.0);
            let c = |rng: &mut Rng| match rng.below(12) {
                0 => 0.0,
                1 => 1.0,
                2 => rng.range(-1.0, 2.0),
                3 if rng.chance(0.1) => f32::NAN,
                _ => rng.f(),
            };
            f.push(if rng.chance(0.05) { Point::blanked(x, y) } else { Point::lit(x, y, c(rng), c(rng), c(rng)) });
        }
    }
    f
}

/// Output scan limits (T-171) as `POST /api/outputs/scan_limits` could
/// send them: mostly valid, sometimes out of range (the store refuses
/// those, `sanitized` brings a hand-edited file into range).
fn random_output_scan(rng: &mut Rng) -> crate::power::OutputScan {
    let pps = |rng: &mut Rng| if rng.chance(0.1) { rng.below(300_000) as u32 } else { 1_000 + rng.below(99_000) as u32 };
    let (a, b) = (pps(rng), pps(rng));
    crate::power::OutputScan {
        pps_min: a.min(b),
        pps_max: a.max(b),
        min_points: if rng.chance(0.2) { 0 } else { rng.below(1_500) },
        max_points: if rng.chance(0.5) { 0 } else { rng.below(3_000) },
        min_fps: if rng.chance(0.05) { f32::NAN } else { rng.range(-5.0, 150.0) },
    }
}

/// T-171, before the safety stage: completing a frame (`pad_frame`)
/// never draws a lit segment the frame didn't draw; the output's limits
/// (`fit_frame_limits`) give a frame within the minimum and the maxima;
/// then the safety stage and the DAC guard still leave nothing lit in a
/// Blank zone. And the scan speed range: whatever the preset, the live
/// scan speed and the output's rate, no look is traced faster than
/// `pps_max` (unless the rate is above twice it: then at 50 %, warned).
fn run_scan_limits_case(seed: u64) -> Result<(), String> {
    use crate::output::{content_budget, fit_frame_dark, fit_frame_limits, pad_frame};
    let mut rng = Rng::new(seed);
    let frame = random_strokes(&mut rng);
    let scan = random_output_scan(&mut rng).sanitized();
    let lim = scan.frame_limits();
    let dac = rng.chance(0.3).then(|| 16 + rng.below(800));
    let seg = |f: &[Point]| -> Vec<[(u32, u32); 2]> {
        f.windows(2).filter(|w| w[1].is_lit() && (w[0].x, w[0].y) != (w[1].x, w[1].y)).map(|w| [(w[0].x.to_bits(), w[0].y.to_bits()), (w[1].x.to_bits(), w[1].y.to_bits())]).collect()
    };
    // A frame that doesn't move (the galvos still) is left as it is.
    let moves = |f: &[Point]| f.iter().any(|p| (p.x - f[0].x).abs() > 1e-6 || (p.y - f[0].y).abs() > 1e-6);
    let padded = pad_frame(frame.clone(), lim.min);
    let bits = |p: &Point| [p.x, p.y, p.r, p.g, p.b].map(f32::to_bits);
    let want = if moves(&frame) { lim.min } else { frame.len() };
    if padded.len() < want || padded.len() < frame.len() || padded.iter().zip(&frame).any(|(a, b)| bits(a) != bits(b)) {
        return Err(format!("padding {} to {} gave {} points or changed the frame", frame.len(), lim.min, padded.len()));
    }
    let before = seg(&frame);
    if let Some(s) = seg(&padded).into_iter().find(|s| !before.contains(s)) {
        return Err(format!("padding drew a new lit segment {s:?}"));
    }
    let lit_spots = |f: &[Point]| f.iter().filter(|p| p.is_lit()).map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<std::collections::HashSet<_>>();
    if !lit_spots(&padded).is_subset(&lit_spots(&frame)) {
        return Err("padding lit a spot the frame didn't light".into());
    }
    let fitted = fit_frame_limits(frame.clone(), lim, dac);
    let max = match (lim.max, dac.map(content_budget)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    if max.is_some_and(|m| fitted.len() > m.max(2)) || (moves(&fitted) && fitted.len() < max.map_or(lim.min, |m| lim.min.min(m))) {
        return Err(format!("{} points for limits {lim:?} and DAC {dac:?}", fitted.len()));
    }
    let cfg = SafetySettings { zones: random_safety(&mut rng).zones, horizon: Horizon { y: -1.0, ..Default::default() }, ..Default::default() };
    let shaped = safety::apply(fitted, 0.0, &cfg, &mut safety::StrobeLimiter::default());
    let sent = match dac {
        Some(m) => fit_frame_dark(&shaped, m).into_owned(),
        None => shaped,
    };
    let v = zones::blank_violations(&sent, &cfg.zones);
    if v > 0 {
        return Err(format!("{v} lit segments through a Blank zone after the scan limits"));
    }
    // The scan speed range.
    let pps = 1_000 + rng.below(120_000) as u32;
    let limits = scan.scan_limits(pps);
    let base = crate::optimize::OptimizeParams { enabled: true, ..Default::default() };
    let mixer = crate::layers::Mixer { optimizer: base.clone(), ..Default::default() };
    let presets = crate::scan::ScanPresetStore::load_or_create(std::env::temp_dir().join("laser-studio-test-unused/scan_presets.json"));
    let master = if rng.chance(0.1) { f32::NAN } else { rng.range(-1.0, 3.0) };
    let chosen = *rng.pick(&[None, Some(crate::scan::GRAPHICS)]);
    let p = presets.resolve(chosen, &mixer, &limits, master);
    let traced = pps as f32 * p.lit_step / base.lit_step;
    let bound = (scan.pps_max as f32).max(pps as f32 * crate::scan::MIN_SCAN_PCT / 100.0);
    if traced > bound * 1.001 {
        return Err(format!("traced at {traced} points/s for {pps} with pps_max {} (master {master})", scan.pps_max));
    }
    Ok(())
}

/// The engine applies the output's points per frame (T-171) before the
/// safety stage: a short frame reaches the output completed, and a Blank
/// zone over the whole field still leaves all of it dark.
#[test]
fn the_engine_completes_short_frames_before_the_safety_stage() {
    let run = |min_points: usize, blank_all: bool| -> Vec<Point> {
        let mut s = test_support::shared();
        s.settings = Settings { content: Content::Shape { shape: "square".into() }, ..Settings::default() };
        let scan = crate::power::OutputScan { min_points, ..Default::default() };
        s.outputs.set_scan(MAIN_OUTPUT, scan, false).unwrap();
        if blank_all {
            let zone = Zone { kind: ZoneKind::Blank, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]], ..Default::default() };
            s.safety.set(SafetySettings { zones: vec![zone], ..s.safety.get() }, true).unwrap();
        }
        let mut rig = Rig::new(s, &format!("pad-{min_points}-{blank_all}"), 171);
        rig.s().request_arm(ArmSource::Ui).unwrap();
        for _ in 0..5 {
            rig.tick();
        }
        let sent = rig.wire.lock().unwrap().last.clone();
        sent
    };
    let plain = run(0, false);
    assert!(plain.iter().any(|p| p.is_lit()) && plain.len() < 2_000, "{} points", plain.len());
    let padded = run(2_000, false);
    assert!(padded.len() >= 2_000, "{} points", padded.len());
    assert!(padded.iter().filter(|p| p.is_lit()).count() > plain.iter().filter(|p| p.is_lit()).count(), "a square is retraced, lit");
    let blank = run(2_000, true);
    assert!(blank.len() >= 2_000 && blank.iter().all(|p| !p.is_lit()), "the zones run after the completion");
}

#[test]
fn scan_limits_never_add_light_and_never_trace_faster_than_pps_max() {
    run_property("scan_limits_never_add_light_and_never_trace_faster_than_pps_max", 171, 1000, run_scan_limits_case);
}

/// Invariant 7 directly, then the engine's order after it (safety stage,
/// caps): the caps, zones and horizon still hold.
fn run_colour_case(seed: u64) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let frame = random_strokes(&mut rng);
    // Whatever the store would hold: wild values sanitised.
    let color = random_color(&mut rng).sanitized();
    let mut after = frame.clone();
    color.apply(&mut after);
    check_colour(&frame, &after).map_err(|e| format!("{e} ({color:?})"))?;
    let cfg = random_safety(&mut rng);
    let caps = random_limits(&mut rng);
    let out = safety::apply(after, 0.0, &cfg, &mut safety::StrobeLimiter::default());
    let mut out = out;
    crate::power::cap(&mut out, &caps);
    check_points(&out, &caps, &cfg, "colour then safety then caps")
}

#[test]
fn output_colour_never_lights_travel_nor_gets_past_the_safety_stage() {
    run_property("output_colour_never_lights_travel_nor_gets_past_the_safety_stage", 7, 2000, run_colour_case);
}

/// The checker must catch a colour delay that rotates colours over the
/// whole frame (onto the travel), which is what a naive shift does.
#[test]
fn the_colour_check_catches_a_delay_onto_travel() {
    let frame = vec![Point::blanked(0.0, 0.0), Point::lit(0.1, 0.0, 1.0, 0.0, 0.0), Point::lit(0.2, 0.0, 0.0, 1.0, 0.0), Point::blanked(0.3, 0.0)];
    let mut naive = frame.clone();
    let colours: Vec<(f32, f32, f32)> = frame.iter().map(|p| (p.r, p.g, p.b)).collect();
    for (i, p) in naive.iter_mut().enumerate() {
        (p.r, p.g, p.b) = colours[(i + colours.len() - 1) % colours.len()];
    }
    assert!(check_colour(&frame, &naive).is_err());
    let mut ours = frame.clone();
    ColorCalibration { color_delay_points: 1, ..Default::default() }.apply(&mut ours);
    assert!(check_colour(&frame, &ours).is_ok());
    assert_eq!((ours[2].r, ours[2].g), (1.0, 0.0), "shifted inside the stroke");
}

/// Through the real engine: the brightest colour rendering (full gains,
/// a curve well above linear, the largest delay) on a white ramp under a
/// Blank zone covering the field lights nothing; the safety gains and
/// the caps still apply after it.
#[test]
fn output_colour_runs_before_zones_safety_gains_and_caps() {
    let bright = ColorCalibration { white_balance: [1.0; 3], curve: [ChannelCurve::Gamma { gamma: 0.2 }; 3], color_delay_points: 8 };
    let lit_frame = |safety: SafetySettings, caps: OutputLimits| -> Vec<Point> {
        let mut s = test_support::shared();
        s.safety.set(safety, true).unwrap();
        s.outputs.set(MAIN_OUTPUT, Some(caps), None, false, true).unwrap();
        s.outputs.set_color(MAIN_OUTPUT, bright, false, true).unwrap();
        s.test_pattern.brightness = 1.0;
        testpattern::select(&mut s, Some(testpattern::TestPattern::WhiteRamp));
        let mut rig = Rig::new(s, "colour-order", 0);
        rig.tick();
        let frame = rig.s().frame.clone();
        frame
    };
    let open = OutputLimits { max_power: 1.0, max_color: [1.0; 3] };
    let no_horizon = Horizon { y: -1.0, ..Default::default() };
    // Reference: lit, and brighter than the plain ramp at its first step.
    let f = lit_frame(SafetySettings { horizon: no_horizon, ..Default::default() }, open);
    assert!(f.iter().any(|p| p.is_lit()), "the ramp is lit");
    assert!(f.iter().filter(|p| p.is_lit()).all(|p| p.g > 0.6), "gamma 0.2 lifts the 10 % step");
    // A Blank zone over the whole field: nothing.
    let all = Zone { kind: ZoneKind::Blank, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]], ..Default::default() };
    let f = lit_frame(SafetySettings { zones: vec![all], horizon: no_horizon, ..Default::default() }, open);
    assert!(!f.iter().any(|p| p.is_lit()), "colour rendering lit a Blank zone");
    // The safety gain (green off) and the caps (red at 30 %) after it.
    let f = lit_frame(SafetySettings { color_gain: [1.0, 0.0, 1.0], horizon: no_horizon, ..Default::default() }, OutputLimits { max_power: 1.0, max_color: [0.3, 1.0, 1.0] });
    assert!(f.iter().any(|p| p.is_lit()));
    assert!(f.iter().all(|p| p.g == 0.0 && p.r <= 0.3 + 1e-6), "safety gain and cap after the colours");
}

/// A deliberate regression must be caught: without the power caps, the
/// pipeline property fails. (Checks the checker, on the same frames.)
#[test]
fn the_caps_check_catches_an_uncapped_frame() {
    let caps = OutputLimits { max_power: 0.5, max_color: [1.0, 0.4, 1.0] };
    let cfg = SafetySettings::default();
    let frame = vec![Point::lit(0.0, 0.5, 0.5, 0.2, 0.5), Point::lit(0.1, 0.5, 0.5, 0.21, 0.5)];
    assert!(check_points(&frame, &caps, &cfg, "uncapped").is_err());
    let mut capped = frame.clone();
    crate::power::cap(&mut capped, &caps);
    assert!(check_points(&capped, &caps, &cfg, "capped").is_ok());
}

/// T-405: a user cue stored with master modifiers draws through them (its
/// own copy, before the master stage), and the safety stage still applies
/// after: its frame never gets past the horizon.
#[test]
fn a_user_cues_own_master_applies_before_the_safety_stage() {
    let frame_of = |master: Option<live::LiveModifiers>, horizon: Option<f32>| -> Vec<Point> {
        let mut s = test_support::shared();
        let y = horizon.unwrap_or(-1.0);
        s.safety.set(SafetySettings { horizon: Horizon { y, lines: true, ramp: 0.0, level: 0.0 }, ..Default::default() }, true).unwrap();
        let mut g = s.grid.clone();
        let page = g.new_page("Mes looks", &s.presets).unwrap();
        s.grid = g;
        crate::usercues::refresh(&mut s);
        s.settings = Settings { content: crate::engine::Content::Shape { shape: "square".into() }, scale: 0.8, rotation_speed: 0.0, ..Settings::default() };
        s.live = master.clone().unwrap_or_default();
        let id = crate::usercues::store(&mut s, page, 0, None, crate::usercues::StoreFilter::All, master.is_some()).unwrap();
        // Back to a neutral master: only the cue's own copy moves it.
        s.live = live::LiveModifiers::default();
        s.settings = Settings::default();
        assert!(controls::press_cue(&mut s, &id, None, true));
        let mut rig = Rig::new(s, "user-cue-master", 0);
        for _ in 0..3 {
            rig.tick();
        }
        assert!(!rig.s().gate.is_armed(), "never armed");
        let frame = rig.s().frame.clone();
        frame
    };
    let lit = |f: &[Point]| f.iter().filter(|p| p.r + p.g + p.b > 0.0).cloned().collect::<Vec<_>>();
    let extent = |f: &[Point]| lit(f).iter().fold(0.0f32, |m, p| m.max(p.x.abs()));
    let plain = frame_of(None, None);
    let small = frame_of(Some(live::LiveModifiers { size: 0.5, ..Default::default() }), None);
    assert!(extent(&plain) > 0.7, "{}", extent(&plain));
    assert!((extent(&small) - extent(&plain) * 0.5).abs() < 0.05, "{} vs {}", extent(&small), extent(&plain));
    // Moved down by its own master, it is cut by the horizon all the same.
    let low = frame_of(Some(live::LiveModifiers { pos_y: -0.5, ..Default::default() }), Some(0.0));
    assert!(!lit(&low).is_empty());
    assert!(lit(&low).iter().all(|p| p.y >= -1e-3), "a lit point below the horizon");
}


// ---------- transitions between cues (T-158) ----------

/// Segments `i` (from point `i` to `i + 1`) of `src` that pass within a
/// hair of `q`.
fn segments_near(src: &[Point], q: &Point) -> Vec<usize> {
    let near = |a: &Point, b: &Point| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        let u = if len2 > 0.0 { (((q.x - a.x) * dx + (q.y - a.y) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let (px, py) = (a.x + u * dx, a.y + u * dy);
        ((q.x - px).powi(2) + (q.y - py).powi(2)).sqrt() < 1e-4
    };
    (0..src.len().saturating_sub(1)).filter(|&i| near(&src[i], &src[i + 1])).collect()
}

/// Every lit step of `out` (to a lit point) runs along `src`'s lit path:
/// both its ends lie on segments of `src` with only lit segments (both
/// ends lit) from the first to the second. A step across blanked travel
/// fails.
fn check_on_strokes(src: &[Point], out: &[Point], what: &str) -> Result<(), String> {
    let lit_seg: Vec<bool> = (0..src.len().saturating_sub(1)).map(|i| src[i].is_lit() && src[i + 1].is_lit()).collect();
    for k in 1..out.len() {
        if !out[k].is_lit() {
            continue;
        }
        if src.len() == 1 {
            let on = src[0].is_lit() && (out[k].x - src[0].x).abs() < 1e-4 && (out[k].y - src[0].y).abs() < 1e-4;
            if !on {
                return Err(format!("{what}: lit point {k} off a one-point frame"));
            }
            continue;
        }
        let (from, to) = (segments_near(src, &out[k - 1]), segments_near(src, &out[k]));
        let ok = from.iter().any(|&i| to.iter().any(|&j| i <= j && (i..=j).all(|s| lit_seg[s])));
        if !ok {
            return Err(format!("{what}: lit step {} → {k} ({:?} → {:?}) leaves the lit strokes", k - 1, out[k - 1], out[k]));
        }
    }
    Ok(())
}

/// Invariant 11 on one morph: `out` is `morph(a, b, p)`.
fn check_morph(a: &[Point], b: &[Point], p: f32, out: &[Point]) -> Result<(), String> {
    let n = a.len().max(b.len()).max(2);
    if out.len() != n {
        return Err(format!("{} and {} points morphed into {}", a.len(), b.len(), out.len()));
    }
    let brightest = |f: &[Point], c: usize| f.iter().filter(|q| q.is_lit()).map(|q| [q.r, q.g, q.b][c]).filter(|v| v.is_finite()).fold(0.0f32, |m, v| m.max(v.min(1.0)));
    for (k, q) in out.iter().enumerate() {
        if !(q.x.is_finite() && q.y.is_finite()) {
            return Err(format!("point {k} not finite: {q:?}"));
        }
        for (c, v) in [q.r, q.g, q.b].into_iter().enumerate() {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) || v > brightest(a, c).max(brightest(b, c)) + 1e-5 {
                return Err(format!("point {k} channel {c} = {v}: brighter than both frames"));
            }
        }
    }
    // Lit steps are blends of the two ends, which run along each frame's
    // lit strokes: the lit points are the same at every p, and each point
    // moves in a straight line between its two ends.
    let (start, end) = (transition::morph(a, b, 0.0), transition::morph(a, b, 1.0));
    for k in 0..n {
        if out[k].is_lit() != start[k].is_lit() || out[k].is_lit() != end[k].is_lit() {
            return Err(format!("point {k}: lit at p = {p} but not at both ends"));
        }
        let (x, y) = (start[k].x + (end[k].x - start[k].x) * p, start[k].y + (end[k].y - start[k].y) * p);
        if (out[k].x - x).abs() > 1e-4 || (out[k].y - y).abs() > 1e-4 {
            return Err(format!("point {k} at ({}, {}) off its straight path ({x}, {y})", out[k].x, out[k].y));
        }
    }
    check_on_strokes(a, &start, "p = 0")?;
    check_on_strokes(b, &end, "p = 1")
}

fn run_morph_case(seed: u64) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let (a, b) = (random_strokes(&mut rng), random_strokes(&mut rng));
    let p = match rng.below(6) {
        0 => 0.0,
        1 => 1.0,
        _ => rng.f(),
    };
    let out = transition::morph(&a, &b, p);
    check_morph(&a, &b, p, &out).map_err(|e| format!("{e} ({} → {} points, p {p})", a.len(), b.len()))?;
    // Then through the safety stage and caps, like the engine does after the mixer.
    let cfg = random_safety(&mut rng);
    let caps = random_limits(&mut rng);
    let mut frame = safety::apply(out.clone(), 0.0, &cfg, &mut safety::StrobeLimiter::default());
    crate::power::cap(&mut frame, &caps);
    check_points(&frame, &caps, &cfg, "morph then safety then caps")
}

#[test]
fn a_morph_never_lights_blanked_travel() {
    run_property("a_morph_never_lights_blanked_travel", 11, 2000, run_morph_case);
}

/// The checker must catch a morph that lights every sample lit in both
/// frames without asking for the same stroke: with strokes longer than
/// the sample spacing and a short jump, it lights the jump.
#[test]
fn the_morph_check_catches_a_step_across_travel() {
    let stroke = |y: f32, rev: bool| -> Vec<Point> {
        (0..10).map(|i| i as f32 / 9.0).map(|u| if rev { 1.0 - u } else { u }).map(|u| Point::lit(-0.5 + u, y, 1.0, 1.0, 1.0)).collect()
    };
    let mut a = stroke(-0.02, false);
    a.push(Point::blanked(0.5, -0.02));
    a.push(Point::blanked(0.5, 0.02));
    a.extend(stroke(0.02, true));
    let b: Vec<Point> = (0..22).map(|i| Point::lit(-0.5 + i as f32 / 21.0, 0.5, 1.0, 0.0, 0.0)).collect();
    let n = a.len().max(b.len());
    let (ra, rb) = (transition::resample(&a, n), transition::resample(&b, n));
    let naive: Vec<Point> = ra.iter().zip(&rb).map(|(sa, sb)| if sa.stroke.is_some() && sb.stroke.is_some() { sa.point } else { Point::blanked(sa.point.x, sa.point.y) }).collect();
    assert!(check_on_strokes(&a, &naive, "naive").is_err(), "the naive morph lights a's jump");
    let ours = transition::morph(&a, &b, 0.0);
    assert!(check_on_strokes(&a, &ours, "ours").is_ok());
    assert!(check_morph(&a, &b, 0.5, &transition::morph(&a, &b, 0.5)).is_ok());
}

/// Through the real engine: with a long fade and a bar quantisation, an
/// e-stop drops the fading cue and the waiting launch at once, nothing lit
/// leaves, and the waiting cue doesn't start after the reset.
#[test]
fn the_estop_cuts_every_transition_and_waiting_launch() {
    let mut s = test_support::shared();
    s.deck.transition = Transition { kind: TransitionKind::Fade, length: TransitionLength::Seconds(30.0), in_flash: true };
    s.safety.set(SafetySettings { horizon: Horizon { y: -1.0, ..Default::default() }, ..Default::default() }, true).unwrap();
    let ids: Vec<String> = s.presets.iter().filter(|p| p.settings.gate == crate::engine::GateMode::Off).take(3).map(|p| p.id.clone()).collect();
    assert!(controls::press_cue(&mut s, &ids[0], None, true));
    s.request_arm(ArmSource::Ui).expect("arms in the test studio");
    let mut rig = Rig::new(s, "transition-estop", 0);
    for _ in 0..3 {
        rig.tick();
    }
    {
        let mut s = rig.s();
        assert!(controls::press_cue(&mut s, &ids[1], None, true));
        assert_eq!(s.deck.ending.len(), 1, "the first cue fades out");
        s.deck.quantize = Quantize::Bar;
        let t = s.now_s();
        let shift = 1.5 - s.tempo.beat_at(t).rem_euclid(4.0);
        s.tempo.nudge(shift);
        assert!(controls::press_cue(&mut s, &ids[2], None, true));
        assert_eq!(s.deck.pending.len(), 1, "the third waits for the bar");
    }
    rig.tick();
    assert!(rig.wire.lock().unwrap().last.iter().any(|p| p.is_lit()), "the fade is on the output");
    rig.s().emergency_stop(ArmSource::Keyboard);
    {
        let s = rig.s();
        assert!(s.deck.ending.is_empty() && s.deck.pending.is_empty(), "cut at once");
        assert!(s.deck.active.iter().all(|a| a.fade.is_none()));
    }
    rig.tick();
    assert!(!rig.wire.lock().unwrap().last.iter().any(|p| p.is_lit()), "nothing lit after the e-stop");
    {
        let mut s = rig.s();
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
    }
    for _ in 0..(5 * 60) {
        rig.tick();
    }
    let s = rig.s();
    assert!(!s.deck.active.iter().any(|a| a.cue == ids[2]), "the dropped launch never starts");
    assert!(!s.gate.is_armed(), "the reset doesn't re-arm");
}

/// T-410, through the real engine: a cue list step coming from black with
/// a morph (the black rule makes it a fade) never shows a point that is
/// not on its final geometry, frame after frame; the e-stop pauses the
/// list and it doesn't start again by itself.
#[test]
fn a_cue_list_fade_from_black_never_shows_intermediate_geometry() {
    use crate::cuelist::{CueList, Step, StepSource, StepTrigger};
    let mut s = test_support::shared();
    // A still look: a circle, no rotation, no music.
    let look = Settings { brightness: 1.0, ..Settings::default() };
    s.scenes.replace_in_memory(vec![crate::scenes::Scene { name: "rond".into(), settings: look, duration_secs: 8.0 }]);
    let morph = |source: StepSource, trigger: StepTrigger| Step { source, trigger, transition: TransitionKind::Morph, fade_in_s: 1.0, fade_out_s: 1.0, ..Step::default() };
    let list = CueList {
        name: "noir".into(),
        steps: vec![morph(StepSource::Black, StepTrigger::Go), morph(StepSource::Scene { name: "rond".into() }, StepTrigger::Go), morph(StepSource::Black, StepTrigger::After { s: 5.0 })],
        ..CueList::default()
    };
    crate::cuelist::save(&mut s, list).unwrap();
    crate::cuelist::load(&mut s, "noir").unwrap();
    s.look_on = false;
    let mut rig = Rig::new(s, "cuelist-black", 0);
    crate::cuelist::go(&mut rig.s(), controls::ControlSource::Ui).unwrap();
    rig.tick();
    crate::cuelist::go(&mut rig.s(), controls::ControlSource::Ui).unwrap();
    let mut frames = Vec::new();
    for _ in 0..50 {
        rig.tick();
        frames.push(rig.s().frame.clone());
    }
    // Fully in by now (1 s fade): the final geometry.
    for _ in 0..20 {
        rig.tick();
    }
    let last = rig.s().frame.clone();
    assert!(last.iter().any(|p| p.is_lit()), "the step is on show");
    let lit_last: Vec<&Point> = last.iter().filter(|p| p.is_lit()).collect();
    let mut seen_partial = false;
    for (i, f) in frames.iter().enumerate() {
        for p in f.iter().filter(|p| p.is_lit()) {
            let near = lit_last.iter().any(|q| (q.x - p.x).abs() < 1e-3 && (q.y - p.y).abs() < 1e-3);
            assert!(near, "frame {i}: lit point ({}, {}) is not on the final geometry", p.x, p.y);
            seen_partial |= p.g < 0.99;
        }
    }
    assert!(seen_partial, "the step faded in");
    // The e-stop pauses the list; the timed step never comes by itself.
    rig.s().emergency_stop(ArmSource::Keyboard);
    for _ in 0..(8 * 60) {
        rig.tick();
    }
    let s = rig.s();
    assert_eq!((s.cue_list.index, s.cue_list.paused), (Some(1), true));
    assert!(!s.gate.is_armed());
}

// ---------- several projectors (T-277) ----------

/// A venue with `n` more projectors (`p2`…), each driving the recording
/// output `rec2`…, installed through the output manager's hand-over (the
/// engine's real path). Returns their wires.
fn add_projectors(rig: &mut Rig, n: usize, max_points: Option<usize>) -> Vec<(String, Arc<Mutex<Wire>>)> {
    let mut venue = rig.s().venue.get().clone();
    for i in 0..n {
        venue.projectors.push(crate::venue::Projector { id: format!("p{}", i + 2), name: format!("P{}", i + 2), output: Some(format!("rec{}", i + 2)), ..Default::default() });
    }
    {
        let mut s = rig.s();
        s.venue.set(venue).expect("valid venue");
        for i in 0..n {
            s.outputs.ensure(&format!("p{}", i + 2)).unwrap();
        }
        s.sync_projectors();
    }
    (0..n)
        .map(|i| {
            let (id, out) = (format!("p{}", i + 2), format!("rec{}", i + 2));
            let wire = Arc::new(Mutex::new(Wire::default()));
            rig.s().output_mgr.inject_projector(&id, &out, Box::new(Recorder(Arc::clone(&wire), max_points)));
            (id, wire)
        })
        .collect()
}

/// A lit square, armed from the UI, with no page needed.
fn armed_square_rig(tag: &str) -> Rig {
    let mut s = test_support::shared();
    s.settings = Settings { content: crate::engine::Content::Shape { shape: "square".into() }, scale: 0.8, rotation_speed: 0.0, ..Settings::default() };
    Rig::new(s, tag, 0)
}

fn lit(w: &Mutex<Wire>) -> usize {
    w.lock().unwrap().last.iter().filter(|p| p.is_lit()).count()
}

/// Invariants 1 to 3, 5, 8 and 9 for every projector of a random venue:
/// each output is sent light only while the one gate is armed, the e-stop
/// is not latched and its stage is armed; each frame stays within that
/// projector's own caps, out of the Blank zones, under the horizon rule
/// and the Public zones' rule; arming waits for every projector's output;
/// an e-stop darkens every projector in the frame it lands; losing any
/// projector's output disarms them all, and it comes back disarmed.
fn run_multi_case(seed: u64, cov: &mut [usize; 3]) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let (s, hold_to_run) = random_case(&mut rng);
    let max_points = seed.is_multiple_of(4).then(|| 16 + (seed as usize).wrapping_mul(7919) % 600);
    let mut rig = Rig::with_max_points(s, "multi", seed, max_points);
    let n = 1 + rng.below(3);
    let wires = add_projectors(&mut rig, n, max_points);
    let ids: Vec<String> = wires.iter().map(|(id, _)| id.clone()).collect();
    {
        let mut s = rig.s();
        if s.request_arm(ArmSource::Ui).is_ok() {
            return Err("armed before the projectors' outputs were installed".into());
        }
        for id in &ids {
            let _ = s.outputs.set(id, Some(random_limits(&mut rng)), None, false, true);
            if rng.chance(0.6) {
                let _ = s.outputs.set_color(id, random_color(&mut rng), false, true);
            }
        }
        // Projection zones bound to nothing, a projector or an output.
        if rng.chance(0.8) {
            let mut cfg = random_projection(&mut rng);
            for z in &mut cfg.zones {
                z.output = match rng.below(4) {
                    0 => None,
                    1 => Some(MAIN_OUTPUT.into()),
                    2 => Some(rng.pick(&ids).clone()),
                    _ => Some(format!("rec{}", 2 + rng.below(n))),
                };
            }
            let _ = s.projection.set(cfg);
        }
        if rng.chance(0.5) {
            mark_public(&mut s, &mut rng);
        }
    }
    rig.tick();
    if !rig.s().gate.interlock_ok(interlock::PROJECTORS) {
        return Err(format!("the projectors interlock stays open with every output installed: {:?}", rig.s().gate.blocking()));
    }
    let mut beats = 0;
    // A projector whose output is unplugged, and since which frame.
    let mut unplugged: Option<(usize, usize)> = None;
    for frame in 0..40 {
        for _ in 0..rng.below(3) {
            let was = rig.s().gate.is_armed();
            let explicit = random_action(&mut rig, &mut rng, &mut beats);
            if rig.s().gate.is_armed() && !was && !explicit {
                return Err(format!("frame {frame}: armed by a non-arming action"));
            }
        }
        if rng.chance(0.05) {
            // Another projector's caps, as `POST /api/outputs/limits` would.
            let mut s = rig.s();
            let armed = s.gate.is_armed();
            let id = rng.pick(&ids).clone();
            let _ = s.outputs.set(&id, Some(random_limits(&mut rng)), None, armed, true);
        }
        // Unplug one projector's output now and then, plug it back later.
        match unplugged {
            None if rng.chance(0.04) => {
                let i = rng.below(n);
                wires[i].1.lock().unwrap().fault = Some("débranché".into());
                unplugged = Some((i, frame));
            }
            Some((i, since)) if frame > since + 3 && rng.chance(0.3) => {
                let (id, w) = &wires[i];
                w.lock().unwrap().fault = None;
                rig.s().output_mgr.inject_projector(id, &format!("rec{}", i + 2), Box::new(Recorder(Arc::clone(w), max_points)));
                unplugged = None;
            }
            _ => {}
        }
        let unplug_now = unplugged.is_some_and(|(_, since)| since == frame);
        let trip = rng.chance(0.03);
        if trip {
            rig.s().emergency_stop(ArmSource::Keyboard);
        }
        let (may_emit, held, caps, cfg, scanning) = {
            let s = rig.s();
            let caps: Vec<OutputLimits> = std::iter::once(s.outputs.active_limits()).chain(ids.iter().map(|id| s.outputs.config_for(id).limits)).collect();
            (s.gate.is_armed() && !s.estop.is_latched(), s.presence.status().midi_hold, caps, s.safety.get(), s.audience.is_scanning())
        };
        let armed_before = rig.s().gate.is_armed();
        rig.tick();
        if rig.s().gate.is_armed() && !armed_before {
            return Err(format!("frame {frame}: the engine frame armed the laser"));
        }
        if unplug_now && rig.s().gate.is_armed() {
            return Err(format!("frame {frame}: a projector's output was lost and the laser stays armed"));
        }
        if unplugged.is_some() && rig.s().gate.interlock_ok(interlock::PROJECTORS) {
            return Err(format!("frame {frame}: a projector has no output and arming is allowed"));
        }
        let all: Vec<&Arc<Mutex<Wire>>> = std::iter::once(&rig.wire).chain(wires.iter().map(|(_, w)| w)).collect();
        for (k, w) in all.iter().enumerate() {
            let w = w.lock().unwrap();
            let what = format!("frame {frame}, projector {k}");
            let lit = w.last.iter().filter(|p| p.is_lit()).count();
            if lit > 0 && !may_emit {
                return Err(format!("{what}: {lit} lit points sent while disarmed or e-stopped"));
            }
            if lit > 0 && trip {
                return Err(format!("{what}: {lit} lit points in the frame the e-stop landed"));
            }
            if lit > 0 && !w.armed {
                return Err(format!("{what}: {lit} lit points sent to a disarmed output"));
            }
            if lit > 0 && hold_to_run && !held {
                return Err(format!("{what}: {lit} lit points with hold-to-run not held"));
            }
            check_points(&w.last, &caps[k], &cfg, &format!("{what}, sent"))?;
            check_public(&w.last, &caps[k], &cfg, scanning, &format!("{what}, sent"))?;
            if let Some(max) = max_points.filter(|&m| w.last.len() > m) {
                return Err(format!("{what}: {} points sent to an output that takes {max}", w.last.len()));
            }
            cov[0] += (k > 0 && lit > 0) as usize;
        }
        let s = rig.s();
        for (k, f) in s.projector_frames.iter().enumerate() {
            check_points(&f.points, &caps[k + 1], &cfg, &format!("frame {frame}, projector {}, preview", k + 1))?;
        }
        if s.projector_frames.len() != n {
            return Err(format!("frame {frame}: {} projector frames for {n} projectors", s.projector_frames.len()));
        }
        cov[1] += unplug_now as usize;
        cov[2] += 1;
    }
    Ok(())
}

#[test]
fn every_projector_keeps_every_output_invariant() {
    let name = "every_projector_keeps_every_output_invariant";
    let mut cov = [0usize; 3];
    let seeds = seeds(277, 300);
    let _guard = property_guard(name, seeds.len(), 300);
    for &seed in &seeds {
        if let Err(e) = run_multi_case(seed, &mut cov) {
            panic!("{name}: {e} (replay: SAFETY_SEED={seed} cargo test -p laser-studio {name})");
        }
    }
    if seeds.len() >= 100 {
        let [lit, unplugs, frames] = cov;
        assert!(lit * 20 > frames, "only {lit} lit frames sent to the other projectors in {frames}");
        assert!(unplugs > seeds.len() / 10, "only {unplugs} unplugged outputs in {} cases", seeds.len());
    }
}

/// `s` on rig `n` of a projector-independence check: `PREVIEWS[n]`
/// more projectors in its venue, preview only (no output).
fn rig_with_previews(mut s: Shared, n: usize, tag: &str, seed: u64) -> Rig {
    let extra = PREVIEWS[n];
    if extra > 0 {
        let mut venue = s.venue.get().clone();
        for i in 0..extra {
            venue.projectors.push(crate::venue::Projector { id: format!("p{}", i + 2), ..Default::default() });
        }
        s.venue.set(venue).unwrap();
    }
    Rig::new(s, &format!("{tag}-{n}"), seed)
}

/// The frame the main output of the first rig was just sent, when every
/// other rig's main output was sent the same one, bit for bit.
fn same_main_frame(rigs: &[&Rig], what: impl Fn() -> String) -> Result<Vec<Point>, String> {
    let first = rigs[0].wire.lock().unwrap().last.clone();
    for (n, rig) in rigs.iter().enumerate().skip(1) {
        let other = rig.wire.lock().unwrap().last.clone();
        if other != first {
            let drift = first.iter().zip(&other).map(|(p, q)| [p.x - q.x, p.y - q.y, p.r - q.r, p.g - q.g, p.b - q.b].iter().fold(0.0f32, |m, d| m.max(d.abs()))).fold(0.0f32, f32::max);
            let previews = PREVIEWS[n];
            return Err(format!("{}: the main output with {previews} preview projector(s) differs ({} vs {} points, up to {drift})", what(), first.len(), other.len()));
        }
    }
    Ok(first)
}

/// The preview projectors of each rig of a projector-independence check.
/// The second rig, without any, is the control: the same session must
/// replay bit for bit (a simulated clock everywhere), else a difference
/// with projectors would prove nothing.
const PREVIEWS: [usize; 4] = [0, 0, 1, 2];

/// Same random session on every rig of `PREVIEWS` (no, one and two more
/// preview-only projectors in the venue, T-277, T-361): every frame the
/// main output is sent is the same, bit for bit. The looks are rendered
/// once per frame and every projector draws from them; nothing (animator,
/// generator, transition, LFO, live stage, optimizer) advances per projector.
#[test]
fn a_preview_projector_leaves_the_main_output_exactly_as_it_was() {
    let lit = std::cell::Cell::new(0usize);
    run_property("a_preview_projector_leaves_the_main_output_exactly_as_it_was", 361, 12, |seed| {
        let mut rigs: Vec<(Rig, Rng, u64)> = (0..PREVIEWS.len())
            .map(|n| {
                let mut rng = Rng::new(seed);
                let mut s = test_support::shared();
                s.safety.set(random_safety(&mut rng), true).unwrap();
                s.outputs.set(MAIN_OUTPUT, Some(random_limits(&mut rng)), None, false, true).unwrap();
                s.calibration = random_calibration(&mut rng);
                let rig = rig_with_previews(s, n, "golden", seed);
                rig.s().request_arm(ArmSource::Ui).unwrap();
                (rig, rng, 0)
            })
            .collect();
        for frame in 0..90 {
            for (rig, rng, beats) in &mut rigs {
                if rng.chance(0.3) {
                    random_action(rig, rng, beats);
                }
                rig.tick();
            }
            let sent = same_main_frame(&rigs.iter().map(|(r, _, _)| r).collect::<Vec<_>>(), || format!("frame {frame}"))?;
            lit.set(lit.get() + sent.iter().any(|p| p.is_lit()) as usize);
        }
        Ok(())
    });
    assert!(lit.get() > 0, "the sessions sent light");
}

/// Every cue of the catalogue (generators, texts, graphics, cue programs
/// with their evolving layers and lanes...), one after the other in one
/// session, on every rig of `PREVIEWS` (T-361): the main output is the
/// same, bit for bit, in every frame of every cue.
#[test]
fn every_catalogue_cue_leaves_the_main_output_exactly_as_it_was() {
    const FRAMES: usize = 12;
    let presets = test_support::shared().presets;
    assert!(presets.iter().any(|p| p.id == "faisceaux-045"), "the scissors that drifted (T-361) are in the catalogue");
    assert!(presets.iter().any(|p| p.settings.program.is_some()), "cue programs (with evolving layers) are in the catalogue");
    let ids: Vec<String> = presets.iter().map(|p| p.id.clone()).collect();
    let _guard = test_support::time_guard("every_catalogue_cue_leaves_the_main_output_exactly_as_it_was", Duration::from_secs(600));
    let mut rigs: Vec<Rig> = (0..PREVIEWS.len()).map(|n| rig_with_previews(test_support::shared(), n, "catalogue", 0)).collect();
    for rig in &rigs {
        rig.s().request_arm(ArmSource::Ui).unwrap();
    }
    let mut dark = Vec::new();
    for id in &ids {
        for rig in &rigs {
            let mut s = rig.s();
            // Alone, from its start (the previous cue may still fade out).
            s.deck.stop_all();
            assert!(controls::press_cue(&mut s, id, Some(ClickMode::Restart), true), "{id} plays");
            controls::press_cue(&mut s, id, Some(ClickMode::Restart), false);
        }
        let mut lit = false;
        for frame in 0..FRAMES {
            for rig in &mut rigs {
                rig.tick();
            }
            let sent = same_main_frame(&rigs.iter().collect::<Vec<_>>(), || format!("cue {id}, frame {frame}")).unwrap();
            lit |= sent.iter().any(|p| p.is_lit());
        }
        if !lit {
            dark.push(id.clone());
        }
    }
    // A cue that draws nothing would prove nothing.
    assert!(dark.len() * 20 < ids.len(), "{} of {} cues sent no light: {dark:?}", dark.len(), ids.len());
}

#[test]
fn each_projector_draws_only_its_zones_through_its_own_caps() {
    let mut rig = armed_square_rig("multi-zones");
    let wires = add_projectors(&mut rig, 2, None);
    {
        let mut s = rig.s();
        // Zone 1 (left) on the main projector, zone 2 (right) on p2, nothing for p3.
        let zone = |id: u8, x: f32, output: &str| ProjectionZone {
            id,
            name: format!("Z{id}"),
            output: Some(output.into()),
            geometry: Geometry { size_x: 0.4, size_y: 0.4, pos_x: x, ..Default::default() },
            ..Default::default()
        };
        s.projection.set(ProjectionConfig { zones: vec![zone(1, -0.5, MAIN_OUTPUT), zone(2, 0.5, "p2")], default_zones: vec![1, 2] }).unwrap();
        s.outputs.set("p2", Some(OutputLimits { max_power: 0.2, max_color: [1.0, 1.0, 1.0] }), None, false, false).unwrap();
    }
    rig.tick();
    rig.s().request_arm(ArmSource::Ui).unwrap();
    for _ in 0..3 {
        rig.tick();
    }
    let main = rig.wire.lock().unwrap().last.clone();
    let p2 = wires[0].1.lock().unwrap().last.clone();
    assert!(main.iter().any(|p| p.is_lit()) && main.iter().filter(|p| p.is_lit()).all(|p| p.x < 0.0), "main: only the left zone");
    assert!(p2.iter().any(|p| p.is_lit()) && p2.iter().filter(|p| p.is_lit()).all(|p| p.x > 0.0), "p2: only the right zone");
    assert!(p2.iter().all(|p| p.r.max(p.g).max(p.b) <= 0.2 + 1e-6), "p2's own cap");
    assert_eq!(lit(&wires[1].1), 0, "p3 has no zone");
    let frames = rig.s().projector_frames.clone();
    assert_eq!(frames.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["p2", "p3"]);
    assert!(frames[0].lit > 0 && frames[1].lit == 0);
}

#[test]
fn the_estop_darkens_every_projector_and_a_lost_one_disarms_all() {
    let mut rig = armed_square_rig("multi-stop");
    let wires = add_projectors(&mut rig, 2, None);
    rig.tick();
    rig.s().request_arm(ArmSource::Ui).unwrap();
    rig.tick();
    assert!(lit(&rig.wire) > 0 && wires.iter().all(|(_, w)| lit(w) > 0), "every projector lit while armed");
    // Escape: every projector dark in the very next frame, every stage disarmed.
    rig.s().emergency_stop(ArmSource::Keyboard);
    rig.tick();
    assert_eq!(lit(&rig.wire), 0);
    assert!(wires.iter().all(|(_, w)| lit(w) == 0 && !w.lock().unwrap().armed));
    {
        let mut s = rig.s();
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        s.request_arm(ArmSource::Ui).unwrap();
    }
    rig.tick();
    assert!(wires.iter().all(|(_, w)| lit(w) > 0), "re-armed by the operator");
    // p3's output is lost: all disarmed, arming refused until it is back.
    wires[1].1.lock().unwrap().fault = Some("câble".into());
    rig.tick();
    assert!(!rig.s().gate.is_armed());
    assert!(rig.s().request_arm(ArmSource::Ui).is_err(), "a projector without its output blocks arming");
    rig.tick();
    assert_eq!(lit(&rig.wire), 0);
    assert_eq!(lit(&wires[0].1), 0);
    let lost = rig.s().gate.last_disarm().map(|(r, _)| r.clone());
    assert_eq!(lost, Some(interlock::DisarmReason::OutputLost));
    // Back: installed disarmed, the operator re-arms.
    wires[1].1.lock().unwrap().fault = None;
    rig.s().output_mgr.inject_projector("p3", "rec3", Box::new(Recorder(Arc::clone(&wires[1].1), None)));
    rig.tick();
    assert!(!rig.s().gate.is_armed() && lit(&wires[1].1) == 0, "it comes back disarmed");
    rig.s().request_arm(ArmSource::Ui).unwrap();
    rig.tick();
    assert!(lit(&wires[1].1) > 0);
}

// ---------- live effects through the engine (T-142) ----------

/// « Noir » held: the armed output and the preview go dark at once, a
/// test pattern too, the gate stays armed, and the light is back on the
/// frame it is released.
#[test]
fn black_hold_darkens_the_output_and_never_disarms() {
    let mut rig = armed_square_rig("black-hold");
    rig.s().request_arm(ArmSource::Ui).unwrap();
    rig.tick();
    assert!(lit(&rig.wire) > 0);
    for pattern in [None, Some(testpattern::TestPattern::WhiteRamp)] {
        {
            let mut s = rig.s();
            s.test_pattern.brightness = 1.0;
            testpattern::select(&mut s, pattern);
            controls::apply(&mut s, "transport.black_hold", ControlInput::Value(1.0), true).unwrap();
        }
        rig.tick();
        assert_eq!(lit(&rig.wire), 0, "{pattern:?}: dark while held");
        assert!(!rig.s().frame.iter().any(|p| p.is_lit()), "{pattern:?}: preview dark");
        assert!(rig.s().gate.is_armed() && rig.wire.lock().unwrap().armed, "{pattern:?}: still armed");
        controls::apply(&mut rig.s(), "transport.black_hold", ControlInput::Value(0.0), true).unwrap();
        rig.tick();
        assert!(lit(&rig.wire) > 0, "{pattern:?}: lit again once released");
    }
}

/// « Figer » held: a spinning look stands still (same frame again and
/// again), then moves once released. The static-beam guard still runs
/// after it.
#[test]
fn freeze_holds_the_motion_while_held() {
    let mut rig = armed_square_rig("freeze");
    controls::apply(&mut rig.s(), "master.rot_z.speed", ControlInput::Value(180.0), false).unwrap();
    rig.tick();
    let frame = |rig: &Rig| rig.s().frame.clone();
    let a = frame(&rig);
    rig.tick();
    assert_ne!(frame(&rig), a, "spins");
    controls::apply(&mut rig.s(), "transport.freeze", ControlInput::Value(1.0), true).unwrap();
    rig.tick();
    let held = frame(&rig);
    for _ in 0..10 {
        rig.tick();
        assert_eq!(frame(&rig), held, "frozen");
    }
    controls::apply(&mut rig.s(), "transport.freeze", ControlInput::Value(0.0), true).unwrap();
    rig.tick();
    rig.tick();
    assert_ne!(frame(&rig), held, "moves again");
}

/// Prism and mirror copies through the whole engine: the lit runs that
/// go out are entered from the dark, and the safety stage still applies
/// (a Blank zone over the left half stays dark).
#[test]
fn prism_and_mirror_copies_go_through_the_safety_stage() {
    let mut rig = armed_square_rig("prism");
    {
        let mut s = rig.s();
        let left = Zone { kind: ZoneKind::Blank, points: vec![[-1.0, -1.0], [0.0, -1.0], [0.0, 1.0], [-1.0, 1.0]], ..Default::default() };
        s.safety.set(SafetySettings { zones: vec![left], horizon: Horizon { y: -1.0, ..Default::default() }, ..Default::default() }, true).unwrap();
        controls::apply(&mut s, "master.prism", ControlInput::Value(4.0), true).unwrap();
        controls::apply(&mut s, "master.mirror", ControlInput::Value(3.0), true).unwrap();
        s.request_arm(ArmSource::Ui).unwrap();
    }
    rig.tick();
    let w = rig.wire.lock().unwrap();
    let pts = &w.last;
    assert!(pts.iter().any(|p| p.is_lit()));
    assert!(pts.iter().filter(|p| p.is_lit()).all(|p| p.x >= -1e-3), "nothing lit in the Blank zone");
    let runs = (1..pts.len()).filter(|&i| pts[i].is_lit() && !pts[i - 1].is_lit()).count();
    assert!(runs >= 4, "{runs} lit runs");
}
