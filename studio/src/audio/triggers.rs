//! Audio event triggers (T-240): rules *when this happens in the music,
//! do that*, to play alone or automate part of a show: a cue on the drop,
//! a flash on every kick, the calm look on a break.
//!
//! Events: `kick`, `snare`, `drop` (the analysis counters), `break` and
//! `silence` (the section starting), `buildup_above` x (the build-up
//! crossing x upwards), `every_bars` n (every n bars of the tempo clock
//! while music plays). Actions: launch a cue, the next cue of the page,
//! flash a cue for n beats, a momentary black for n beats. An action can
//! wait for the next beat or bar (`quantize`).
//!
//! **Quantisation and transitions (T-158 / T-159).** A cue action waits
//! here, once, for the coarser of the rule's `quantize` and the deck's
//! own (`CueDeck::quantize`); it then presses the cue on that boundary,
//! inside the deck's catch-up window, so the deck starts it at once
//! instead of waiting a second time. The cue then starts with the deck's
//! transition like any press. A black only follows the rule's own.
//!
//! **Not an external input (T-408).** The triggers are the studio's own
//! analysis, so they don't go through the external gate and its scopes
//! (that gate is for DMX, OSC and timecode coming from the network); they
//! get their own, stricter, rules instead:
//!
//! - **Never arms.** An action is a cue press (`controls::press_cue_from`,
//!   source `audio`) or a brightness gain on the engine's copy: it never
//!   calls `controls::apply`, so arming, the blackout, the e-stop, safety,
//!   calibration and every other control are out of reach. Disarmed, the
//!   cues change in the preview and nothing else happens.
//! - **Off by default.** The *Automatisme audio* switch
//!   (`audio.triggers.enabled`, MIDI-assignable) is memory only and off at
//!   every start; each rule has its own `enabled` on top.
//! - **E-stop.** An emergency stop switches the automation off (logged);
//!   resetting the stop doesn't switch it back on, and it can't be
//!   switched on while the stop is latched.
//! - **Stale audio stops it.** Without fresh audio (`Active::None`), on a
//!   change of source, or with the switch off, pending actions are
//!   dropped, a trigger's flash is released, its black ends, and the event
//!   counters are taken afresh when the audio returns: an old event never
//!   fires late.
//! - **No flood.** Each rule acts at most once per `min_interval_beats`
//!   (default 1 beat, never under `MIN_INTERVAL_S`), all rules together
//!   at most `MAX_ACTIONS_PER_S` a second, and a black may only start
//!   `1 / max_flash_hz` (T-245) after the previous one. Everything then
//!   goes through the output safety stage (strobe limiter, zones, caps).
//! - **Show mode.** Editing the rules (`POST /api/audio/triggers`) is an
//!   edit, refused in show mode (T-283); the switch is a live control
//!   that keeps working, like the external gate.
//! - **Logged.** Rule changes and the switch go to the safety log
//!   (`audio_triggers`, `audio_automation`); a cue started while armed is
//!   logged as any cue, with the source `audio`.
//!
//! The rules are saved to `audio_triggers.json` in the data directory.

use super::Active;
use crate::controls;
use crate::cues::ClickMode;
use crate::engine::{AudioFeatures, Section};
use crate::interlock::ArmSource;
use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Rules allowed at once.
pub const MAX_TRIGGERS: usize = 16;
/// `min_interval_beats` is kept within this range (default 1).
pub const MIN_INTERVAL_BEATS: f32 = 0.25;
pub const MAX_INTERVAL_BEATS: f32 = 64.0;
/// Whatever the tempo, one rule never acts twice within this.
pub const MIN_INTERVAL_S: f64 = 0.1;
/// Detection jitter allowed on the interval, so kicks exactly one beat
/// apart with a one-beat interval are all taken (two frames).
const INTERVAL_SLACK_S: f64 = 0.035;
/// All rules together: at most this many actions a second...
pub const MAX_ACTIONS_PER_S: f64 = 4.0;
/// ...with bursts of at most this many (simultaneous rules).
const ACTION_BURST: f64 = 4.0;
/// A build-up rule fires again only once the build-up fell this far under
/// its threshold.
const BUILDUP_HYSTERESIS: f32 = 0.1;
/// A quantised action waits for its beat at most this long past it (the
/// tempo clock may be moved meanwhile).
const QUANTIZE_SLACK_S: f64 = 0.5;

/// What a rule listens for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioEvent {
    Kick,
    Snare,
    Drop,
    /// A break starts.
    Break,
    /// The build-up crosses this value (0..1) upwards.
    BuildupAbove(f32),
    /// The music stops (a silence of the analysis starts).
    Silence,
    /// Every n bars of the tempo clock (on the bar's first beat), while
    /// the music plays.
    EveryBars(u32),
}

impl AudioEvent {
    pub fn label_fr(&self) -> String {
        match self {
            AudioEvent::Kick => "Kick".into(),
            AudioEvent::Snare => "Caisse claire".into(),
            AudioEvent::Drop => "Drop".into(),
            AudioEvent::Break => "Break".into(),
            AudioEvent::BuildupAbove(x) => format!("Montée ≥ {} %", (x * 100.0).round()),
            AudioEvent::Silence => "Silence".into(),
            AudioEvent::EveryBars(n) => format!("Toutes les {n} mesures"),
        }
    }
}

/// What a rule does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerAction {
    /// Starts this cue (nothing if it already plays).
    LaunchCue(String),
    /// The next cue of the page shown, after the one playing.
    NextCue,
    /// This cue as a flash for `beats` beats.
    FlashCue { id: String, beats: f32 },
    /// The output dark for `beats` beats (not the transport blackout:
    /// nothing to release, arming untouched).
    BlackHold { beats: f32 },
}

