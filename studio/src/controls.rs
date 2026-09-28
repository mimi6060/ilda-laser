//! Stable control ids: one registry of every knob, button and trigger in
//! the studio, so the UI, MIDI controllers (APC40 & co), OSC and timeline
//! envelopes all drive exactly the same things through one entry point,
//! `apply`.
//!
//! Ids are lowercase, dot-separated and **never renamed** once shipped -
//! saved MIDI mappings refer to them. Add an alias instead of renaming.

use crate::cues::{self, ClickMode, CueDeck, CLICK_MODES, CLICK_MODE_LABELS};
use crate::engine::Settings;
use crate::interlock::{ArmSource, DisarmReason};
use crate::layers::{Layer, LAYER_COUNT};
use crate::presets::{Preset, CATEGORIES};
use crate::live::{
    ChaseSpread, ColorOverride, ColorParams, LiveModifiers, PaletteMode, Rate, COLOR_MODE_LABELS, COLOR_STEPS_BEATS,
    COLOR_STEP_LABELS, PALETTE_LABELS, ROT_PRESETS_FREE, ROT_PRESETS_SYNC, ROT_PRESET_LABELS,
};
use crate::tempo;
use crate::Shared;
use serde::Serialize;
use std::collections::HashMap;

/// Size of one cue-grid page: 5 rows x 8 columns, the APC40 clip grid.
pub const GRID_ROWS: usize = 5;
pub const GRID_COLS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    None,
    Percent,
    DegPerSec,
    Bpm,
    Deg,
    Hz,
}

