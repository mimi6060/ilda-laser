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
//! The legacy per-look `AudioReact` (`settings.audio`) is untouched and
//! still renders as before; routes are master-level and come on top.

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
}

/// One target and this frame's sum of its routes.
struct Group {
    target: Target,
    sum: f32,
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
}

impl RouteStore {
    /// A missing or unreadable file starts empty; routes that aren't valid
    /// any more (unknown source or target) are dropped.
    pub fn load_or_create(path: PathBuf, reg: &ControlRegistry) -> Self {
        let routing: AudioRouting = crate::load_json(&path);
        let mut store = Self { path, routing: AudioRouting::default(), bound: Vec::new(), groups: Vec::new() };
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
                    groups.push(Group { target, sum: 0.0 });
                    groups.len() - 1
                }
            };
            let same = old.get(i).filter(|b| b.source == source && old_groups[b.group].target.id() == r.target);
            let shaper = match same {
                Some(b) => {
                    let mut s = b.shaper.clone();
                    s.retune(&r.shape);
                    s
                }
                None => r.shape.clone(),
            };
            bound.push(Bound { source, group, shaper, enabled: r.enabled, input: 0.0 });
        }
        self.bound = bound;
        self.groups = groups;
        self.routing = routing;
    }

    /// One frame, on the engine's copies: every route reads `features`
    /// (`dt` = real frame time, `beat_len_s` = 60 / BPM of the one clock),
    /// then each target moves by the sum of its enabled routes × the audio
    /// share of the crossfader. Allocates nothing. Returns true when a
    /// colour target moved: call `lfo::recolor_live` once after.
    pub fn apply(&mut self, features: &AudioFeatures, dt: f32, beat_len_s: f32, settings: &mut Settings, live: &mut LiveModifiers) -> bool {
        for g in &mut self.groups {
            g.sum = 0.0;
        }
        for b in &mut self.bound {
            // Disabled routes keep running for their meter, so turning one
            // on doesn't fire on an old event.
            let v = b.shaper.feed(b.source, features, dt, beat_len_s);
            b.input = match b.source {
                Source::Value(id) => features.value(id).unwrap_or(0.0),
                Source::Event(_) => 0.0,
            };
            if b.enabled {
                self.groups[b.group].sum += v;
            }
        }
        let share = self.routing.audio_share();
        let mut recolor = false;
        for g in &self.groups {
            if g.sum != 0.0 && share > 0.0 {
                recolor |= g.target.apply(g.sum * share, settings, live);
            }
        }
        recolor
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
        store.apply(f, FPS60, 0.5, &mut settings, &mut live);
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
        s.apply(&noisy, FPS60, 0.5, &mut settings, &mut live);
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
        routes.apply(&f, FPS60, 0.5, &mut settings, &mut live);
        f.kick += 1;
        routes.apply(&f, FPS60, 0.5, &mut settings, &mut live);
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
                s.apply(&f, FPS60, 0.5, &mut settings, &mut live);
                let meters = s.meters().fold(0.0, |a, m| a + m.value);
                assert!(meters.is_finite());
            }
        });
        assert_eq!(n, 0);
    }
}
