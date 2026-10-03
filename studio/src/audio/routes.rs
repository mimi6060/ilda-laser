//! Audio routes (T-153): any analysis value or event (bands, level, kick,
//! snare, hat, build-up, drop…) through a `Shaper` (T-238) to any control
//! an LFO may move.
//!
//! Stored like the LFOs: at most `MAX_ROUTES`, numbers sanitised, saved to
//! `audio_routes.json` in the data directory and in project files, unknown
//! sources or targets dropped on load. Routes are **bound** (source parsed,
//! target looked up in the registry through the LFO allow-list) only when
//! they change; each frame the engine calls `RouteStore::apply` on its
//! copies of the look and the live modifiers, which allocates nothing.
//!
//! Several routes on one control add up (`base + Σ`, clamped once), after
//! the LFOs. The *Temps ↔ Audio* crossfader (`mix`, 0..1, 0.5 = both in
//! full) scales the routes by `min(1, 2·mix)` and the LFOs by
//! `min(1, 2·(1 − mix))`.
//!
//! **Safety**: the same rules as the LFOs, through `shape::Target` and
//! `lfo::offset`: never the stored values, never transport (arm,
//! blackout), tempo, cues, grid, calibration or safety settings;
//! brightness only dims below the operator's fader; the frame then goes
//! through calibration, the strobe limiter and the horizon (safety.rs) as
//! any other. Nothing here can arm.
//!
//! **Audio safety (T-245, audio/safety.rs)**: routes on a brightness or
//! visibility control share one `FlashLimiter` per control (at most
//! `max_flash_hz` dips a second); when the audio is stale, gone, silent or
//! switching source, every route fades to neutral over its release.
//!
//! The legacy per-look `AudioReact` (`settings.audio`) is untouched and
//! still renders as before; routes are master-level and come on top.

use super::safety::{darkness_sign, fade_step, FlashLimiter, Guard};
use super::shape::{Shaper, Source, Target};
use crate::controls::ControlRegistry;
use crate::engine::{AudioFeatures, Settings};
use crate::live::LiveModifiers;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Routes allowed at once.
pub const MAX_ROUTES: usize = 16;

/// Every source a route can listen to, with its French label, continuous
/// values first (`AUDIO_VALUES`), then events (`AUDIO_EVENTS`).
pub const SOURCES: [(&str, &str); 19] = [
    ("bass", "Basses"),
    ("sub", "Sub-basses"),
    ("bass_band", "Basses (bande)"),
    ("low_mid", "Bas-médiums"),
    ("mid", "Médiums"),
    ("high", "Aigus"),
    ("level", "Niveau"),
    ("buildup", "Montée"),
    ("kick_strength", "Force du kick"),
    ("snare_strength", "Force de la caisse claire"),
    ("hat_strength", "Force du charleston"),
    ("centroid", "Brillance"),
    ("bpm_confidence", "Confiance du tempo"),
    ("kick", "Kick (coup)"),
    ("snare", "Caisse claire (coup)"),
    ("hat", "Charleston (coup)"),
    ("beat", "Temps (coup)"),
    ("onset", "Attaque (coup)"),
    ("drop", "Drop (coup)"),
];

/// One route: `source` → `shape` → `target`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioRoute {
    /// An `AUDIO_VALUES` or `AUDIO_EVENTS` id (`bass`, `kick`…).
    pub source: String,
    /// A control id an LFO may modulate (`lfo::modulatable`).
    pub target: String,
    /// Gate, curve, attack / release / decay, range (−1..1 of the
    /// target's range: `max` is the *Quantité*).
    pub shape: Shaper,
    pub enabled: bool,
}

impl Default for AudioRoute {
    fn default() -> Self {
        Self { source: "bass".into(), target: "master.size".into(), shape: Shaper::default(), enabled: true }
    }
}

/// Everything saved: the routes and the *Temps ↔ Audio* crossfader.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioRouting {
    /// 0 = only the LFOs (time), 1 = only the routes (audio), 0.5 = both.
    pub mix: f32,
    pub routes: Vec<AudioRoute>,
}

impl Default for AudioRouting {
    fn default() -> Self {
        Self { mix: 0.5, routes: Vec::new() }
    }
}

impl AudioRouting {
    /// Numbers in range (NaN → default).
    pub fn sanitized(mut self) -> Self {
        self.mix = if self.mix.is_finite() { self.mix.clamp(0.0, 1.0) } else { 0.5 };
        for r in &mut self.routes {
            r.shape = std::mem::take(&mut r.shape).sanitized();
        }
        self
    }

