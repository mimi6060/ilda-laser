//! Safety invariants (T-264): property tests over the whole output path.
//!
//! Each case builds a studio state, then drives the real engine frame
//! (`Engine::tick`, the very function the 60 fps thread runs) on a
//! simulated clock, with random actions between frames: cues, looks and
//! live modifiers sent as an API client could send them, LFOs, audio
//! routes and features, MIDI, the timeline, test patterns, zones, caps,
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
//!
//! Plus the strobe limiter seen from the output: no fast flashing for
//! longer than the allowed burst.
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
use crate::cues::{ClickMode, CueSlot};
use crate::engine::{Calibration, Settings};
use crate::interlock::ArmSource;
use crate::midi::{self, decode::MidiMsg, MidiEvent};
use crate::output::Output;
use crate::patterns::Point;
use crate::power::{OutputLimits, MAIN_OUTPUT};
use crate::presence::{Presence, PresenceSettings};
use crate::projection::{Geometry, ProjectionConfig, ProjectionZone, Warp, MAX_ZONES, SQUARE};
use crate::safety::{self, SafetySettings};
use crate::zones::{self, Horizon, Zone, ZoneKind};
use crate::{audience, dwell, interlock, lfo, live, test_support, testpattern, timeline, Engine, Shared, FRAME_INTERVAL};
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
        let shared = Mutex::new(s);
        let wire = Arc::new(Mutex::new(Wire::default()));
        let engine = Engine::new(&shared, Some(Box::new(Recorder(Arc::clone(&wire), max_points))), dir.join("live.json"), dir.join("layers.json"));
        Rig { shared, engine: Some(engine), wire, now: Instant::now(), dir }
    }

    fn s(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    /// One engine frame, 1/60 s after the previous one.
    fn tick(&mut self) {
        self.now += FRAME_INTERVAL;
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
            let slot = CueSlot { zones, ..s.deck.slot(&id) };
            s.deck.set_slot(&id, slot);
        }
        _ => {
            let ids: Vec<u8> = s.projection.get().zones.iter().map(|z| z.id).collect();
            s.projection.align = if rng.chance(0.3) { None } else { Some(*rng.pick(&ids)) };
        }
    }
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

fn one_event_show(rng: &mut Rng, cue: &str) -> timeline::Show {
    let event = timeline::Event {
        id: 1,
        start: rng.range(0.0, 2.0) as f64,
        len: rng.range(0.1, 30.0) as f64,
        source: timeline::EventSource::Cue { id: cue.into() },
        modifiers: if rng.chance(0.5) { mutated(&live::LiveModifiers::default(), rng, 0.2) } else { Default::default() },
        ..Default::default()
    };
    timeline::Show { name: "fuzz".into(), tracks: vec![timeline::Track { layer: 1 + rng.below(4) as u8, events: vec![event], ..Default::default() }], ..Default::default() }
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
            let show = one_event_show(rng, &id);
            s.timeline.load(show);
            let _ = controls::timeline_play(&mut s);
        }
        18 => {
            let bpm = rng.range(40.0, 220.0) as f64;
            s.tempo.set_bpm_manual(bpm, t);
            s.mixer = mutated(&s.mixer.clone(), rng, 0.2);
            s.mixer_dirty = true;
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
    /// Frames with a lit preview and a Public zone, scanning off / on.
    lit_with_public_above: usize,
    lit_with_public_scanning: usize,
}

/// Runs one fuzz case; `Err` describes the first broken invariant.
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
    let mut beats = 0;
    for frame in 0..frames {
        if zrng.chance(0.15) {
            random_routing(&mut rig.s(), &mut zrng);
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
        rig.tick();
        if rig.s().gate.is_armed() && !armed_before {
            return Err(format!("frame {frame}: the engine frame armed the laser"));
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
        check_points(&w.last, &caps, &cfg, &format!("frame {frame}, sent"))?;
        check_public(&w.last, &caps, &cfg, scanning, &format!("frame {frame}, sent"))?;
        if let Some(max) = max_points.filter(|&m| w.last.len() > m) {
            return Err(format!("frame {frame}: {} points sent to an output that takes {max}", w.last.len()));
        }
        drop(w);
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
    }
    Ok(())
}

fn run_property(name: &str, property: u64, cases: usize, case: impl Fn(u64) -> Result<(), String>) {
    for seed in seeds(property, cases) {
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
    for &seed in &seeds {
        if let Err(e) = run_case(seed, 30, &mut cov) {
            panic!("{name}: {e} (replay: SAFETY_SEED={seed} cargo test -p laser-studio {name})");
        }
    }
    if seeds.len() > 100 {
        let share = |n: usize| n as f64 / cov.frames as f64;
        eprintln!("coverage: Public zones lit {:.3} above, {:.3} scanning", share(cov.lit_with_public_above), share(cov.lit_with_public_scanning));
        assert!(share(cov.sent_lit) > 0.1, "only {} of {} frames sent light", cov.sent_lit, cov.frames);
        assert!(share(cov.lit_with_zones) > 0.1, "only {} of {} lit frames had a Blank zone", cov.lit_with_zones, cov.frames);
        assert!(share(cov.lit_with_horizon) > 0.05, "only {} of {} lit frames had a full horizon", cov.lit_with_horizon, cov.frames);
        assert!(share(cov.lit_with_projection) > 0.2, "only {} of {} lit frames went through a projection zone", cov.lit_with_projection, cov.frames);
        assert!(share(cov.lit_with_public_above) > 0.1, "only {} of {} lit frames had a Public zone, scanning off", cov.lit_with_public_above, cov.frames);
        assert!(share(cov.lit_with_public_scanning) > 0.05, "only {} of {} lit frames had a Public zone while scanning", cov.lit_with_public_scanning, cov.frames);
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
    let mut rng = Rng::new(seed);
    let mut s = test_support::shared();
    let mut cfg = SafetySettings { strobe_max_hz: rng.range(1.0, 4.0), strobe_burst_s: rng.range(0.5, 3.0), ..Default::default() };
    if rng.chance(0.5) {
        cfg.dwell.profile = dwell::DwellProfile::Strict;
    }
    s.safety.set(cfg.clone(), true).map_err(|e| format!("{e:?}"))?;
    let presets: Vec<_> = s.presets.iter().filter(|p| p.settings.gate == crate::engine::GateMode::Off).map(|p| (p.id.clone(), p.settings.clone())).collect();
    let (cue, look) = rng.pick(&presets).clone();
    s.settings = look;
    let bpm = rng.range(90.0, 200.0) as f64;
    s.tempo.set_bpm_manual(bpm, 0.0);
    let hz = rng.range(6.0, 15.0);
    let way = rng.below(4);
    match way {
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
    let mut rig = Rig::new(s, "strobe", seed);
    let mut meter = FlashMeter { peak: 0.0, on: false, onsets: Vec::new() };
    let frames = 9 * 60;
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
            "way {way}: {:.2} s of output flashing faster than {:.2} Hz (burst limit {:.2} s, {} flashes in 9 s)",
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
    run_property("strobe_never_exceeds_the_burst_limit", 2, 40, run_strobe_case);
}

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
    for seed in seeds(4, 1000) {
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