impl TriggerAction {
    /// The cue it names, if any.
    pub fn cue(&self) -> Option<&str> {
        match self {
            TriggerAction::LaunchCue(id) | TriggerAction::FlashCue { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// When an action happens once its event is seen: the deck's own
/// quantisation (T-159) type, `off`, `beat` or `bar`.
pub use crate::cues::Quantize;

/// The coarser of two quantisations (Off < Beat < Bar).
fn coarser(a: Quantize, b: Quantize) -> Quantize {
    let rank = |q: Quantize| match q {
        Quantize::Off => 0,
        Quantize::Beat => 1,
        Quantize::Bar => 2,
    };
    if rank(a) >= rank(b) { a } else { b }
}

/// One rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioTrigger {
    pub event: AudioEvent,
    pub action: TriggerAction,
    /// At most one action per this many beats (`MIN_INTERVAL_BEATS`..).
    pub min_interval_beats: f32,
    pub quantize: Quantize,
    pub enabled: bool,
}

impl Default for AudioTrigger {
    fn default() -> Self {
        Self { event: AudioEvent::Drop, action: TriggerAction::NextCue, min_interval_beats: 1.0, quantize: Quantize::Off, enabled: true }
    }
}

/// Saved in `audio_triggers.json`. The switch is not: it is off at start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerConfig {
    pub triggers: Vec<AudioTrigger>,
}

fn clamp_or(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { fallback }
}

impl TriggerConfig {
    /// Numbers in range (NaN → default), at most `MAX_TRIGGERS`.
    pub fn sanitized(mut self) -> Self {
        self.triggers.truncate(MAX_TRIGGERS);
        for t in &mut self.triggers {
            t.min_interval_beats = clamp_or(t.min_interval_beats, MIN_INTERVAL_BEATS, MAX_INTERVAL_BEATS, 1.0);
            match &mut t.event {
                AudioEvent::BuildupAbove(x) => *x = clamp_or(*x, 0.05, 1.0, 0.5),
                AudioEvent::EveryBars(n) => *n = (*n).clamp(1, 64),
                _ => {}
            }
            match &mut t.action {
                TriggerAction::LaunchCue(id) => *id = id.trim().to_string(),
                TriggerAction::FlashCue { id, beats } => {
                    *id = id.trim().to_string();
                    *beats = clamp_or(*beats, 0.25, 16.0, 1.0);
                }
                TriggerAction::BlackHold { beats } => *beats = clamp_or(*beats, 0.25, 4.0, 0.5),
                TriggerAction::NextCue => {}
            }
        }
        self
    }

    /// At most `MAX_TRIGGERS`, each cue known (`cue_exists`). French, for
    /// the UI.
    pub fn validate(&self, cue_exists: impl Fn(&str) -> bool) -> Result<()> {
        if self.triggers.len() > MAX_TRIGGERS {
            bail!("{MAX_TRIGGERS} déclencheurs audio au maximum");
        }
        for t in &self.triggers {
            if let Some(id) = t.action.cue() {
                if id.trim().is_empty() {
                    bail!("choisissez un cue pour « {} »", t.event.label_fr());
                }
                if !cue_exists(id.trim()) {
                    bail!("cue inconnu : « {id} »");
                }
            }
        }
        Ok(())
    }
}

/// What the engine must do on the shared state after a step.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Launch(String),
    Next,
    FlashOn(String),
    FlashOff(String),
}

/// One frame of input to the rules.
#[derive(Clone, Copy, Debug)]
pub struct TriggerInput<'a> {
    pub features: &'a AudioFeatures,
    /// Where the features came from (`Active::None`: stale or no audio).
    pub active: Active,
    /// The source just changed (T-245).
    pub source_changed: bool,
    /// The switch is on and the e-stop is not latched.
    pub on: bool,
    /// Seconds on the studio clock, and the tempo clock at that time.
    pub t: f64,
    pub beat: f64,
    pub bpm: f64,
    pub beats_per_bar: u8,
    /// The T-245 flash cap, for the blacks.
    pub max_flash_hz: f32,
    /// The deck's quantised launch (T-159): cue actions wait for the
    /// coarser of it and the rule's own, here, so the deck doesn't wait
    /// again.
    pub deck_quantize: Quantize,
}

/// The counters and section seen on the last frame, to see events.
#[derive(Clone, Copy, Debug)]
struct Seen {
    kick: u64,
    snare: u64,
    drop: u64,
    section: Section,
    bar: i64,
}

#[derive(Clone, Debug, Default)]
struct RuleState {
    last_fire_t: Option<f64>,
    /// A quantised action: the beat it waits for, and the time it gives up.
    pending: Option<(f64, f64)>,
    /// Build-up rules: ready to fire (the build-up was under the
    /// threshold since the last time).
    ready: bool,
    fired: u64,
    limited: u64,
}

/// Per-rule figures for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RuleStatus {
    pub fired: u64,
    pub limited: u64,
    /// Seconds since this rule last acted (studio clock), if it did.
    pub ago_s: Option<f64>,
    pub pending: bool,
}

/// The running part: what was seen, each rule's limiter, the trigger's
/// own flashes and black. Engine state, never saved.
#[derive(Debug)]
pub struct TriggerEngine {
    seen: Option<Seen>,
    rules: Vec<RuleState>,
    /// Flashes started by a rule: cue and the time it ends.
    flashes: Vec<(String, f64)>,
    black_until: Option<f64>,
    last_black_start: Option<f64>,
    tokens: f64,
    last_t: Option<f64>,
    /// The rules ran on the last frame (switch on, fresh audio).
    live: bool,
    last_frame_t: f64,
}

impl Default for TriggerEngine {
    fn default() -> Self {
        Self { seen: None, rules: Vec::new(), flashes: Vec::new(), black_until: None, last_black_start: None, tokens: ACTION_BURST, last_t: None, live: false, last_frame_t: 0.0 }
    }
}

fn bar_of(beat: f64, beats_per_bar: u8) -> i64 {
    (beat / beats_per_bar.max(1) as f64).floor() as i64
}

impl TriggerEngine {
    /// The output gain of a black (0 while one holds, else 1).
    pub fn gain(&self) -> f32 {
        if self.black_until.is_some() { 0.0 } else { 1.0 }
    }

    /// Everything a stop ends: pending actions dropped, flashes released,
    /// the black ended, the counters forgotten.
    fn stand_down(&mut self, out: &mut Vec<Command>) {
        self.seen = None;
        for r in &mut self.rules {
            r.pending = None;
            r.ready = false;
        }
        out.extend(self.flashes.drain(..).map(|(id, _)| Command::FlashOff(id)));
        self.black_until = None;
    }