    /// Share of the routes, 0..1 (full from the middle up).
    pub fn audio_share(&self) -> f32 {
        (2.0 * self.mix).clamp(0.0, 1.0)
    }

    /// Share of the LFOs, 0..1 (full from the middle down).
    pub fn time_share(&self) -> f32 {
        (2.0 * (1.0 - self.mix)).clamp(0.0, 1.0)
    }
}

/// At most `MAX_ROUTES`, each from a known source to a control an LFO may
/// move. The messages are the UI's (French).
pub fn validate(routing: &AudioRouting, reg: &ControlRegistry) -> Result<()> {
    if routing.routes.len() > MAX_ROUTES {
        bail!("{MAX_ROUTES} liens audio au maximum");
    }
    for r in &routing.routes {
        if Source::parse(&r.source).is_none() {
            bail!("source audio inconnue : « {} »", r.source);
        }
        if Target::bind(reg, &r.target).is_none() {
            bail!("« {} » ne peut pas être piloté par l'audio", r.target);
        }
    }
    Ok(())
}

/// A route ready for the frame loop.
struct Bound {
    source: Source,
    /// Index in `groups` (one per distinct target).
    group: usize,
    shaper: Shaper,
    enabled: bool,
    /// Last continuous input, 0..1 (events: 0), for the meter.
    input: f32,
    /// Share of the output, 0..1: falls to 0 over the release when the
    /// audio goes away (T-245).
    fade: f32,
}

/// One target and this frame's sum of its routes.
struct Group {
    target: Target,
    sum: f32,
    /// Brightness-like targets: which sign dims, and the flash cap.
    dark: Option<(f32, FlashLimiter)>,
}

/// What each route did on the last frame, for the UI's meters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Meter {
    /// Shaped output, −1..1 of the target's range (before the mix).
    pub value: f32,
    /// Continuous input 0..1 (`null` for an event source).
    pub input: Option<f32>,
    /// Whether the gate is open (continuous sources).
    pub open: bool,
}

/// The routes, saved to `audio_routes.json` on every change, and their
/// bound, running form.
pub struct RouteStore {
    path: PathBuf,
    routing: AudioRouting,
    bound: Vec<Bound>,
    groups: Vec<Group>,
    /// Fading out for a change of source (T-245).
    switching: bool,
    /// A flash was held back by the cap on the last frame.
    limited: bool,
}

impl RouteStore {
    /// A missing or unreadable file starts empty; routes that aren't valid
    /// any more (unknown source or target) are dropped.
    pub fn load_or_create(path: PathBuf, reg: &ControlRegistry) -> Self {
        let routing: AudioRouting = crate::load_json(&path);
        let mut store = Self { path, routing: AudioRouting::default(), bound: Vec::new(), groups: Vec::new(), switching: false, limited: false };
        store.install(routing.sanitized(), reg);
        store
    }

