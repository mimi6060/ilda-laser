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
//! 16. cue and layer modifiers (T-144): random level controls (from the
//!     UI or a controller, on layers and on cues on show or not), mangled
//!     cell, layer and working-copy modifiers, « Enregistrer dans le cue »
//!     and the smoothing run in the sessions, so 1-15 hold with every
//!     level on; they never arm, and they apply before the safety stage
//!     (`cue_and_layer_modifiers_apply_before_the_safety_stage`).
//! 17. the executor bank (T-418): random banks (cues, layers, group
//!     masters, the cue list; mangled ones sanitised) played from the UI,
//!     a controller or the external gate (faders, Flash / Temp / Noir
//!     tenu held and let go, pages, `exec.cur.*`) run in the sessions, so
//!     1-16 hold with executor levels on; they never arm, and every cue's
//!     executor level stays within 0..1.
//!
//! 17. speed groups and the *Cadence* (T-419): random groups (multiples,
//!     free BPMs, mangled values refused), cells, layers and modulators in
//!     random groups and random fade rates run in the sessions (1-16 hold
//!     with them); a ×4 group never gets flashing past the strobe limiter
//!     (`speed_groups_never_get_past_the_strobe_limiter`).
//!
//! 18. safety profiles (T-260): random venue profiles (one damaged) are
//!     loaded during the sessions from every source, confirmed or not;
//!     only the operator's sources load one, a load never arms and always
//!     leaves the laser disarmed, and a refused one changes no setting
//!     (1-17 hold with whatever is in force).
//! 19. macros (T-425): random macro lists (any line the command line
//!     accepts, macros launching macros, waits in seconds, beats and Go,
//!     refused lines that must not save) launched, advanced and stopped
//!     from the UI or a controller run in the sessions (1-18 hold with
//!     them); they never arm, no macro runs across a frame with the
//!     e-stop latched, and a strobe built from timed steps stays within
//!     the burst (`macros_never_get_past_the_strobe_limiter`).
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
//! **Coverage** (T-473): every session has a show phase where the
//! operator keeps the laser armed and lit while every stream keeps
//! playing, and the random sessions assert, with about a 2× margin, how
//! many frames, show-phase frames and sessions sent light. A new dimming
//! feature must not push these to their edge: brighten its own stream
//! rather than leave controls out of `random_action`.
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
use crate::projection::{Geometry, ProjectionConfig, ProjectionZone, Warp, ZoneGroup, MAX_ZONES, SQUARE};
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