    /// One frame. Returns the cue commands for the shared state (empty, and
    /// allocation free, on most frames).
    pub fn step(&mut self, rules: &[AudioTrigger], input: &TriggerInput) -> Vec<Command> {
        let mut out = Vec::new();
        let t = input.t;
        self.rules.resize_with(rules.len(), RuleState::default);
        let dt = self.last_t.map_or(0.0, |l| (t - l).max(0.0));
        self.last_t = Some(t);
        self.last_frame_t = t;
        self.tokens = (self.tokens + dt * MAX_ACTIONS_PER_S).min(ACTION_BURST);
        let beat_s = 60.0 / input.bpm.max(1.0);

        // Flashes and the black end on their own time.
        let mut i = 0;
        while i < self.flashes.len() {
            if t >= self.flashes[i].1 {
                out.push(Command::FlashOff(self.flashes.swap_remove(i).0));
            } else {
                i += 1;
            }
        }
        if self.black_until.is_some_and(|until| t >= until) {
            self.black_until = None;
        }

        self.live = input.on && input.active != Active::None && !input.source_changed;
        if !self.live {
            self.stand_down(&mut out);
            return out;
        }
        let f = input.features;
        let now = Seen { kick: f.kick, snare: f.snare, drop: f.drop, section: f.section, bar: bar_of(input.beat, input.beats_per_bar) };
        let Some(before) = self.seen.replace(now) else {
            // Taking note: nothing seen before this frame is an event.
            for (r, rule) in self.rules.iter_mut().zip(rules) {
                r.ready = matches!(rule.event, AudioEvent::BuildupAbove(x) if f.buildup < x);
            }
            return out;
        };
        let grew = |a: u64, b: u64| a > b;
        let new_bar = now.bar != before.bar;
        for (k, rule) in rules.iter().enumerate() {
            let happened = match rule.event {
                AudioEvent::Kick => grew(now.kick, before.kick),
                AudioEvent::Snare => grew(now.snare, before.snare),
                AudioEvent::Drop => grew(now.drop, before.drop),
                AudioEvent::Break => now.section == Section::Break && before.section != Section::Break,
                AudioEvent::Silence => now.section == Section::Silence && before.section != Section::Silence,
                AudioEvent::BuildupAbove(x) => {
                    let r = &mut self.rules[k];
                    if f.buildup < x - BUILDUP_HYSTERESIS || (f.buildup < x && f.section != Section::Buildup) {
                        r.ready = true;
                    }
                    let fire = r.ready && f.buildup >= x;
                    if fire {
                        r.ready = false;
                    }
                    fire
                }
                AudioEvent::EveryBars(n) => new_bar && !f.silent && now.bar.rem_euclid(n.max(1) as i64) == 0,
            };
            if !happened || !rule.enabled {
                continue;
            }
            // The limiter: this rule's interval, then all rules together.
            let r = &mut self.rules[k];
            let interval = (rule.min_interval_beats as f64 * beat_s).max(MIN_INTERVAL_S);
            if r.last_fire_t.is_some_and(|last| t - last < interval - INTERVAL_SLACK_S) || self.tokens < 1.0 {
                r.limited += 1;
                continue;
            }
            self.tokens -= 1.0;
            r.last_fire_t = Some(t);
            r.fired += 1;
            let quantize = match rule.action {
                TriggerAction::BlackHold { .. } => rule.quantize,
                _ => coarser(rule.quantize, input.deck_quantize),
            };
            let target = match quantize {
                Quantize::Off => None,
                Quantize::Beat => Some((input.beat + 1e-6).ceil()),
                Quantize::Bar => {
                    let bpb = input.beats_per_bar.max(1) as f64;
                    Some(((input.beat + 1e-6) / bpb).ceil() * bpb)
                }
            };
            match target {
                Some(beat) => r.pending = Some((beat, t + (beat - input.beat).max(0.0) * beat_s + QUANTIZE_SLACK_S)),
                None => self.act(&rule.action, input, beat_s, &mut out),
            }
        }
        // Quantised actions whose beat has come.
        for (k, rule) in rules.iter().enumerate() {
            let due = self.rules[k].pending.is_some_and(|(beat, give_up)| input.beat >= beat - 1e-9 || t >= give_up);
            if due {
                self.rules[k].pending = None;
                self.act(&rule.action, input, beat_s, &mut out);
            }
        }
        out
    }

    fn act(&mut self, action: &TriggerAction, input: &TriggerInput, beat_s: f64, out: &mut Vec<Command>) {
        let t = input.t;
        match action {
            TriggerAction::LaunchCue(id) => out.push(Command::Launch(id.clone())),
            TriggerAction::NextCue => out.push(Command::Next),
            TriggerAction::FlashCue { id, beats } => {
                let until = t + *beats as f64 * beat_s;
                match self.flashes.iter_mut().find(|(c, _)| c == id) {
                    Some(f) => f.1 = f.1.max(until),
                    None => {
                        self.flashes.push((id.clone(), until));
                        out.push(Command::FlashOn(id.clone()));
                    }
                }
            }
            TriggerAction::BlackHold { beats } => {
                // A black is a dip of the light: the T-245 cap applies.
                let period = 1.0 / super::safety::MAX_FLASH_HZ.min(input.max_flash_hz.max(super::safety::MIN_FLASH_HZ)) as f64;
                let held = self.black_until.is_some();
                if held || self.last_black_start.is_none_or(|s| t - s >= period - 1e-6) {
                    if !held {
                        self.last_black_start = Some(t);
                    }
                    let until = t + *beats as f64 * beat_s;
                    self.black_until = Some(self.black_until.map_or(until, |u| u.max(until)));
                }
            }
        }
    }

    /// Drops every quantised action waiting and ends the black (e-stop).
    fn drop_pending(&mut self) {
        for r in &mut self.rules {
            r.pending = None;
        }
        self.black_until = None;
    }

    pub fn status(&self) -> Vec<RuleStatus> {
        self.rules
            .iter()
            .map(|r| RuleStatus { fired: r.fired, limited: r.limited, ago_s: r.last_fire_t.map(|l| (self.last_frame_t - l).max(0.0)), pending: r.pending.is_some() })
            .collect()
    }

    /// Actions taken since start, all rules.
    pub fn fired_total(&self) -> u64 {
        self.rules.iter().map(|r| r.fired).sum()
    }