const PALETTE_MODE_LABELS: [&str; 2] = ["Plus proche", "Pas à pas"];
const CHASE_SPREAD_LABELS: [&str; 3] = ["Tout", "Par trait", "Par point"];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlKind {
    Continuous { min: f32, max: f32, default: f32, unit: Unit },
    Toggle { default: bool },
    Momentary,
    Trigger,
    /// One of `options`; set by index (value) or spread over 0..1 (norm).
    Choice { options: Vec<&'static str>, default: usize },
}

#[derive(Clone, Debug, Serialize)]
pub struct ControlDesc {
    pub id: String,
    pub label_fr: String,
    pub group: &'static str,
    pub kind: ControlKind,
    /// Whether controllers (MIDI/OSC) may use it. Arming the laser is not
    /// external: only the on-screen button and Space arm it.
    pub external: bool,
}

/// How a caller expresses a value: native units, or normalised 0..1 (what
/// a MIDI fader naturally gives).
#[derive(Clone, Copy, Debug)]
pub enum ControlInput {
    Value(f32),
    Norm(f32),
}

#[derive(Debug, PartialEq)]
pub enum ControlError {
    Unknown(String),
    Refused(&'static str),
}

pub struct ControlRegistry {
    descs: Vec<ControlDesc>,
    by_id: HashMap<String, usize>,
}

impl ControlRegistry {
    pub fn build(presets: &[Preset]) -> Self {
        let mut descs = Vec::new();
        let mut add = |id: String, label_fr: String, group: &'static str, kind: ControlKind, external: bool| {
            descs.push(ControlDesc { id, label_fr, group, kind, external });
        };
        let cont = |min: f32, max: f32, default: f32, unit: Unit| ControlKind::Continuous { min, max, default, unit };
        let d = Settings::default();

        add("look.size".into(), "Taille du look".into(), "look", cont(0.05, 1.0, d.scale, Unit::Percent), true);
        add("look.brightness".into(), "Luminosité du look".into(), "look", cont(0.0, 1.0, d.brightness, Unit::Percent), true);
        add("look.rotation_speed".into(), "Rotation du look".into(), "look", cont(-360.0, 360.0, 0.0, Unit::DegPerSec), true);

        let m = LiveModifiers::default();
        add("master.brightness".into(), "Luminosité maître".into(), "master", cont(0.0, 1.0, m.brightness, Unit::Percent), true);
        add("master.size".into(), "Taille maître".into(), "master", cont(0.0, 2.0, m.size, Unit::Percent), true);
        add("master.size_x".into(), "Taille X".into(), "master", cont(-2.0, 2.0, m.size_x, Unit::Percent), true);
        add("master.size_y".into(), "Taille Y".into(), "master", cont(-2.0, 2.0, m.size_y, Unit::Percent), true);
        add("master.pos_x".into(), "Position X".into(), "master", cont(-1.0, 1.0, 0.0, Unit::None), true);
        add("master.pos_y".into(), "Position Y".into(), "master", cont(-1.0, 1.0, 0.0, Unit::None), true);
        for (axis, name) in ["x", "y", "z"].iter().zip(["X", "Y", "Z"]) {
            add(format!("master.rot_{axis}.angle"), format!("Angle {name}"), "master", cont(-180.0, 180.0, 0.0, Unit::Deg), true);
            add(format!("master.rot_{axis}.speed"), format!("Rotation {name}"), "master", cont(-720.0, 720.0, 0.0, Unit::DegPerSec), true);
        }
        add(
            "master.rot.preset".into(),
            "Vitesse de rotation".into(),
            "master",
            ControlKind::Choice { options: ROT_PRESET_LABELS.to_vec(), default: 0 },
            true,
        );
        add("master.rot.sync".into(), "Rotation synchro tempo".into(), "master", ControlKind::Toggle { default: false }, true);
        add("master.rot.reverse".into(), "Inverser la rotation".into(), "master", ControlKind::Momentary, true);
        add("master.perspective".into(), "Perspective".into(), "master", cont(0.0, 1.0, m.perspective, Unit::Percent), true);
        add("master.speed".into(), "Vitesse d'animation".into(), "master", cont(0.0, 4.0, m.speed, Unit::Percent), true);
        add("master.reset".into(), "Réinitialiser le direct".into(), "master", ControlKind::Trigger, true);

        let c = ColorParams::default();
        let choice = |options: &[&'static str], default: usize| ControlKind::Choice { options: options.to_vec(), default };
        add("master.color.mode".into(), "Mode couleur".into(), "master", choice(&COLOR_MODE_LABELS, 0), true);
        add("master.color.hue".into(), "Teinte".into(), "master", cont(0.0, 360.0, c.hue, Unit::Deg), true);
        add("master.color.palette".into(), "Palette".into(), "master", choice(&PALETTE_LABELS, c.palette), true);
        add("master.color.palette_mode".into(), "Mode de palette".into(), "master", choice(&PALETTE_MODE_LABELS, 0), true);
        add("master.color.offset".into(), "Décalage de palette".into(), "master", cont(0.0, 15.0, 0.0, Unit::None), true);
        add("master.color.rate".into(), "Pas couleur (temps)".into(), "master", choice(&COLOR_STEP_LABELS, 3), true);
        add("master.color.rate_hz".into(), "Vitesse couleur libre".into(), "master", cont(0.0, 10.0, 0.0, Unit::Hz), true);
        add("master.color.spread".into(), "Étalement arc-en-ciel".into(), "master", cont(0.0, 4.0, c.spread, Unit::None), true);
        add("master.color.chase_spread".into(), "Répartition du chenillard".into(), "master", choice(&CHASE_SPREAD_LABELS, 1), true);
        for (i, name) in ["Rouge", "Vert", "Bleu"].iter().enumerate() {
            let id = ["red", "green", "blue"][i];
            let default = c.rgb[i] as f32 / 255.0;
            add(format!("master.color.{id}"), format!("Couleur fixe : {name}"), "master", cont(0.0, 1.0, default, Unit::Percent), true);
        }

        add("audio.enabled".into(), "Réagit à la musique".into(), "audio", ControlKind::Toggle { default: false }, true);
        add("audio.size".into(), "Taille suit les basses".into(), "audio", cont(0.0, 1.0, d.audio.size, Unit::Percent), true);
        add("audio.rotate".into(), "Rotation suit les basses".into(), "audio", cont(0.0, 1.0, d.audio.rotate, Unit::Percent), true);
        add("audio.flash".into(), "Flash sur le beat".into(), "audio", cont(0.0, 1.0, d.audio.flash, Unit::Percent), true);
        add("audio.color_on_beat".into(), "Couleur change au beat".into(), "audio", ControlKind::Toggle { default: true }, true);

        // Both latch the emergency stop (T-251): a controller's blackout pad
        // is the stop button. Only the UI resets it, and nothing external arms.
        add("transport.blackout".into(), "Blackout".into(), "transport", ControlKind::Trigger, true);
        add("safety.estop".into(), "Arrêt d'urgence".into(), "safety", ControlKind::Trigger, true);
        add("transport.arm".into(), "Allumer le laser".into(), "transport", ControlKind::Toggle { default: false }, false);

        add("tempo.tap".into(), "Tap tempo".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.resync".into(), "Recaler sur le 1".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.bpm".into(), "BPM".into(), "tempo", cont(tempo::MIN_BPM as f32, tempo::MAX_BPM as f32, 120.0, Unit::Bpm), true);
        add("tempo.nudge_up".into(), "Avancer la phase".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.nudge_down".into(), "Retarder la phase".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.double".into(), "Tempo ×2".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.half".into(), "Tempo ÷2".into(), "tempo", ControlKind::Trigger, true);

        add("page.next".into(), "Page de cues suivante".into(), "page", ControlKind::Trigger, true);
        add("page.prev".into(), "Page de cues précédente".into(), "page", ControlKind::Trigger, true);
        for (i, name) in CATEGORIES.iter().enumerate() {
            add(format!("page.{}", i + 1), format!("Page {name}"), "page", ControlKind::Trigger, true);
        }

        add(
            "cue.mode".into(),
            "Mode de clic des cues".into(),
            "cue",
            ControlKind::Choice { options: CLICK_MODE_LABELS.to_vec(), default: 0 },
            true,
        );
        add("cue.multi".into(), "Plusieurs cues à la fois".into(), "cue", ControlKind::Toggle { default: false }, true);
        let max = CueDeck::default().max_active as f32;
        add("cue.max_active".into(), "Cues simultanés max".into(), "cue", cont(1.0, cues::MAX_ACTIVE_LIMIT as f32, max, Unit::None), true);
        add("cue.stop_all".into(), "Arrêter tous les cues".into(), "cue", ControlKind::Trigger, true);

        for n in 1..=LAYER_COUNT {
            add(format!("layer.{n}.dimmer"), format!("Gradateur calque {n}"), "layer", cont(0.0, 1.0, 1.0, Unit::Percent), true);
            add(format!("layer.{n}.mute"), format!("Muet calque {n}"), "layer", ControlKind::Toggle { default: false }, true);
            add(format!("layer.{n}.solo"), format!("Solo calque {n}"), "layer", ControlKind::Toggle { default: false }, true);
            add(format!("layer.{n}.clear"), format!("Vider le calque {n}"), "layer", ControlKind::Trigger, true);
        }

        // Momentary: pad down (value 1) presses, pad up (value 0) releases -
        // what flash and solo need. Toggle/restart cues ignore the release.
        for (page, category) in CATEGORIES.iter().enumerate() {
            let cues: Vec<&Preset> = presets.iter().filter(|p| p.category == *category).collect();
            for (i, cue) in cues.iter().take(GRID_ROWS * GRID_COLS).enumerate() {
                let (row, col) = (i / GRID_COLS + 1, i % GRID_COLS + 1);
                add(format!("grid.{}.{row}.{col}", page + 1), cue.name.clone(), "grid", ControlKind::Momentary, true);
            }
        }

        let by_id = descs.iter().enumerate().map(|(i, d)| (d.id.clone(), i)).collect();
        Self { descs, by_id }
    }

    pub fn list(&self) -> &[ControlDesc] {
        &self.descs
    }

    /// Looks up an id, accepting `live.<param>` as an alias of `master.<param>`.
    pub fn get(&self, id: &str) -> Option<&ControlDesc> {
        let canonical = match id.strip_prefix("live.") {
            Some(rest) => format!("master.{rest}"),
            None => id.to_string(),
        };
        self.by_id.get(&canonical).map(|&i| &self.descs[i])
    }
}

/// Convert and clamp an input to native units for a continuous control.
fn native(min: f32, max: f32, input: ControlInput) -> f32 {
    match input {
        ControlInput::Value(v) => v.clamp(min, max),
        ControlInput::Norm(n) => min + (max - min) * n.clamp(0.0, 1.0),
    }
}

fn truthy(input: ControlInput) -> bool {
    match input {
        ControlInput::Value(v) | ControlInput::Norm(v) => v >= 0.5,
    }
}

/// Apply one control change to the shared state. `from_external` is true
/// for MIDI/OSC, false for the on-screen UI.
pub fn apply(s: &mut Shared, id: &str, input: ControlInput, from_external: bool) -> Result<(), ControlError> {
    let desc = s.controls.get(id).cloned().ok_or_else(|| ControlError::Unknown(id.to_string()))?;
    if from_external && !desc.external {
        return Err(ControlError::Refused("ce contrôle ne peut pas être piloté depuis un contrôleur externe"));
    }
    let value = |i| match desc.kind {
        ControlKind::Continuous { min, max, .. } => native(min, max, i),
        _ => 0.0,
    };

    match desc.id.as_str() {
        "look.size" => s.settings.scale = value(input),
        "look.brightness" => s.settings.brightness = value(input),
        "look.rotation_speed" => s.settings.rotation_speed = value(input),
        "master.brightness" => set_live(s, |m| m.brightness = value(input)),
        "master.size" => set_live(s, |m| m.size = value(input)),
        "master.size_x" => set_live(s, |m| m.size_x = value(input)),
        "master.size_y" => set_live(s, |m| m.size_y = value(input)),
        "master.pos_x" => set_live(s, |m| m.pos_x = value(input)),
        "master.pos_y" => set_live(s, |m| m.pos_y = value(input)),
        "master.rot.preset" => {
            let index = choice_index(&desc.kind, input);
            set_live(s, |m| {
                let table = if m.rot_sync { ROT_PRESETS_SYNC } else { ROT_PRESETS_FREE };
                m.rot_speed[2] = table[index];
            })
        }
        "master.rot.sync" => set_live(s, |m| {
            // Keep the same step when switching modes (Moyen stays Moyen).
            let on = truthy(input);
            if on == m.rot_sync {
                return;
            }
            let (from, to) = if on { (ROT_PRESETS_FREE, ROT_PRESETS_SYNC) } else { (ROT_PRESETS_SYNC, ROT_PRESETS_FREE) };
            if let Some(i) = from.iter().position(|&v| v == m.rot_speed[2]) {
                m.rot_speed[2] = to[i];
            }
            m.rot_sync = on;
        }),
        "master.rot.reverse" => set_live(s, |m| m.rot_reverse = truthy(input)),
        "master.perspective" => set_live(s, |m| m.perspective = value(input)),
        "master.speed" => set_live(s, |m| m.speed = value(input)),
        "master.reset" => set_live(s, |m| *m = LiveModifiers::default()),
        "master.color.mode" => {
            let index = choice_index(&desc.kind, input);
            set_live(s, |m| m.color = m.color_params.build(index))
        }
        "master.color.hue" => set_color(s, |c| c.hue = value(input)),
        "master.color.palette" => {
            let index = choice_index(&desc.kind, input);
            set_color(s, |c| c.palette = index)
        }
        "master.color.palette_mode" => {
            let mode = [PaletteMode::Nearest, PaletteMode::Step][choice_index(&desc.kind, input)];
            set_color(s, |c| c.palette_mode = mode)
        }
        "master.color.offset" => set_color(s, |c| c.offset = value(input).round() as usize),
        "master.color.rate" => {
            let rate = Rate::Beats(COLOR_STEPS_BEATS[choice_index(&desc.kind, input)]);
            set_color_rate(s, rate)
        }
        "master.color.rate_hz" => set_color_rate(s, Rate::Hz(value(input))),
        "master.color.spread" => set_color(s, |c| c.spread = value(input)),
        "master.color.chase_spread" => {
            let spread = [ChaseSpread::Whole, ChaseSpread::Stroke, ChaseSpread::Point][choice_index(&desc.kind, input)];
            set_color(s, |c| c.chase_spread = spread)
        }
        "master.color.red" | "master.color.green" | "master.color.blue" => {
            let channel = ["master.color.red", "master.color.green", "master.color.blue"].iter().position(|&i| i == desc.id).unwrap_or(0);
            let v = (value(input) * 255.0).round() as u8;
            set_color(s, |c| c.rgb[channel] = v)
        }
        "audio.enabled" => s.settings.audio.enabled = truthy(input),
        "audio.size" => s.settings.audio.size = value(input),
        "audio.rotate" => s.settings.audio.rotate = value(input),
        "audio.flash" => s.settings.audio.flash = value(input),
        "audio.color_on_beat" => s.settings.audio.color_on_beat = truthy(input),
        "transport.blackout" | "safety.estop" => s.emergency_stop(if from_external { ArmSource::Midi } else { ArmSource::Ui }),
        // Only reachable from the UI (external is false): arming goes
        // through the gate, like the laser button.
        "transport.arm" => {
            if truthy(input) {
                s.request_arm(ArmSource::Ui).map_err(|_| ControlError::Refused("armement impossible : un verrou est actif"))?
            } else {
                s.gate.disarm(DisarmReason::User, ArmSource::Ui)
            }
        }
        "tempo.tap" => {
            let t = s.now_s();
            s.tempo.tap(t);
        }
        "tempo.resync" => {
            let t = s.now_s();
            s.tempo.resync(t);
        }
        "tempo.bpm" => {
            let t = s.now_s();
            s.tempo.set_bpm_manual(value(input) as f64, t);
        }
        "tempo.nudge_up" => s.tempo.nudge(1.0 / 32.0),
        "tempo.nudge_down" => s.tempo.nudge(-1.0 / 32.0),
        "tempo.double" | "tempo.half" => {
            let (t, factor) = (s.now_s(), if desc.id == "tempo.double" { 2.0 } else { 0.5 });
            let bpm = s.tempo.bpm * factor;
            s.tempo.set_bpm_manual(bpm, t);
        }
        "cue.mode" => {
            s.deck.click_mode = CLICK_MODES[choice_index(&desc.kind, input)];
            s.deck.save();
        }
        "cue.multi" => {
            s.deck.multi = truthy(input);
            s.deck.save();
        }
        "cue.max_active" => {
            with_deck(s, |d, _| d.set_max_active(value(input).round() as u8));
            s.deck.save();
        }
        "cue.stop_all" => with_deck(s, |d, _| d.stop_all()),
        "page.next" => s.cue_page = (s.cue_page + 1) % CATEGORIES.len(),
        "page.prev" => s.cue_page = (s.cue_page + CATEGORIES.len() - 1) % CATEGORIES.len(),
        other => {
            if let Some(rest) = other.strip_prefix("master.rot_") {
                let axis = match &rest[..1] {
                    "x" => 0,
                    "y" => 1,
                    _ => 2,
                };
                let v = value(input);
                if rest.ends_with(".angle") {
                    set_live(s, |m| m.rot_angle[axis] = v);
                } else {
                    set_live(s, |m| m.rot_speed[axis] = v);
                }
                return Ok(());
            }
            if let Some((n, param)) = layer_control(other) {
                match param {
                    "dimmer" => set_layer(s, n, |l| l.dimmer = value(input)),
                    "mute" => set_layer(s, n, |l| l.mute = truthy(input)),
                    "solo" => set_layer(s, n, |l| l.solo = truthy(input)),
                    _ => with_deck(s, |d, _| d.clear_layer(n)),
                }
                return Ok(());
            }
            if let Some(n) = other.strip_prefix("page.").and_then(|n| n.parse::<usize>().ok()) {
                s.cue_page = n - 1;
            } else if let Some(cell) = other.strip_prefix("grid.") {
                let preset_id = grid_cell_preset(&s.presets, cell).ok_or_else(|| ControlError::Unknown(id.to_string()))?;
                press_cue(s, &preset_id, None, truthy(input));
            } else {
                return Err(ControlError::Unknown(id.to_string()));
            }
        }
    }
    if desc.id.starts_with("look.") || desc.id.starts_with("audio.") {
        s.settings_rev += 1;
    }
    Ok(())
}

/// "layer.<n>.<param>" → (n, param), for registered ids only.
fn layer_control(id: &str) -> Option<(u8, &str)> {
    let (n, param) = id.strip_prefix("layer.")?.split_once('.')?;
    Some((n.parse().ok()?, param))
}

fn set_layer(s: &mut Shared, n: u8, f: impl FnOnce(&mut Layer)) {
    f(s.mixer.layer_mut(n));
    s.mixer_dirty = true;
}

fn set_live(s: &mut Shared, f: impl FnOnce(&mut LiveModifiers)) {
    f(&mut s.live);
    s.live_dirty = true;
}

/// Change a colour setting; the active colour mode picks it up at once. It
/// never switches mode by itself (that's `master.color.mode`).
fn set_color(s: &mut Shared, f: impl FnOnce(&mut ColorParams)) {
    set_live(s, |m| {
        f(&mut m.color_params);
        m.color = m.color_params.build(m.color.mode_index());
    })
}

/// « Pas » drives the mode that uses a rate (rainbow or chase); in the
/// other modes it presets both.
fn set_color_rate(s: &mut Shared, rate: Rate) {
    let mode = s.live.color.mode_index();
    set_color(s, |c| match mode {
        4 => c.rainbow_rate = rate,
        5 => c.chase_step = rate,
        _ => (c.rainbow_rate, c.chase_step) = (rate, rate),
    })
}

fn choice_index(kind: &ControlKind, input: ControlInput) -> usize {
    let n = match kind {
        ControlKind::Choice { options, .. } => options.len(),
        _ => 1,
    };
    let i = match input {
        ControlInput::Value(v) => v.round().max(0.0) as usize,
        ControlInput::Norm(x) => (x.clamp(0.0, 1.0) * n as f32) as usize,
    };
    i.min(n - 1)
}

/// Current value of a control, for LED feedback and UI sync. Triggers have
/// none.
pub fn current(s: &Shared, desc: &ControlDesc) -> Option<serde_json::Value> {
    use serde_json::json;
    Some(match desc.id.as_str() {
        "look.size" => json!(s.settings.scale),
        "look.brightness" => json!(s.settings.brightness),
        "look.rotation_speed" => json!(s.settings.rotation_speed),
        "master.brightness" => json!(s.live.brightness),
        "master.size" => json!(s.live.size),
        "master.size_x" => json!(s.live.size_x),
        "master.size_y" => json!(s.live.size_y),
        "master.pos_x" => json!(s.live.pos_x),
        "master.pos_y" => json!(s.live.pos_y),
        "master.rot.sync" => json!(s.live.rot_sync),
        "master.rot.reverse" => json!(s.live.rot_reverse),
        "master.perspective" => json!(s.live.perspective),
        "master.speed" => json!(s.live.speed),
        "master.color.mode" => json!(s.live.color.mode_index()),
        "master.color.hue" => json!(s.live.color_params.hue),
        "master.color.palette" => json!(s.live.color_params.palette),
        "master.color.palette_mode" => json!(s.live.color_params.palette_mode as usize),
        "master.color.offset" => json!(s.live.color_params.offset),
        "master.color.spread" => json!(s.live.color_params.spread),
        "master.color.chase_spread" => json!(s.live.color_params.chase_spread as usize),
        "master.color.rate" | "master.color.rate_hz" => {
            let c = &s.live.color_params;
            let rate = if matches!(s.live.color, ColorOverride::Rainbow { .. }) { c.rainbow_rate } else { c.chase_step };
            match (desc.id.as_str(), rate) {
                ("master.color.rate", Rate::Beats(b)) => json!(COLOR_STEPS_BEATS.iter().position(|&x| x == b)),
                ("master.color.rate_hz", Rate::Hz(hz)) => json!(hz),
                _ => serde_json::Value::Null,
            }
        }
        "master.color.red" => json!(s.live.color_params.rgb[0] as f32 / 255.0),
        "master.color.green" => json!(s.live.color_params.rgb[1] as f32 / 255.0),
        "master.color.blue" => json!(s.live.color_params.rgb[2] as f32 / 255.0),
        "audio.enabled" => json!(s.settings.audio.enabled),
        "audio.size" => json!(s.settings.audio.size),
        "audio.rotate" => json!(s.settings.audio.rotate),
        "audio.flash" => json!(s.settings.audio.flash),
        "audio.color_on_beat" => json!(s.settings.audio.color_on_beat),
        "transport.arm" => json!(s.gate.is_armed() && !s.estop.is_latched()),
        "tempo.bpm" => json!(s.tempo.bpm),
        "cue.mode" => json!(CLICK_MODES.iter().position(|&m| m == s.deck.click_mode).unwrap_or(0)),
        "cue.multi" => json!(s.deck.multi),
        "cue.max_active" => json!(s.deck.max_active),
        id if id.starts_with("layer.") => {
            let (n, param) = layer_control(id)?;
            let l = s.mixer.layer(n);
            match param {
                "dimmer" => json!(l.dimmer),
                "mute" => json!(l.mute),
                "solo" => json!(l.solo),
                _ => return None,
            }
        }
        id => {
            // Grid cells light up while their cue plays (LED feedback).
            let cue = grid_cell_preset(&s.presets, id.strip_prefix("grid.")?)?;
            json!(s.deck.active.iter().any(|a| a.cue == cue))
        }
    })
}

/// "<page>.<row>.<col>" → the preset in that grid cell.
fn grid_cell_preset(presets: &[Preset], cell: &str) -> Option<String> {
    let parts: Vec<usize> = cell.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [page, row, col] = parts[..] else { return None };
    if !(1..=GRID_ROWS).contains(&row) || !(1..=GRID_COLS).contains(&col) {
        return None;
    }
    let category = CATEGORIES.get(page.checked_sub(1)?)?;
    presets
        .iter()
        .filter(|p| p.category == *category)
        .nth((row - 1) * GRID_COLS + (col - 1))
        .map(|p| p.id.clone())
}

/// A cue's look as it starts: the preset, but with the operator's
/// brightness, and the operator's music settings unless the cue is built
/// around the music.
fn cue_settings(s: &Shared, id: &str) -> Option<Settings> {
    let mut settings = s.presets.iter().find(|p| p.id == id)?.settings.clone();
    settings.brightness = s.settings.brightness;
    if !settings.audio.enabled {
        settings.audio = s.settings.audio.clone();
    }
    Some(settings)
}

/// Play a cue from the start, whatever its click mode (the old « play »
/// action, kept for `/api/presets/play`).
pub fn play_preset(s: &mut Shared, id: &str) -> bool {
    press_cue(s, id, Some(ClickMode::Restart), true)
}

/// A cue's key, pad or button went down (`down`) or up. `mode` overrides
/// the cue's click mode (Shift + letter flashes). False if no such cue.
pub fn press_cue(s: &mut Shared, id: &str, mode: Option<ClickMode>, down: bool) -> bool {
    let Some(settings) = cue_settings(s, id) else { return false };
    let first_new = s.deck.next_id();
    with_deck(s, |deck, at| {
        if down {
            deck.press(id, mode, at, || settings);
        } else {
            deck.release(id);
        }
    });
    if s.deck.active.iter().any(|a| !a.held && a.id >= first_new) {
        // A latched cue takes over from the scene/playlist, as a cue click
        // always did. A flash doesn't: the look comes back on release.
        s.playlist = None;
        s.look_on = false;
    }
    true
}

/// Run a change on the deck, keeping `Shared::settings` the live look of
/// the newest cue (see cues.rs), and parking the manual look while cues
/// play.
fn with_deck(s: &mut Shared, f: impl FnOnce(&mut CueDeck, cues::At)) {
    let was_empty = s.deck.active.is_empty();
    let before = s.settings.clone();
    if let Some(top) = s.deck.active.last_mut() {
        top.settings = s.settings.clone();
    }
    let now = s.now_s();
    f(&mut s.deck, cues::At { s: now, beat: s.tempo.beat_at(now) });
    match s.deck.active.last() {
        Some(top) => {
            if was_empty {
                s.deck.parked = Some(std::mem::replace(&mut s.settings, top.settings.clone()));
            } else {
                s.settings = top.settings.clone();
            }
        }
        None => {
            let parked = s.deck.parked.take();
            if let (Some(look), true, false) = (parked, s.look_on, was_empty) {
                s.settings = look;
            }
        }
    }
    s.active_cue = s.deck.primary().map(|a| a.cue.clone());
    if s.settings != before {
        s.settings_rev += 1;
    }
}

/// Show a look by itself (a scene, the playlist, new content picked by
/// hand): every cue stops.
pub fn show_look(s: &mut Shared, settings: Settings) {
    s.deck.stop_all();
    s.deck.parked = None;
    s.settings = settings;
    s.settings_rev += 1;
    s.look_on = true;
    s.active_cue = None;
}

/// A whole look sent by the look panel (`POST /api/settings`). Picking
/// something else to draw (another shape, content kind or generator) is
/// the operator taking over by hand: the cues stop and the look shows by
/// itself, like a scene. Other edits (size, colour, text, generator
/// parameters…) edit the newest cue while cues play (T-155), or the
/// manual look otherwise. Either way the playlist stops.
pub fn set_look(s: &mut Shared, settings: Settings) {
    if !s.deck.active.is_empty() && !settings.content.same_drawing(&s.settings.content) {
        show_look(s, settings);
    } else {
        s.settings = settings;
        s.settings_rev += 1;
        s.look_on |= s.deck.active.is_empty();
    }
    s.playlist = None;
}

/// Markdown table of every control id, for docs/controls.md.
pub fn markdown(reg: &ControlRegistry) -> String {
    let mut out = String::from(
        "# Identifiants de contrôle\n\n_Généré par `cargo test -p laser-studio controls` — ne pas éditer à la main._\n\n\
         | id | libellé | groupe | type | externe |\n|---|---|---|---|---|\n",
    );
    for d in reg.list() {
        let kind = match &d.kind {
            ControlKind::Continuous { min, max, .. } => format!("continu {min}…{max}"),
            ControlKind::Toggle { .. } => "bascule".into(),
            ControlKind::Momentary => "momentané".into(),
            ControlKind::Trigger => "déclencheur".into(),
            ControlKind::Choice { options, .. } => format!("choix : {}", options.join(" / ")),
        };
        out.push_str(&format!("| `{}` | {} | {} | {kind} | {} |\n", d.id, d.label_fr, d.group, if d.external { "oui" } else { "non" }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Content;
    use crate::presets::catalog;
    use crate::test_support::shared;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_well_formed() {
        let reg = ControlRegistry::build(&catalog());
        let mut seen = HashSet::new();
        for d in reg.list() {
            assert!(seen.insert(d.id.clone()), "duplicate id {}", d.id);
            assert!(d.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'), "bad id {}", d.id);
        }
    }

    #[test]
    fn norm_maps_to_exact_bounds_and_values_are_clamped() {
        assert_eq!(native(-360.0, 360.0, ControlInput::Norm(0.0)), -360.0);
        assert_eq!(native(-360.0, 360.0, ControlInput::Norm(1.0)), 360.0);
        assert_eq!(native(0.05, 1.0, ControlInput::Value(5.0)), 1.0);
        assert_eq!(native(0.05, 1.0, ControlInput::Norm(-3.0)), 0.05);
    }

    #[test]
    fn look_and_master_size_are_separate() {
        let mut s = shared();
        apply(&mut s, "look.size", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.settings.scale, 1.0);
        apply(&mut s, "master.size", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.live.size, 2.0);
        apply(&mut s, "live.size", ControlInput::Value(0.2), true).unwrap();
        assert_eq!(s.live.size, 0.2);
        assert!(s.live_dirty);
    }

    #[test]
    fn rotation_presets_and_sync_keep_the_same_step() {
        let mut s = shared();
        apply(&mut s, "master.rot.preset", ControlInput::Value(2.0), true).unwrap();
        assert_eq!(s.live.rot_speed[2], ROT_PRESETS_FREE[2]);
        apply(&mut s, "master.rot.sync", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.live.rot_speed[2], ROT_PRESETS_SYNC[2]);
        apply(&mut s, "master.rot.sync", ControlInput::Value(0.0), true).unwrap();
        assert_eq!(s.live.rot_speed[2], ROT_PRESETS_FREE[2]);
        apply(&mut s, "master.rot.preset", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.live.rot_speed[2], ROT_PRESETS_FREE[3]);
        apply(&mut s, "master.rot_x.angle", ControlInput::Value(45.0), true).unwrap();
        assert_eq!(s.live.rot_angle[0], 45.0);
        apply(&mut s, "master.reset", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.live, LiveModifiers::default());
    }

    #[test]
    fn colour_modes_remember_their_settings() {
        let mut s = shared();
        apply(&mut s, "master.color.hue", ControlInput::Value(200.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Normal, "a setting alone never switches mode");
        apply(&mut s, "master.color.mode", ControlInput::Value(2.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Hue { hue: 200.0 });
        apply(&mut s, "master.color.hue", ControlInput::Norm(0.5), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Hue { hue: 180.0 });
        apply(&mut s, "master.color.mode", ControlInput::Value(0.0), true).unwrap();
        apply(&mut s, "master.color.mode", ControlInput::Value(2.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Hue { hue: 180.0 });
        assert!(s.live_dirty);

        apply(&mut s, "master.color.green", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "master.color.mode", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Fixed { rgb: [255, 255, 0] });

        apply(&mut s, "master.color.palette", ControlInput::Value(4.0), true).unwrap();
        apply(&mut s, "master.color.palette_mode", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "master.color.offset", ControlInput::Value(2.4), true).unwrap();
        apply(&mut s, "master.color.mode", ControlInput::Value(3.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Palette { palette: 4, mode: PaletteMode::Step, offset: 2 });
        assert_eq!(current(&s, s.controls.get("master.color.mode").unwrap()), Some(serde_json::json!(3)));

        apply(&mut s, "master.reset", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Normal);
    }

    #[test]
    fn colour_step_drives_the_active_rate() {
        let mut s = shared();
        apply(&mut s, "master.color.mode", ControlInput::Value(5.0), true).unwrap();
        apply(&mut s, "master.color.rate", ControlInput::Value(0.0), true).unwrap();
        apply(&mut s, "master.color.chase_spread", ControlInput::Value(2.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Chase { palette: 0, step: Rate::Beats(0.125), spread: ChaseSpread::Point });
        assert_eq!(s.live.color_params.rainbow_rate, Rate::Beats(4.0), "the rainbow keeps its own rate");
        apply(&mut s, "master.color.mode", ControlInput::Value(4.0), true).unwrap();
        apply(&mut s, "master.color.rate_hz", ControlInput::Value(2.0), true).unwrap();
        apply(&mut s, "master.color.spread", ControlInput::Value(3.0), true).unwrap();
        assert_eq!(s.live.color, ColorOverride::Rainbow { spread: 3.0, rate: Rate::Hz(2.0) });
        assert_eq!(current(&s, s.controls.get("master.color.rate_hz").unwrap()), Some(serde_json::json!(2.0)));
    }

    #[test]
    fn unknown_ids_are_errors_not_panics() {
        let mut s = shared();
        assert!(matches!(apply(&mut s, "master.nope", ControlInput::Value(1.0), false), Err(ControlError::Unknown(_))));
        assert!(matches!(apply(&mut s, "grid.1.9.9", ControlInput::Value(1.0), false), Err(ControlError::Unknown(_))));
        assert!(matches!(apply(&mut s, "grid.x", ControlInput::Value(1.0), false), Err(ControlError::Unknown(_))));
    }

    #[test]
    fn controllers_cannot_arm_but_can_always_blackout() {
        let mut s = shared();
        assert!(matches!(apply(&mut s, "transport.arm", ControlInput::Value(1.0), true), Err(ControlError::Refused(_))));
        assert!(!s.gate.is_armed());
        apply(&mut s, "transport.arm", ControlInput::Value(1.0), false).unwrap();
        assert!(s.gate.is_armed());
        apply(&mut s, "transport.blackout", ControlInput::Value(1.0), true).unwrap();
        assert!(!s.gate.is_armed());
        assert!(s.estop.is_latched(), "a controller blackout latches the e-stop");
    }

    #[test]
    fn midi_estop_latches_and_nothing_external_rearms() {
        let mut s = shared();
        apply(&mut s, "transport.arm", ControlInput::Value(1.0), false).unwrap();
        apply(&mut s, "safety.estop", ControlInput::Norm(1.0), true).unwrap();
        assert!(!s.gate.is_armed());
        let status = s.gate.status(&s.estop);
        assert_eq!(status.estop.unwrap().source, ArmSource::Midi);
        // Not from a controller, not from the UI either while latched.
        assert!(apply(&mut s, "transport.arm", ControlInput::Value(1.0), true).is_err());
        assert!(matches!(apply(&mut s, "transport.arm", ControlInput::Value(1.0), false), Err(ControlError::Refused(_))));
        // Every control reachable from a controller leaves the laser disarmed.
        s.gate.reset_estop(&s.estop);
        let ids: Vec<String> = s.controls.list().iter().filter(|d| d.external).map(|d| d.id.clone()).collect();
        for id in ids {
            for v in [0.0, 1.0] {
                let _ = apply(&mut s, &id, ControlInput::Norm(v), true);
                assert!(!s.gate.is_armed(), "{id} armed the laser");
            }
        }
    }

    #[test]
    fn ui_arm_toggle_off_is_a_plain_disarm() {
        let mut s = shared();
        apply(&mut s, "transport.arm", ControlInput::Value(1.0), false).unwrap();
        apply(&mut s, "transport.arm", ControlInput::Value(0.0), false).unwrap();
        assert!(!s.gate.is_armed());
        assert!(!s.estop.is_latched());
        assert_eq!(s.gate.status(&s.estop).last_disarm.unwrap().reason, "user");
    }

    #[test]
    fn grid_cells_play_cues_of_their_page() {
        let mut s = shared();
        apply(&mut s, "grid.2.1.1", ControlInput::Value(1.0), true).unwrap();
        let first_tunnel = s.presets.iter().find(|p| p.category == CATEGORIES[1]).unwrap().id.clone();
        assert_eq!(s.active_cue.as_deref(), Some(first_tunnel.as_str()));
        apply(&mut s, "grid.1.2.3", ControlInput::Value(1.0), true).unwrap();
        let eleventh = s.presets.iter().filter(|p| p.category == CATEGORIES[0]).nth(10).unwrap().id.clone();
        assert_eq!(s.active_cue.as_deref(), Some(eleventh.as_str()));
    }

    #[test]
    fn pages_wrap_around() {
        let mut s = shared();
        apply(&mut s, "page.prev", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.cue_page, CATEGORIES.len() - 1);
        apply(&mut s, "page.next", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.cue_page, 0);
        apply(&mut s, "page.3", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.cue_page, 2);
    }

    #[test]
    fn tempo_controls_drive_the_clock() {
        let mut s = shared();
        apply(&mut s, "tempo.bpm", ControlInput::Value(128.0), true).unwrap();
        assert_eq!(s.tempo.bpm, 128.0);
        apply(&mut s, "tempo.half", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.tempo.bpm, 64.0);
        apply(&mut s, "tempo.double", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.tempo.bpm, 128.0);
        apply(&mut s, "tempo.bpm", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.tempo.bpm, tempo::MAX_BPM);
    }

    #[test]
    fn playing_a_cue_keeps_brightness() {
        let mut s = shared();
        s.settings.brightness = 0.2;
        assert!(play_preset(&mut s, "tunnels-001"));
        assert_eq!(s.settings.brightness, 0.2);
    }

    fn cue_at(s: &Shared, page: usize, n: usize) -> String {
        s.presets.iter().filter(|p| p.category == CATEGORIES[page]).nth(n).unwrap().id.clone()
    }

    #[test]
    fn grid_cells_toggle_and_ignore_the_release() {
        let mut s = shared();
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "grid.1.1.1", ControlInput::Value(0.0), true).unwrap();
        assert_eq!(s.active_cue, Some(cue_at(&s, 0, 0)));
        let desc = s.controls.get("grid.1.1.1").cloned().unwrap();
        assert_eq!(current(&s, &desc), Some(serde_json::json!(true)));
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.active_cue, None);
        assert!(s.deck.active.is_empty() && !s.look_on, "the last cue stopped: nothing plays");
    }

    #[test]
    fn a_flash_pad_returns_to_the_cue_and_its_edits() {
        let mut s = shared();
        apply(&mut s, "cue.multi", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "look.size", ControlInput::Value(0.77), true).unwrap();
        s.deck.set_slot(&cue_at(&s, 0, 1), cues::CueSlot { mode: Some(ClickMode::Flash), group: None, layer: None });
        apply(&mut s, "grid.1.1.2", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.active_cue, Some(cue_at(&s, 0, 1)));
        assert_eq!(cues::looks(&s.deck, &s.settings, s.look_on).len(), 2);
        apply(&mut s, "grid.1.1.2", ControlInput::Norm(0.0), true).unwrap();
        assert_eq!(s.active_cue, Some(cue_at(&s, 0, 0)));
        assert_eq!(s.settings.scale, 0.77, "the edited look of the cue below comes back");
    }

    #[test]
    fn a_flash_over_a_scene_gives_the_scene_back() {
        let mut s = shared();
        let scene = Settings { scale: 0.42, ..Default::default() };
        show_look(&mut s, scene.clone());
        assert!(press_cue(&mut s, "tunnels-001", Some(ClickMode::Flash), true));
        assert_ne!(s.settings, scene);
        assert!(s.look_on);
        press_cue(&mut s, "tunnels-001", None, false);
        assert_eq!(s.settings, scene);
        assert_eq!(cues::looks(&s.deck, &s.settings, s.look_on), vec![(0, scene)]);
        // A latched cue takes over; stopping it leaves the output dark.
        press_cue(&mut s, "tunnels-001", Some(ClickMode::Toggle), true);
        assert!(!s.look_on);
        apply(&mut s, "cue.stop_all", ControlInput::Value(1.0), true).unwrap();
        assert!(cues::looks(&s.deck, &s.settings, s.look_on).is_empty());
    }

    #[test]
    fn a_new_drawing_by_hand_stops_the_cues() {
        let mut s = shared();
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        assert!(s.active_cue.is_some());
        let square = Settings { content: Content::Shape { shape: "square".into() }, ..s.settings.clone() };
        set_look(&mut s, square.clone());
        assert_eq!(s.active_cue, None);
        assert!(s.deck.active.is_empty() && s.look_on);
        assert_eq!(cues::looks(&s.deck, &s.settings, s.look_on), vec![(0, square)]);
        let desc = s.controls.get("grid.1.1.1").cloned().unwrap();
        assert_eq!(current(&s, &desc), Some(serde_json::json!(false)), "its LED goes off");
    }

    #[test]
    fn editing_the_cue_by_hand_keeps_it_playing() {
        let mut s = shared();
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        let cue = s.active_cue.clone();
        let mut edited = s.settings.clone();
        edited.scale = 0.3;
        edited.color = [255, 0, 0];
        if let Content::Generator { params, .. } = &mut edited.content {
            params.count += 1;
        }
        set_look(&mut s, edited.clone());
        assert_eq!(s.active_cue, cue);
        assert_eq!(s.settings, edited);
        assert_eq!(s.deck.active.len(), 1);
    }

    #[test]
    fn scenes_clear_the_cue_but_master_controls_and_tempo_do_not() {
        let mut s = shared();
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        let cue = s.active_cue.clone();
        for id in ["master.size", "master.brightness", "tempo.double"] {
            apply(&mut s, id, ControlInput::Norm(0.8), true).unwrap();
            assert_eq!(s.active_cue, cue, "{id} keeps the cue");
        }
        show_look(&mut s, Settings::default());
        assert_eq!(s.active_cue, None);
    }

    #[test]
    fn the_look_revision_follows_every_change_of_the_look() {
        let mut s = shared();
        let mut last = s.settings_rev;
        let mut changed = |s: &Shared| {
            let bumped = s.settings_rev > last;
            last = s.settings_rev;
            bumped
        };
        apply(&mut s, "look.size", ControlInput::Value(0.6), true).unwrap();
        assert!(changed(&s), "look control");
        apply(&mut s, "audio.flash", ControlInput::Value(0.6), true).unwrap();
        assert!(changed(&s), "music control");
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        assert!(changed(&s), "cue started");
        apply(&mut s, "grid.1.1.1", ControlInput::Value(0.0), true).unwrap();
        assert!(!changed(&s), "an ignored release");
        apply(&mut s, "master.size", ControlInput::Value(1.5), true).unwrap();
        assert!(!changed(&s), "master modifiers are not the look");
        let same = s.settings.clone();
        set_look(&mut s, same);
        assert!(changed(&s), "look panel");
        show_look(&mut s, Settings::default());
        assert!(changed(&s), "scene");
    }

    #[test]
    fn cue_mode_and_limit_controls() {
        let mut s = shared();
        apply(&mut s, "cue.mode", ControlInput::Value(2.0), true).unwrap();
        assert_eq!(s.deck.click_mode, ClickMode::Solo);
        apply(&mut s, "cue.mode", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.deck.click_mode, ClickMode::Restart);
        apply(&mut s, "cue.multi", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "cue.mode", ControlInput::Value(0.0), true).unwrap();
        for n in 0..5 {
            apply(&mut s, &format!("grid.1.1.{}", n + 1), ControlInput::Value(1.0), true).unwrap();
        }
        assert_eq!(s.deck.active.len(), 4);
        assert!(!s.deck.active.iter().any(|a| a.cue == cue_at(&s, 0, 0)), "the oldest stopped");
        apply(&mut s, "cue.max_active", ControlInput::Value(2.0), true).unwrap();
        assert_eq!(s.deck.active.len(), 2);
        assert_eq!(s.active_cue, Some(cue_at(&s, 0, 4)));
    }

    #[test]
    fn layer_controls_drive_the_mixer_and_clear_their_cues() {
        let mut s = shared();
        apply(&mut s, "layer.2.dimmer", ControlInput::Norm(0.25), true).unwrap();
        apply(&mut s, "layer.3.mute", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "layer.4.solo", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.mixer.layers[1].dimmer, 0.25);
        assert!(s.mixer.layers[2].mute && s.mixer.layers[3].solo && s.mixer_dirty);
        assert_eq!(current(&s, s.controls.get("layer.2.dimmer").unwrap()), Some(serde_json::json!(0.25)));
        assert_eq!(current(&s, s.controls.get("layer.4.solo").unwrap()), Some(serde_json::json!(true)));
        assert_eq!(current(&s, s.controls.get("layer.1.clear").unwrap()), None);
        assert!(matches!(apply(&mut s, "layer.5.mute", ControlInput::Value(1.0), true), Err(ControlError::Unknown(_))));

        apply(&mut s, "cue.multi", ControlInput::Value(1.0), true).unwrap();
        let (a, b) = (cue_at(&s, 0, 0), cue_at(&s, 0, 1));
        s.deck.set_slot(&b, cues::CueSlot { layer: Some(2), ..Default::default() });
        apply(&mut s, "grid.1.1.1", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "grid.1.1.2", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "layer.2.clear", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.deck.active.iter().map(|c| c.cue.as_str()).collect::<Vec<_>>(), [a.as_str()]);
        assert_eq!(s.active_cue, Some(a), "the cue left on layer 1 is the primary again");
    }

    /// Keeps docs/controls.md in sync with the registry.
    #[test]
    fn write_controls_doc() {
        let md = markdown(&ControlRegistry::build(&catalog()));
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/controls.md");
        std::fs::write(path, md).unwrap();
    }
}