pub(crate) fn random_safety(rng: &mut Rng) -> SafetySettings {
    let zones = (0..rng.below(4))
        .map(|i| Zone {
            id: i as u32 + 1,
            name: format!("Z{i}"),
            kind: if rng.chance(0.6) { ZoneKind::Blank } else { ZoneKind::Dim },
            level: rng.f(),
            points: random_polygon(rng),
            audience: false,
            // Soft edges and gradients (T-446), on Blank zones too: there
            // they must change nothing (invariant 3 still holds).
            feather: if rng.chance(0.4) { rng.range(0.0, zones::MAX_FEATHER) } else { 0.0 },
            gradient: rng.chance(0.3).then(|| zones::ZoneGradient {
                from: [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)],
                to: [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)],
                level_to: rng.f(),
            }),
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

/// A venue's safety profile (T-260): random zones, limiter, caps,
/// presence and audience permission.
fn random_profile(rng: &mut Rng, name: &str) -> crate::profiles::SafetyProfile {
    crate::profiles::SafetyProfile {
        name: name.into(),
        audience_scan_allowed: rng.chance(0.3),
        safety: random_safety(rng),
        limits: [(MAIN_OUTPUT.to_string(), random_limits(rng))].into_iter().collect(),
        presence: rng.chance(0.5).then(|| PresenceSettings { hold_to_run: rng.chance(0.5), ..Default::default() }),
        ..Default::default()
    }
}

pub(crate) fn random_limits(rng: &mut Rng) -> OutputLimits {
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
    ProjectionConfig { zones, default_zones, groups: Vec::new() }
}

/// Zone groups with random masters (T-420): zones that may overlap
/// between groups (products), levels out of range on purpose.
fn random_groups(rng: &mut Rng, ids: &[u8]) -> Vec<ZoneGroup> {
    if rng.chance(0.3) {
        return Vec::new();
    }
    (0..1 + rng.below(4))
        .map(|i| {
            let zones = (0..1 + rng.below(3)).map(|_| *rng.pick(ids)).collect();
            // Created lit (mostly): the moves below reach 0 and out of range.
            let master = if rng.chance(0.2) { random_master(rng) } else { rng.range(0.8, 1.0) };
            ZoneGroup { name: format!("G{i}"), zones, master }
        })
        .collect()
}

/// A group master as a control or a damaged file could ask for it.
fn random_master(rng: &mut Rng) -> f32 {
    // Mostly lit, so the sessions keep sending light (coverage).
    match rng.below(50) {
        0 => 0.0,
        1 => rng.range(-2.0, 3.0),
        2..=9 => 1.0,
        10..=13 => rng.f(),
        _ => rng.range(0.8, 1.0),
    }
}

/// Zone groups on the current zones (T-420), as `POST /api/projection`
/// would set them (a new group takes the level it comes with).
fn random_zone_groups(s: &mut Shared, rng: &mut Rng) {
    let mut cfg = s.projection.get().clone();
    let ids: Vec<u8> = cfg.zones.iter().map(|z| z.id).collect();
    cfg.groups = random_groups(rng, &ids);
    let _ = s.projection.set(cfg);
}

/// Group masters moved the way a fader, MIDI, OSC or DMX would (through
/// the controls, values out of range included), or « Tout à 100 % ».
fn random_group_masters(s: &mut Shared, rng: &mut Rng) {
    let n = s.projection.get().groups.len();
    if n == 0 || rng.chance(0.1) {
        let _ = controls::apply(s, "group.all_full", ControlInput::Value(1.0), rng.chance(0.5));
        return;
    }
    for _ in 0..1 + rng.below(n) {
        let id = format!("group.{}.master", 1 + rng.below(n));
        let input = if rng.chance(0.5) { ControlInput::Norm(random_master(rng)) } else { ControlInput::Value(random_master(rng)) };
        let _ = controls::apply(s, &id, input, rng.chance(0.5));
    }
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

/// Speed groups and the *Cadence* (T-419) as a client or a file could
/// set them, mangled now and then (NaN, negative BPMs, unknown multiples
/// and groups: refused), cells and layers put in random groups (some that
/// don't exist).
fn random_speed(s: &mut Shared, rng: &mut Rng) {
    use crate::speed::{SpeedMode, SPEED_MULTS};
    let (beat, t, bpb) = s.speed_clock();
    s.speed.change_on_bar = rng.chance(0.5);
    for g in 0..6u8 {
        let mode = match rng.below(5) {
            0 | 1 => SpeedMode::Multiple { mult: *rng.pick(&SPEED_MULTS) },
            2 => SpeedMode::Multiple { mult: rng.range(-1.0, 9.0) },
            3 => SpeedMode::FreeBpm { bpm: rng.range(-100.0, 600.0) as f64 },
            _ => SpeedMode::FreeBpm { bpm: if rng.chance(0.5) { f64::NAN } else { f64::INFINITY } },
        };
        let _ = s.speed.set_mode(g, mode, beat, t, bpb);
    }
    let rate = if rng.chance(0.1) { f32::NAN } else { rng.range(-1.0, 4.0) };
    let _ = s.speed.set_rate(rate);
    let (id, _) = random_preset(s, rng);
    let slot = s.deck.slot(&id);
    s.deck.set_slot(&id, CueSlot { speed_group: rng.chance(0.7).then(|| rng.below(7) as u8), ..slot });
    s.mixer.layer_mut(1 + rng.below(crate::layers::LAYER_COUNT) as u8).speed_group = rng.below(7) as u8;
    if rng.chance(0.2) {
        let (bar_beat, t_eff) = (s.tempo.beat_at(s.now_s()), t);
        s.speed.resync(bar_beat, t_eff, bpb);
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
            // A..D, and now and then a group that doesn't exist (T-419).
            speed_group: rng.below(6) as u8,
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
            // Group masters (T-420) included: their own stream (`mrng` in
            // `run_case`) moves them more often, this one like any other.
            // The executors (T-418) have their own stream (`brng`): 226
            // ids would otherwise take a third of these picks.
            // The macros (T-425) too (`grng`-like stream `krng`), so this
            // stream's picks stay what they were.
            let ids: Vec<String> = s.controls.list().iter().filter(|d| d.group != "exec" && d.group != "macro").map(|d| d.id.clone()).collect();
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
            // The layers' modifiers (T-144) have their own stream (`random_level`).
            let mods: Vec<live::LiveModifiers> = s.mixer.layers.iter().map(|l| l.modifiers.clone()).collect();
            s.mixer = mutated(&s.mixer.clone(), rng, 0.2);
            for (l, m) in s.mixer.layers.iter_mut().zip(mods) {
                l.modifiers = m;
            }
            s.mixer_dirty = true;
            if rng.chance(0.5) {
                random_scan(&mut s, rng);
            }
            if rng.chance(0.5) {
                random_speed(&mut s, rng);
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
        cfg.zones.push(Zone { id, name: format!("Public {id}"), kind: ZoneKind::Dim, level: rng.range(0.0, audience::MAX_AUDIENCE_LEVEL), points: random_polygon(rng), audience: true, ..Default::default() });
    }
    for z in &mut cfg.zones {
        if rng.chance(0.5) {
            z.audience = true;
        }
        if z.audience {
            z.level = z.level.min(audience::MAX_AUDIENCE_LEVEL);
            if let Some(g) = &mut z.gradient {
                g.level_to = g.level_to.min(audience::MAX_AUDIENCE_LEVEL);
            }
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
        if let Some(g) = &mut z.gradient {
            g.level_to = g.level_to.min(audience::MAX_AUDIENCE_LEVEL);
        }
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
    // The venue's safety profile allows it (T-260), confirmed as the
    // operator would.
    let allow = crate::profiles::SaveRequest { name: "Fuzz public".into(), venue: None, notes: None, audience_scan_allowed: Some(true) };
    crate::profiles::save(s, allow, true, ArmSource::Ui).expect("profile saved");
}

/// How much light the fuzz really exercised, so a generator that stops
/// lighting anything can't make the checks pass vacuously.
#[derive(Default)]
struct Coverage {
    frames: usize,
    /// Frames that sent lit points to the output.
    sent_lit: usize,
    /// Sessions, and those that sent light at least once.
    sessions: usize,
    sessions_lit: usize,
    /// Frames of the show phase (`keep_show_on`), and those that sent
    /// light: the share that tells a dimming feature drowning the fuzz
    /// apart from sessions that were never armed.
    show_frames: usize,
    show_sent_lit: usize,
    /// Frames that sent light while a zone group's master was between 0
    /// and 100 % (T-420): the masters dim the output, not only the preview.
    sent_lit_with_group_master: usize,
    /// Frames with a lit preview while at least one Blank zone was set.
    lit_with_zones: usize,
    /// Frames with a lit preview below a full horizon at level 0.
    lit_with_horizon: usize,
    /// Frames with a lit preview through a non-neutral projection zone.
    lit_with_projection: usize,
    /// Frames with a lit preview while a zone group's master was below
    /// 100 % (T-420).
    lit_with_group_master: usize,
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
    /// Frames with a lit preview while an FX grid line played (T-146).
    lit_with_fx: usize,
    /// Frames with a lit preview while a cue or layer level was not
    /// neutral (T-144).
    lit_with_levels: usize,
    /// Frames with a lit preview while a cue played below full executor
    /// level (T-418).
    lit_with_exec: usize,
}

/// A random executor bank (T-418), as `POST /api/executors` could send it
/// (or a mangled file: `set_bank` sanitises).
fn random_bank(s: &mut Shared, rng: &mut Rng) {
    use crate::executors::{ButtonFn, ExecPage, ExecTarget, Executor, ExecutorBank, FaderFn};
    const BUTTONS: [ButtonFn; 6] = [ButtonFn::None, ButtonFn::Toggle, ButtonFn::Flash, ButtonFn::Temp, ButtonFn::BlackHeld, ButtonFn::Go];
    let pages = (0..1 + rng.below(3))
        .map(|_| ExecPage {
            name: String::new(),
            handles: (0..crate::executors::EXEC_PER_PAGE)
                .map(|_| {
                    let target = match rng.below(12) {
                        0..=6 => ExecTarget::Cue { id: random_preset(s, rng).0 },
                        7 => ExecTarget::Layer { n: 1 + rng.below(5) as u8 },
                        8 => ExecTarget::GroupMaster { group: 1 + rng.below(3) },
                        9 => ExecTarget::CueList,
                        // A speed group (T-419), now and then one past D.
                        10 => ExecTarget::SpeedGroup { n: 1 + rng.below(5) as u8 },
                        _ => ExecTarget::None,
                    };
                    let fader = *rng.pick(&[FaderFn::FadeIn, FaderFn::FadeIn, FaderFn::Level, FaderFn::Speed]);
                    Executor { label: String::new(), target, fader, buttons: [*rng.pick(&BUTTONS), *rng.pick(&BUTTONS)] }
                })
                .collect(),
        })
        .collect();
    s.executors.set_bank(ExecutorBank { pages });
}

/// A random macro list (T-425), as `POST /api/macros` could send it:
/// lines from the palette (values drawn at random, kept only when they
/// parse), macros launching others, random waits; now and then a refused
/// line, which must make the whole list refused.
fn random_macros(s: &mut Shared, rng: &mut Rng) -> Result<(), String> {
    use crate::macros::{Macro, MacroStep, Wait};
    let actions = crate::command::actions(&s.controls, &s.presets);
    let n = 1 + rng.below(4);
    let mut refused = false;
    let mut list = Vec::new();
    for i in 0..n {
        let mut steps = Vec::new();
        for _ in 0..1 + rng.below(8) {
            let command = match rng.below(12) {
                0 if rng.chance(0.3) => {
                    refused = true;
                    rng.pick(&["armer", "transport.arm on", "safety.hold on", "estop.reset"]).to_string()
                }
                // Only later macros: no cycle.
                1 if i + 1 < n => format!("macro.m{}.go", i + 2 + rng.below(n - i - 1)),
                2 => String::new(),
                3 => "noir".to_string(),
                _ => {
                    let a = rng.pick(&actions);
                    let line = if a.needs_value {
                        let v = if rng.chance(0.3) { rng.pick(&["on", "off", "1", "2"]).to_string() } else { format!("{:.1}", rng.range(-20.0, 220.0)) };
                        format!("{} {v}", a.command)
                    } else {
                        a.command.clone()
                    };
                    if crate::command::parse(&line, &s.controls, &s.presets).is_ok() { line } else { String::new() }
                }
            };
            let wait = match rng.below(6) {
                0 | 1 => Wait::None,
                2 | 3 => Wait::Seconds(rng.range(0.05, 1.5)),
                4 => Wait::Beats(*rng.pick(&[0.125, 0.25, 0.5, 1.0, 2.0])),
                _ => Wait::Go,
            };
            steps.push(MacroStep { command, wait, enabled: rng.chance(0.9) });
        }
        list.push(Macro { id: format!("m{}", i + 1), name: format!("Fuzz {}", i + 1), steps, ..Default::default() });
    }
    let r = crate::macros::set(s, list);
    if refused && r.is_ok() {
        return Err("a macro with a refused line was saved".into());
    }
    Ok(())
}

/// One macro move from the UI, a controller or the external gate:
/// launched, Go, stopped, all stopped.
fn random_macro_move(s: &mut Shared, rng: &mut Rng, now: Instant) {
    let n = s.macros.list().len().max(1);
    let (id, input) = match rng.below(10) {
        0..=5 => (format!("macro.m{}.go", 1 + rng.below(n)), ControlInput::Value(1.0)),
        6 | 7 => ("macro.go".to_string(), ControlInput::Value(1.0)),
        8 => ("macro.stop_all".to_string(), ControlInput::Value(1.0)),
        _ => {
            let id = format!("m{}", 1 + rng.below(n));
            crate::macros::stop(s, &id);
            return;
        }
    };
    if rng.chance(0.2) {
        let _ = crate::external::set_open(s, true, ArmSource::Ui);
        let src = *rng.pick(&crate::external::SourceKind::ALL);
        let _ = crate::external::apply(s, src, "rig", &id, input, now);
    } else {
        let _ = controls::apply(s, &id, input, rng.chance(0.5));
    }
}

/// One executor move from the UI, a controller or the external gate:
/// faders (mostly lit), buttons pressed and let go, pages.
fn random_exec_move(s: &mut Shared, rng: &mut Rng, now: Instant) {
    let pages = s.executors.bank.pages.len();
    // Now and then a page the bank doesn't have (refused).
    let p = if rng.chance(0.05) { pages + 1 } else { 1 + rng.below(pages) };
    let n = 1 + rng.below(crate::executors::EXEC_PER_PAGE);
    let page = if rng.chance(0.3) { "cur".to_string() } else { p.to_string() };
    let (id, input) = match rng.below(10) {
        0..=4 => {
            let v = match rng.below(10) {
                0 | 1 => 0.0,
                2 => rng.range(-2.0, 3.0),
                _ => rng.range(0.2, 1.0),
            };
            (format!("exec.{page}.{n}.fader"), ControlInput::Value(v))
        }
        5..=7 => {
            let b = if rng.chance(0.5) { "button" } else { "button2" };
            // Held buttons are mostly let go of.
            (format!("exec.{page}.{n}.{b}"), ControlInput::Value(if rng.chance(0.4) { 1.0 } else { 0.0 }))
        }
        8 => (rng.pick(&["exec.page.next", "exec.page.prev"]).to_string(), ControlInput::Value(1.0)),
        _ => (format!("exec.page.{}", 1 + rng.below(crate::executors::EXEC_MAX_PAGES)), ControlInput::Value(1.0)),
    };
    if rng.chance(0.2) {
        let _ = crate::external::set_open(s, true, ArmSource::Ui);
        let src = *rng.pick(&crate::external::SourceKind::ALL);
        let _ = crate::external::apply(s, src, "rig", &id, input, now);
    } else {
        let _ = controls::apply(s, &id, input, rng.chance(0.5));
    }
}

/// Cue and layer modifiers (T-144) as the page, a controller or a bad
/// client could set them: level controls on any layer and on a cue on
/// show (or one that isn't), mangled stored and working copies,
/// « Enregistrer dans le cue », the smoothing switched and its inertia.
fn random_level(s: &mut Shared, rng: &mut Rng) {
    let playing = s.deck.active.first().map(|a| a.cue.clone());
    let cue = match &playing {
        Some(id) if rng.chance(0.8) => id.clone(),
        _ => random_preset(s, rng).0,
    };
    match rng.below(6) {
        0..=2 => {
            let param = *rng.pick(crate::levels::PARAMS);
            let target = if rng.chance(0.5) { format!("layer.{}", 1 + rng.below(4)) } else { format!("cue.{cue}") };
            // Mostly lit: a level dimmed to black would hide the rest.
            let input = match param {
                "brightness" | "trace" if rng.chance(0.8) => ControlInput::Norm(rng.range(0.5, 1.0)),
                _ if rng.chance(0.8) => ControlInput::Norm(rng.f()),
                _ => ControlInput::Value(rng.range(-5.0, 30.0)),
            };
            let _ = controls::apply(s, &format!("{target}.{param}"), input, rng.chance(0.5));
        }
        3 => {
            // Cell modifiers from a mangled file (sanitised by the deck)…
            let mut slot = s.deck.slot(&cue);
            slot.modifiers = mutated(&live::LiveModifiers::default(), rng, 0.3);
            s.deck.set_slot(&cue, slot);
        }
        4 => {
            // …and layer and working copies the engine must cope with as they are.
            let n = rng.below(4);
            s.mixer.layers[n].modifiers = mutated(&live::LiveModifiers { size: 0.9, ..Default::default() }, rng, 0.3);
            if let Some(id) = playing {
                s.cue_mods.insert(id, mutated(&live::LiveModifiers { size: 0.9, ..Default::default() }, rng, 0.3));
            }
        }
        _ => {
            let _ = crate::levels::store(s, &cue, rng.chance(0.5));
            let _ = controls::apply(s, "smoothing.on", ControlInput::Value(rng.below(2) as f32), rng.chance(0.5));
            let _ = controls::apply(s, "smoothing.inertia", ControlInput::Norm(rng.f()), rng.chance(0.5));
        }
    }
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

/// The FX grid (T-146) as an API client could set it: the starter
/// cells mangled (numbers, targets, strobes, colours), random *Action*
/// faders, destination and click mode.
fn random_fx(s: &mut Shared, rng: &mut Rng) {
    let cells = mutated(&crate::fx::starter_grid().cells(), rng, 0.15);
    let reg = controls::ControlRegistry::build(&s.presets);
    s.fx.replace_cells(cells, &reg);
    for line in 0..crate::fx::LINES {
        s.fx.set_action(line, if rng.chance(0.2) { rng.range(-1.0, 2.0) } else { 1.0 });
    }
    s.fx.set_dest(*rng.pick(&crate::fx::DESTS));
    s.fx.set_click(*rng.pick(&crate::fx::CLICKS));
}

/// An FX grid move from a controller or the page: a cell pressed or
/// released, a fader, *Arrêter FX*, the destination.
fn random_fx_move(s: &mut Shared, rng: &mut Rng) {
    let id = match rng.below(10) {
        0 => "fx.stop".to_string(),
        1 => format!("fx.{}.action", 1 + rng.below(crate::fx::LINES)),
        2 => "fx.dest".to_string(),
        _ => format!("fx.{}.{}", 1 + rng.below(crate::fx::LINES), 1 + rng.below(crate::fx::CELLS)),
    };
    let input = if rng.chance(0.7) { ControlInput::Value(rng.chance(0.6) as u8 as f32) } else { ControlInput::Norm(rng.f()) };
    let _ = controls::apply(s, &id, input, rng.chance(0.5));
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

/// The operator during the show phase (T-473), once per frame before
/// the other streams play: the page open (it beats), the hold-to-run pad
/// down, « Noir » let go, the e-stop reset and an arm request from the UI
/// whenever the laser is off; on its first frame, group masters back to
/// 100 % and a cue on. Everything the other streams do afterwards in the
/// frame (disarm, e-stop, dimming, « Noir »…) still lands and is checked:
/// this only makes sure each session spends a stretch armed and lit.
fn keep_show_on(rig: &mut Rig, rng: &mut Rng, first: bool) {
    let now = rig.now;
    let mut s = rig.s();
    if s.presence.enforced {
        s.presence.beat("fuzz-show", now, true, false);
    }
    if s.presence.settings.hold_to_run {
        let _ = controls::apply(&mut s, "safety.hold", ControlInput::Value(1.0), true);
    }
    if s.live.black {
        let _ = controls::apply(&mut s, "transport.black_hold", ControlInput::Value(0.0), false);
    }
    if first {
        let _ = controls::apply(&mut s, "master.reset", ControlInput::Value(1.0), false);
        let _ = controls::apply(&mut s, "group.all_full", ControlInput::Value(1.0), false);
        for n in 1..=crate::layers::LAYER_COUNT {
            let _ = controls::apply(&mut s, &format!("layer.{n}.dimmer"), ControlInput::Value(1.0), false);
            let _ = controls::apply(&mut s, &format!("layer.{n}.mute"), ControlInput::Value(0.0), false);
        }
        let (id, _) = random_preset(&s, rng);
        controls::press_cue(&mut s, &id, None, true);
    }
    if s.estop.is_latched() {
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
    }
    if !s.gate.is_armed() {
        let _ = s.request_arm(*rng.pick(&ARM_SOURCES));
    }
}

fn run_case(seed: u64, frames: usize, cov: &mut Coverage) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let (s, _) = random_case(&mut rng);
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
    // Zone groups and their masters (T-420), from their own stream too.
    let mut mrng = Rng::new(seed ^ 0x0420_0420);
    if mrng.chance(0.6) {
        random_zone_groups(&mut rig.s(), &mut mrng);
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
    // The FX grid (T-146), from its own stream: random cells played on
    // top of everything; 1-15 hold with them.
    let mut xrng = Rng::new(seed ^ 0x0146_0146);
    let fx_on = xrng.chance(0.4);
    if fx_on {
        random_fx(&mut rig.s(), &mut xrng);
    }
    // Live effects (T-142), from their own stream.
    let mut erng = Rng::new(seed ^ 0x0142_0142);
    let effects = erng.chance(0.25);
    // « Préparer » (T-274), from its own stream.
    let mut prng = Rng::new(seed ^ 0x0274_0274);
    // Cue and layer modifiers (T-144), from their own stream.
    let mut vrng = Rng::new(seed ^ 0x0144_0144);
    let level_mods = vrng.chance(0.35);
    // The show phase (T-473), from its own stream: a third of the session,
    // anywhere in it, where the operator keeps the laser on (see
    // `keep_show_on`). Without it the share of lit frames hung on how
    // often a random action happened to arm, and every new dimming feature
    // pushed the coverage gate to its edge.
    let mut orng = Rng::new(seed ^ 0x0473_0473);
    let show_len = frames / 3;
    let show = {
        let start = orng.below(frames - show_len + 1);
        start..start + show_len
    };
    let mut session_lit = false;
    // Safety profiles (T-260), from their own stream: two venues, one
    // of them damaged on disk (out of range values).
    let mut hrng = Rng::new(seed ^ 0x0260_0260);
    {
        let mut s = rig.s();
        let a = random_profile(&mut hrng, "Fuzz A");
        let b = random_profile(&mut hrng, "Fuzz B");
        let mut bad = random_profile(&mut hrng, "Fuzz abîmé");
        bad.safety.horizon.y = f32::NAN;
        for p in [a, b, bad] {
            s.profiles.insert_for_tests(p);
        }
    }
    // The executor bank (T-418), from its own stream.
    let mut brng = Rng::new(seed ^ 0x0418_0418);
    let exec_on = brng.chance(0.6);
    if exec_on {
        random_bank(&mut rig.s(), &mut brng);
    }
    // Macros (T-425), from their own stream.
    let mut krng = Rng::new(seed ^ 0x0425_0425);
    let macros_on = krng.chance(0.4);
    if macros_on {
        random_macros(&mut rig.s(), &mut krng)?;
    }
    let mut beats = 0;
    for frame in 0..frames {
        let in_show = show.contains(&frame);
        if in_show {
            keep_show_on(&mut rig, &mut orng, frame == show.start);
            if exec_on && frame == show.start {
                // The show runs through an executor (T-418): a held Noir
                // tenu let go, a cue faded in part-way, from `brng`.
                let mut s = rig.s();
                crate::executors::release_all(&mut s);
                let p = s.executors.rt.page;
                let fade = (0..crate::executors::EXEC_PER_PAGE).find(|&n| {
                    let e = &s.executors.bank.pages[p].handles[n];
                    matches!(e.target, crate::executors::ExecTarget::Cue { .. }) && e.fader == crate::executors::FaderFn::FadeIn
                });
                if let Some(n) = fade {
                    let v = brng.range(0.4, 0.9);
                    let _ = controls::apply(&mut s, &format!("exec.cur.{}.fader", n + 1), ControlInput::Value(v), brng.chance(0.5));
                }
            }
        }
        {
            let mut s = rig.s();
            let armed = s.gate.is_armed();
            if effects && erng.chance(0.3) {
                random_live_effect(&mut s, &mut erng);
            }
            if level_mods && vrng.chance(0.4) {
                random_level(&mut s, &mut vrng);
            }
            // Held buttons are let go of: « Noir » and « Figer » don't
            // stay down for the whole session.
            if (s.live.black || s.live.freeze || s.live.strobe.hold) && erng.chance(0.7) {
                let id = *erng.pick(&["transport.black_hold", "transport.freeze", "master.strobe.hold"]);
                let _ = controls::apply(&mut s, id, ControlInput::Value(0.0), erng.chance(0.5));
            }
            if s.gate.is_armed() != armed {
                return Err(format!("frame {frame}: a live effect or a level changed the arming"));
            }
        }
        if exec_on && brng.chance(0.35) {
            let now = rig.now;
            let mut s = rig.s();
            let armed = s.gate.is_armed();
            if brng.chance(0.03) {
                random_bank(&mut s, &mut brng);
            } else {
                random_exec_move(&mut s, &mut brng, now);
            }
            if s.gate.is_armed() != armed {
                return Err(format!("frame {frame}: an executor changed the arming"));
            }
        }
        if macros_on && krng.chance(0.2) {
            let now = rig.now;
            let mut s = rig.s();
            let armed = s.gate.is_armed();
            if krng.chance(0.05) {
                random_macros(&mut s, &mut krng).map_err(|e| format!("frame {frame}: {e}"))?;
            } else {
                random_macro_move(&mut s, &mut krng, now);
            }
            if s.gate.is_armed() && !armed {
                return Err(format!("frame {frame}: a macro move armed"));
            }
        }
        if macros_on {
            // A macro never runs a frame with the stop latched: the engine
            // ends them all before any step.
            let latched = rig.s().estop.is_latched();
            if latched {
                let armed = rig.s().gate.is_armed();
                rig.tick();
                let s = rig.s();
                if !s.macros.player.runs.is_empty() {
                    return Err(format!("frame {frame}: a macro runs with the e-stop latched"));
                }
                if s.gate.is_armed() && !armed {
                    return Err(format!("frame {frame}: a macro armed"));
                }
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
        if fx_on && xrng.chance(0.25) {
            let mut s = rig.s();
            let armed = s.gate.is_armed();
            random_fx_move(&mut s, &mut xrng);
            if s.gate.is_armed() != armed {
                return Err(format!("frame {frame}: an FX grid move changed the arming"));
            }
        }
        if zrng.chance(0.15) {
            random_routing(&mut rig.s(), &mut zrng);
        }
        if mrng.chance(0.6) {
            let mut s = rig.s();
            if s.projection.get().groups.is_empty() || mrng.chance(0.2) {
                random_zone_groups(&mut s, &mut mrng);
            } else {
                random_group_masters(&mut s, &mut mrng);
            }
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
        // Safety profiles (T-260), from their own stream: random profiles
        // loaded from any source, confirmed or not. Only the operator may
        // load one; it never arms, always leaves the laser disarmed, and a
        // refused one (looser and not confirmed, unknown, damaged, from a
        // controller or the network) changes no setting. The frames that
        // follow are checked against whatever is in force (1-17).
        if hrng.chance(0.03) {
            let mut s = rig.s();
            if hrng.chance(0.3) {
                let name = if hrng.chance(0.5) { "Fuzz A" } else { "Fuzz B" };
                let p = random_profile(&mut hrng, name);
                s.profiles.insert_for_tests(p);
            }
            let name = *hrng.pick(&["Fuzz A", "Fuzz B", crate::profiles::SAFE_PREVIEW, "Fuzz public", "Inconnu", "Fuzz abîmé"]);
            let src = *hrng.pick(&[ArmSource::Ui, ArmSource::Keyboard, ArmSource::Api, ArmSource::Midi, ArmSource::Network, ArmSource::Osc, ArmSource::Dmx, ArmSource::Remote, ArmSource::System]);
            let confirm = hrng.chance(0.5);
            let was_armed = s.gate.is_armed();
            let before = (s.safety.get(), s.outputs.active_limits(), s.presence.settings.clone());
            let r = crate::profiles::load(&mut s, name, confirm, src);
            let after = (s.safety.get(), s.outputs.active_limits(), s.presence.settings.clone());
            use crate::profiles::LoadError;
            match &r {
                Ok(_) | Err(LoadError::Failed(_)) => {
                    if s.gate.is_armed() {
                        return Err(format!("frame {frame}: profile « {name} » loaded and the laser stayed armed"));
                    }
                    if !crate::profiles::operator_source(src) {
                        return Err(format!("frame {frame}: profile loaded from {src:?}"));
                    }
                }
                Err(LoadError::Source) => {
                    if crate::profiles::operator_source(src) || after != before || s.gate.is_armed() != was_armed {
                        return Err(format!("frame {frame}: a profile load from {src:?} changed something"));
                    }
                }
                Err(LoadError::Loosens(_)) => {
                    if confirm || after != before || s.gate.is_armed() != was_armed {
                        return Err(format!("frame {frame}: a refused loosening (confirmed: {confirm}) changed something"));
                    }
                }
                Err(LoadError::Unknown(_)) | Err(LoadError::Damaged(_)) => {
                    if after != before || s.gate.is_armed() {
                        return Err(format!("frame {frame}: an unknown or damaged profile changed the settings or left the laser armed"));
                    }
                }
            }
            if s.gate.is_armed() && !was_armed {
                return Err(format!("frame {frame}: a profile load armed the laser"));
            }
        }
        // « Préparer » (T-274), from its own stream: prepare, adjust, the
        // prepared view rendered, cleared or sent. Only « Envoyer » plays
        // anything, as a cue press, and nothing of it arms.
        if prng.chance(0.15) {
            let mut s = rig.s();
            let was = s.gate.is_armed();
            random_prepared(&mut s, &mut prng, true);
            if s.gate.is_armed() != was {
                return Err(format!("frame {frame}: « Préparer » changed the arming"));
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
        // pad (`safety.hold`) may be pressed by a random control. Hold-to-run
        // is read as in force: a safety profile load (above) may change it.
        let (may_emit, caps, held, scanning_before, hold_to_run) = {
            let s = rig.s();
            (s.gate.is_armed() && !s.estop.is_latched(), s.outputs.active_limits(), s.presence.status().midi_hold, s.audience.is_scanning(), s.presence.enforced && s.presence.settings.hold_to_run)
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
        session_lit |= lit > 0;
        cov.show_frames += in_show as usize;
        cov.show_sent_lit += (in_show && lit > 0) as usize;
        cov.sent_lit_with_group_master += (lit > 0 && s.projection.get().groups.iter().any(|g| g.master > 0.0 && g.master < 1.0 && !g.zones.is_empty())) as usize;
        cov.lit_with_zones += (preview_lit && cfg.zones.iter().any(|z| z.kind == ZoneKind::Blank)) as usize;
        cov.lit_with_horizon += (preview_lit && cfg.horizon.lines && cfg.horizon.level == 0.0) as usize;
        cov.lit_with_projection += (preview_lit && s.projection.get().zones.iter().any(|z| !z.geometry.is_identity())) as usize;
        cov.lit_with_group_master += (preview_lit && s.projection.get().groups.iter().any(|g| g.master < 1.0 && !g.zones.is_empty())) as usize;
        cov.lit_with_remote += (preview_lit && !s.netin.fixtures.remotes.is_empty()) as usize;
        cov.lit_with_transition += (preview_lit && !s.deck.transition_fx(t).is_empty()) as usize;
        cov.lit_with_effects += (preview_lit && with_effects) as usize;
        cov.lit_with_fx += (preview_lit && s.fx.view()["lines"].as_array().is_some_and(|l| l.iter().any(|l| l["level"].as_f64().unwrap_or(0.0) > 0.0))) as usize;
        if s.estop.is_latched() && s.fx.any_playing() {
            return Err(format!("frame {frame}: an FX grid effect survived the e-stop"));
        }
        let levels_on = s.mixer.layers.iter().any(|l| !crate::levels::is_neutral(&l.modifiers)) || s.cue_mods.values().any(|m| !crate::levels::is_neutral(m));
        cov.lit_with_levels += (preview_lit && levels_on) as usize;
        if let Some(a) = s.deck.active.iter().chain(s.deck.ending.iter().map(|e| &e.cue)).find(|a| !(0.0..=1.0).contains(&a.exec_level)) {
            return Err(format!("frame {frame}: cue {} at executor level {}", a.cue, a.exec_level));
        }
        cov.lit_with_exec += (preview_lit && s.deck.active.iter().any(|a| a.exec_level > 0.0 && a.exec_level < 1.0)) as usize;
        if black_held && preview_lit && !limiter_holding {
            return Err(format!("frame {frame}: the preview is lit while « Noir » is held"));
        }
        if s.estop.is_latched() && (!s.deck.ending.is_empty() || !s.deck.pending.is_empty()) {
            return Err(format!("frame {frame}: a transition or a waiting launch survived the e-stop"));
        }
    }
    cov.sessions += 1;
    cov.sessions_lit += session_lit as usize;
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
        let show = cov.show_sent_lit as f64 / cov.show_frames.max(1) as f64;
        let sessions = cov.sessions_lit as f64 / cov.sessions.max(1) as f64;
        eprintln!("coverage: sessions lit {sessions:.3}, show phase lit {show:.3}, sent lit with a group master dimmed {:.3}", share(cov.sent_lit_with_group_master));
        eprintln!("coverage: Public zones lit {:.3} above, {:.3} scanning; transitions {:.3}; sent lit {:.3}, effects {:.3}, levels {:.3}, executors {:.3}", share(cov.lit_with_public_above), share(cov.lit_with_public_scanning), share(cov.lit_with_transition), share(cov.sent_lit), share(cov.lit_with_effects), share(cov.lit_with_levels), share(cov.lit_with_exec));
        // Gates at about half of what the fuzz measures (T-473): sent lit
        // ~0.33, show phase ~0.55, sessions ~0.87, group masters ~0.23 at
        // 1000 cases. A new dimming feature that eats the margin must
        // brighten its own stream, not drop controls from the fuzz.
        assert!(share(cov.sent_lit) > 0.15, "only {} of {} frames sent light", cov.sent_lit, cov.frames);
        assert!(show > 0.25, "only {} of {} show phase frames sent light", cov.show_sent_lit, cov.show_frames);
        assert!(sessions > 0.4, "only {} of {} sessions ever sent light", cov.sessions_lit, cov.sessions);
        assert!(share(cov.sent_lit_with_group_master) > 0.1, "only {} of {} frames sent light through a group master below 100 %", cov.sent_lit_with_group_master, cov.frames);
        assert!(share(cov.lit_with_zones) > 0.1, "only {} of {} lit frames had a Blank zone", cov.lit_with_zones, cov.frames);
        assert!(share(cov.lit_with_horizon) > 0.05, "only {} of {} lit frames had a full horizon", cov.lit_with_horizon, cov.frames);
        assert!(share(cov.lit_with_projection) > 0.2, "only {} of {} lit frames went through a projection zone", cov.lit_with_projection, cov.frames);
        assert!(share(cov.lit_with_group_master) > 0.05, "only {} of {} lit frames had a group master below 100 %", cov.lit_with_group_master, cov.frames);
        assert!(share(cov.lit_with_remote) > 0.02, "only {} of {} lit frames went through a DMX fixture instance", cov.lit_with_remote, cov.frames);
        assert!(share(cov.lit_with_public_above) > 0.1, "only {} of {} lit frames had a Public zone, scanning off", cov.lit_with_public_above, cov.frames);
        assert!(share(cov.lit_with_public_scanning) > 0.05, "only {} of {} lit frames had a Public zone while scanning", cov.lit_with_public_scanning, cov.frames);
        assert!(share(cov.lit_with_transition) > 0.01, "only {} of {} lit frames ran a transition", cov.lit_with_transition, cov.frames);
        assert!(share(cov.lit_with_effects) > 0.05, "only {} of {} lit frames went through a live effect", cov.lit_with_effects, cov.frames);
        assert!(share(cov.lit_with_fx) > 0.02, "only {} of {} lit frames went through the FX grid", cov.lit_with_fx, cov.frames);
        assert!(share(cov.lit_with_levels) > 0.05, "only {} of {} lit frames went through a cue or layer level", cov.lit_with_levels, cov.frames);
        assert!(share(cov.lit_with_exec) > 0.035, "only {} of {} lit frames had a cue below full executor level", cov.lit_with_exec, cov.frames);
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

/// `run_strobe_case` with the way forced (the seed's stream unchanged).
/// Way 5: a cell of the FX grid (T-146) with a fast strobe, or a square
/// modulator on a brightness, played on top.
fn run_strobe_way(seed: u64, way: Option<usize>) -> Result<(), String> {
    strobe_case_full(seed, None, way)
}

/// T-419: a layer in a ×4 speed group (beat gate, beat-synced square
/// LFO in the group) flashes no longer than the burst: the limiter acts on
/// real output time, after the groups.
#[test]
fn speed_groups_never_get_past_the_strobe_limiter() {
    let seeds = env_u64("SAFETY_CASES").map_or(6, |n| (n / 50).clamp(6, 40));
    let mut failures = Vec::new();
    for i in 0..seeds {
        let seed = 7_000_419 + i;
        for way in [0, 1] {
            if let Err(e) = strobe_case_sped(seed, None, Some(way), Some(4.0)) {
                failures.push(format!("{e} (seed {seed})"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-425: a strobe built from a macro's timed steps (brightness full /
/// off every 50-120 ms, two macros chained: longer than any burst) is
/// cut by the limiter like any other: it acts on the output, after
/// everything a macro can change.
fn macro_strobe_case(seed: u64) -> Result<(), String> {
    use crate::macros::{Macro, MacroStep, Wait};
    let mut rng = Rng::new(seed);
    let mut s = test_support::shared();
    let cfg = SafetySettings { strobe_max_hz: rng.range(1.0, 4.0), strobe_burst_s: rng.range(0.5, 3.0), ..Default::default() };
    s.safety.set(cfg.clone(), true).map_err(|e| format!("{e:?}"))?;
    let presets: Vec<_> = s.presets.iter().filter(|p| p.settings.gate == crate::engine::GateMode::Off).map(|p| p.settings.clone()).collect();
    s.settings = rng.pick(&presets).clone();
    let half = rng.range(crate::macros::SECONDS.0, 0.12);
    let target = *rng.pick(&["master.brightness", "look.brightness"]);
    let strobe = |last: Option<&str>| {
        let mut steps: Vec<MacroStep> = (0..crate::macros::MAX_STEPS - 1)
            .map(|i| MacroStep { command: format!("{target} {}", if i % 2 == 0 { 100 } else { 0 }), wait: Wait::Seconds(half), enabled: true })
            .collect();
        steps.push(MacroStep { command: last.unwrap_or("").to_string(), wait: Wait::None, enabled: true });
        steps
    };
    let list = vec![
        Macro { id: "m1".into(), name: "Stroboscope".into(), steps: strobe(Some("macro.m2.go")), ..Default::default() },
        Macro { id: "m2".into(), name: "Encore".into(), steps: strobe(None), ..Default::default() },
    ];
    crate::macros::set(&mut s, list).map_err(|e| e.message_fr)?;
    s.request_arm(ArmSource::Ui).map_err(|e| e.join("; "))?;
    let mut rig = Rig::new(s, "macro-strobe", seed);
    let epoch = rig.s().epoch;
    rig.now = epoch;
    crate::macros::start(&mut rig.s(), "m1", controls::ControlSource::Midi).map_err(str::to_string)?;
    let mut meter = FlashMeter { peak: 0.0, on: false, onsets: Vec::new() };
    let secs = 2.0 * crate::macros::MAX_STEPS as f64 * half as f64 + 1.0;
    for _ in 0..(secs * 60.0) as usize {
        rig.tick();
        let w = rig.wire.lock().unwrap();
        meter.feed(&w.last, rig.now.saturating_duration_since(rig.s().epoch).as_secs_f64(), 1.0 / 60.0);
    }
    if meter.onsets.len() < 4 {
        return Err(format!("the macro strobe never flashed ({} onsets)", meter.onsets.len()));
    }
    let run = meter.longest_fast_run(1.0 / cfg.strobe_max_hz as f64);
    if run > cfg.strobe_burst_s as f64 + 0.1 {
        return Err(format!("{run:.2} s of output flashing faster than {:.2} Hz from a macro (burst limit {:.2} s)", cfg.strobe_max_hz, cfg.strobe_burst_s));
    }
    Ok(())
}

#[test]
fn macros_never_get_past_the_strobe_limiter() {
    let seeds = env_u64("SAFETY_CASES").map_or(4, |n| (n / 100).clamp(4, 20));
    let failures: Vec<String> = (0..seeds).map(|i| 9_000_425 + i).filter_map(|seed| macro_strobe_case(seed).err().map(|e| format!("{e} (seed {seed})"))).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
    strobe_case_full(seed, forced, None)
}

/// `strobe_case`, optionally with the way forced too (T-146).
fn strobe_case_full(seed: u64, forced: Option<Forced>, forced_way: Option<usize>) -> Result<(), String> {
    strobe_case_sped(seed, forced, forced_way, None)
}

/// `strobe_case_full` with the look's layer in speed group A at `mult`
/// (T-419), the beat gate and a beat-synced square LFO in that group: a
/// fast group must not get the flashing past the limiter.
fn strobe_case_sped(seed: u64, forced: Option<Forced>, forced_way: Option<usize>, speed: Option<f32>) -> Result<(), String> {
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
    if let Some(w) = forced_way {
        way = w;
    }
    match way {
        5 => {
            use crate::fx::FxCell;
            let cell = if seed.is_multiple_of(2) {
                let strobe = live::Strobe { on: true, hold: false, rate: live::Rate::Hz(hz), duty: 0.5 };
                FxCell { name: "strobe".into(), modifiers: Some(live::LiveModifiers { strobe, ..Default::default() }), ..Default::default() }
            } else {
                let target = if seed % 4 == 1 { "master.brightness" } else { "look.brightness" };
                let m = lfo::Modulator { target: target.into(), wave: lfo::Wave::Square, rate: live::Rate::Hz(hz), depth: 1.0, offset: 0.0, ..Default::default() };
                FxCell { name: "flash".into(), modulators: vec![m], ..Default::default() }
            };
            let reg = controls::ControlRegistry::build(&s.presets);
            s.fx.set_cells(vec![vec![Some(cell)]], &reg).map_err(|e| format!("{e:#}"))?;
            let _ = controls::apply(&mut s, "fx.1.1", ControlInput::Value(1.0), true);
        }
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
    if let Some(mult) = speed {
        s.speed.change_on_bar = false;
        s.speed.set_mode(1, crate::speed::SpeedMode::Multiple { mult }, 0.0, 0.0, 4).map_err(str::to_string)?;
        s.mixer.layer_mut(1).speed_group = 1;
        if way == 1 {
            let target = *rng.pick(&["master.brightness", "look.brightness"]);
            let m = lfo::Modulator { target: target.into(), wave: lfo::Wave::Square, rate: live::Rate::Beats(0.25), depth: 1.0, speed_group: 1, ..Default::default() };
            s.lfos.replace_in_memory(vec![m]);
        }
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

/// T-419: through the real engine, a cue whose cell is in group A at ×½
/// moves exactly half as far as a cue in the default group over 32 master
/// beats; the cell wins over its layer's group (C at ×4). Each look gets
/// its group's `BeatClock` (the evolving cue's local beat is read back).
#[test]
fn a_cue_in_a_half_speed_group_moves_half_as_far_through_the_engine() {
    use crate::speed::SpeedMode;
    let mut s = test_support::shared();
    s.tempo.set_bpm_manual(120.0, 0.0);
    s.speed.change_on_bar = false;
    s.speed.set_mode(1, SpeedMode::Multiple { mult: 0.5 }, 0.0, 0.0, 4).unwrap();
    s.speed.set_mode(3, SpeedMode::Multiple { mult: 4.0 }, 0.0, 0.0, 4).unwrap();
    s.mixer.layer_mut(2).speed_group = 3;
    let (a, b) = (s.presets[0].id.clone(), s.presets[1].id.clone());
    s.deck.set_slot(&b, CueSlot { layer: Some(2), speed_group: Some(1), ..Default::default() });
    let cue = crate::evolving::EvolvingCue { length_beats: 64.0, looped: false, ..crate::evolving::test_cue(false) };
    let look = Settings { content: Content::Evolving(cue), brightness: 1.0, ..Default::default() };
    for id in [&a, &b] {
        assert!(controls::send_prepared(&mut s, id, Some(ClickMode::Toggle), look.clone(), None));
    }
    let ids: Vec<(String, u64)> = s.deck.active.iter().map(|c| (c.cue.clone(), c.id)).collect();
    let id_of = |cue: &str| ids.iter().find(|(c, _)| c == cue).map(|(_, id)| *id).unwrap();
    let (ia, ib) = (id_of(&a), id_of(&b));
    assert_eq!((s.look_speed_group(1, ia), s.look_speed_group(2, ib)), (0, 1), "cell > layer > default");
    let mut rig = Rig::new(s, "speed-groups", 0);
    let pos = |rig: &Rig, id: u64| rig.s().evolving.iter().find(|(i, _)| *i == id).map(|(_, p)| p.pos).expect("on show");
    for _ in 0..120 {
        rig.tick();
    }
    let (a0, b0) = (pos(&rig, ia), pos(&rig, ib));
    // 32 beats at 120 BPM.
    for _ in 0..16 * 60 {
        rig.tick();
    }
    let (da, db) = (pos(&rig, ia) - a0, pos(&rig, ib) - b0);
    // The rig's frames are whole nanoseconds: 32 beats give or take a hair.
    assert!((da - 32.0).abs() < 1e-2, "default group: {da}");
    // The two phases come from the one clock: exactly half, no drift.
    assert!((db - da / 2.0).abs() < 1e-9, "group A at ×½: {db} for {da}");
}

/// Default seed count of the strobe property (and of the cue sweeps).
const STROBE_SEEDS: usize = 40;

/// T-354: a burst limit (0.52 s) shorter than two allowed periods
/// (1.13 Hz); the limiter used to wait for a third fast flash before
/// starting the burst clock, so 0.63 s of fast flashing went out.
/// T-146: 20 preview frames from the real engine on a clock that starts
/// at the studio's epoch, after `setup`.
fn fx_frames(tag: &str, setup: impl FnOnce(&mut Shared)) -> Vec<Vec<Point>> {
    let mut s = test_support::shared();
    s.tempo.set_bpm_manual(120.0, 0.0);
    setup(&mut s);
    let mut rig = Rig::new(s, tag, 0x0146);
    let epoch = rig.s().epoch;
    rig.now = epoch;
    (0..20)
        .map(|_| {
            rig.tick();
            rig.s().frame.clone()
        })
        .collect()
}

/// T-146: *Action* at 0 draws exactly the frame drawn without effects;
/// at 1 the effect shows. *Cue sélectionné* puts it on the newest cue.
#[test]
fn fx_action_zero_draws_the_frame_drawn_without_effects() {
    let play = |s: &mut Shared| {
        for id in ["fx.1.1", "fx.2.5", "fx.3.1"] {
            controls::apply(s, id, ControlInput::Value(1.0), false).unwrap();
        }
    };
    let plain = fx_frames("fx-plain", |_| {});
    assert!(plain.iter().any(|f| f.iter().any(|p| p.is_lit())), "the look draws something");
    let zero = fx_frames("fx-zero", |s| {
        play(s);
        for line in 1..=crate::fx::LINES {
            controls::apply(s, &format!("fx.{line}.action"), ControlInput::Value(0.0), false).unwrap();
        }
    });
    assert_eq!(zero, plain);
    let full = fx_frames("fx-full", play);
    assert_ne!(full, plain);
    // On the newest cue: the effect shows on it.
    let cue = test_support::shared().presets[0].id.clone();
    let with_cue = |fx: bool| {
        let cue = cue.clone();
        fx_frames("fx-cue", move |s| {
            controls::press_cue(s, &cue, Some(ClickMode::Toggle), true);
            if fx {
                controls::apply(s, "fx.dest", ControlInput::Value(1.0), false).unwrap();
                controls::apply(s, "fx.2.5", ControlInput::Value(1.0), false).unwrap();
            }
        })
    };
    assert_ne!(with_cue(true), with_cue(false));
}

/// T-146: a strobe or a square brightness modulator in an FX grid cell
/// is held by the same limiter.
#[test]
fn an_fx_grid_strobe_never_exceeds_the_burst_limit() {
    run_property("an_fx_grid_strobe_never_exceeds_the_burst_limit", 0x0146, 12, |seed| run_strobe_way(seed, Some(5)));
}

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


/// T-144: the cue and layer levels sit before the safety stage. Sizes
/// compose (cue 0.5 × master 2 = the plain size), brightness only ever
/// goes down, and a figure pushed down by its cue and its layer is still
/// cut by the horizon.
#[test]
fn cue_and_layer_modifiers_apply_before_the_safety_stage() {
    let frame_of = |moves: &[(&str, f32)], horizon: Option<f32>| -> Vec<Point> {
        let mut s = test_support::shared();
        let y = horizon.unwrap_or(-1.0);
        s.safety.set(SafetySettings { horizon: Horizon { y, lines: true, ramp: 0.0, level: 0.0 }, ..Default::default() }, true).unwrap();
        let mut g = s.grid.clone();
        let page = g.new_page("Mes looks", &s.presets).unwrap();
        s.grid = g;
        crate::usercues::refresh(&mut s);
        s.settings = Settings { content: crate::engine::Content::Shape { shape: "square".into() }, scale: 0.8, rotation_speed: 0.0, ..Settings::default() };
        let id = crate::usercues::store(&mut s, page, 0, None, crate::usercues::StoreFilter::All, false).unwrap();
        s.settings = Settings::default();
        assert!(controls::press_cue(&mut s, &id, None, true));
        for (control, v) in moves {
            let control = control.replace("{cue}", &id);
            controls::apply(&mut s, &control, ControlInput::Value(*v), false).unwrap();
        }
        let mut rig = Rig::new(s, "levels", 0);
        for _ in 0..3 {
            rig.tick();
        }
        assert!(!rig.s().gate.is_armed(), "never armed");
        let frame = rig.s().frame.clone();
        frame
    };
    let lit = |f: &[Point]| f.iter().filter(|p| p.r + p.g + p.b > 0.0).cloned().collect::<Vec<_>>();
    let extent = |f: &[Point]| lit(f).iter().fold(0.0f32, |m, p| m.max(p.x.abs()));
    let peak = |f: &[Point]| lit(f).iter().fold(0.0f32, |m, p| m.max(p.r).max(p.g).max(p.b));
    let plain = frame_of(&[], None);
    assert!(extent(&plain) > 0.7, "{}", extent(&plain));
    let composed = frame_of(&[("cue.{cue}.size", 0.5), ("master.size", 2.0)], None);
    assert!((extent(&composed) - extent(&plain)).abs() < 0.05, "{} vs {}", extent(&composed), extent(&plain));
    let layer = frame_of(&[("layer.1.size", 0.5)], None);
    assert!((extent(&layer) - extent(&plain) * 0.5).abs() < 0.05, "{} vs {}", extent(&layer), extent(&plain));
    let dim = frame_of(&[("cue.{cue}.brightness", 0.5), ("layer.1.brightness", 0.8)], None);
    assert!(!lit(&dim).is_empty());
    assert!(peak(&dim) <= peak(&plain) * 0.4 + 0.02, "{} vs {}", peak(&dim), peak(&plain));
    let low = frame_of(&[("cue.{cue}.pos_y", -0.3), ("layer.1.pos_y", -0.5)], Some(0.0));
    assert!(!lit(&low).is_empty());
    assert!(lit(&low).iter().all(|p| p.y >= -1e-3), "a lit point below the horizon");
}

/// T-418: a cue at executor level 0,5 is drawn at half its colours,
/// before the mixer and the safety stage; another cue playing beside it
/// is untouched. Two squares, left and right, through the real engine.
#[test]
fn an_executor_level_dims_its_cue_only() {
    use crate::executors::{ExecTarget, Executor, ExecutorBank, FaderFn};
    let frame_of = |level: Option<f32>| -> Vec<Point> {
        let mut s = test_support::shared();
        s.deck.multi = true;
        let mut g = s.grid.clone();
        let page = g.new_page("Mes looks", &s.presets).unwrap();
        s.grid = g;
        crate::usercues::refresh(&mut s);
        s.settings = Settings { content: crate::engine::Content::Shape { shape: "square".into() }, scale: 0.4, rotation_speed: 0.0, ..Settings::default() };
        let a = crate::usercues::store(&mut s, page, 0, None, crate::usercues::StoreFilter::All, false).unwrap();
        let b = crate::usercues::store(&mut s, page, 1, None, crate::usercues::StoreFilter::All, false).unwrap();
        s.settings = Settings::default();
        let mut bank = ExecutorBank::default();
        bank.pages[0].handles[0] = Executor { target: ExecTarget::Cue { id: a.clone() }, fader: FaderFn::FadeIn, ..Default::default() };
        s.executors.set_bank(bank);
        match level {
            Some(v) => controls::apply(&mut s, "exec.1.1.fader", ControlInput::Value(v), true).unwrap(),
            None => assert!(controls::press_cue(&mut s, &a, None, true)),
        }
        assert!(controls::press_cue(&mut s, &b, None, true));
        controls::apply(&mut s, &format!("cue.{a}.pos_x"), ControlInput::Value(-0.5), false).unwrap();
        controls::apply(&mut s, &format!("cue.{b}.pos_x"), ControlInput::Value(0.5), false).unwrap();
        let mut rig = Rig::new(s, "executors", 0);
        for _ in 0..3 {
            rig.tick();
        }
        assert!(!rig.s().gate.is_armed(), "never armed");
        assert_eq!(rig.s().deck.active.len(), 2);
        let frame = rig.s().frame.clone();
        frame
    };
    let peak = |f: &[Point], left: bool| f.iter().filter(|p| p.is_lit() && (p.x < 0.0) == left).fold(0.0f32, |m, p| m.max(p.r).max(p.g).max(p.b));
    let (full, half) = (frame_of(None), frame_of(Some(0.5)));
    let (l1, r1, l2, r2) = (peak(&full, true), peak(&full, false), peak(&half, true), peak(&half, false));
    assert!(l1 > 0.1 && r1 > 0.1, "both lit at full: {l1} {r1}");
    assert!((l2 - l1 * 0.5).abs() < 0.03, "the executor's cue at half: {l2} vs {l1}");
    assert!((r2 - r1).abs() < 1e-3, "the other cue untouched: {r2} vs {r1}");
    // Never above full, whatever the fader is sent.
    let over = frame_of(Some(4.0));
    assert!(peak(&over, true) <= l1 + 1e-3);
}

// ---------- group masters (T-420) ----------

/// One case of `a_group_master_never_brightens_the_output`: the same
/// random session (look, safety settings, caps, calibration, projection
/// zones with groups, sometimes a cue with T-144 cue and layer levels) rendered twice on the same simulated clock, once
/// with every master at 100 % and once with random masters at or below
/// it. No frame sent to the output with the masters down has a lit
/// sample the frame at 100 % doesn't match or beat on every channel: the masters sit before the safety stage, which still dims after them,
/// and nothing after them can turn a dimmer into more light.
fn run_group_master_case(seed: u64) -> Result<bool, String> {
    let session = |masters: Option<u64>| -> Result<Vec<Vec<Point>>, String> {
        // `random_case` without presence (arming must succeed).
        let mut rng = Rng::new(seed);
        let mut s = test_support::shared();
        s.safety.set(random_safety(&mut rng), true).map_err(|e| format!("setup: safety refused: {e:?}"))?;
        s.outputs.set(MAIN_OUTPUT, Some(random_limits(&mut rng)), None, false, true).map_err(|e| format!("setup: caps refused: {e:?}"))?;
        s.calibration = random_calibration(&mut rng);
        s.settings = random_preset(&s, &mut rng).1;
        let mut zrng = Rng::new(seed ^ 0x0420_0420);
        let mut cfg = random_projection(&mut zrng);
        for z in &mut cfg.zones {
            z.enabled = true;
            z.output = None;
            z.geometry.warp.enabled = false;
        }
        let ids: Vec<u8> = cfg.zones.iter().map(|z| z.id).collect();
        cfg.default_zones = ids.clone();
        cfg.groups = random_groups(&mut zrng, &ids);
        if cfg.groups.is_empty() {
            cfg.groups = vec![ZoneGroup { name: "G".into(), zones: ids.clone(), master: 1.0 }];
        }
        for g in &mut cfg.groups {
            g.master = 1.0;
        }
        s.projection.set(cfg).map_err(|e| format!("setup: projection refused: {e:#}"))?;
        // A playing cue with cue and layer levels (T-144) in some cases,
        // the same in both sessions: the group masters dim on top of the
        // whole level chain (cue → layer → FX → master).
        let mut vrng = Rng::new(seed ^ 0x0144_0420);
        if vrng.chance(0.5) {
            let (id, _) = random_preset(&s, &mut vrng);
            controls::press_cue(&mut s, &id, None, true);
            for _ in 0..1 + vrng.below(3) {
                random_level(&mut s, &mut vrng);
            }
        }
        let mut rig = Rig::new(s, "group-master", seed);
        rig.tick();
        rig.s().request_arm(ArmSource::Ui).map_err(|e| format!("setup: arm refused: {e:?}"))?;
        if let Some(mseed) = masters {
            let mut mrng = Rng::new(mseed);
            let mut s = rig.s();
            for n in 1..=s.projection.get().groups.len() {
                // Anywhere below 100 % here (0 and out of range included).
                let level = if mrng.chance(0.3) { random_master(&mut mrng) } else { mrng.f() };
                let input = if mrng.chance(0.5) { ControlInput::Norm(level) } else { ControlInput::Value(level) };
                controls::apply(&mut s, &format!("group.{n}.master"), input, true).map_err(|e| format!("group.{n}.master refused: {e:?}"))?;
            }
        }
        let mut frames = Vec::new();
        for _ in 0..20 {
            rig.tick();
            frames.push(rig.wire.lock().unwrap().last.clone());
        }
        if rig.s().gate.is_armed() != rig.wire.lock().unwrap().armed {
            return Err("the output's arm state left the gate's".into());
        }
        Ok(frames)
    };
    let full = session(None)?;
    let dimmed = session(Some(seed ^ 0x5eed))?;
    // A dark zone's points can be travel for the optimiser and the
    // safety stage's segment splits, so frames may differ in length: every
    // lit sample with the masters down must sit where the frame at 100 %
    // has a sample at least as bright on every channel, and no channel's
    // peak may rise.
    let peak = |f: &[Point]| f.iter().fold([0.0f32; 3], |m, p| [m[0].max(p.r), m[1].max(p.g), m[2].max(p.b)]);
    let energy = |f: &[Point]| f.iter().map(|p| p.r + p.g + p.b).sum::<f32>();
    let mut dimmer = false;
    for (i, (a, b)) in full.iter().zip(&dimmed).enumerate() {
        let (pa, pb) = (peak(a), peak(b));
        dimmer |= energy(b) < energy(a) - 1e-3;
        if (0..3).any(|c| pb[c] > pa[c] + 1e-5) {
            return Err(format!("frame {i}: peak {pb:?} with the masters down, {pa:?} at 100 %"));
        }
        for q in b.iter().filter(|q| q.is_lit()) {
            let covered = a.iter().any(|p| (p.x - q.x).abs() < 1e-4 && (p.y - q.y).abs() < 1e-4 && p.r + 1e-5 >= q.r && p.g + 1e-5 >= q.g && p.b + 1e-5 >= q.b);
            if !covered {
                return Err(format!("frame {i}: {q:?} lit with the masters down, nothing as bright there at 100 %"));
            }
        }
    }
    Ok(dimmer)
}

#[test]
fn a_group_master_never_brightens_the_output() {
    let (cases, dimmed) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
    run_property("a_group_master_never_brightens_the_output", 420, 60, |seed| {
        cases.set(cases.get() + 1);
        dimmed.set(dimmed.get() + run_group_master_case(seed)? as usize);
        Ok(())
    });
    // Not vacuous: most cases really dim something that reached the output.
    assert!(dimmed.get() * 3 >= cases.get(), "only {} of {} cases dimmed the output", dimmed.get(), cases.get());
}

/// The masters sit before the safety stage: a zone's master at 100 %
/// still has its light cut by a Blank zone and the caps, and a master at
/// 0 darkens its zone only (points kept).
#[test]
fn group_masters_run_before_zones_and_caps() {
    let mut rig = armed_square_rig("group-master-order");
    {
        let mut s = rig.s();
        let zone = |id: u8, x: f32| ProjectionZone { id, name: format!("Z{id}"), geometry: Geometry { size_x: 0.4, size_y: 0.4, pos_x: x, ..Default::default() }, ..Default::default() };
        let groups = vec![ZoneGroup { name: "Gauche".into(), zones: vec![1], master: 1.0 }, ZoneGroup { name: "Droite".into(), zones: vec![2], master: 1.0 }];
        s.projection.set(ProjectionConfig { zones: vec![zone(1, -0.5), zone(2, 0.5)], default_zones: vec![1, 2], groups }).unwrap();
        // A Blank zone over the right half and a cap of 0.3.
        let blank = Zone { id: 1, name: "B".into(), kind: ZoneKind::Blank, level: 0.0, points: vec![[0.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, 1.0]], audience: false, ..Default::default() };
        s.safety.set(SafetySettings { zones: vec![blank], ..Default::default() }, true).unwrap();
        s.outputs.set(MAIN_OUTPUT, Some(OutputLimits { max_power: 0.3, max_color: [1.0, 1.0, 1.0] }), None, false, true).unwrap();
    }
    rig.tick();
    rig.s().request_arm(ArmSource::Ui).unwrap();
    for _ in 0..3 {
        rig.tick();
    }
    let before = rig.wire.lock().unwrap().last.clone();
    assert!(before.iter().any(|p| p.is_lit()), "the left zone is lit");
    assert!(before.iter().filter(|p| p.is_lit()).all(|p| p.x < 0.0), "the Blank zone still cuts the right zone at 100 %");
    assert!(before.iter().all(|p| p.r.max(p.g).max(p.b) <= 0.3 + 1e-6), "the cap holds");
    controls::apply(&mut rig.s(), "group.1.master", ControlInput::Value(0.0), true).unwrap();
    for _ in 0..3 {
        rig.tick();
    }
    let after = rig.wire.lock().unwrap().last.clone();
    assert!(!after.is_empty(), "points kept: the galvos keep scanning");
    assert!(after.iter().all(|p| !p.is_lit()), "zone 1 dark, zone 2 still behind the Blank zone");
    assert!(rig.s().gate.is_armed(), "a master never disarms either");
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
        s.projection.set(ProjectionConfig { zones: vec![zone(1, -0.5, MAIN_OUTPUT), zone(2, 0.5, "p2")], default_zones: vec![1, 2], ..Default::default() }).unwrap();
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

// ---------- latency compensation (T-246) ----------

/// The *Décalage de sortie* moves what the effects draw, never when a
/// frame goes out: at either end of its range (and asked far beyond it),
/// Escape, the blackout control and a disarm darken the very next frame,
/// and the gate is never touched by the offset.
#[test]
fn a_large_output_offset_never_delays_a_stop() {
    for offset in [1_000.0, -1_000.0] {
        let mut rig = armed_square_rig(if offset > 0.0 { "latency-plus" } else { "latency-minus" });
        controls::apply(&mut rig.s(), "tempo.output_offset", ControlInput::Value(offset), true).unwrap();
        let applied = rig.s().tempo.output_offset_ms();
        assert_eq!(applied.abs(), crate::tempo::MAX_OUTPUT_OFFSET_MS, "clamped to the range");
        assert!(!rig.s().gate.is_armed(), "the offset never arms");
        rig.tick();
        assert_eq!(lit(&rig.wire), 0, "disarmed at start, offset or not");
        // The effects (deck, timeline) read the predicted beat; the clock doesn't move.
        {
            let s = rig.s();
            let t = s.now_s();
            assert!((s.timeline_clock().beat - s.tempo.beat_at(t + applied as f64 / 1_000.0)).abs() < 1e-9);
        }
        let stops: [fn(&mut Shared); 3] = [
            |s| s.emergency_stop(ArmSource::Keyboard),
            |s| controls::apply(s, "transport.blackout", ControlInput::Value(1.0), true).unwrap(),
            |s| s.gate.disarm(interlock::DisarmReason::User, ArmSource::Ui),
        ];
        for (i, stop) in stops.into_iter().enumerate() {
            {
                let mut s = rig.s();
                let estop = Arc::clone(&s.estop);
                s.gate.reset_estop(&estop);
                s.request_arm(ArmSource::Ui).unwrap();
            }
            rig.tick();
            assert!(lit(&rig.wire) > 0, "offset {offset}, stop {i}: lit while armed");
            stop(&mut rig.s());
            rig.tick();
            assert_eq!(lit(&rig.wire), 0, "offset {offset}, stop {i}: dark on the very next frame");
            assert!(!rig.wire.lock().unwrap().armed, "offset {offset}, stop {i}: the output disarmed at once");
            rig.tick();
            assert_eq!(lit(&rig.wire), 0, "offset {offset}, stop {i}: and it stays dark");
        }
        // A refused value (not a number) changes nothing.
        assert!(controls::apply(&mut rig.s(), "tempo.output_offset", ControlInput::Value(f32::NAN), true).is_err());
        assert_eq!(rig.s().tempo.output_offset_ms(), applied);
    }
}

/// T-475: lit frames sent to the output after an e-stop, counted, with
/// the offset at -100, 0 and +100 ms: none. Both ways a stop lands: the
/// HTTP handler (latch only, the gate not synced because the lock was
/// busy) and Escape through the shared state. Every frame after the stop
/// reaches the output dark, and what `/api/frame` reports (`output_lit`)
/// is 0 from that first frame on.
#[test]
fn no_lit_frame_reaches_the_output_after_an_estop_whatever_the_offset() {
    for offset in [-100.0, 0.0, 100.0] {
        for via_http in [true, false] {
            let mut rig = armed_square_rig(&format!("estop-count-{offset}-{via_http}"));
            controls::apply(&mut rig.s(), "tempo.output_offset", ControlInput::Value(offset), true).unwrap();
            assert_eq!(rig.s().tempo.output_offset_ms(), offset);
            rig.s().request_arm(ArmSource::Ui).unwrap();
            for _ in 0..5 {
                rig.tick();
            }
            assert!(lit(&rig.wire) > 0, "offset {offset}: lit while armed");
            let estop = Arc::clone(&rig.s().estop);
            if via_http {
                estop.trip(ArmSource::Api);
            } else {
                rig.s().emergency_stop(ArmSource::Keyboard);
            }
            let before = rig.wire.lock().unwrap().frames;
            let (mut sent, mut lit_frames) = (0, 0);
            for _ in 0..60 {
                rig.tick();
                sent += 1;
                if lit(&rig.wire) > 0 {
                    lit_frames += 1;
                }
                assert_eq!(rig.s().output_lit, 0, "offset {offset}, http {via_http}: the reported frame is dark");
            }
            assert_eq!(rig.wire.lock().unwrap().frames - before, sent, "every tick sent a frame");
            assert_eq!(lit_frames, 0, "offset {offset}, http {via_http}: lit frames sent after the e-stop");
            assert!(!rig.wire.lock().unwrap().armed, "offset {offset}, http {via_http}: the output disarmed");
        }
    }
    // A stop that lands while a frame renders: the engine read the gate
    // armed under the lock before the trip; the stage re-reads the latch
    // and sends that frame, and every later one, dark.
    let wire = Arc::new(Mutex::new(Wire::default()));
    let mut stage = crate::output::OutputStage::new(Some(Box::new(Recorder(Arc::clone(&wire), None))));
    let estop = interlock::EStop::default();
    let frame = vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0), Point::lit(0.5, 0.5, 1.0, 1.0, 1.0)];
    assert_eq!(stage.emit(&frame, true, true, &estop).lit, 2);
    estop.trip(ArmSource::Api);
    let lit_frames = (0..10).filter(|_| stage.emit(&frame, true, true, &estop).lit > 0 || lit(&wire) > 0).count();
    assert_eq!(lit_frames, 0, "lit frames sent after the e-stop with a stale gate");
    assert!(!wire.lock().unwrap().armed);
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

// ---------- « Préparer » (T-274) ----------

/// One « Préparer » action as the page could send it: a cue prepared
/// (sometimes one that doesn't exist), adjusted with values past their
/// ranges, the prepared view rendered (the page polling), cleared, and
/// with `may_send`, « Envoyer ».
fn random_prepared(s: &mut Shared, rng: &mut Rng, may_send: bool) {
    use crate::prepared;
    match rng.below(if may_send { 6 } else { 5 }) {
        0 | 1 => {
            let id = if rng.chance(0.95) { random_preset(s, rng).0 } else { "pas-un-cue".to_string() };
            let _ = prepared::prepare(s, &id);
        }
        2 => {
            let pick = |rng: &mut Rng, lo: f32, hi: f32| rng.chance(0.6).then(|| rng.range(lo, hi));
            let a = prepared::Adjust {
                scale: pick(rng, -1.0, 3.0),
                brightness: pick(rng, -1.0, 3.0),
                rotation_speed: pick(rng, -2000.0, 2000.0),
                color: rng.chance(0.5).then(|| [rng.below(256) as u8, rng.below(256) as u8, rng.below(256) as u8]),
            };
            let _ = prepared::adjust(s, &a);
        }
        3 => {
            for _ in 0..1 + rng.below(3) {
                let _ = prepared::render(s);
            }
        }
        4 => prepared::clear(s),
        _ => {
            let _ = prepared::send(s);
        }
    }
}

/// Rig `n` of a « Préparer » check: armed, with one more projector on a
/// recording output (T-277).
fn prepared_rig(seed: u64, n: usize) -> (Rig, Arc<Mutex<Wire>>, Rng) {
    let mut rng = Rng::new(seed);
    let mut s = test_support::shared();
    s.safety.set(random_safety(&mut rng), true).unwrap();
    s.outputs.set(MAIN_OUTPUT, Some(random_limits(&mut rng)), None, false, true).unwrap();
    s.calibration = random_calibration(&mut rng);
    let (_, look) = random_preset(&s, &mut rng);
    s.settings = look;
    let mut rig = Rig::new(s, &format!("prepared-{n}"), seed);
    let (_, wire) = add_projectors(&mut rig, 1, None).pop().unwrap();
    rig.tick();
    rig.s().request_arm(ArmSource::Ui).unwrap();
    (rig, wire, rng)
}

/// Nothing prepared reaches any output before « Envoyer » (T-274): the
/// same random session (cues, looks, live moves, transitions, arming,
/// e-stop...) on three armed rigs, each with a second projector; on the
/// third, a cue is prepared, adjusted, re-prepared, rendered and cleared
/// at random all along (its own stream). Every frame, the main output and
/// the other projector's output are the same on the three rigs, bit for
/// bit (the first two are the control: the session replays exactly).
/// Then « Envoyer » on the third plays the prepared cue through the deck.
#[test]
fn nothing_prepared_reaches_any_output_before_it_is_sent() {
    let lit = std::cell::Cell::new(0usize);
    run_property("nothing_prepared_reaches_any_output_before_it_is_sent", 274, 12, |seed| {
        let mut rigs: Vec<(Rig, Arc<Mutex<Wire>>, Rng, u64)> = (0..3)
            .map(|n| {
                let (rig, wire, rng) = prepared_rig(seed, n);
                (rig, wire, rng, 0)
            })
            .collect();
        let mut prng = Rng::new(seed ^ 0x0274_0274);
        {
            let mut s = rigs[2].0.s();
            let (id, _) = random_preset(&s, &mut prng);
            let _ = crate::prepared::prepare(&mut s, &id);
        }
        for frame in 0..90 {
            for (rig, _, rng, beats) in &mut rigs {
                if rng.chance(0.3) {
                    random_action(rig, rng, beats);
                }
            }
            for _ in 0..1 + prng.below(3) {
                random_prepared(&mut rigs[2].0.s(), &mut prng, false);
            }
            for (rig, _, _, _) in &mut rigs {
                rig.tick();
            }
            let main: Vec<Vec<Point>> = rigs.iter().map(|(r, _, _, _)| r.wire.lock().unwrap().last.clone()).collect();
            let other: Vec<Vec<Point>> = rigs.iter().map(|(_, w, _, _)| w.lock().unwrap().last.clone()).collect();
            if main[0] != main[1] || other[0] != other[1] {
                return Err(format!("frame {frame}: the control session did not replay exactly"));
            }
            if main[2] != main[0] {
                return Err(format!("frame {frame}: the main output changed with a cue prepared ({} vs {} points)", main[2].len(), main[0].len()));
            }
            if other[2] != other[0] {
                return Err(format!("frame {frame}: the second projector's output changed with a cue prepared"));
            }
            if rigs[2].0.s().frame != rigs[0].0.s().frame {
                return Err(format!("frame {frame}: /api/frame's points changed with a cue prepared"));
            }
            lit.set(lit.get() + main[0].iter().any(|p| p.is_lit()) as usize);
        }
        // « Envoyer »: the prepared cue is pressed like any cue.
        let mut s = rigs[2].0.s();
        let (id, _) = random_preset(&s, &mut prng);
        if s.deck.slot(&id).show.is_none() {
            crate::prepared::prepare(&mut s, &id).map_err(|e| format!("prepare {id}: {e:?}"))?;
            crate::prepared::send(&mut s).map_err(|e| format!("send {id}: {e:?}"))?;
            if !s.deck.active.iter().any(|a| a.cue == id) && !s.deck.pending.iter().any(|p| p.cue == id) {
                return Err(format!("« Envoyer » did not play {id}"));
            }
        }
        Ok(())
    });
    assert!(lit.get() > 0, "the sessions sent light");
}

/// Armed, nothing on show: a prepared cue, its view rendered every
/// frame, leaves every output dark; « Envoyer » lights it, through the
/// same safety stage as any cue.
#[test]
fn a_prepared_cue_stays_dark_until_it_is_sent() {
    let mut rig = armed_square_rig("prepared-dark");
    let wires = add_projectors(&mut rig, 1, None);
    rig.s().look_on = false;
    rig.tick();
    rig.s().request_arm(ArmSource::Ui).unwrap();
    let id = {
        let mut s = rig.s();
        let id = s.presets.iter().find(|p| p.id.starts_with("faisceaux")).unwrap_or(&s.presets[0]).id.clone();
        crate::prepared::prepare(&mut s, &id).unwrap();
        crate::prepared::adjust(&mut s, &crate::prepared::Adjust { brightness: Some(1.0), ..Default::default() }).unwrap();
        id
    };
    for frame in 0..60 {
        let preview = crate::prepared::render(&mut rig.s());
        assert!(preview.iter().any(|p| p.is_lit()), "frame {frame}: the prepared view draws");
        rig.tick();
        assert_eq!(lit(&rig.wire), 0, "frame {frame}: the main output stays dark");
        assert_eq!(lit(&wires[0].1), 0, "frame {frame}: the second projector stays dark");
        assert!(rig.s().frame.iter().all(|p| !p.is_lit()), "frame {frame}: /api/frame stays dark");
    }
    assert!(rig.s().gate.is_armed(), "still armed");
    assert_eq!(crate::prepared::send(&mut rig.s()).unwrap(), id);
    for _ in 0..10 {
        rig.tick();
    }
    assert!(lit(&rig.wire) > 0, "sent: the main output shows the cue");
    assert!(lit(&wires[0].1) > 0, "sent: the second projector too");
}

/// T-442: an ILDA recording through the real engine. The recorder gets
/// the calibrated frame from before the safety stage, one per deadline,
/// disarmed, without arming or lighting the output; the file it makes
/// carries the safety chain (nothing lit below the full horizon, nothing
/// over the caps), not the raw look.
#[test]
fn a_live_recording_never_arms_and_its_file_carries_the_safety_chain() {
    use crate::record::{self, RecordRequest, RecordState};
    let mut s = test_support::shared();
    s.settings = Settings { content: Content::Shape { shape: "circle".into() }, scale: 0.6, brightness: 1.0, rotation_speed: 90.0, ..Settings::default() };
    s.calibration = Calibration { offset_x: 0.2, ..Calibration::default() };
    let horizon = Horizon { y: 0.0, lines: true, ramp: 0.0, level: 0.0 };
    s.safety.set(SafetySettings { horizon, ..Default::default() }, true).unwrap();
    let caps = OutputLimits { max_power: 0.4, max_color: [1.0; 3] };
    s.outputs.set(MAIN_OUTPUT, Some(caps), None, false, true).unwrap();
    let mut rig = Rig::new(s, "ilda-record", 0);
    let rx = {
        let mut s = rig.s();
        let now = s.now_s();
        let req = RecordRequest { seconds: 0.4, fps: 25, ..Default::default() };
        s.recorder.start(&req, now, MAIN_OUTPUT).unwrap().1
    };
    for _ in 0..60 {
        rig.tick();
        assert!(!rig.s().gate.is_armed(), "recording never arms");
        let w = rig.wire.lock().unwrap();
        assert!(!w.armed && w.last.iter().all(|p| !p.is_lit()), "the output stays dark");
    }
    assert_eq!(rig.s().recorder.status().state, RecordState::Writing);
    let raw: Vec<Vec<Point>> = rx.try_iter().collect();
    assert_eq!(raw.len(), 10);
    // Captured before the safety stage, after the calibration: lit below
    // the horizon, above the cap, moved right.
    let lit: Vec<&Point> = raw.iter().flatten().filter(|p| p.is_lit()).collect();
    assert!(lit.iter().any(|p| p.y < -0.3), "lit below the horizon before the chain");
    assert!(lit.iter().any(|p| p.r.max(p.g).max(p.b) > 0.45), "brighter than the cap before the chain");
    let cx = lit.iter().map(|p| p.x).sum::<f32>() / lit.len() as f32;
    assert!((cx - 0.2).abs() < 0.05, "calibrated: {cx}");
    // The file: the output chain baked in.
    let chain = crate::ilda_library::ExportSafety::of(&rig.s(), MAIN_OUTPUT).unwrap();
    let (bytes, ..) = record::bake(&raw, 25, &chain).unwrap();
    let back = crate::ilda::read(&bytes).unwrap();
    assert_eq!(back.len(), 10);
    let mut lit_file = 0;
    for p in back.iter().flat_map(|f| &f.points).filter(|p| p.is_lit()) {
        assert!(p.y >= -1e-3, "lit below the horizon: {p:?}");
        assert!(p.r.max(p.g).max(p.b) <= 0.4 + 1e-6, "over the cap: {p:?}");
        lit_file += 1;
    }
    assert!(lit_file > 0, "the upper half still shows");
}