    /// A rule list changed: the rules' states start afresh (the limiter
    /// times kept, so editing can't fire twice in a row).
    fn reset_rules(&mut self) {
        for r in &mut self.rules {
            r.pending = None;
            r.ready = false;
        }
        self.seen = None;
    }
}

/// `Shared.triggers`: the rules, the switch, the running engine.
#[derive(Debug)]
pub struct TriggerStore {
    path: Option<PathBuf>,
    config: TriggerConfig,
    /// *Automatisme audio*: memory only, off at start.
    enabled: bool,
    /// `EStop::trips` when last looked at.
    estop_trips: u64,
    pub engine: TriggerEngine,
}

impl TriggerStore {
    /// From `audio_triggers.json` (missing or damaged: no rule). The
    /// switch starts off.
    pub fn load(path: PathBuf) -> Self {
        let config: TriggerConfig = crate::load_json(&path);
        Self { path: Some(path), config: config.sanitized(), enabled: false, estop_trips: 0, engine: TriggerEngine::default() }
    }

    pub fn in_memory() -> Self {
        Self { path: None, config: TriggerConfig::default(), enabled: false, estop_trips: 0, engine: TriggerEngine::default() }
    }

    pub fn config(&self) -> &TriggerConfig {
        &self.config
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Replaces the rules in memory (checked by the caller).
    fn replace(&mut self, config: TriggerConfig) {
        self.config = config;
        self.engine.reset_rules();
    }
}

/// Switches the automation off after an emergency stop, however it was
/// tripped (the HTTP fast path and the listeners trip the latch without
/// the lock). Called by `Shared::emergency_stop` and every engine frame.
pub fn sync_estop(s: &mut Shared) {
    let trips = s.estop.trips();
    if trips != s.triggers.estop_trips || s.estop.is_latched() {
        s.triggers.estop_trips = trips;
        // Nothing waits any more, like the deck's quantised launches
        // (T-159); the trigger's flashes end on the next frame.
        s.triggers.engine.drop_pending();
        if s.triggers.enabled {
            s.triggers.enabled = false;
            log::info!("Automatisme audio coupé (arrêt d'urgence)");
            s.gate.log().record("audio_automation", Some(ArmSource::System.id()), json!({ "on": false, "reason": "estop" }));
        }
    }
}

/// The *Automatisme audio* switch (`audio.triggers.enabled`). Switching on
/// is refused while the emergency stop is latched. Logged; never arms.
pub fn set_enabled(s: &mut Shared, on: bool, src: ArmSource) -> Result<(), &'static str> {
    sync_estop(s);
    if on == s.triggers.enabled {
        return Ok(());
    }
    if on && s.estop.is_latched() {
        return Err("arrêt d'urgence enclenché : réinitialisez-le avant d'activer l'automatisme audio");
    }
    s.triggers.enabled = on;
    log::info!("Automatisme audio {}", if on { "activé" } else { "coupé" });
    s.gate.log().record("audio_automation", Some(src.id()), json!({ "on": on, "reason": "operator" }));
    Ok(())
}

/// `POST /api/audio/triggers` `{ "triggers": [...] }`: checked (cues must
/// exist), numbers clamped, saved, logged. Returns the saved rules.
pub fn set_config(s: &mut Shared, config: TriggerConfig) -> Result<TriggerConfig> {
    let config = config.sanitized();
    config.validate(|id| s.presets.iter().any(|p| p.id == id))?;
    if config == s.triggers.config {
        return Ok(config);
    }
    if let Some(path) = &s.triggers.path {
        let text = serde_json::to_string_pretty(&config).context("failed to serialize audio triggers")?;
        crate::datafile::replace(path, text.as_bytes()).with_context(|| format!("failed to write {}", path.display()))?;
    }
    let before = s.triggers.config.clone();
    s.gate.log().record("audio_triggers", Some(ArmSource::Ui.id()), json!({ "count": config.triggers.len(), "before": before, "after": config }));
    s.triggers.replace(config.clone());
    Ok(config)
}

/// Starts `id` unless it already plays or waits (a latched instance, a
/// quantised launch, or its show).
fn launch(s: &mut Shared, id: &str, t: f64) {
    if s.deck.active.iter().any(|a| a.cue == id && !a.held) || s.deck.pending.iter().any(|p| p.cue == id) || controls::show_cue_playing(s, id) {
        return;
    }
    controls::press_cue_at(s, id, Some(ClickMode::Restart), true, Some("audio"), t);
}

/// The cue after the one playing on the page shown (the first when none
/// of the page plays).
fn next_on_page(s: &Shared) -> Option<String> {
    let cells: Vec<&str> = s.grid.view().get(s.cue_page)?.cells.iter().flatten().map(String::as_str).collect();
    if cells.is_empty() {
        return None;
    }
    let i = s.active_cue.as_deref().and_then(|a| cells.iter().position(|c| *c == a));
    Some(cells[i.map_or(0, |i| (i + 1) % cells.len())].to_string())
}

/// One engine frame, under the lock, before the looks are read: the
/// rules see this frame's features and press cues. `active`: where the
/// features came from; `t` and `clock`: the frame's time.
pub fn frame(s: &mut Shared, features: &AudioFeatures, active: Active, source_changed: bool, t: f64, clock: &crate::engine::BeatClock, max_flash_hz: f32) {
    sync_estop(s);
    if s.triggers.config.triggers.is_empty() && s.triggers.engine.flashes.is_empty() && s.triggers.engine.black_until.is_none() {
        s.triggers.engine.live = false;
        s.triggers.engine.seen = None;
        return;
    }
    let input = TriggerInput {
        features,
        active,
        source_changed,
        on: s.triggers.enabled && !s.estop.is_latched(),
        t,
        beat: clock.beat,
        bpm: clock.bpm,
        beats_per_bar: clock.beats_per_bar,
        max_flash_hz,
        deck_quantize: s.deck.quantize,
    };
    let commands = {
        let store = &mut s.triggers;
        store.engine.step(&store.config.triggers, &input)
    };
    for c in commands {
        match c {
            Command::Launch(id) => launch(s, &id, t),
            Command::Next => {
                if let Some(id) = next_on_page(s) {
                    launch(s, &id, t);
                }
            }
            Command::FlashOn(id) => {
                if s.deck.slot(&id).show.is_none() {
                    controls::press_cue_at(s, &id, Some(ClickMode::Flash), true, Some("audio"), t);
                }
            }
            Command::FlashOff(id) => {
                controls::press_cue_at(s, &id, Some(ClickMode::Flash), false, Some("audio"), t);
            }
        }
    }
    // A flash released by the stop (the switch goes off with it) must not
    // leave a fade-out behind: the e-stop cuts every transition.
    if s.estop.is_latched() {
        s.deck.cut_transitions();
    }
}