    pub fn routing(&self) -> &AudioRouting {
        &self.routing
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replaces the routes in memory only (the caller checked them with
    /// `validate` and saves the file: project open, T-286).
    pub fn replace_in_memory(&mut self, routing: AudioRouting, reg: &ControlRegistry) {
        self.install(routing.sanitized(), reg);
    }

    /// Replaces the routes (checked, numbers clamped) and saves.
    pub fn set(&mut self, routing: AudioRouting, reg: &ControlRegistry) -> Result<()> {
        validate(&routing, reg)?;
        let routing = routing.sanitized();
        let json = serde_json::to_string_pretty(&routing).context("failed to serialize audio routes")?;
        std::fs::write(&self.path, json).with_context(|| format!("failed to write {}", self.path.display()))?;
        self.install(routing, reg);
        Ok(())
    }

    /// Binds the routes (allocates: only when they change). A route whose
    /// source and target are unchanged at the same place keeps its running
    /// envelope, so dragging a slider doesn't restart it.
    /// Routes that can't be bound (unknown source or target) are dropped,
    /// and only the first `MAX_ROUTES` kept.
    fn install(&mut self, mut routing: AudioRouting, reg: &ControlRegistry) {
        routing.routes.retain(|r| Source::parse(&r.source).is_some() && Target::bind(reg, &r.target).is_some());
        routing.routes.truncate(MAX_ROUTES);
        let old = std::mem::take(&mut self.bound);
        let old_groups = std::mem::take(&mut self.groups);
        let mut groups: Vec<Group> = Vec::new();
        let mut bound = Vec::with_capacity(routing.routes.len());
        for (i, r) in routing.routes.iter().enumerate() {
            let (Some(source), Some(target)) = (Source::parse(&r.source), Target::bind(reg, &r.target)) else { continue };
            let group = match groups.iter().position(|g| g.target.id() == target.id()) {
                Some(g) => g,
                None => {
                    // A brightness target keeps its flash limiter (its
                    // timing) across edits.
                    let dark = darkness_sign(target.id()).map(|sign| {
                        let kept = old_groups.iter().find(|g| g.target.id() == target.id()).and_then(|g| g.dark);
                        (sign, kept.map_or_else(FlashLimiter::default, |(_, l)| l))
                    });
                    groups.push(Group { target, sum: 0.0, dark });
                    groups.len() - 1
                }
            };
            let same = old.get(i).filter(|b| b.source == source && old_groups[b.group].target.id() == r.target);
            let (shaper, fade) = match same {
                Some(b) => {
                    let mut s = b.shaper.clone();
                    s.retune(&r.shape);
                    (s, b.fade)
                }
                None => (r.shape.clone(), 0.0),
            };
            bound.push(Bound { source, group, shaper, enabled: r.enabled, input: 0.0, fade });
        }
        self.bound = bound;
        self.groups = groups;
        self.routing = routing;
    }

    /// One frame, on the engine's copies: every route reads `features`
    /// (`dt` = real frame time, `beat_len_s` = 60 / BPM of the one clock),
    /// then each target moves by the sum of its enabled routes × the audio
    /// share of the crossfader. `guard` (T-245): without live audio every
    /// route fades to neutral over its release; brightness-like targets go
    /// through the flash cap. Allocates nothing. Returns true when a
    /// colour target moved: call `lfo::recolor_live` once after.
    pub fn apply(&mut self, features: &AudioFeatures, dt: f32, beat_len_s: f32, guard: &Guard, settings: &mut Settings, live: &mut LiveModifiers) -> bool {
        if guard.source_changed {
            // Another source's counters aren't events: note them afresh.
            self.switching = true;
            for b in &mut self.bound {
                b.shaper.forget_events();
            }
        }
        let on = guard.live && !self.switching;
        for g in &mut self.groups {
            g.sum = 0.0;
        }
        let mut faded = true;
        for b in &mut self.bound {
            // Disabled routes keep running for their meter, so turning one
            // on doesn't fire on an old event.
            let v = b.shaper.feed(b.source, features, dt, beat_len_s);
            let (attack, release) = (b.shaper.attack.seconds(beat_len_s), b.shaper.release.seconds(beat_len_s));
            b.fade = fade_step(b.fade, on, dt, attack, release);
            if !on && b.fade == 0.0 {
                // At neutral: the envelopes start from rest when the audio
                // returns (an old peak never comes back).
                b.shaper.rest();
            }
            faded &= b.fade == 0.0;
            b.input = match b.source {
                Source::Value(id) => features.value(id).unwrap_or(0.0),
                Source::Event(_) => 0.0,
            };
            if b.enabled {
                self.groups[b.group].sum += v * b.fade;
            }
        }
        if self.switching && faded {
            self.switching = false;
        }
        let share = self.routing.audio_share();
        let mut recolor = false;
        self.limited = false;
        for g in &mut self.groups {
            let mut amount = g.sum * share;
            if let Some((sign, limiter)) = &mut g.dark {
                // Darkness ≥ 0 (a brightness can only be dimmed anyway).
                let dark = limiter.step((amount * *sign).max(0.0), dt, guard.max_flash_hz, !on);
                self.limited |= limiter.limited();
                amount = if amount * *sign > 0.0 || dark > 0.0 { dark * *sign } else { amount };
            }
            if amount != 0.0 && share > 0.0 {
                recolor |= g.target.apply(amount, settings, live);
            }
        }
        recolor
    }

    /// A flash was held back by the cap on the last frame.
    pub fn flash_limited(&self) -> bool {
        self.limited
    }

    /// One meter per route, in the order of `routing().routes`.
    pub fn meters(&self) -> impl Iterator<Item = Meter> + '_ {
        self.bound.iter().map(|b| Meter {
            value: b.shaper.value(),
            input: (!b.source.is_event()).then_some(b.input),
            open: b.shaper.gate_open(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::shape::{Curve, Span};
    use crate::engine::{AUDIO_EVENTS, AUDIO_VALUES};
    use crate::presets;

    const FPS60: f32 = 1.0 / 60.0;

    fn reg() -> ControlRegistry {
        ControlRegistry::build(&presets::catalog())
    }

    /// `base` with a few settings changed (the running state is private).
    fn with(mut base: Shaper, edit: impl FnOnce(&mut Shaper)) -> Shaper {
        edit(&mut base);
        base
    }

    fn route(source: &str, target: &str, shape: Shaper) -> AudioRoute {
        AudioRoute { source: source.into(), target: target.into(), shape, enabled: true }
    }

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-routes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("audio_routes.json")
    }

    fn store(routes: Vec<AudioRoute>) -> RouteStore {
        let mut s = RouteStore::load_or_create(PathBuf::from("/nonexistent/audio_routes.json"), &reg());
        let routing = AudioRouting { routes, ..Default::default() };
        validate(&routing, &reg()).unwrap();
        s.replace_in_memory(routing, &reg());
        s
    }

    /// master.size after one frame of `store` on `f`.
    fn size_after(store: &mut RouteStore, f: &AudioFeatures) -> f32 {
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        store.apply(f, FPS60, 0.5, &Guard::LIVE, &mut settings, &mut live);
        live.size
    }

    #[test]
    fn every_feature_is_a_source_with_a_label() {
        let ids: Vec<&str> = SOURCES.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids.len(), AUDIO_VALUES.len() + AUDIO_EVENTS.len());
        for id in AUDIO_VALUES.iter().chain(AUDIO_EVENTS.iter()) {
            assert!(ids.contains(id), "{id} has no label");
        }
        assert!(ids.iter().all(|id| Source::parse(id).is_some()));
    }

    #[test]
    fn bass_on_size_follows_the_bass_and_nothing_else() {
        let fast = with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(0.0); s.release = Span::Ms(0.0); });
        let mut s = store(vec![route("bass", "master.size", fast)]);
        // 50 % bass: base 1 + 0.5 × range 2 = 2 (clamped), 25 % → 1.5.
        assert_eq!(size_after(&mut s, &AudioFeatures { bass: 0.25, ..Default::default() }), 1.5);
        assert_eq!(size_after(&mut s, &AudioFeatures { bass: 0.0, ..Default::default() }), 1.0);
        // Everything else loud, no bass: nothing moves.
        let noisy = AudioFeatures { level: 1.0, bands: crate::engine::Bands::from_array([0.0, 0.0, 1.0, 1.0, 1.0]), kick: 9, snare: 3, buildup: 1.0, ..Default::default() };
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        s.apply(&noisy, FPS60, 0.5, &Guard::LIVE, &mut settings, &mut live);
        assert_eq!((settings, live), (Settings::default(), LiveModifiers::default()));
    }

    #[test]
    fn attack_and_release_shape_a_one_frame_pulse() {
        // A one-frame bass pulse: rises by the attack, then falls with the
        // release (e^−1 after 150 ms, ± 1 frame).
        let shape = with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(10.0); s.release = Span::Ms(150.0); s.max = 0.5; });
        let mut s = store(vec![route("bass", "master.size", shape)]);
        let quiet = AudioFeatures::default();
        size_after(&mut s, &quiet);
        let peak = size_after(&mut s, &AudioFeatures { bass: 1.0, ..Default::default() }) - 1.0;
        let expected_peak = 1.0 - (-FPS60 / 0.010).exp(); // share of the way in one frame
        assert!((peak - expected_peak).abs() < 1e-3, "{peak} vs {expected_peak}");
        let mut out = Vec::new();
        for _ in 0..30 {
            out.push(size_after(&mut s, &quiet) - 1.0);
        }
        assert!(out.windows(2).all(|w| w[1] <= w[0]), "falls monotonically");
        let frames_to_1_over_e = out.iter().position(|v| *v <= peak / std::f32::consts::E).unwrap() + 1;
        assert!((frames_to_1_over_e as f32 - 0.150 / FPS60).abs() <= 1.0, "{frames_to_1_over_e} frames");
    }

    #[test]
    fn a_kick_route_pulses_and_decays() {
        let shape = with(Shaper::default(), |s| { s.attack = Span::Ms(0.0); s.decay = Span::Ms(100.0); s.max = 0.25; });
        let mut s = store(vec![route("kick", "master.size", shape)]);
        let mut f = AudioFeatures { kick: 4, kick_strength: 0.8, ..Default::default() };
        assert_eq!(size_after(&mut s, &f), 1.0, "the first frame only primes the counter");
        f.kick = 5;
        // Strength 0.8 × max 0.25 of the 0..2 range, on the kick's own frame.
        assert!((size_after(&mut s, &f) - 1.4).abs() < 1e-5);
        for _ in 0..12 {
            size_after(&mut s, &f);
        }
        assert!(size_after(&mut s, &f) < 1.05, "back down after the decay");
    }

    #[test]
    fn routes_on_one_target_add_up_before_the_clamp() {
        let fast = with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(0.0); s.release = Span::Ms(0.0); });
        let up = route("bass", "master.size", with(fast.clone(), |s| { s.max = 1.0; }));
        let down = route("mid", "master.size", with(fast, |s| { s.max = -1.0; }));
        let mut s = store(vec![up, down]);
        let f = AudioFeatures { bass: 1.0, bands: crate::engine::Bands::from_array([0.0, 0.0, 0.0, 1.0, 0.0]), ..Default::default() };
        // +1 and −1 of the range cancel out (clamping each in turn would give 0).
        assert_eq!(size_after(&mut s, &f), 1.0);
    }

    #[test]
    fn disabled_routes_and_the_crossfader() {
        let fast = with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(0.0); s.release = Span::Ms(0.0); s.max = 0.25; });
        let mut s = store(vec![AudioRoute { enabled: false, ..route("bass", "master.size", fast.clone()) }]);
        let f = AudioFeatures { bass: 1.0, ..Default::default() };
        assert_eq!(size_after(&mut s, &f), 1.0);
        assert_eq!(s.meters().next().unwrap().value, 0.25, "the meter still shows it");
        let mut s = store(vec![route("bass", "master.size", fast)]);
        for (mix, size, lfo_share) in [(0.0, 1.0, 1.0), (0.25, 1.25, 1.0), (0.5, 1.5, 1.0), (0.75, 1.5, 0.5), (1.0, 1.5, 0.0)] {
            let routing = AudioRouting { mix, ..s.routing().clone() };
            s.replace_in_memory(routing, &reg());
            assert!((size_after(&mut s, &f) - size).abs() < 1e-6, "mix {mix}");
            assert_eq!(s.routing().time_share(), lfo_share);
        }
    }

    #[test]
    fn refused_targets_and_sources() {
        let reg = reg();
        for id in ["transport.arm", "transport.blackout", "tempo.bpm", "cue.max_active", "grid.1.1.1", "master.rot.sync", "calibration.x_scale", "safety.strobe_max_hz", "nope"] {
            let r = AudioRouting { routes: vec![route("bass", id, Shaper::default())], ..Default::default() };
            let e = validate(&r, &reg).unwrap_err().to_string();
            assert!(e.contains("ne peut pas être piloté"), "{id}: {e}");
        }
        let r = AudioRouting { routes: vec![route("volume", "master.size", Shaper::default())], ..Default::default() };
        assert!(validate(&r, &reg).unwrap_err().to_string().contains("source audio inconnue"));
        let r = AudioRouting { routes: vec![AudioRoute::default(); MAX_ROUTES + 1], ..Default::default() };
        assert!(validate(&r, &reg).is_err());
    }

    #[test]
    fn audio_never_arms_and_only_moves_copies() {
        let s = crate::test_support::shared();
        let fast = with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(0.0); });
        let mut routes = store(vec![route("bass", "master.size", fast.clone()), route("kick", "master.brightness", fast)]);
        let (mut settings, mut live) = (s.settings.clone(), s.live.clone());
        let mut f = AudioFeatures { bass: 1.0, ..Default::default() };
        routes.apply(&f, FPS60, 0.5, &Guard::LIVE, &mut settings, &mut live);
        f.kick += 1;
        routes.apply(&f, FPS60, 0.5, &Guard::LIVE, &mut settings, &mut live);
        assert_eq!(live.size, 2.0);
        assert!(live.brightness <= s.live.brightness, "brightness only dims");
        assert_eq!((s.live.size, s.gate.is_armed()), (1.0, false));
    }

    #[test]
    fn store_validates_sanitizes_saves_and_reloads() {
        let reg = reg();
        let path = temp_path("store");
        let mut store = RouteStore::load_or_create(path.clone(), &reg);
        assert!(store.routing().routes.is_empty());
        assert_eq!(store.routing().mix, 0.5);
        let bad = AudioRouting { routes: vec![route("bass", "transport.arm", Shaper::default())], ..Default::default() };
        assert!(store.set(bad, &reg).is_err());
        assert!(!path.exists(), "nothing written on a refusal");
        let wild = with(Shaper::default(), |s| { s.gain = 99.0; s.gate = f32::NAN; s.curve = Curve::Sqrt; s.max = -3.0; });
        let routing = AudioRouting { mix: 7.0, routes: vec![route("kick", "master.color.hue", wild), route("buildup", "master.speed", Shaper::default())] };
        store.set(routing, &reg).unwrap();
        let first = &store.routing().routes[0].shape;
        assert_eq!((first.gain, first.gate, first.curve, first.max), (8.0, 0.05, Curve::Sqrt, -1.0));
        assert_eq!(store.routing().mix, 1.0);
        let reloaded = RouteStore::load_or_create(path.clone(), &reg);
        assert_eq!(reloaded.routing(), store.routing());
        // A file with a route that isn't allowed any more: it's dropped.
        std::fs::write(&path, r#"{"routes":[{"source":"bass","target":"tempo.bpm"},{"source":"high"}]}"#).unwrap();
        let reloaded = RouteStore::load_or_create(path.clone(), &reg);
        assert_eq!(reloaded.routing().routes, vec![AudioRoute { source: "high".into(), ..Default::default() }]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn editing_a_route_keeps_its_running_envelope() {
        let slow = with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(0.0); s.release = Span::Ms(1000.0); });
        let mut s = store(vec![route("bass", "master.size", slow.clone())]);
        size_after(&mut s, &AudioFeatures { bass: 1.0, ..Default::default() });
        let routing = AudioRouting { routes: vec![route("bass", "master.size", with(slow.clone(), |s| { s.max = 0.5; }))], ..Default::default() };
        s.replace_in_memory(routing, &reg());
        assert!(size_after(&mut s, &AudioFeatures::default()) > 1.45, "still up, now with the new range");
        // Another source: starts from rest.
        let routing = AudioRouting { routes: vec![route("mid", "master.size", slow)], ..Default::default() };
        s.replace_in_memory(routing, &reg());
        assert_eq!(size_after(&mut s, &AudioFeatures::default()), 1.0);
    }

    #[test]
    fn serde_defaults_and_format() {
        let r: AudioRouting = serde_json::from_str(r#"{"routes":[{"target":"master.pos_x","shape":{"attack":{"beats":0.25}}}]}"#).unwrap();
        assert_eq!(r.mix, 0.5);
        let route = &r.routes[0];
        assert_eq!((route.source.as_str(), route.enabled, route.shape.attack, route.shape.release), ("bass", true, Span::Beats(0.25), Span::Ms(150.0)));
        let json = serde_json::to_value(AudioRouting { routes: vec![AudioRoute::default()], ..Default::default() }).unwrap();
        assert_eq!(json["routes"][0]["shape"]["curve"], "linear");
        assert_eq!(json["routes"][0]["source"], "bass");
    }

    #[test]
    fn a_frame_of_routes_does_not_allocate() {
        let mut s = store(vec![
            route("bass", "master.size", Shaper::default()),
            route("kick", "master.brightness", with(Shaper::default(), |s| { s.min = 0.0; s.max = -1.0; })),
            route("mid", "master.color.hue", Shaper::default()),
            route("buildup", "master.size", Shaper::default()),
        ]);
        let mut f = AudioFeatures { bass: 0.5, ..Default::default() };
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        let n = crate::audio::capture::tests::allocations_during(|| {
            for i in 0..600 {
                f.kick += (i % 30 == 0) as u64;
                f.bass = (i as f32 * 0.05).sin().abs();
                s.apply(&f, FPS60, 0.5, &Guard::LIVE, &mut settings, &mut live);
                let meters = s.meters().fold(0.0, |a, m| a + m.value);
                assert!(meters.is_finite());
            }
        });
        assert_eq!(n, 0);
    }

    // ---------- audio safety (T-245) ----------

    use crate::audio::safety::tests::count_flashes;
    use crate::engine::{Animator, BeatClock, Calibration};
    use crate::patterns::Point;
    use crate::safety::{self, SafetySettings, StrobeLimiter};

    /// The engine's path for one frame, as main.rs runs it: routes on
    /// copies of the look and live modifiers, render, live stage,
    /// calibration, then the output safety stage last.
    struct Pipe {
        animator: Animator,
        limiter: StrobeLimiter,
        t: f64,
    }

    impl Pipe {
        fn new() -> Self {
            Self { animator: Animator::default(), limiter: StrobeLimiter::default(), t: 0.0 }
        }

        /// (frame before the safety stage, output frame, modulated live copy).
        fn frame(&mut self, routes: &mut RouteStore, f: &AudioFeatures, guard: &Guard, look: &Settings, base: &LiveModifiers, cfg: &SafetySettings) -> (Vec<Point>, Vec<Point>, LiveModifiers) {
            let (mut settings, mut live) = (look.clone(), base.clone());
            routes.apply(f, FPS60, 0.5, guard, &mut settings, &mut live);
            let points = self.animator.render(&settings, *f, FPS60, &BeatClock::default());
            let calibration = Calibration::default();
            let before: Vec<Point> = crate::live::apply(&points, &live, &crate::live::LiveState::default(), &[])
                .into_iter()
                .map(|p| {
                    let (x, y) = calibration.apply(p.x, p.y);
                    Point { x, y, ..p }
                })
                .collect();
            let out = safety::apply(before.clone(), self.t, cfg, &mut self.limiter);
            self.t += FPS60 as f64;
            (before, out, live)
        }
    }

    /// Light levels normalised to their peak, for `count_flashes`.
    fn normalised(levels: &[f32]) -> Vec<f32> {
        let peak = levels.iter().cloned().fold(0.0, f32::max).max(1e-6);
        levels.iter().map(|l| l / peak).collect()
    }

    /// A kick-driven flash on the master brightness: dark between hits,
    /// lit on each one.
    fn kick_flash() -> AudioRoute {
        route("kick", "master.brightness", with(Shaper::default(), |s| { s.attack = Span::Ms(0.0); s.decay = Span::Ms(30.0); s.min = -1.0; s.max = 0.0; }))
    }

    #[test]
    fn kicks_at_20_hz_on_the_brightness_flash_at_most_max_flash_hz() {
        let look = Settings { brightness: 1.0, ..Default::default() };
        for max_hz in [10.0, 6.0, 3.0] {
            let mut s = store(vec![kick_flash()]);
            let mut pipe = Pipe::new();
            let guard = Guard { max_flash_hz: max_hz, ..Guard::LIVE };
            let mut f = AudioFeatures { level: 0.8, ..Default::default() };
            let (mut before, mut out) = (Vec::new(), Vec::new());
            let mut limited = false;
            for i in 0..(8 * 60) {
                if i % 3 == 0 {
                    f.kick += 1; // 20 kicks a second
                }
                let (b, o, _) = pipe.frame(&mut s, &f, &guard, &look, &LiveModifiers::default(), &SafetySettings::default());
                before.push(safety::level(&b));
                out.push(safety::level(&o));
                limited |= s.flash_limited();
            }
            assert!(limited, "the cap held flashes back");
            // The audio cap itself, measured before the output stage...
            let capped = count_flashes(normalised(&before)) as f32 / 8.0;
            assert!(capped <= max_hz + 0.15, "{max_hz} Hz cap: {capped} flashes/s");
            assert!(capped >= max_hz * 0.7, "{max_hz} Hz cap: still flashing ({capped}/s)");
            // ...and what really goes out: never more, and above the T-101
            // limit (4 Hz) the strobe limiter holds it steady after its burst.
            let sent = count_flashes(normalised(&out)) as f32 / 8.0;
            assert!(sent <= max_hz + 0.15, "{max_hz} Hz: {sent} flashes/s out");
            assert_eq!(pipe.limiter.status().active, max_hz > 4.0, "{max_hz} Hz: {:?}", pipe.limiter.status());
        }
    }

    #[test]
    fn an_audio_cut_during_a_peak_falls_to_neutral_within_the_release() {
        let release = 0.150;
        let size = route("bass", "master.size", with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(0.0); s.release = Span::Ms(150.0); s.max = 0.5; }));
        let dim = route("kick", "master.brightness", with(Shaper::default(), |s| { s.attack = Span::Ms(0.0); s.decay = Span::Ms(5000.0); s.min = -1.0; s.max = 0.0; }));
        for routes in [vec![size.clone()], vec![dim.clone()], vec![size, dim]] {
            let mut s = store(routes);
            let mut f = AudioFeatures { bass: 1.0, kick: 1, ..Default::default() };
            let mut settings: Settings;
            let mut live = LiveModifiers::default();
            for _ in 0..30 {
                (settings, live) = (Settings::default(), LiveModifiers::default());
                s.apply(&f, FPS60, 0.5, &Guard::LIVE, &mut settings, &mut live);
            }
            assert!(live.size > 1.0 || live.brightness < 1.0, "at a peak: {live:?}");
            // Cut: the hub's features fall (τ 100 ms) and the guard says
            // not live from this frame on.
            let cut = Guard { live: false, ..Guard::LIVE };
            let mut frames = 0;
            loop {
                f.bass *= (-FPS60 / 0.1f32).exp();
                (settings, live) = (Settings::default(), LiveModifiers::default());
                s.apply(&f, FPS60, 0.5, &cut, &mut settings, &mut live);
                frames += 1;
                if (settings.clone(), live.clone()) == (Settings::default(), LiveModifiers::default()) {
                    break;
                }
                assert!(frames < 120, "never back to neutral: {live:?}");
            }
            assert!(frames as f32 <= (release / FPS60).ceil() + 1.0, "{frames} frames");
            // And it stays there.
            for _ in 0..60 {
                (settings, live) = (Settings::default(), LiveModifiers::default());
                s.apply(&f.neutral(), FPS60, 0.5, &cut, &mut settings, &mut live);
                assert_eq!((settings, live), (Settings::default(), LiveModifiers::default()));
            }
        }
    }

    #[test]
    fn a_change_of_source_fades_out_and_ignores_the_new_counters() {
        let pulse = route("kick", "master.size", with(Shaper::default(), |s| { s.attack = Span::Ms(0.0); s.release = Span::Ms(50.0); s.decay = Span::Ms(100.0); s.max = 0.5; }));
        let mut s = store(vec![pulse]);
        let mut f = AudioFeatures { kick: 3, kick_strength: 1.0, ..Default::default() };
        size_after(&mut s, &f);
        f.kick = 4;
        assert!(size_after(&mut s, &f) > 1.9, "a kick of the browser");
        // Native takes over with its own, much larger counter: no kick.
        f.kick = 500;
        let (mut settings, mut live) = (Settings::default(), LiveModifiers::default());
        s.apply(&f, FPS60, 0.5, &Guard { source_changed: true, ..Guard::LIVE }, &mut settings, &mut live);
        let mut sizes = vec![live.size];
        for _ in 0..20 {
            sizes.push(size_after(&mut s, &f));
        }
        assert!(sizes.windows(2).all(|w| w[1] <= w[0]), "only falls: {sizes:?}");
        assert_eq!(*sizes.last().unwrap(), 1.0);
        // The next native kick is one.
        f.kick = 501;
        assert!(size_after(&mut s, &f) > 1.5);
    }

    #[test]
    fn the_worst_case_keeps_the_fader_the_zones_and_never_arms() {
        let shared = crate::test_support::shared();
        // Every route pushing up as hard as it can, brightness included.
        let up = with(Shaper::default(), |s| { s.gate = 0.0; s.attack = Span::Ms(0.0); s.gain = 8.0; s.min = 1.0; s.max = 1.0; });
        let targets = ["master.brightness", "look.brightness", "master.size", "master.size_x", "master.size_y", "audio.flash", "master.pos_x", "master.perspective"];
        let routes: Vec<AudioRoute> = targets.iter().flat_map(|t| [route("bass", t, up.clone()), route("kick", t, up.clone())]).collect();
        let mut s = store(routes);
        let look = Settings { brightness: 0.7, ..Default::default() };
        let base = LiveModifiers { brightness: 0.6, ..Default::default() };
        // Blank zone over the right half of the output.
        let blank = crate::zones::Zone {
            kind: crate::zones::ZoneKind::Blank,
            points: vec![[0.0, -1.5], [1.5, -1.5], [1.5, 1.5], [0.0, 1.5]],
            ..Default::default()
        };
        let cfg = SafetySettings { zones: vec![blank], ..Default::default() };
        let mut pipe = Pipe::new();
        let mut f = AudioFeatures { level: 1.0, bass: 1.0, beat: 0, ..Default::default() };
        let mut lit_left = false;
        for i in 0..240 {
            f.kick += 1;
            f.beat += (i % 2) as u64;
            let (_, out, live) = pipe.frame(&mut s, &f, &Guard::LIVE, &look, &base, &cfg);
            assert!(live.brightness <= base.brightness, "the master fader is a ceiling");
            for p in &out {
                assert!((-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y), "calibration clamp: {p:?}");
                assert!(p.r.max(p.g).max(p.b) <= look.brightness * base.brightness + 1e-4, "brightness: {p:?}");
                if p.x > 0.02 {
                    assert_eq!((p.r, p.g, p.b), (0.0, 0.0, 0.0), "lit inside the blank zone: {p:?}");
                }
                lit_left |= p.x < -0.02 && p.g > 0.0;
            }
        }
        assert!(lit_left, "something still drawn outside the zone");
        assert!(!shared.gate.is_armed());
    }
}
