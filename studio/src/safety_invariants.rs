//! Safety invariants (T-264): property tests over the whole output path.
//!
//! Each case builds a studio state, then drives the real engine frame
//! (`Engine::tick`, the very function the 60 fps thread runs) on a
//! simulated clock, with random actions between frames: cues, looks and
//! live modifiers sent as an API client could send them, LFOs, audio
//! routes and features, MIDI, the timeline, test patterns, zones, caps,
//! calibration, arm / disarm / e-stop. A recording output captures what
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
//! 6. no sequence of controls, MIDI or API calls arms, except the
//!    explicit user paths (UI / keyboard / API arm request, and the MIDI
//!    opt-in gesture when the operator allowed it).
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

use crate::controls::{self, ControlInput};
use crate::cues::ClickMode;
use crate::engine::{Calibration, Settings};
use crate::interlock::ArmSource;
use crate::midi::{self, decode::MidiMsg, MidiEvent};
use crate::output::Output;
use crate::patterns::Point;
use crate::power::{OutputLimits, MAIN_OUTPUT};
use crate::presence::{Presence, PresenceSettings};
use crate::safety::{self, SafetySettings};
use crate::zones::{self, Horizon, Zone, ZoneKind};
use crate::{dwell, interlock, lfo, live, test_support, testpattern, timeline, Engine, Shared, FRAME_INTERVAL};
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

struct Recorder(Arc<Mutex<Wire>>);

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
        let dir = std::env::temp_dir().join(format!("laser-studio-invariants-{tag}-{seed}-{}", std::process::id()));
        let shared = Mutex::new(s);
        let wire = Arc::new(Mutex::new(Wire::default()));
        let engine = Engine::new(&shared, Some(Box::new(Recorder(Arc::clone(&wire)))), dir.join("live.json"), dir.join("layers.json"));
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
            let _ = controls::apply(&mut s, &id, input, external);
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
}

/// Runs one fuzz case; `Err` describes the first broken invariant.
fn run_case(seed: u64, frames: usize, cov: &mut Coverage) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let (s, hold_to_run) = random_case(&mut rng);
    let mut rig = Rig::new(s, "pipeline", seed);
    let mut beats = 0;
    for frame in 0..frames {
        for _ in 0..rng.below(3) {
            let was = rig.s().gate.is_armed();
            let explicit = random_action(&mut rig, &mut rng, &mut beats);
            let is = rig.s().gate.is_armed();
            if is && !was && !explicit {
                return Err(format!("frame {frame}: armed by a non-arming action"));
            }
        }
        // The fuzz's pages never hold the hold-to-run key; the controller
        // pad (`safety.hold`) may be pressed by a random control.
        let (may_emit, caps, cfg, held) = {
            let s = rig.s();
            (s.gate.is_armed() && !s.estop.is_latched(), s.outputs.active_limits(), s.safety.get(), s.presence.status().midi_hold)
        };
        let armed_before = rig.s().gate.is_armed();
        rig.tick();
        if rig.s().gate.is_armed() && !armed_before {
            return Err(format!("frame {frame}: the engine frame armed the laser"));
        }
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
        drop(w);
        let s = rig.s();
        check_points(&s.frame, &caps, &cfg, &format!("frame {frame}, preview"))?;
        let preview_lit = s.frame.iter().any(|p| p.is_lit());
        cov.frames += 1;
        cov.sent_lit += (lit > 0) as usize;
        cov.lit_with_zones += (preview_lit && cfg.zones.iter().any(|z| z.kind == ZoneKind::Blank)) as usize;
        cov.lit_with_horizon += (preview_lit && cfg.horizon.lines && cfg.horizon.level == 0.0) as usize;
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

/// Invariants 1, 2, 3, 5 and 6 over random sessions.
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
        assert!(share(cov.sent_lit) > 0.1, "only {} of {} frames sent light", cov.sent_lit, cov.frames);
        assert!(share(cov.lit_with_zones) > 0.1, "only {} of {} lit frames had a Blank zone", cov.lit_with_zones, cov.frames);
        assert!(share(cov.lit_with_horizon) > 0.05, "only {} of {} lit frames had a full horizon", cov.lit_with_horizon, cov.frames);
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

/// `fit_frame` (DACs with a small frame buffer) must not undo the safety
/// stage: a thinned frame may only lose light, never light a straight
/// shortcut through a Blank zone.
fn run_thinning_case(seed: u64) -> Result<(), String> {
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
    let shaped = safety::apply(raw, 0.0, &cfg, &mut safety::StrobeLimiter::default());
    if zones::blank_violations(&shaped, &cfg.zones) > 0 {
        return Err("the safety stage itself left light in a Blank zone".into());
    }
    let max = 2 + rng.below(shaped.len());
    let thin = crate::output::fit_frame(&shaped, max);
    let v = zones::blank_violations(&thin, &cfg.zones);
    if v > 0 {
        return Err(format!("thinning {} → {max} points lit {v} segments through a Blank zone", shaped.len()));
    }
    Ok(())
}

/// Known gap, see the PR note: `fit_frame` keeps a kept point lit when
/// every skipped point was lit, but a lit polyline that went *around* a
/// Blank zone becomes a straight chord that can cut *through* it.
#[test]
#[ignore = "bug: DAC frame thinning (output::fit_frame) can draw a lit chord through a Blank zone"]
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