/// `GET /api/audio/triggers` and the panel's status.
pub fn view(s: &Shared) -> Value {
    json!({
        "enabled": s.triggers.enabled,
        "triggers": s.triggers.config.triggers,
        "max": MAX_TRIGGERS,
        "status": status(s),
    })
}

/// What `/api/frame` shows: the switch, whether the rules run, the black,
/// each rule's counts.
pub fn status(s: &Shared) -> Value {
    let e = &s.triggers.engine;
    json!({
        "enabled": s.triggers.enabled,
        "live": e.live,
        "black": e.black_until.is_some(),
        "rules": e.status(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::BeatClock;
    use crate::test_support;

    const FPS60: f64 = 1.0 / 60.0;

    fn rule(event: AudioEvent, action: TriggerAction) -> AudioTrigger {
        AudioTrigger { event, action, ..Default::default() }
    }

    fn input(f: &AudioFeatures, t: f64, bpm: f64) -> TriggerInput<'_> {
        TriggerInput { features: f, active: Active::Native, source_changed: false, on: true, t, beat: t * bpm / 60.0, bpm, beats_per_bar: 4, max_flash_hz: 10.0, deck_quantize: Quantize::Off }
    }

    #[test]
    fn the_rules_load_with_defaults_and_serialize_readably() {
        let c: TriggerConfig = serde_json::from_str(r#"{"triggers":[{"event":"drop","action":{"launch_cue":"x"}},{"event":{"buildup_above":0.7},"action":{"flash_cue":{"id":"y","beats":2}},"quantize":"bar"},{"event":{"every_bars":4},"action":"next_cue"},{"event":"kick","action":{"black_hold":{"beats":0.5}},"enabled":false}]}"#).unwrap();
        assert_eq!(c.triggers.len(), 4);
        assert_eq!((c.triggers[0].min_interval_beats, c.triggers[0].quantize, c.triggers[0].enabled), (1.0, Quantize::Off, true));
        assert_eq!(c.triggers[1].quantize, Quantize::Bar);
        assert!(!c.triggers[3].enabled);
        let back: TriggerConfig = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let wild = TriggerConfig {
            triggers: vec![AudioTrigger { event: AudioEvent::BuildupAbove(f32::NAN), action: TriggerAction::BlackHold { beats: 100.0 }, min_interval_beats: 0.0, ..Default::default() }; 20],
        }
        .sanitized();
        assert_eq!(wild.triggers.len(), MAX_TRIGGERS);
        assert_eq!(wild.triggers[0], AudioTrigger { event: AudioEvent::BuildupAbove(0.5), action: TriggerAction::BlackHold { beats: 4.0 }, min_interval_beats: MIN_INTERVAL_BEATS, ..Default::default() });
        let unknown = TriggerConfig { triggers: vec![rule(AudioEvent::Kick, TriggerAction::LaunchCue("nope".into()))] };
        assert!(unknown.validate(|id| id == "x").unwrap_err().to_string().contains("nope"));
        assert!(TriggerConfig { triggers: vec![rule(AudioEvent::Kick, TriggerAction::LaunchCue(" ".into()))] }.validate(|_| true).is_err());
        assert!(TriggerConfig { triggers: vec![rule(AudioEvent::Kick, TriggerAction::NextCue)] }.validate(|_| false).is_ok());
    }

    #[test]
    fn the_first_frame_only_takes_note_and_a_new_kick_fires() {
        let mut e = TriggerEngine::default();
        let rules = [rule(AudioEvent::Kick, TriggerAction::LaunchCue("x".into()))];
        let f = AudioFeatures { kick: 9, ..Default::default() };
        assert!(e.step(&rules, &input(&f, 1.0, 120.0)).is_empty(), "9 old kicks are not events");
        assert!(e.step(&rules, &input(&f, 1.0 + FPS60, 120.0)).is_empty());
        let f = AudioFeatures { kick: 10, ..Default::default() };
        assert_eq!(e.step(&rules, &input(&f, 1.1, 120.0)), [Command::Launch("x".into())]);
    }

    #[test]
    fn kicks_at_128_bpm_with_a_one_beat_interval_act_at_most_once_a_beat() {
        // Kicks on every eighth note (twice a beat) for 8 s.
        let mut e = TriggerEngine::default();
        let rules = [rule(AudioEvent::Kick, TriggerAction::NextCue)];
        let (bpm, beat_s) = (128.0, 60.0 / 128.0);
        let mut times = Vec::new();
        for i in 0..(8.0 / FPS60) as usize {
            let t = i as f64 * FPS60;
            let f = AudioFeatures { kick: (t / (beat_s / 2.0)) as u64, ..Default::default() };
            if !e.step(&rules, &input(&f, t, bpm)).is_empty() {
                times.push(t);
            }
        }
        assert!(times.len() >= 15, "{} actions in 17 beats", times.len());
        for w in times.windows(2) {
            assert!(w[1] - w[0] >= beat_s - INTERVAL_SLACK_S - 1e-9, "two actions {} s apart", w[1] - w[0]);
        }
        assert!(e.status()[0].limited > 0, "the extra kicks were limited");
    }

    #[test]
    fn many_rules_together_never_flood() {
        // Sixteen kick rules, a kick every frame: the global cap holds.
        let mut e = TriggerEngine::default();
        let rules: Vec<_> = (0..MAX_TRIGGERS).map(|_| AudioTrigger { min_interval_beats: MIN_INTERVAL_BEATS, ..rule(AudioEvent::Kick, TriggerAction::NextCue) }).collect();
        let mut n = 0;
        for i in 0..600 {
            let f = AudioFeatures { kick: i, ..Default::default() };
            n += e.step(&rules, &input(&f, i as f64 * FPS60, 200.0)).len();
        }
        assert!(n as f64 <= 10.0 * MAX_ACTIONS_PER_S + ACTION_BURST, "{n} actions in 10 s");
    }

    #[test]
    fn switched_off_stale_or_switching_source_nothing_fires_and_flashes_end() {
        let rules = [rule(AudioEvent::Kick, TriggerAction::FlashCue { id: "x".into(), beats: 8.0 }), rule(AudioEvent::Kick, TriggerAction::BlackHold { beats: 4.0 })];
        for stop in 0..3 {
            let mut e = TriggerEngine::default();
            let f0 = AudioFeatures::default();
            e.step(&rules, &input(&f0, 0.0, 120.0));
            let f1 = AudioFeatures { kick: 1, ..Default::default() };
            assert_eq!(e.step(&rules, &input(&f1, 0.5, 120.0)), [Command::FlashOn("x".into())]);
            assert_eq!(e.gain(), 0.0, "black");
            // Then a kick arrives as the audio goes away.
            let f2 = AudioFeatures { kick: 2, ..Default::default() };
            let mut inp = input(&f2, 1.0, 120.0);
            match stop {
                0 => inp.on = false,
                1 => inp.active = Active::None,
                _ => inp.source_changed = true,
            }
            assert_eq!(e.step(&rules, &inp), [Command::FlashOff("x".into())], "case {stop}");
            assert_eq!(e.gain(), 1.0, "the black ends");
            // Back: the kicks seen meanwhile are no events.
            let f3 = AudioFeatures { kick: 5, ..Default::default() };
            assert!(e.step(&rules, &input(&f3, 3.0, 120.0)).is_empty(), "case {stop}");
        }
    }

    #[test]
    fn sections_buildup_and_bars_are_edges() {
        let rules = [
            rule(AudioEvent::Break, TriggerAction::LaunchCue("b".into())),
            rule(AudioEvent::BuildupAbove(0.6), TriggerAction::LaunchCue("u".into())),
            rule(AudioEvent::Silence, TriggerAction::LaunchCue("s".into())),
            AudioTrigger { min_interval_beats: 0.25, ..rule(AudioEvent::EveryBars(2), TriggerAction::LaunchCue("n".into())) },
        ];
        let mut e = TriggerEngine::default();
        let mut log = Vec::new();
        let mut run = |section: Section, buildup: f32, from: f64, to: f64, e: &mut TriggerEngine| {
            let mut t = from;
            while t < to {
                let f = AudioFeatures { section, buildup, silent: section == Section::Silence, ..Default::default() };
                for c in e.step(&rules, &input(&f, t, 120.0)) {
                    log.push((t, c));
                }
                t += FPS60;
            }
        };
        // 120 BPM: a bar every 2 s, every 2 bars every 4 s.
        run(Section::Normal, 0.0, 0.1, 9.0, &mut e);
        run(Section::Break, 0.0, 9.0, 10.0, &mut e);
        run(Section::Buildup, 0.3, 10.0, 11.0, &mut e);
        run(Section::Buildup, 0.7, 11.0, 11.5, &mut e);
        run(Section::Buildup, 0.65, 11.5, 12.0, &mut e); // within the hysteresis: no second one
        run(Section::Silence, 0.0, 12.0, 14.5, &mut e);
        let names: Vec<(i64, &str)> = log
            .iter()
            .map(|(t, c)| match c {
                Command::Launch(id) => ((t * 10.0).round() as i64, id.as_str()),
                _ => panic!("{c:?}"),
            })
            .collect();
        assert_eq!(names, [(40, "n"), (80, "n"), (90, "b"), (110, "u"), (120, "s")], "every 2 bars at 4 s and 8 s, not in the silence at 12 s");
    }

    #[test]
    fn a_quantised_action_waits_for_the_next_beat_or_bar() {
        for (q, want) in [(Quantize::Beat, 2.0), (Quantize::Bar, 4.0)] {
            let rules = [AudioTrigger { quantize: q, ..rule(AudioEvent::Drop, TriggerAction::NextCue) }];
            let mut e = TriggerEngine::default();
            let mut at = None;
            for i in 0..300 {
                let t = i as f64 * FPS60;
                let f = AudioFeatures { drop: (t > 0.6) as u64, ..Default::default() };
                // 60 BPM: beat = t.
                let mut inp = input(&f, t, 60.0);
                inp.beat = t + 1.0;
                if !e.step(&rules, &inp).is_empty() {
                    at.get_or_insert(inp.beat);
                }
            }
            let at = at.expect("acted");
            assert!((at - want).abs() < 0.02, "{q:?}: {at}");
        }
    }

    #[test]
    fn blacks_and_flashes_hold_for_their_beats_and_blacks_respect_the_flash_cap() {
        let rules = [AudioTrigger { min_interval_beats: MIN_INTERVAL_BEATS, ..rule(AudioEvent::Kick, TriggerAction::BlackHold { beats: 0.25 }) }];
        let mut e = TriggerEngine::default();
        let mut starts = 0;
        let mut was_black = false;
        // A kick every frame for 4 s at 240 BPM, a cap of 2 blacks a second.
        for i in 0..240 {
            let f = AudioFeatures { kick: i, ..Default::default() };
            let mut inp = input(&f, i as f64 * FPS60, 240.0);
            inp.max_flash_hz = 2.0;
            e.step(&rules, &inp);
            let black = e.gain() == 0.0;
            starts += (black && !was_black) as usize;
            was_black = black;
        }
        assert!((7..=9).contains(&starts), "{starts} blacks in 4 s at 2 / s");

        let rules = [rule(AudioEvent::Kick, TriggerAction::FlashCue { id: "x".into(), beats: 2.0 })];
        let mut e = TriggerEngine::default();
        let mut log = Vec::new();
        for i in 0..120 {
            let t = i as f64 * FPS60;
            let f = AudioFeatures { kick: (i >= 6) as u64, ..Default::default() };
            for c in e.step(&rules, &input(&f, t, 120.0)) {
                log.push((t, c));
            }
        }
        // On at 0.1 s, off two beats (1 s) later, within a frame.
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!(log[0].1, Command::FlashOn("x".into()));
        assert_eq!(log[1].1, Command::FlashOff("x".into()));
        assert!((log[0].0 - 0.1).abs() < 1e-9 && log[1].0 - log[0].0 >= 1.0 - 1e-9 && log[1].0 - log[0].0 < 1.0 + FPS60 + 1e-9, "{log:?}");
    }

    /// T-240's acceptance: on the T-236 synthetic arrangement, a drop rule
    /// launches its cue once, within a beat of the drop.
    #[test]
    fn the_drop_of_the_synthetic_arrangement_launches_the_cue_once_on_time() {
        let case = crate::audio::bench::arrangement_case();
        let bpm = 128.0;
        let beat_s = 60.0 / bpm;
        let rules = [rule(AudioEvent::Drop, TriggerAction::LaunchCue("drop-cue".into()))];
        let mut e = TriggerEngine::default();
        let mut launched = Vec::new();
        for (t, f) in crate::audio::bench::feature_hops(&case.signal, case.rate) {
            for c in e.step(&rules, &input(&f, t, bpm)) {
                assert_eq!(c, Command::Launch("drop-cue".into()));
                launched.push(t);
            }
        }
        assert_eq!(case.truth.drops.len(), 1);
        assert_eq!(launched.len(), 1, "launched at {launched:?}");
        let drop = case.truth.drops[0];
        assert!((launched[0] - drop).abs() <= beat_s, "launched at {} s, drop at {drop} s", launched[0]);
    }

    fn rig() -> Shared {
        let mut s = test_support::shared();
        s.look_on = true;
        s
    }

    fn clock(t: f64) -> BeatClock {
        BeatClock { beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 }
    }

    fn some_cue(s: &Shared, n: usize) -> String {
        s.presets[n].id.clone()
    }

    #[test]
    fn disarmed_a_trigger_changes_the_cue_in_the_preview_and_never_arms() {
        let mut s = rig();
        let cue = some_cue(&s, 3);
        set_config(&mut s, TriggerConfig { triggers: vec![rule(AudioEvent::Drop, TriggerAction::LaunchCue(cue.clone()))] }).unwrap();
        let f0 = AudioFeatures::default();
        let f1 = AudioFeatures { drop: 1, ..Default::default() };
        // Off by default: nothing.
        frame(&mut s, &f0, Active::Browser, false, 1.0, &clock(1.0), 10.0);
        frame(&mut s, &f1, Active::Browser, false, 1.1, &clock(1.1), 10.0);
        assert!(s.deck.active.is_empty(), "the switch starts off");
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let f2 = AudioFeatures { drop: 2, ..Default::default() };
        frame(&mut s, &f1, Active::Browser, false, 2.0, &clock(2.0), 10.0);
        frame(&mut s, &f2, Active::Browser, false, 2.1, &clock(2.1), 10.0);
        assert_eq!(s.active_cue.as_deref(), Some(cue.as_str()));
        assert!(!s.gate.is_armed(), "never arms");
        // A second drop while it plays: it keeps playing (no toggle off).
        let f3 = AudioFeatures { drop: 3, ..Default::default() };
        frame(&mut s, &f3, Active::Browser, false, 5.0, &clock(5.0), 10.0);
        assert_eq!(s.deck.active.iter().filter(|a| a.cue == cue).count(), 1);
    }

    #[test]
    fn an_estop_switches_the_automation_off_and_a_reset_does_not_switch_it_on() {
        let (log, sink) = crate::safety_log::testing::memory_log();
        let mut s = rig();
        s.gate.set_log(log);
        set_config(&mut s, TriggerConfig { triggers: vec![rule(AudioEvent::Kick, TriggerAction::NextCue)] }).unwrap();
        set_enabled(&mut s, true, ArmSource::Midi).unwrap();
        s.emergency_stop(ArmSource::Keyboard);
        assert!(!s.triggers.enabled());
        assert!(set_enabled(&mut s, true, ArmSource::Ui).is_err(), "refused while latched");
        let f0 = AudioFeatures::default();
        let f1 = AudioFeatures { kick: 1, ..Default::default() };
        frame(&mut s, &f0, Active::Browser, false, 1.0, &clock(1.0), 10.0);
        frame(&mut s, &f1, Active::Browser, false, 1.6, &clock(1.6), 10.0);
        assert!(s.deck.active.is_empty());
        let estop = std::sync::Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        frame(&mut s, &f1, Active::Browser, false, 2.0, &clock(2.0), 10.0);
        assert!(!s.triggers.enabled(), "a reset doesn't switch it back on");
        // A latch tripped without the lock (HTTP fast path) is seen by the frame.
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        s.estop.trip(ArmSource::Api);
        frame(&mut s, &f1, Active::Browser, false, 3.0, &clock(3.0), 10.0);
        assert!(!s.triggers.enabled());
        s.gate.log().flush(std::time::Duration::from_secs(2));
        let kinds: Vec<(String, Value)> = sink.events().iter().filter(|e| e.kind.starts_with("audio_")).map(|e| (e.kind.clone(), e.detail.clone())).collect();
        assert_eq!(kinds[0].0, "audio_triggers");
        assert_eq!(kinds[1], ("audio_automation".to_string(), json!({ "on": true, "reason": "operator" })));
        assert_eq!(kinds[2], ("audio_automation".to_string(), json!({ "on": false, "reason": "estop" })));
        assert_eq!(kinds.len(), 5);
    }

    /// The engine's clock at `t`, from the studio's own tempo clock (what
    /// the deck reads too).
    fn tempo_clock(s: &Shared, t: f64) -> BeatClock {
        BeatClock { beat: s.tempo.beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar }
    }

    /// Runs frames from `from` to `to` (s) with `f`, then returns the first
    /// frame time at which `cue` was playing.
    fn run_until_playing(s: &mut Shared, f: &AudioFeatures, from: f64, to: f64, cue: &str) -> Option<f64> {
        let mut t = from;
        while t < to {
            crate::controls::deck_frame(s, t);
            let c = tempo_clock(s, t);
            frame(s, f, Active::Native, false, t, &c, 10.0);
            if s.deck.active.iter().any(|a| a.cue == cue) {
                return Some(t);
            }
            t += FPS60;
        }
        None
    }

    #[test]
    fn a_quantised_trigger_and_the_decks_quantisation_wait_once_for_the_coarser() {
        for (rule_q, deck_q, grid) in [(Quantize::Off, Quantize::Beat, 1.0), (Quantize::Beat, Quantize::Beat, 1.0), (Quantize::Bar, Quantize::Beat, 4.0), (Quantize::Beat, Quantize::Bar, 4.0)] {
            let mut s = rig();
            s.deck.quantize = deck_q;
            let cue = some_cue(&s, 4);
            let t0 = 10.0;
            let beat_s = 60.0 / s.tempo.bpm;
            set_config(&mut s, TriggerConfig { triggers: vec![AudioTrigger { quantize: rule_q, ..rule(AudioEvent::Drop, TriggerAction::LaunchCue(cue.clone())) }] }).unwrap();
            set_enabled(&mut s, true, ArmSource::Ui).unwrap();
            // The drop a third of a beat after t0's beat.
            let t_drop = t0 + beat_s * (s.tempo.beat_at(t0).ceil() - s.tempo.beat_at(t0) + 0.33);
            run_until_playing(&mut s, &AudioFeatures::default(), t0, t_drop, &cue);
            let at = run_until_playing(&mut s, &AudioFeatures { drop: 1, ..Default::default() }, t_drop, t_drop + 5.0 * beat_s * grid, &cue).expect("launched");
            let started = s.deck.active.iter().find(|a| a.cue == cue).unwrap().started_beat;
            let want = (s.tempo.beat_at(t_drop) / grid).ceil() * grid;
            assert!((started - want).abs() < 1e-6, "{rule_q:?}/{deck_q:?}: started on {started}, want {want}");
            assert!(s.tempo.beat_at(at) - want < 0.1, "{rule_q:?}/{deck_q:?}: no second wait");
            assert!(!s.gate.is_armed());
        }
    }

    #[test]
    fn an_estop_drops_the_waiting_trigger_actions_and_the_decks_waiting_launches() {
        let mut s = rig();
        s.deck.quantize = Quantize::Bar;
        let (a, b) = (some_cue(&s, 4), some_cue(&s, 5));
        set_config(&mut s, TriggerConfig { triggers: vec![AudioTrigger { quantize: Quantize::Bar, ..rule(AudioEvent::Drop, TriggerAction::LaunchCue(a.clone())) }] }).unwrap();
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let t = 10.0 + 0.1;
        let c = tempo_clock(&s, t);
        frame(&mut s, &AudioFeatures::default(), Active::Native, false, t, &c, 10.0);
        let c = tempo_clock(&s, t + 0.02);
        frame(&mut s, &AudioFeatures { drop: 1, ..Default::default() }, Active::Native, false, t + 0.02, &c, 10.0);
        assert!(s.triggers.engine.status()[0].pending, "the trigger waits for the bar");
        // A cue pressed by hand waits in the deck.
        crate::controls::press_cue_at(&mut s, &b, None, true, Some("ui"), t + 0.03);
        assert_eq!(s.deck.pending.len(), 1);
        s.emergency_stop(ArmSource::Keyboard);
        assert!(s.deck.pending.is_empty(), "the deck's launch dropped");
        assert!(!s.triggers.engine.status()[0].pending, "the trigger's action dropped");
        // Long after the bar: nothing starts.
        let estop = std::sync::Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        assert!(run_until_playing(&mut s, &AudioFeatures { drop: 1, ..Default::default() }, t + 0.05, t + 6.0, &a).is_none());
        assert!(s.deck.active.is_empty());
    }

    #[test]
    fn next_cue_walks_the_page_and_flashes_come_and_go() {
        let mut s = rig();
        let page: Vec<String> = s.grid.view()[0].cells.iter().flatten().cloned().collect();
        assert!(page.len() >= 2);
        set_config(&mut s, TriggerConfig { triggers: vec![rule(AudioEvent::Kick, TriggerAction::NextCue)] }).unwrap();
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let mut t = 1.0;
        for k in 0..3u64 {
            let f = AudioFeatures { kick: k, ..Default::default() };
            frame(&mut s, &f, Active::Native, false, t, &clock(t), 10.0);
            t += 1.0;
        }
        assert_eq!(s.active_cue.as_deref(), Some(page[1].as_str()), "first, then second");

        let flash = page[0].clone();
        set_config(&mut s, TriggerConfig { triggers: vec![rule(AudioEvent::Snare, TriggerAction::FlashCue { id: flash.clone(), beats: 1.0 })] }).unwrap();
        let held = |s: &Shared| s.deck.active.iter().any(|a| a.cue == flash && a.held);
        frame(&mut s, &AudioFeatures::default(), Active::Native, false, t, &clock(t), 10.0);
        frame(&mut s, &AudioFeatures { snare: 1, ..Default::default() }, Active::Native, false, t + 0.1, &clock(t + 0.1), 10.0);
        assert!(held(&s));
        frame(&mut s, &AudioFeatures { snare: 1, ..Default::default() }, Active::Native, false, t + 0.7, &clock(t + 0.7), 10.0);
        assert!(!held(&s), "released after one beat");
    }

    #[test]
    fn a_flash_released_by_the_estop_leaves_no_fade_out() {
        let mut s = rig();
        s.deck.transition = crate::transition::Transition { kind: crate::transition::TransitionKind::Fade, in_flash: true, ..crate::transition::Transition::CUT };
        let page: Vec<String> = s.grid.view()[0].cells.iter().flatten().cloned().collect();
        let flash = page[0].clone();
        set_config(&mut s, TriggerConfig { triggers: vec![rule(AudioEvent::Snare, TriggerAction::FlashCue { id: flash.clone(), beats: 4.0 })] }).unwrap();
        set_enabled(&mut s, true, ArmSource::Ui).unwrap();
        let t = 1.0;
        frame(&mut s, &AudioFeatures::default(), Active::Native, false, t, &clock(t), 10.0);
        frame(&mut s, &AudioFeatures { snare: 1, ..Default::default() }, Active::Native, false, t + 0.1, &clock(t + 0.1), 10.0);
        assert!(s.deck.active.iter().any(|a| a.cue == flash && a.held));
        s.emergency_stop(ArmSource::Keyboard);
        frame(&mut s, &AudioFeatures { snare: 1, ..Default::default() }, Active::Native, false, t + 0.2, &clock(t + 0.2), 10.0);
        assert!(s.deck.ending.is_empty() && s.deck.pending.is_empty(), "the e-stop cuts every transition");
    }
}
