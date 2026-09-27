//! Stable control ids: one registry of every knob, button and trigger in
//! the studio, so the UI, MIDI controllers (APC40 & co), OSC and timeline
//! envelopes all drive exactly the same things through one entry point,
//! `apply`.
//!
//! Ids are lowercase, dot-separated and **never renamed** once shipped -
//! saved MIDI mappings refer to them. Add an alias instead of renaming.

use crate::engine::Settings;
use crate::presets::{Preset, CATEGORIES};
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
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlKind {
    Continuous { min: f32, max: f32, default: f32, unit: Unit },
    Toggle { default: bool },
    Momentary,
    Trigger,
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

        add("master.size".into(), "Taille".into(), "master", cont(0.05, 1.0, d.scale, Unit::Percent), true);
        add("master.brightness".into(), "Luminosité".into(), "master", cont(0.0, 1.0, d.brightness, Unit::Percent), true);
        add("master.rotation_speed".into(), "Vitesse de rotation".into(), "master", cont(-360.0, 360.0, 0.0, Unit::DegPerSec), true);

        add("audio.enabled".into(), "Réagit à la musique".into(), "audio", ControlKind::Toggle { default: false }, true);
        add("audio.size".into(), "Taille suit les basses".into(), "audio", cont(0.0, 1.0, d.audio.size, Unit::Percent), true);
        add("audio.rotate".into(), "Rotation suit les basses".into(), "audio", cont(0.0, 1.0, d.audio.rotate, Unit::Percent), true);
        add("audio.flash".into(), "Flash sur le beat".into(), "audio", cont(0.0, 1.0, d.audio.flash, Unit::Percent), true);
        add("audio.color_on_beat".into(), "Couleur change au beat".into(), "audio", ControlKind::Toggle { default: true }, true);

        add("transport.blackout".into(), "Blackout".into(), "transport", ControlKind::Trigger, true);
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

        for (page, category) in CATEGORIES.iter().enumerate() {
            let cues: Vec<&Preset> = presets.iter().filter(|p| p.category == *category).collect();
            for (i, cue) in cues.iter().take(GRID_ROWS * GRID_COLS).enumerate() {
                let (row, col) = (i / GRID_COLS + 1, i % GRID_COLS + 1);
                add(format!("grid.{}.{row}.{col}", page + 1), cue.name.clone(), "grid", ControlKind::Trigger, true);
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
        "master.size" => s.settings.scale = value(input),
        "master.brightness" => s.settings.brightness = value(input),
        "master.rotation_speed" => s.settings.rotation_speed = value(input),
        "audio.enabled" => s.settings.audio.enabled = truthy(input),
        "audio.size" => s.settings.audio.size = value(input),
        "audio.rotate" => s.settings.audio.rotate = value(input),
        "audio.flash" => s.settings.audio.flash = value(input),
        "audio.color_on_beat" => s.settings.audio.color_on_beat = truthy(input),
        "transport.blackout" => s.armed = false,
        // Only reachable from the UI (external is false): arming goes
        // through the same path as the laser button.
        "transport.arm" => s.armed = truthy(input),
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
        "page.next" => s.cue_page = (s.cue_page + 1) % CATEGORIES.len(),
        "page.prev" => s.cue_page = (s.cue_page + CATEGORIES.len() - 1) % CATEGORIES.len(),
        other => {
            if let Some(n) = other.strip_prefix("page.").and_then(|n| n.parse::<usize>().ok()) {
                s.cue_page = n - 1;
            } else if let Some(cell) = other.strip_prefix("grid.") {
                let preset_id = grid_cell_preset(&s.presets, cell).ok_or_else(|| ControlError::Unknown(id.to_string()))?;
                play_preset(s, &preset_id);
            } else {
                return Err(ControlError::Unknown(id.to_string()));
            }
        }
    }
    Ok(())
}

/// Current value of a control, for LED feedback and UI sync. Triggers have
/// none.
pub fn current(s: &Shared, desc: &ControlDesc) -> Option<serde_json::Value> {
    use serde_json::json;
    Some(match desc.id.as_str() {
        "master.size" => json!(s.settings.scale),
        "master.brightness" => json!(s.settings.brightness),
        "master.rotation_speed" => json!(s.settings.rotation_speed),
        "audio.enabled" => json!(s.settings.audio.enabled),
        "audio.size" => json!(s.settings.audio.size),
        "audio.rotate" => json!(s.settings.audio.rotate),
        "audio.flash" => json!(s.settings.audio.flash),
        "audio.color_on_beat" => json!(s.settings.audio.color_on_beat),
        "transport.arm" => json!(s.armed),
        "tempo.bpm" => json!(s.tempo.bpm),
        _ => return None,
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

/// Play a cue: it sets the look, but keeps the operator's brightness, and
/// the operator's music settings unless the cue is built around the music.
pub fn play_preset(s: &mut Shared, id: &str) -> bool {
    let Some(mut settings) = s.presets.iter().find(|p| p.id == id).map(|p| p.settings.clone()) else {
        return false;
    };
    settings.brightness = s.settings.brightness;
    if !settings.audio.enabled {
        settings.audio = s.settings.audio.clone();
    }
    s.settings = settings;
    s.playlist = None;
    s.active_cue = Some(id.to_string());
    true
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
        };
        out.push_str(&format!("| `{}` | {} | {} | {kind} | {} |\n", d.id, d.label_fr, d.group, if d.external { "oui" } else { "non" }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn master_size_changes_the_look() {
        let mut s = shared();
        apply(&mut s, "master.size", ControlInput::Norm(1.0), true).unwrap();
        assert_eq!(s.settings.scale, 1.0);
        apply(&mut s, "live.size", ControlInput::Value(0.2), true).unwrap();
        assert_eq!(s.settings.scale, 0.2);
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
        assert!(!s.armed);
        s.armed = true;
        apply(&mut s, "transport.blackout", ControlInput::Value(1.0), true).unwrap();
        assert!(!s.armed);
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

    /// Keeps docs/controls.md in sync with the registry.
    #[test]
    fn write_controls_doc() {
        let md = markdown(&ControlRegistry::build(&catalog()));
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/controls.md");
        std::fs::write(path, md).unwrap();
    }
}
