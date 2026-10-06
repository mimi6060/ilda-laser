//! Stable control ids: one registry of every knob, button and trigger in
//! the studio, so the UI, MIDI controllers (APC40 & co), OSC and timeline
//! envelopes all drive exactly the same things through one entry point,
//! `apply`.
//!
//! Ids are lowercase, dot-separated and **never renamed** once shipped -
//! saved MIDI mappings refer to them. Add an alias instead of renaming.

use crate::cues::{self, ClickMode, CueDeck, BEAT_EVERY, BEAT_EVERY_LABELS, CLICK_MODES, CLICK_MODE_LABELS, QUANTIZE_LABELS, QUANTIZE_MODES};
use crate::transition::{LENGTHS, LENGTH_LABELS, TRANSITION_KINDS, TRANSITION_LABELS};
use crate::engine::Settings;
use crate::grid::CueGrid;
use crate::interlock::{ArmSource, DisarmReason};
use crate::layers::{Layer, LAYER_COUNT};
use crate::presets::{Preset, CATEGORIES};
use crate::live::{
    ChaseSpread, ColorOverride, ColorParams, LiveModifiers, PaletteMode, Rate, COLOR_MODE_LABELS, COLOR_STEPS_BEATS,
    COLOR_STEP_LABELS, MAX_PRISM, MIRRORS, MIRROR_LABELS, PALETTE_LABELS, ROT_PRESETS_FREE, ROT_PRESETS_SYNC, ROT_PRESET_LABELS,
    STROBE_DIVISIONS, STROBE_MAX_DUTY, STROBE_MAX_HZ, STROBE_MIN_DUTY, STROBE_MIN_HZ, STROBE_SYNC_LABELS,
};
use crate::tempo;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

/// Who last drove a control or started a cue (« En cours », T-423). For
/// display only: what a source may do is decided by `ArmSource` and the
/// `external` flag, never by this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSource {
    #[default]
    Ui,
    Keyboard,
    Midi,
    Dmx,
    Osc,
    Timeline,
    /// Audio triggers (T-240).
    Audio,
    /// Macros (T-425) and remotes (T-428) will set these.
    Macro,
    Remote,
    Api,
    /// OSC or Art-Net through the older network inputs (netin/).
    Network,
    System,
    /// The command line and its palette (T-424).
    Command,
}

impl ControlSource {
    pub fn label_fr(self) -> &'static str {
        match self {
            ControlSource::Ui => "Interface",
            ControlSource::Keyboard => "Clavier",
            ControlSource::Midi => "MIDI",
            ControlSource::Dmx => "DMX",
            ControlSource::Osc => "OSC",
            ControlSource::Timeline => "Timeline",
            ControlSource::Audio => "Audio",
            ControlSource::Macro => "Macro",
            ControlSource::Remote => "Télécommande",
            ControlSource::Api => "API",
            ControlSource::Network => "Réseau",
            ControlSource::System => "Système",
            ControlSource::Command => "Ligne de commande",
        }
    }

    /// From a safety-log source id (`ArmSource::id`); unknown or none = the UI.
    pub fn from_id(id: Option<&str>) -> Self {
        match id {
            Some("keyboard") => ControlSource::Keyboard,
            Some("midi") => ControlSource::Midi,
            Some("dmx") => ControlSource::Dmx,
            Some("osc") => ControlSource::Osc,
            Some("api") => ControlSource::Api,
            Some("network") => ControlSource::Network,
            Some("system") => ControlSource::System,
            Some("audio") => ControlSource::Audio,
            _ => ControlSource::Ui,
        }
    }
}

impl From<ArmSource> for ControlSource {
    fn from(src: ArmSource) -> Self {
        match src {
            ArmSource::Ui => ControlSource::Ui,
            ArmSource::Keyboard => ControlSource::Keyboard,
            ArmSource::Midi => ControlSource::Midi,
            ArmSource::Api => ControlSource::Api,
            ArmSource::System => ControlSource::System,
            ArmSource::Network => ControlSource::Network,
            ArmSource::Osc => ControlSource::Osc,
            ArmSource::Dmx => ControlSource::Dmx,
            ArmSource::Remote => ControlSource::Remote,
        }
    }
}

impl From<crate::external::SourceKind> for ControlSource {
    fn from(kind: crate::external::SourceKind) -> Self {
        match kind {
            crate::external::SourceKind::Dmx => ControlSource::Dmx,
            crate::external::SourceKind::Osc => ControlSource::Osc,
            crate::external::SourceKind::Timecode => ControlSource::Timeline,
            crate::external::SourceKind::Remote => ControlSource::Remote,
        }
    }
}

/// The last change of a control: who, and when (seconds since `epoch`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Touch {
    pub source: ControlSource,
    pub at_s: f64,
}

/// A controller's pad grid: 5 rows x 8 columns, the APC40 clip grid (and
/// the default cue grid, grid.rs). Pad n is the cell at row n / 8, column
/// n % 8 of the cue grid, whatever the grid's size.
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
    Ms,
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// The registry with the default cue grid (8x5, pages from the categories).
    pub fn build(presets: &[Preset]) -> Self {
        let mut grid = CueGrid::default();
        grid.resolve(presets);
        Self::build_for(presets, &grid)
    }

    /// `grid` must be resolved against `presets` (grid::refresh).
    pub fn build_for(presets: &[Preset], grid: &CueGrid) -> Self {
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
        // « Lissage » of the live moves, every level (T-144).
        let sm = crate::levels::Smoothing::default();
        add("smoothing.on".into(), "Lissage des mouvements".into(), "master", ControlKind::Toggle { default: false }, true);
        add("smoothing.inertia".into(), "Inertie du lissage".into(), "master", cont(crate::levels::MIN_INERTIA, crate::levels::MAX_INERTIA, sm.mass, Unit::None), true);
        // Live effects (T-142). The strobe only cuts light, before the
        // strobe limiter; « Noir » dims, it never disarms.
        let st = m.strobe;
        add("master.strobe.on".into(), "Stroboscope".into(), "master", ControlKind::Toggle { default: false }, true);
        add("master.strobe.hold".into(), "Flash strobe (maintenir)".into(), "master", ControlKind::Momentary, true);
        let hz = match st.rate { Rate::Hz(hz) => hz, Rate::Beats(_) => 8.0 };
        add("master.strobe.rate".into(), "Cadence du strobe".into(), "master", cont(STROBE_MIN_HZ, STROBE_MAX_HZ, hz, Unit::Hz), true);
        add(
            "master.strobe.sync".into(),
            "Cadence du strobe (temps)".into(),
            "master",
            ControlKind::Choice { options: STROBE_SYNC_LABELS.to_vec(), default: 0 },
            true,
        );
        add("master.strobe.duty".into(), "Rapport cyclique du strobe".into(), "master", cont(STROBE_MIN_DUTY, STROBE_MAX_DUTY, st.duty, Unit::Percent), true);
        add("master.trace".into(), "Points visibles".into(), "master", cont(0.0, 1.0, m.trace, Unit::Percent), true);
        add("master.dots".into(), "Pointillés".into(), "master", cont(0.0, 1.0, m.dots, Unit::Percent), true);
        add("master.scan".into(), "Vitesse de balayage".into(), "master", cont(crate::scan::MIN_MASTER_SCAN, crate::scan::MAX_MASTER_SCAN, m.scan, Unit::Percent), true);
        add("master.mirror".into(), "Miroir".into(), "master", ControlKind::Choice { options: MIRROR_LABELS.to_vec(), default: 0 }, true);
        add("master.prism".into(), "Prisme".into(), "master", cont(1.0, MAX_PRISM as f32, m.prism as f32, Unit::None), true);
        add("transport.freeze".into(), "Figer (maintenir)".into(), "transport", ControlKind::Momentary, true);
        add("transport.black_hold".into(), "Noir (maintenir, sans désarmer)".into(), "transport", ControlKind::Momentary, true);

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
        // The audio triggers' switch (T-240): memory only, off at start,
        // switched off by an e-stop. Turning it on never arms anything.
        add("audio.triggers.enabled".into(), "Automatisme audio (déclencheurs)".into(), "audio", ControlKind::Toggle { default: false }, true);

        // Both latch the emergency stop (T-251): a controller's blackout pad
        // is the stop button. Only the UI resets it, and nothing external arms.
        add("transport.blackout".into(), "Blackout".into(), "transport", ControlKind::Trigger, true);
        add("safety.estop".into(), "Arrêt d'urgence".into(), "safety", ControlKind::Trigger, true);
        // Hold-to-run pad (T-252): only lets an armed laser emit, never arms.
        add("safety.hold".into(), "Maintien pour émettre (homme mort)".into(), "safety", ControlKind::Momentary, true);
        add("transport.arm".into(), "Allumer le laser".into(), "transport", ControlKind::Toggle { default: false }, false);

        add("tempo.tap".into(), "Tap tempo".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.resync".into(), "Recaler sur le 1".into(), "tempo", ControlKind::Trigger, true);
        // T-235: resync on the detected "1" (*Temps 1 suggéré › Accepter*).
        add("tempo.downbeat_accept".into(), "Accepter le temps 1 suggéré".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.bpm".into(), "BPM".into(), "tempo", cont(tempo::MIN_BPM as f32, tempo::MAX_BPM as f32, 120.0, Unit::Bpm), true);
        add("tempo.nudge_up".into(), "Avancer la phase".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.nudge_down".into(), "Retarder la phase".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.double".into(), "Tempo ×2".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.half".into(), "Tempo ÷2".into(), "tempo", ControlKind::Trigger, true);
        // Tempo auto (T-234): the clock follows the audio detection; a tap
        // takes it back. Guider = taps that only guide the detection.
        add("tempo.auto".into(), "Tempo auto (suit la musique)".into(), "tempo", ControlKind::Toggle { default: false }, true);
        add("tempo.guide".into(), "Guider la détection (tap)".into(), "tempo", ControlKind::Trigger, true);
        add("tempo.new_track".into(), "Nouveau morceau".into(), "tempo", ControlKind::Trigger, true);
        // Décalage de sortie (T-246): the effects read the beat this far
        // ahead. Only what is drawn; never when a frame goes out.
        let max = tempo::MAX_OUTPUT_OFFSET_MS;
        add("tempo.output_offset".into(), "Décalage de sortie".into(), "tempo", cont(-max, max, 0.0, Unit::Ms), true);
        // Speed groups and the *Cadence* (T-419): timing only, never a
        // level or the gate; the strobe and flash limiters keep the last word.
        for n in 1..=crate::speed::SPEED_GROUPS {
            let name = crate::speed::GROUP_NAMES[n];
            add(format!("speed.{n}.mult"), format!("Vitesse {name} : multiple"), "tempo", ControlKind::Choice { options: crate::speed::MULT_LABELS.to_vec(), default: 2 }, true);
            add(format!("speed.{n}.free"), format!("Vitesse {name} : BPM libre"), "tempo", ControlKind::Toggle { default: false }, true);
            add(format!("speed.{n}.bpm"), format!("Vitesse {name} : BPM"), "tempo", cont(tempo::MIN_BPM as f32, tempo::MAX_BPM as f32, 120.0, Unit::Bpm), true);
        }
        add("master.rate".into(), "Cadence des fondus".into(), "tempo", cont(crate::speed::MIN_RATE, crate::speed::MAX_RATE, 1.0, Unit::Percent), true);

        add("timeline.play".into(), "Lecture de la timeline".into(), "timeline", ControlKind::Trigger, true);
        add("timeline.pause".into(), "Pause de la timeline".into(), "timeline", ControlKind::Trigger, true);
        add("timeline.stop".into(), "Arrêt de la timeline".into(), "timeline", ControlKind::Trigger, true);
        // One button for both (the APC40's Play): pause if playing, else play.
        add("timeline.toggle".into(), "Lecture / pause de la timeline".into(), "timeline", ControlKind::Trigger, true);
        add("timeline.loop".into(), "Boucle de la timeline".into(), "timeline", ControlKind::Toggle { default: false }, true);

        add("page.next".into(), "Page de cues suivante".into(), "page", ControlKind::Trigger, true);
        add("page.prev".into(), "Page de cues précédente".into(), "page", ControlKind::Trigger, true);
        // At least one per category, so saved mappings to page.N stay known
        // when the operator has fewer pages (selecting a missing page does nothing).
        for i in 0..grid.view().len().max(CATEGORIES.len()) {
            let name = grid.view().get(i).map_or_else(|| format!("{}", i + 1), |p| p.name.clone());
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
        // Transitions (T-158) and quantised launch (T-159), from the grid bar.
        let choice = |labels: &[&'static str], default: usize| ControlKind::Choice { options: labels.to_vec(), default };
        add("cue.transition".into(), "Transition entre cues".into(), "cue", choice(&TRANSITION_LABELS, 0), true);
        add("cue.transition_length".into(), "Durée de transition".into(), "cue", choice(&LENGTH_LABELS, 2), true);
        add("cue.quantize".into(), "Quantification du lancement".into(), "cue", choice(&QUANTIZE_LABELS, 0), true);
        add("cue.beat_mode".into(), "Mode beat (change de cue tous les N temps)".into(), "cue", choice(&BEAT_EVERY_LABELS, 0), true);
        // « Tout arrêter » (T-423): unprotected cues, the timeline and the
        // playlist. Never the arm state, the modulators or the masters.
        add("running.stop_all".into(), "Tout arrêter".into(), "running", ControlKind::Trigger, true);
        // The loaded cue list (T-410): playback only, so controllers may
        // drive it; none of them arms, and Go waits for an e-stop reset.
        add("list.go".into(), "Liste de cues : Go".into(), "list", ControlKind::Trigger, true);
        add("list.back".into(), "Liste de cues : Retour".into(), "list", ControlKind::Trigger, true);
        add("list.pause".into(), "Liste de cues : Pause".into(), "list", ControlKind::Trigger, true);
        add("list.top".into(), "Liste de cues : Début".into(), "list", ControlKind::Trigger, true);
        // Macros (T-425): Go for the ones waiting, stop them all; each
        // macro's own `macro.<id>.go` comes with the list (`add_macros`).
        // Playback only: a macro never arms.
        add("macro.go".into(), "Go macro".into(), "macro", ControlKind::Trigger, true);
        add("macro.stop_all".into(), "Arrêter les macros".into(), "macro", ControlKind::Trigger, true);

        // The FX grid (T-146): a cell plays like a cue pad (down / up), a
        // drop on each press; all playback, none of them arms.
        for line in 1..=crate::fx::LINES {
            for cell in 1..=crate::fx::CELLS {
                add(format!("fx.{line}.{cell}"), format!("Effet {line}.{cell}"), "fx", ControlKind::Momentary, true);
            }
            add(format!("fx.{line}.action"), format!("Action des effets, ligne {line}"), "fx", cont(0.0, 1.0, 1.0, Unit::Percent), true);
        }
        add("fx.stop".into(), "Arrêter les effets".into(), "fx", ControlKind::Trigger, true);
        add("fx.dest".into(), "Destination des effets".into(), "fx", choice(&crate::fx::DEST_LABELS, 0), true);
        add("fx.mode".into(), "Clic des effets".into(), "fx", choice(&crate::fx::CLICK_LABELS, 0), true);

        for n in 1..=LAYER_COUNT {
            add(format!("layer.{n}.dimmer"), format!("Gradateur calque {n}"), "layer", cont(0.0, 1.0, 1.0, Unit::Percent), true);
            add(format!("layer.{n}.mute"), format!("Muet calque {n}"), "layer", ControlKind::Toggle { default: false }, true);
            add(format!("layer.{n}.solo"), format!("Solo calque {n}"), "layer", ControlKind::Toggle { default: false }, true);
            add(format!("layer.{n}.clear"), format!("Vider le calque {n}"), "layer", ControlKind::Trigger, true);
        }

        // Group masters (T-420): inhibitive dimmers, one per zone group
        // (the 16 first), 1 by default; they only ever dim, never arm.
        // « Tout à 100 % » puts them all back (`master.reset` doesn't).
        for n in 1..=crate::projection::GROUP_MASTER_CONTROLS {
            add(format!("group.{n}.master"), format!("Maître groupe {n}"), "group", cont(0.0, 1.0, 1.0, Unit::Percent), true);
        }
        add("group.all_full".into(), "Maîtres de groupe : tout à 100 %".into(), "group", ControlKind::Trigger, true);

        // The executor bank (T-418): every page's handles, the page shown
        // and its `exec.cur.*` aliases. Playback only: none of them arms,
        // and their dimming stays before the safety stage.
        for (id, label, handle) in crate::executors::control_ids() {
            let kind = match handle {
                crate::executors::Handle::Fader => cont(0.0, 1.0, 0.0, Unit::Percent),
                crate::executors::Handle::Button(_) => ControlKind::Momentary,
            };
            add(id, label, "exec", kind, true);
        }
        add("exec.page.next".into(), "Page d'exécuteurs suivante".into(), "exec", ControlKind::Trigger, true);
        add("exec.page.prev".into(), "Page d'exécuteurs précédente".into(), "exec", ControlKind::Trigger, true);
        for p in 1..=crate::executors::EXEC_MAX_PAGES {
            add(format!("exec.page.{p}"), format!("Page d'exécuteurs {p}"), "exec", ControlKind::Trigger, true);
        }

        // Momentary: pad down (value 1) presses, pad up (value 0) releases -
        // what flash and solo need. Toggle/restart cues ignore the release.
        let names: HashMap<&str, &str> = presets.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
        let cols = grid.cols as usize;
        for (page, view) in grid.view().iter().enumerate() {
            for (i, cue) in view.cells.iter().enumerate() {
                let Some(name) = cue.as_deref().and_then(|id| names.get(id)) else { continue };
                let (row, col) = (i / cols + 1, i % cols + 1);
                add(format!("grid.{}.{row}.{col}", page + 1), name.to_string(), "grid", ControlKind::Momentary, true);
            }
        }

        let by_id = descs.iter().enumerate().map(|(i, d)| (d.id.clone(), i)).collect();
        Self { descs, by_id }
    }

    pub fn list(&self) -> &[ControlDesc] {
        &self.descs
    }

    /// `macro.<id>.go` for each macro (T-425), labelled with its name: a
    /// trigger controllers may use (it never arms).
    pub fn add_macros(&mut self, macros: &[crate::macros::Macro]) {
        for m in macros {
            let id = format!("macro.{}.go", m.id);
            if self.by_id.contains_key(&id) {
                continue;
            }
            self.by_id.insert(id.clone(), self.descs.len());
            self.descs.push(ControlDesc { id, label_fr: m.name.clone(), group: "macro", kind: ControlKind::Trigger, external: true });
        }
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
/// for MIDI, false for the on-screen UI.
pub fn apply(s: &mut Shared, id: &str, input: ControlInput, from_external: bool) -> Result<(), ControlError> {
    apply_as(s, id, input, if from_external { ArmSource::Midi } else { ArmSource::Ui })
}

/// `apply` for a given source. Everything but the UI, keyboard and API is
/// external (MIDI, OSC/Art-Net): only controls marked `external`, and the
/// source is what the safety log records for a stop or a cue.
pub fn apply_as(s: &mut Shared, id: &str, input: ControlInput, src: ArmSource) -> Result<(), ControlError> {
    let from_external = !matches!(src, ArmSource::Ui | ArmSource::Keyboard | ArmSource::Api);
    apply_checked(s, id, input, from_external, src, src.into())
}

/// The page's generic control route (`POST /api/control`): checked like a
/// controller (nothing that arms), but recorded as `source` - « Interface ».
pub fn apply_from(s: &mut Shared, id: &str, input: ControlInput, source: ControlSource) -> Result<(), ControlError> {
    apply_checked(s, id, input, true, ArmSource::Ui, source)
}

/// `from_external` decides what may be driven, `src` is what the safety
/// log records, `source` what « En cours » shows.
fn apply_checked(s: &mut Shared, id: &str, input: ControlInput, from_external: bool, src: ArmSource, source: ControlSource) -> Result<(), ControlError> {
    let r = apply_inner(s, id, input, from_external, src, source);
    if r.is_ok() {
        if let Some(desc) = s.controls.get(id) {
            let touch = Touch { source, at_s: s.now_s() };
            s.touches.insert(desc.id.clone(), touch);
        }
    }
    r
}

fn apply_inner(s: &mut Shared, id: &str, input: ControlInput, from_external: bool, src: ArmSource, source: ControlSource) -> Result<(), ControlError> {
    let Some(desc) = s.controls.get(id).cloned() else { return apply_level(s, id, input, from_external) };
    if from_external && !desc.external {
        return Err(ControlError::Refused("ce contrôle ne peut pas être piloté depuis un contrôleur externe"));
    }
    // The master live modifiers: the same code as the cue and layer levels.
    if let Some(param) = desc.id.strip_prefix("master.") {
        if modify_live(&mut s.live, param, &desc.kind, input) {
            s.live_dirty = true;
            return Ok(());
        }
    }
    let value = |i| match desc.kind {
        ControlKind::Continuous { min, max, .. } => native(min, max, i),
        _ => 0.0,
    };

    match desc.id.as_str() {
        "look.size" => s.settings.scale = value(input),
        "look.brightness" => s.settings.brightness = value(input),
        "look.rotation_speed" => s.settings.rotation_speed = value(input),
        "transport.freeze" => set_live(s, |m| m.freeze = truthy(input)),
        "transport.black_hold" => set_live(s, |m| m.black = truthy(input)),
        "smoothing.on" => set_smoothing(s, |sm| sm.enabled = truthy(input)),
        "smoothing.inertia" => set_smoothing(s, |sm| sm.set_inertia(value(input))),
        "audio.enabled" => s.settings.audio.enabled = truthy(input),
        "audio.size" => s.settings.audio.size = value(input),
        "audio.rotate" => s.settings.audio.rotate = value(input),
        "audio.flash" => s.settings.audio.flash = value(input),
        "audio.color_on_beat" => s.settings.audio.color_on_beat = truthy(input),
        "audio.triggers.enabled" => crate::audio::triggers::set_enabled(s, truthy(input), src).map_err(ControlError::Refused)?,
        "transport.blackout" | "safety.estop" => s.emergency_stop(src),
        "safety.hold" => s.presence.set_midi_hold(truthy(input)),
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
            // The multiple speed groups come back on the "one" too (T-419).
            let (bar_beat, t_eff) = (s.tempo.beat_at(t), s.tempo.effect_t(t));
            s.speed.resync(bar_beat, t_eff, s.tempo.beats_per_bar);
            s.speed_dirty = true;
        }
        "tempo.downbeat_accept" => {
            if !s.tempo.accept_downbeat() {
                return Err(ControlError::Refused("aucun temps 1 suggéré"));
            }
        }
        "tempo.bpm" => {
            let t = s.now_s();
            s.tempo.set_bpm_manual(value(input) as f64, t);
        }
        "tempo.auto" => s.tempo.set_auto(truthy(input)),
        "tempo.guide" => {
            let t = s.now_s();
            if let Some(bpm) = s.tempo.guide_tap(t) {
                s.audio_in.set_guide(Some(bpm));
            }
        }
        "tempo.new_track" => s.tempo_new_track(),
        "tempo.output_offset" => {
            crate::latency::set_offset(s, value(input) as f64).map_err(ControlError::Refused)?;
        }
        "tempo.nudge_up" => s.tempo.nudge(1.0 / 32.0),
        "tempo.nudge_down" => s.tempo.nudge(-1.0 / 32.0),
        "tempo.double" | "tempo.half" => {
            let (t, factor) = (s.now_s(), if desc.id == "tempo.double" { 2.0 } else { 0.5 });
            let bpm = s.tempo.bpm * factor;
            s.tempo.set_bpm_manual(bpm, t);
        }
        "timeline.play" => timeline_play(s).map_err(ControlError::Refused)?,
        "timeline.pause" => {
            let c = s.timeline_clock();
            s.timeline.pause(&c)
        }
        "timeline.stop" => s.timeline.stop(),
        "timeline.toggle" => {
            if s.timeline.is_playing() {
                let c = s.timeline_clock();
                s.timeline.pause(&c)
            } else {
                timeline_play(s).map_err(ControlError::Refused)?
            }
        }
        "timeline.loop" => s.timeline.loop_on = truthy(input),
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
        "cue.stop_all" => stop_cues(s),
        "running.stop_all" => stop_running(s),
        "list.go" => crate::cuelist::go(s, source).map_err(ControlError::Refused)?,
        "list.back" => crate::cuelist::back(s, source).map_err(ControlError::Refused)?,
        "list.pause" => crate::cuelist::pause(s, source).map_err(ControlError::Refused)?,
        "list.top" => crate::cuelist::top(s, source).map_err(ControlError::Refused)?,
        "macro.go" => {
            crate::macros::go(s);
        }
        "macro.stop_all" => crate::macros::stop_all(s),
        "cue.transition" => {
            s.deck.transition.kind = TRANSITION_KINDS[choice_index(&desc.kind, input)];
            s.deck.save();
        }
        "cue.transition_length" => {
            s.deck.transition.length = LENGTHS[choice_index(&desc.kind, input)];
            s.deck.save();
        }
        "cue.quantize" => {
            s.deck.quantize = QUANTIZE_MODES[choice_index(&desc.kind, input)];
            if s.deck.quantize == cues::Quantize::Off {
                // Nothing waits for a boundary any more: what waited starts now.
                let now = s.now_s();
                let at = deck_clock(s, now);
                for p in &mut s.deck.pending {
                    p.at_beat = at.beat;
                }
            }
            s.deck.save();
        }
        "cue.beat_mode" => {
            s.deck.beat_every = BEAT_EVERY[choice_index(&desc.kind, input)];
            s.deck.save();
        }
        "group.all_full" => {
            let n = s.projection.get().groups.len();
            s.projection.set_masters(&vec![1.0; n]);
            s.groups_dirty = true;
        }
        "fx.stop" => s.fx.stop_all(),
        "fx.dest" => s.fx.set_dest(crate::fx::DESTS[choice_index(&desc.kind, input)]),
        "fx.mode" => s.fx.set_click(crate::fx::CLICKS[choice_index(&desc.kind, input)]),
        "master.rate" => {
            let v = finite(input)?;
            s.speed.set_rate(native(crate::speed::MIN_RATE, crate::speed::MAX_RATE, v)).map_err(ControlError::Refused)?;
            s.speed_dirty = true;
        }
        "page.next" => s.cue_page = (s.cue_page + 1) % s.grid.page_count(),
        "page.prev" => s.cue_page = (s.cue_page + s.grid.page_count() - 1) % s.grid.page_count(),
        other => {
            if let Some((line, param)) = fx_control(other) {
                match param {
                    FxParam::Cell(cell) => {
                        let quantize = s.deck.quantize;
                        s.fx.press(line, cell, truthy(input), quantize);
                    }
                    FxParam::Action => s.fx.set_action(line, value(input)),
                }
                return Ok(());
            }
            if let Some((n, param)) = speed_control(other) {
                let input = finite(input)?;
                let (beat, t, bpb) = s.speed_clock();
                let r = match param {
                    "mult" => {
                        let mult = crate::speed::SPEED_MULTS[choice_index(&desc.kind, input)];
                        s.speed.set_mode(n, crate::speed::SpeedMode::Multiple { mult }, beat, t, bpb)
                    }
                    "free" => s.speed.set_free(n, truthy(input), s.tempo.bpm, beat, t, bpb),
                    _ => s.speed.set_mode(n, crate::speed::SpeedMode::FreeBpm { bpm: value(input) as f64 }, beat, t, bpb),
                };
                r.map_err(ControlError::Refused)?;
                s.speed_dirty = true;
                return Ok(());
            }
            if let Some(r) = crate::executors::apply(s, other, input, Some(src.id()), source) {
                return r.map_err(ControlError::Refused);
            }
            if let Some(n) = group_master_control(other) {
                if !s.projection.set_master(n, value(input)) {
                    return Err(ControlError::Refused("ce groupe de zones n'existe pas"));
                }
                s.groups_dirty = true;
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
            if let Some(m) = other.strip_prefix("macro.").and_then(|r| r.strip_suffix(".go")) {
                return crate::macros::start(s, m, source).map_err(ControlError::Refused);
            }
            if let Some(n) = other.strip_prefix("page.").and_then(|n| n.parse::<usize>().ok()) {
                if (1..=s.grid.page_count()).contains(&n) {
                    s.cue_page = n - 1;
                }
            } else if let Some(cell) = other.strip_prefix("grid.") {
                let preset_id = grid_cell_preset(&s.grid, cell).ok_or_else(|| ControlError::Unknown(id.to_string()))?;
                press_cue_as(s, &preset_id, None, truthy(input), Some(src.id()), source);
            } else {
                return Err(ControlError::Unknown(id.to_string()));
            }
        }
    }
    if desc.id.starts_with("look.") || (desc.id.starts_with("audio.") && desc.id != "audio.triggers.enabled") {
        s.settings_rev += 1;
    }
    Ok(())
}

enum FxParam {
    Cell(usize),
    Action,
}

/// "fx.<line>.<cell>" / "fx.<line>.action" (1-based) → (line, param),
/// 0-based, for registered ids only.
fn fx_control(id: &str) -> Option<(usize, FxParam)> {
    let (line, rest) = id.strip_prefix("fx.")?.split_once('.')?;
    let line = line.parse::<usize>().ok()?.checked_sub(1)?;
    let param = match rest {
        "action" => FxParam::Action,
        cell => FxParam::Cell(cell.parse::<usize>().ok()?.checked_sub(1)?),
    };
    Some((line, param))
}

/// "speed.<n>.<param>" → (n, param), for registered ids only (T-419).
fn speed_control(id: &str) -> Option<(u8, &str)> {
    let (n, param) = id.strip_prefix("speed.")?.split_once('.')?;
    Some((n.parse().ok()?, param))
}

/// An input that is a number (T-419 refuses NaN and infinities rather
/// than clamping them to an end of the range).
fn finite(input: ControlInput) -> Result<ControlInput, ControlError> {
    match input {
        ControlInput::Value(v) | ControlInput::Norm(v) if !v.is_finite() => Err(ControlError::Refused("valeur invalide")),
        i => Ok(i),
    }
}

/// "group.<n>.master" → n (1-based), for registered ids only.
fn group_master_control(id: &str) -> Option<usize> {
    id.strip_prefix("group.")?.strip_suffix(".master")?.parse().ok()
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

fn set_smoothing(s: &mut Shared, f: impl FnOnce(&mut crate::levels::Smoothing)) {
    f(&mut s.mixer.smoothing);
    s.mixer.smoothing.sanitize();
    s.mixer_dirty = true;
}

/// One live modifier `param` (a `master.<param>` id without its prefix)
/// changed on `m`: the master's, a layer's or a cue's (T-144). `kind` is
/// the master control's. False for a param that is not a modifier.
pub(crate) fn modify_live(m: &mut LiveModifiers, param: &str, kind: &ControlKind, input: ControlInput) -> bool {
    let value = |i| match *kind {
        ControlKind::Continuous { min, max, .. } => native(min, max, i),
        _ => 0.0,
    };
    match param {
        "brightness" => m.brightness = value(input),
        "size" => m.size = value(input),
        "size_x" => m.size_x = value(input),
        "size_y" => m.size_y = value(input),
        "pos_x" => m.pos_x = value(input),
        "pos_y" => m.pos_y = value(input),
        "rot.preset" => {
            let table = if m.rot_sync { ROT_PRESETS_SYNC } else { ROT_PRESETS_FREE };
            m.rot_speed[2] = table[choice_index(kind, input)];
        }
        "rot.sync" => {
            // Keep the same step when switching modes (Moyen stays Moyen).
            let on = truthy(input);
            if on != m.rot_sync {
                let (from, to) = if on { (ROT_PRESETS_FREE, ROT_PRESETS_SYNC) } else { (ROT_PRESETS_SYNC, ROT_PRESETS_FREE) };
                if let Some(i) = from.iter().position(|&v| v == m.rot_speed[2]) {
                    m.rot_speed[2] = to[i];
                }
                m.rot_sync = on;
            }
        }
        "rot.reverse" => m.rot_reverse = truthy(input),
        "perspective" => m.perspective = value(input),
        "speed" => m.speed = value(input),
        "reset" => *m = LiveModifiers::default(),
        "strobe.on" => m.strobe.on = truthy(input),
        "strobe.hold" => m.strobe.hold = truthy(input),
        // The Hz fader frees the strobe; « Cadence (temps) » syncs it (0 = free).
        "strobe.rate" => m.strobe.rate = Rate::Hz(value(input)),
        "strobe.sync" => {
            m.strobe.rate = match choice_index(kind, input) {
                0 => Rate::Hz(match m.strobe.rate() {
                    Rate::Hz(hz) => hz,
                    Rate::Beats(_) => 8.0,
                }),
                i => Rate::Beats(1.0 / STROBE_DIVISIONS[i - 1]),
            }
        }
        "strobe.duty" => m.strobe.duty = value(input),
        "trace" => m.trace = value(input),
        "dots" => m.dots = value(input),
        "scan" => m.scan = value(input),
        "mirror" => m.mirror = MIRRORS[choice_index(kind, input)],
        "prism" => m.prism = value(input).round() as u8,
        "color.mode" => m.color = m.color_params.build(choice_index(kind, input)),
        "color.hue" => set_color(m, |c| c.hue = value(input)),
        "color.palette" => {
            let index = choice_index(kind, input);
            set_color(m, |c| c.palette = index)
        }
        "color.palette_mode" => {
            let mode = [PaletteMode::Nearest, PaletteMode::Step][choice_index(kind, input)];
            set_color(m, |c| c.palette_mode = mode)
        }
        "color.offset" => set_color(m, |c| c.offset = value(input).round() as usize),
        "color.rate" => set_color_rate(m, Rate::Beats(COLOR_STEPS_BEATS[choice_index(kind, input)])),
        "color.rate_hz" => set_color_rate(m, Rate::Hz(value(input))),
        "color.spread" => set_color(m, |c| c.spread = value(input)),
        "color.chase_spread" => {
            let spread = [ChaseSpread::Whole, ChaseSpread::Stroke, ChaseSpread::Point][choice_index(kind, input)];
            set_color(m, |c| c.chase_spread = spread)
        }
        "color.red" | "color.green" | "color.blue" => {
            let channel = ["color.red", "color.green", "color.blue"].iter().position(|&i| i == param).unwrap_or(0);
            let v = (value(input) * 255.0).round() as u8;
            set_color(m, |c| c.rgb[channel] = v)
        }
        other => {
            // rot_<axis>.angle / rot_<axis>.speed
            let Some(rest) = other.strip_prefix("rot_") else { return false };
            let Some(axis) = ["x", "y", "z"].iter().position(|a| rest.starts_with(a)) else { return false };
            match &rest[1..] {
                ".angle" => m.rot_angle[axis] = value(input),
                ".speed" => m.rot_speed[axis] = value(input),
                _ => return false,
            }
        }
    }
    true
}

/// A cue or layer level control (T-144, `levels::parse`): the master
/// control's range and behaviour on that level's modifiers. A cue that is
/// not on show is refused (its working copy only lives while it plays).
fn apply_level(s: &mut Shared, id: &str, input: ControlInput, from_external: bool) -> Result<(), ControlError> {
    let Some((level, param)) = crate::levels::parse(id) else { return Err(ControlError::Unknown(id.to_string())) };
    let desc = s.controls.get(&format!("master.{param}")).cloned().ok_or_else(|| ControlError::Unknown(id.to_string()))?;
    if from_external && !desc.external {
        return Err(ControlError::Refused("ce contrôle ne peut pas être piloté depuis un contrôleur externe"));
    }
    match level {
        crate::levels::Level::Layer(n) => {
            modify_live(&mut s.mixer.layer_mut(n).modifiers, param, &desc.kind, input);
            s.mixer_dirty = true;
        }
        crate::levels::Level::Cue(cue) => {
            if !s.presets.iter().any(|p| p.id == cue) {
                return Err(ControlError::Unknown(id.to_string()));
            }
            let m = crate::levels::cue_mods_mut(s, cue).ok_or(ControlError::Refused("ce cue ne joue pas"))?;
            modify_live(m, param, &desc.kind, input);
        }
    }
    Ok(())
}

/// Change a colour setting; the active colour mode picks it up at once. It
/// never switches mode by itself (that's `master.color.mode`).
fn set_color(m: &mut LiveModifiers, f: impl FnOnce(&mut ColorParams)) {
    f(&mut m.color_params);
    m.color = m.color_params.build(m.color.mode_index());
}

/// « Pas » drives the mode that uses a rate (rainbow or chase); in the
/// other modes it presets both.
fn set_color_rate(m: &mut LiveModifiers, rate: Rate) {
    let mode = m.color.mode_index();
    set_color(m, |c| match mode {
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
        "master.strobe.on" => json!(s.live.strobe.on),
        "master.strobe.hold" => json!(s.live.strobe.hold),
        "master.strobe.rate" => match s.live.strobe.rate() {
            Rate::Hz(hz) => json!(hz),
            Rate::Beats(_) => serde_json::Value::Null,
        },
        "master.strobe.sync" => json!(s.live.strobe.sync_index()),
        "master.strobe.duty" => json!(s.live.strobe.duty()),
        "master.trace" => json!(s.live.trace),
        "master.dots" => json!(s.live.dots),
        "master.scan" => json!(s.live.scan),
        "master.mirror" => json!(MIRRORS.iter().position(|&m| m == s.live.mirror).unwrap_or(0)),
        "master.prism" => json!(s.live.prism),
        "transport.freeze" => json!(s.live.freeze),
        "transport.black_hold" => json!(s.live.black),
        "smoothing.on" => json!(s.mixer.smoothing.enabled),
        "smoothing.inertia" => json!(s.mixer.smoothing.mass),
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
        "audio.triggers.enabled" => json!(s.triggers.enabled()),
        "transport.arm" => json!(s.gate.is_armed() && !s.estop.is_latched()),
        "safety.hold" => json!(s.presence.status().midi_hold),
        "tempo.bpm" => json!(s.tempo.bpm),
        "tempo.auto" => json!(s.tempo.source == tempo::TempoSource::Audio),
        "tempo.output_offset" => json!(s.tempo.output_offset_ms()),
        "master.rate" => json!(s.speed.rate()),
        id if id.starts_with("speed.") => {
            let (n, param) = speed_control(id)?;
            let mode = s.speed.target(n)?;
            match (param, mode) {
                ("mult", crate::speed::SpeedMode::Multiple { mult }) => json!(crate::speed::SPEED_MULTS.iter().position(|&m| m == mult)),
                ("mult", _) => serde_json::Value::Null,
                ("free", mode) => json!(matches!(mode, crate::speed::SpeedMode::FreeBpm { .. })),
                (_, _) => json!(s.speed.bpm(n, s.tempo.bpm)),
            }
        }
        "timeline.play" => json!(s.timeline.is_playing()),
        "timeline.pause" => json!(s.timeline.transport == crate::timeline::Transport::Paused),
        "timeline.loop" => json!(s.timeline.loop_on),
        "cue.mode" => json!(CLICK_MODES.iter().position(|&m| m == s.deck.click_mode).unwrap_or(0)),
        "cue.multi" => json!(s.deck.multi),
        "cue.max_active" => json!(s.deck.max_active),
        "cue.transition" => json!(TRANSITION_KINDS.iter().position(|&k| k == s.deck.transition.kind).unwrap_or(0)),
        "cue.transition_length" => json!(LENGTHS.iter().position(|&l| l == s.deck.transition.length)),
        "cue.quantize" => json!(QUANTIZE_MODES.iter().position(|&q| q == s.deck.quantize).unwrap_or(0)),
        "cue.beat_mode" => json!(BEAT_EVERY.iter().position(|&n| n == s.deck.beat_every).unwrap_or(0)),
        id if id.starts_with("master.rot_") => {
            // master.rot_<axis>.angle / .speed, read back for the « Direct » panel.
            let rest = &id["master.rot_".len()..];
            let axis = ["x", "y", "z"].iter().position(|a| rest.starts_with(a))?;
            match &rest[1..] {
                ".angle" => json!(s.live.rot_angle[axis]),
                ".speed" => json!(s.live.rot_speed[axis]),
                _ => return None,
            }
        }
        "fx.dest" => json!(crate::fx::DESTS.iter().position(|&d| d == s.fx.grid().dest).unwrap_or(0)),
        "fx.mode" => json!(crate::fx::CLICKS.iter().position(|&c| c == s.fx.grid().click).unwrap_or(0)),
        id if id.starts_with("fx.") => match fx_control(id)? {
            // Cells light up while they play (LED feedback).
            (line, FxParam::Cell(cell)) => json!(s.fx.active(line) == Some(cell)),
            (line, FxParam::Action) => json!(s.fx.action(line)),
        },
        id if id.starts_with("exec.") => crate::executors::current(s, id)?,
        id if id.starts_with("group.") => json!(s.projection.master(group_master_control(id)?)?),
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
            // Grid cells light up while their cue (or show) plays (LED feedback).
            let cue = grid_cell_preset(&s.grid, id.strip_prefix("grid.")?)?;
            json!(s.deck.active.iter().any(|a| a.cue == cue) || show_cue_playing(s, &cue))
        }
    })
}

/// "<page>.<row>.<col>" (1-based, in the cue grid's current size) → the
/// preset in that cell.
pub(crate) fn grid_cell_preset(grid: &CueGrid, cell: &str) -> Option<String> {
    let parts: Vec<usize> = cell.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [page, row, col] = parts[..] else { return None };
    grid.cell(page, row, col).map(str::to_string)
}

/// Controller pad `slot` (0..40, row-major over 8 columns) → the cell id
/// `grid.<page>.<row>.<col>` of page `page` (0-based), if the grid has
/// that cell.
pub fn pad_cell(grid: &CueGrid, page: usize, slot: usize) -> Option<String> {
    let (row, col) = (slot / GRID_COLS + 1, slot % GRID_COLS + 1);
    (slot < GRID_ROWS * GRID_COLS && row <= grid.rows as usize && col <= grid.cols as usize).then(|| format!("grid.{}.{row}.{col}", page + 1))
}

/// A cue's look as it starts: the preset, but with the operator's
/// brightness, and the operator's music settings unless the cue is built
/// around the music.
pub(crate) fn cue_settings(s: &Shared, id: &str) -> Option<Settings> {
    let mut settings = s.deck.with_program(id, s.presets.iter().find(|p| p.id == id)?.settings.clone());
    settings.brightness = s.settings.brightness;
    if !settings.audio.enabled {
        settings.audio = s.settings.audio.clone();
    }
    Some(settings)
}

/// Play a cue from the start, whatever its click mode (the old « play »
/// action, kept for `/api/presets/play`).
pub fn play_preset(s: &mut Shared, id: &str) -> bool {
    press_cue_from(s, id, Some(ClickMode::Restart), true, Some("ui"))
}

/// Start the loaded show. Like a latched cue, it takes over from the
/// scene/playlist; cues played by hand keep playing on top. Refused while
/// the emergency stop is latched. Never touches the arm state.
pub fn timeline_play(s: &mut Shared) -> Result<(), &'static str> {
    if s.estop.is_latched() {
        return Err("arrêt d'urgence actif : réinitialiser avant de lancer la timeline");
    }
    let c = s.timeline_clock();
    if !s.timeline.play(&c) {
        return Err("aucun show chargé");
    }
    s.playlist = None;
    s.look_on = false;
    if s.gate.is_armed() {
        let name = s.timeline.show.as_ref().map(|sh| sh.name.clone()).unwrap_or_default();
        s.gate.log().record("show", None, json!({ "name": name }));
    }
    Ok(())
}

/// Whether the grid cell of `cue` holds a show that is playing now.
pub(crate) fn show_cue_playing(s: &Shared, cue: &str) -> bool {
    let slot = s.deck.slot(cue);
    let loaded = s.timeline.show.as_ref().map(|sh| sh.name.as_str());
    slot.show.is_some() && slot.show.as_deref() == loaded && s.timeline.is_playing()
}

/// A grid cell holding a show (« cue de type show »): a press loads and
/// plays it, a press while it plays stops it; releases are ignored.
fn press_show_cue(s: &mut Shared, cue: &str, show: &str) {
    if show_cue_playing(s, cue) {
        s.timeline.stop();
        return;
    }
    match s.shows.load(show) {
        Ok(loaded) => {
            s.timeline.load(loaded);
            if let Err(why) = timeline_play(s) {
                log::warn!("show {show}: {why}");
            }
        }
        Err(e) => log::warn!("show {show}: {e:#}"),
    }
}

#[cfg(test)]
pub fn press_cue(s: &mut Shared, id: &str, mode: Option<ClickMode>, down: bool) -> bool {
    press_cue_from(s, id, mode, down, None)
}

/// A cue's key, pad or button went down (`down`) or up. `mode` overrides
/// the cue's click mode (Shift + letter flashes). False if no such cue.
/// `src` (`ui`, `midi`) is for the safety log: a cue started while armed
/// is logged (T-259).
pub fn press_cue_from(s: &mut Shared, id: &str, mode: Option<ClickMode>, down: bool, src: Option<&'static str>) -> bool {
    let now = s.now_s();
    press_cue_full(s, id, mode, down, src, ControlSource::from_id(src), now)
}

/// `press_cue_from`, crediting the cues it starts to `source` (« En cours »).
pub fn press_cue_as(s: &mut Shared, id: &str, mode: Option<ClickMode>, down: bool, src: Option<&'static str>, source: ControlSource) -> bool {
    let now = s.now_s();
    press_cue_full(s, id, mode, down, src, source, now)
}

/// `press_cue_from` at `t` on the studio clock: the engine's frame time,
/// for the audio triggers (T-240), so a press made on a quantised
/// boundary is on it for the deck too (no second wait).
pub fn press_cue_at(s: &mut Shared, id: &str, mode: Option<ClickMode>, down: bool, src: Option<&'static str>, t: f64) -> bool {
    press_cue_full(s, id, mode, down, src, ControlSource::from_id(src), t)
}

/// « Envoyer » (T-274): the prepared cue `id` pressed with the prepared
/// look `look` instead of its own, through the same deck path as any
/// press (quantise, transitions, logging). False if no such cue, or if
/// its cell holds a show (never prepared).
pub fn send_prepared(s: &mut Shared, id: &str, mode: Option<ClickMode>, look: Settings, src: Option<&'static str>) -> bool {
    if s.deck.slot(id).show.is_some() {
        return false;
    }
    let now = s.now_s();
    press_cue_with(s, id, mode, true, src, ControlSource::from_id(src), now, Some(look))
}

fn press_cue_full(s: &mut Shared, id: &str, mode: Option<ClickMode>, down: bool, src: Option<&'static str>, source: ControlSource, t: f64) -> bool {
    press_cue_with(s, id, mode, down, src, source, t, None)
}

#[allow(clippy::too_many_arguments)]
fn press_cue_with(s: &mut Shared, id: &str, mode: Option<ClickMode>, down: bool, src: Option<&'static str>, source: ControlSource, t: f64, look: Option<Settings>) -> bool {
    if !s.presets.iter().any(|p| p.id == id) {
        return false;
    }
    if let Some(show) = s.deck.slot(id).show {
        if down {
            press_show_cue(s, id, &show);
        }
        return true;
    }
    let Some(settings) = look.or_else(|| cue_settings(s, id)) else { return false };
    let first_new = s.deck.next_id();
    with_deck_at(s, deck_clock(s, t), |deck, at| {
        if down {
            deck.press(id, mode, at, || settings);
        } else {
            deck.release(id, at);
        }
    });
    for a in s.deck.active.iter_mut().filter(|a| a.id >= first_new) {
        a.source = source;
    }
    // A quantised launch (T-159) keeps its source until it starts.
    for p in s.deck.pending.iter_mut().filter(|p| p.cue == id) {
        p.source = source;
    }
    started(s, first_new, mode, src);
    true
}

/// Stops every instance of cue `id` (a user cue just deleted, T-405),
/// keeping the live look in step like any other deck change.
pub fn stop_cue(s: &mut Shared, id: &str) {
    if s.deck.active.iter().any(|a| a.cue == id) {
        with_deck(s, |deck, _| deck.forget(id));
    } else {
        s.deck.forget(id);
    }
}

/// Stops the one playing instance `instance` (« En cours » › Arrêter),
/// protected or not. False if it isn't playing any more.
pub fn stop_instance(s: &mut Shared, instance: u64) -> bool {
    // Fading out (T-158): its fade ends now.
    if s.deck.ending.iter().any(|e| e.cue.id == instance) {
        s.deck.ending.retain(|e| e.cue.id != instance);
        return true;
    }
    if !s.deck.active.iter().any(|a| a.id == instance) {
        return false;
    }
    with_deck(s, |deck, _| deck.active.retain(|a| a.id != instance));
    true
}

/// Cancels the quantised launch of `cue` waiting for its beat (T-159).
/// False if none waits.
pub fn stop_pending(s: &mut Shared, cue: &str) -> bool {
    let before = s.deck.pending.len();
    s.deck.pending.retain(|p| p.cue != cue);
    s.deck.pending.len() != before
}

/// `cue.stop_all` and « Arrêter les cues »: every cue but the ones
/// protected from « Tout arrêter ». The e-stop doesn't come through here:
/// it cuts the output stage, whatever plays.
pub fn stop_cues(s: &mut Shared) {
    with_deck(s, |d, _| d.stop_unprotected());
}

/// « Tout arrêter » (`running.stop_all`): the macros, unprotected cues, the
/// timeline, the playlist and the cue list (back to *Début*, T-410). Leaves the arm
/// state, the modulators, the audio links and the masters alone.
pub fn stop_running(s: &mut Shared) {
    // The macros first (T-425): none relaunches what this stops.
    crate::macros::stop_all(s);
    stop_cues(s);
    s.timeline.stop();
    s.playlist = None;
    s.cue_list.top();
}

/// Run a change on the deck, keeping `Shared::settings` the live look of
/// the newest cue (see cues.rs), and parking the manual look while cues
/// play.
fn with_deck(s: &mut Shared, f: impl FnOnce(&mut CueDeck, cues::At)) {
    let now = s.now_s();
    with_deck_at(s, deck_clock(s, now), f);
}

/// The deck's clock at `t` (seconds on the studio clock).
fn deck_clock(s: &Shared, t: f64) -> cues::At {
    // The effects' beat (T-246): a launch on the beat lands with the light.
    cues::At { s: t, beat: s.tempo.effect_beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar, rate: s.speed.rate() }
}

/// `with_deck` at a given time (the engine's frame clock).
fn with_deck_at(s: &mut Shared, at: cues::At, f: impl FnOnce(&mut CueDeck, cues::At)) {
    let was_empty = s.deck.active.is_empty();
    let before = s.settings.clone();
    if let Some(top) = s.deck.active.last_mut() {
        top.settings = s.settings.clone();
    }
    f(&mut s.deck, at);
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

/// Ends every held flash/solo, as if each key or pad came up (the UI
/// holding them is gone). Latched cues keep playing.
pub fn release_held(s: &mut Shared) {
    if s.deck.active.iter().any(|a| a.held) {
        with_deck(s, |deck, _| deck.release_all_held());
    }
    // Executor buttons held from that page too (T-418).
    crate::executors::release_all(s);
}

/// Once per engine frame, at `t` (T-158 / T-159): with the e-stop latched,
/// every transition ends and nothing waits any more (the e-stop always
/// cuts at once); otherwise the waiting cues whose boundary has come
/// start, like a press would, and finished transitions end.
pub fn deck_frame(s: &mut Shared, t: f64) {
    let at = deck_clock(s, t);
    if s.estop.is_latched() {
        s.deck.cut_transitions();
    } else if s.deck.pending.iter().any(|p| p.at_beat <= at.beat) {
        let first_new = s.deck.next_id();
        with_deck_at(s, at, |deck, at| {
            deck.fire_due(at);
        });
        started(s, first_new, None, Some("quantize"));
    }
    s.deck.tick(at);
}

/// After a deck change: latched cues started from instance `first_new`
/// on take over from the scene/playlist; a cue started while armed is
/// logged (T-259).
fn started(s: &mut Shared, first_new: u64, mode: Option<ClickMode>, src: Option<&'static str>) {
    if s.deck.active.iter().any(|a| !a.held && a.id >= first_new) {
        // A latched cue takes over from the scene/playlist, as a cue click
        // always did. A flash doesn't: the look comes back on release.
        s.playlist = None;
        s.look_on = false;
    }
    if s.gate.is_armed() {
        let new: Vec<String> = s.deck.active.iter().filter(|a| a.id >= first_new).map(|a| a.cue.clone()).collect();
        for id in new {
            let name = s.presets.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
            s.gate.log().record("cue", src, json!({ "id": id, "name": name, "mode": mode }));
        }
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
    fn every_master_fader_reads_back_for_the_direct_panel() {
        // The « Direct » panel resyncs its sliders from /api/control-values.
        let mut s = shared();
        apply(&mut s, "master.rot_y.angle", ControlInput::Value(45.0), true).unwrap();
        apply(&mut s, "master.rot_z.speed", ControlInput::Value(-90.0), true).unwrap();
        assert_eq!(current(&s, s.controls.get("master.rot_y.angle").unwrap()), Some(serde_json::json!(45.0)));
        assert_eq!(current(&s, s.controls.get("master.rot_z.speed").unwrap()), Some(serde_json::json!(-90.0)));
        for d in s.controls.list().iter().filter(|d| d.id.starts_with("master.")) {
            if matches!(d.kind, ControlKind::Continuous { .. } | ControlKind::Toggle { .. }) {
                assert!(current(&s, d).is_some(), "{} has no value", d.id);
            }
        }
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
        assert_eq!(s.cue_page, s.grid.page_count() - 1);
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
    fn tempo_auto_guide_and_new_track_controls() {
        let mut s = shared();
        apply(&mut s, "tempo.auto", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.tempo.source, tempo::TempoSource::Audio);
        assert_eq!(current(&s, s.controls.get("tempo.auto").unwrap()), Some(serde_json::json!(true)));
        // A tap takes the clock back at once.
        apply(&mut s, "tempo.tap", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.tempo.source, tempo::TempoSource::Tap);
        assert_eq!(current(&s, s.controls.get("tempo.auto").unwrap()), Some(serde_json::json!(false)));
        apply(&mut s, "tempo.auto", ControlInput::Value(1.0), true).unwrap();
        // One guide tap sets nothing yet, and never leaves Tempo auto.
        apply(&mut s, "tempo.guide", ControlInput::Value(1.0), true).unwrap();
        assert_eq!((s.tempo.source, s.tempo.guide_bpm(), s.audio_in.guide().0), (tempo::TempoSource::Audio, None, None));
        s.tempo.guide_tap(100.0);
        s.tempo.guide_tap(100.5);
        let g = s.tempo.guide_tap(101.0);
        s.audio_in.set_guide(g);
        assert_eq!(s.audio_in.guide().0, Some(120.0));
        let requests = s.audio_in.new_track_requests();
        apply(&mut s, "tempo.new_track", ControlInput::Value(1.0), true).unwrap();
        assert_eq!((s.tempo.guide_bpm(), s.audio_in.guide().0), (None, None));
        assert_eq!(s.audio_in.new_track_requests(), requests + 1);
        assert_eq!(s.tempo.source, tempo::TempoSource::Audio, "a new track keeps Tempo auto");
        apply(&mut s, "tempo.auto", ControlInput::Value(0.0), true).unwrap();
        assert_eq!(s.tempo.source, tempo::TempoSource::Manual);
        // Arming is never touched by any of it.
        assert!(!s.gate.is_armed());
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
        s.deck.set_slot(&cue_at(&s, 0, 1), cues::CueSlot { mode: Some(ClickMode::Flash), group: None, layer: None, show: None, ..Default::default() });
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

    #[test]
    fn an_evolving_cue_plays_through_the_deck_on_its_layer_within_budget() {
        use crate::engine::{Animator, AudioFeatures, BeatClock};
        use crate::evolving::{test_cue, EvolvingKey};
        let mut s = shared();
        // A heavy evolving cue (32 beams, then a 32-beam wave) on layer 2,
        // over a catalogue cue on layer 1.
        let mut cue = test_cue(true);
        cue.keys.iter_mut().for_each(|k: &mut EvolvingKey| k.params.count = 32);
        let settings = Settings { content: Content::Evolving(cue), ..Settings::default() };
        s.presets.push(Preset { id: "test-evolving".into(), name: "Test".into(), category: "Faisceaux", settings, tag: None });
        s.deck.set_slot("test-evolving", cues::CueSlot { layer: Some(2), ..Default::default() });
        let base = cue_at(&s, 0, 0);
        assert!(press_cue(&mut s, &base, None, true));
        assert!(press_cue(&mut s, "test-evolving", None, true));
        assert_eq!(s.active_cue.as_deref(), Some("test-evolving"));
        assert!(matches!(s.settings.content, Content::Evolving(_)), "the primary look is the evolving cue");
        let looks = cues::layered_looks(&s.deck, &s.settings, s.look_on);
        assert_eq!(looks.iter().map(|(layer, _, _)| *layer).collect::<Vec<_>>(), [1, 2]);
        let started = s.deck.active[1].started_beat;
        for beat in [0.0, 3.5, 7.9, 8.0, 12.25, 15.9] {
            let clock = BeatClock { beat: started + 1.0 + beat, ..Default::default() };
            let rendered = looks
                .iter()
                .map(|(layer, id, look)| {
                    let start = s.deck.active.iter().find(|a| a.id == *id).unwrap().started_beat;
                    (*layer, Animator::starting_at(start).render(look, AudioFeatures::default(), 1.0 / 60.0, &clock))
                })
                .collect::<Vec<_>>();
            assert!(rendered[1].1.iter().any(|p| p.is_lit()), "evolving cue dark at beat {beat}");
            let (frame, report) = crate::layers::mix(rendered, &s.mixer);
            assert!(frame.len() <= s.mixer.point_budget, "beat {beat}: {} points", frame.len());
            assert!(report.dropped.is_empty() || report.dropped == [2], "{report:?}");
        }
        // Stopping it gives the layer-1 cue back as the primary.
        assert!(press_cue(&mut s, "test-evolving", None, true));
        assert_eq!(s.active_cue, Some(base));
    }

    fn tiny_show(name: &str, cue: &str) -> crate::timeline::Show {
        use crate::timeline::{Event, EventSource, Show, Track};
        let event = Event { id: 1, start: 0.0, len: 30.0, source: EventSource::Cue { id: cue.into() }, ..Default::default() };
        Show { name: name.into(), tracks: vec![Track { events: vec![event], ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn timeline_transport_controls() {
        let mut s = shared();
        let play = |s: &mut Shared| apply(s, "timeline.play", ControlInput::Value(1.0), true);
        assert_eq!(play(&mut s), Err(ControlError::Refused("aucun show chargé")));
        let cue = cue_at(&s, 0, 0);
        s.timeline.load(tiny_show("a", &cue));
        play(&mut s).unwrap();
        assert!(s.timeline.is_playing() && !s.look_on && !s.gate.is_armed());
        assert_eq!(current(&s, s.controls.get("timeline.play").unwrap()), Some(serde_json::json!(true)));
        apply(&mut s, "timeline.loop", ControlInput::Norm(1.0), true).unwrap();
        assert!(s.timeline.loop_on);
        apply(&mut s, "timeline.pause", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(current(&s, s.controls.get("timeline.pause").unwrap()), Some(serde_json::json!(true)));
        apply(&mut s, "timeline.stop", ControlInput::Value(1.0), true).unwrap();
        assert!(!s.timeline.is_playing());
        // The master modifiers stay the operator's: the show doesn't touch them.
        apply(&mut s, "master.size", ControlInput::Value(0.5), true).unwrap();
        play(&mut s).unwrap();
        assert_eq!(s.live.size, 0.5);
    }

    #[test]
    fn timeline_toggle_plays_and_pauses() {
        let mut s = shared();
        let toggle = |s: &mut Shared| apply(s, "timeline.toggle", ControlInput::Value(1.0), true);
        assert_eq!(toggle(&mut s), Err(ControlError::Refused("aucun show chargé")));
        s.timeline.load(tiny_show("a", &cue_at(&s, 0, 0)));
        toggle(&mut s).unwrap();
        assert!(s.timeline.is_playing());
        toggle(&mut s).unwrap();
        assert_eq!(s.timeline.transport, crate::timeline::Transport::Paused);
        toggle(&mut s).unwrap();
        assert!(s.timeline.is_playing(), "resumes");
        toggle(&mut s).unwrap();
        s.emergency_stop(ArmSource::Midi);
        assert!(toggle(&mut s).is_err() && !s.timeline.is_playing(), "no playback under a latched e-stop");
    }

    #[test]
    fn a_grid_cell_can_hold_a_show() {
        let mut s = shared();
        let (cell, cue) = (cue_at(&s, 0, 2), cue_at(&s, 0, 0));
        s.shows.save(&tiny_show("grid show", &cue)).unwrap();
        s.deck.set_slot(&cell, cues::CueSlot { show: Some(" grid show ".into()), ..Default::default() });
        assert_eq!(s.deck.slot(&cell).show.as_deref(), Some("grid show"));
        apply(&mut s, "grid.1.1.3", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "grid.1.1.3", ControlInput::Value(0.0), true).unwrap();
        assert!(s.timeline.is_playing(), "a press plays the show, the release is ignored");
        assert!(s.deck.active.is_empty(), "the cell's own cue doesn't start");
        assert_eq!(current(&s, s.controls.get("grid.1.1.3").unwrap()), Some(serde_json::json!(true)));
        apply(&mut s, "grid.1.1.3", ControlInput::Value(1.0), true).unwrap();
        assert!(!s.timeline.is_playing(), "second press stops it");
    }

    /// T-146: the FX grid from a controller: cells toggle (a drop on each
    /// press), the *Action* faders, stop, destination and click mode read
    /// back for LEDs; none of it arms.
    #[test]
    fn fx_controls_play_the_grid_and_never_arm() {
        let mut s = shared();
        for id in ["fx.1.1", "fx.4.8", "fx.2.action", "fx.stop", "fx.dest", "fx.mode"] {
            assert!(s.controls.get(id).is_some_and(|d| d.external), "{id}");
        }
        apply(&mut s, "fx.1.2", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "fx.1.2", ControlInput::Value(0.0), true).unwrap();
        apply(&mut s, "fx.2.1", ControlInput::Value(1.0), true).unwrap();
        assert_eq!((s.fx.active(0), s.fx.active(1)), (Some(1), Some(0)));
        assert_eq!(current(&s, s.controls.get("fx.1.2").unwrap()), Some(json!(true)));
        assert_eq!(current(&s, s.controls.get("fx.1.1").unwrap()), Some(json!(false)));
        apply(&mut s, "fx.1.5", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.fx.active(0), Some(4), "one effect per line");
        apply(&mut s, "fx.3.action", ControlInput::Norm(0.25), true).unwrap();
        assert_eq!(s.fx.action(2), 0.25);
        apply(&mut s, "fx.dest", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.fx.grid().dest, crate::fx::FxDest::Cue);
        apply(&mut s, "fx.mode", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(current(&s, s.controls.get("fx.mode").unwrap()), Some(json!(1)));
        apply(&mut s, "fx.stop", ControlInput::Value(1.0), true).unwrap();
        assert!(!s.fx.any_playing());
        assert!(!s.gate.is_armed());
    }

    /// Keeps docs/controls.md in sync with the registry, where the local
    /// docs/ folder exists (it isn't part of the repository).
    #[test]
    fn write_controls_doc() {
        let md = markdown(&ControlRegistry::build(&catalog()));
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs");
        if docs.is_dir() {
            std::fs::write(docs.join("controls.md"), md).unwrap();
        }
    }

    /// T-410: the cue list's four controls, external triggers that drive
    /// the loaded list from a controller and never arm.
    #[test]
    fn list_controls_drive_the_cue_list_and_never_arm() {
        use crate::cuelist::{CueList, Step, StepSource};
        let mut s = shared();
        for id in ["list.go", "list.back", "list.pause", "list.top"] {
            let d = s.controls.get(id).unwrap_or_else(|| panic!("{id} registered"));
            assert!(d.external && d.kind == ControlKind::Trigger, "{id}");
        }
        assert!(matches!(apply(&mut s, "list.go", ControlInput::Value(1.0), true), Err(ControlError::Refused(_))), "no list loaded");
        let cue = |i: usize| Step { source: StepSource::Cue { id: s.presets[i].id.clone() }, ..Step::default() };
        let list = CueList { name: "A".into(), steps: vec![cue(0), cue(1)], ..CueList::default() };
        crate::cuelist::save(&mut s, list).unwrap();
        crate::cuelist::load(&mut s, "A").unwrap();
        apply(&mut s, "list.go", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "list.go", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.cue_list.index, Some(1));
        assert_eq!(s.cue_list.source, ControlSource::Midi);
        apply(&mut s, "list.back", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.cue_list.index, Some(0));
        apply(&mut s, "list.pause", ControlInput::Value(1.0), true).unwrap();
        assert!(s.cue_list.paused);
        apply(&mut s, "list.top", ControlInput::Value(1.0), true).unwrap();
        assert_eq!((s.cue_list.index, s.cue_list.paused), (None, false));
        assert!(!s.gate.is_armed());
        // « Tout arrêter » takes the list back to Début too.
        apply(&mut s, "list.go", ControlInput::Value(1.0), true).unwrap();
        apply(&mut s, "running.stop_all", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.cue_list.index, None);
    }

    /// T-420: the group masters, continuous 0..1, external, default 1; a
    /// missing group refuses, `master.reset` leaves them, « Tout à 100 % »
    /// puts them back; nothing arms.
    #[test]
    fn group_masters_are_registered_clamped_and_never_arm() {
        use crate::projection::{ProjectionConfig, ProjectionZone, ZoneGroup, GROUP_MASTER_CONTROLS};
        let mut s = shared();
        for n in 1..=GROUP_MASTER_CONTROLS {
            let d = s.controls.get(&format!("group.{n}.master")).unwrap_or_else(|| panic!("group.{n}.master"));
            assert!(d.external);
            assert_eq!(d.group, "group");
            assert_eq!(d.kind, ControlKind::Continuous { min: 0.0, max: 1.0, default: 1.0, unit: Unit::Percent });
        }
        assert!(s.controls.get(&format!("group.{}.master", GROUP_MASTER_CONTROLS + 1)).is_none());
        assert!(matches!(apply(&mut s, "group.1.master", ControlInput::Value(0.5), true), Err(ControlError::Refused(_))), "no group yet");
        let cfg = ProjectionConfig {
            zones: vec![ProjectionZone { id: 1, ..Default::default() }, ProjectionZone { id: 2, ..Default::default() }],
            default_zones: vec![1],
            groups: vec![ZoneGroup { name: "Centre".into(), zones: vec![1], master: 1.0 }, ZoneGroup { name: "Côtés".into(), zones: vec![2], master: 1.0 }],
        };
        s.projection.set(cfg).unwrap();
        apply(&mut s, "group.2.master", ControlInput::Value(0.5), true).unwrap();
        assert!(s.groups_dirty);
        assert_eq!(current(&s, s.controls.get("group.2.master").unwrap()), Some(json!(0.5)));
        apply(&mut s, "group.1.master", ControlInput::Value(9.0), true).unwrap();
        assert_eq!(s.projection.master(1), Some(1.0), "never above 100 %");
        apply(&mut s, "group.1.master", ControlInput::Norm(-2.0), true).unwrap();
        assert_eq!(s.projection.master(1), Some(0.0));
        apply(&mut s, "group.1.master", ControlInput::Value(f32::NAN), true).unwrap();
        assert_eq!(s.projection.master(1), Some(0.0), "NaN is dark");
        apply(&mut s, "master.reset", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.projection.masters(), [0.0, 0.5], "Réinitialiser le direct leaves the group masters");
        apply(&mut s, "group.all_full", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.projection.masters(), [1.0, 1.0]);
        assert_eq!(current(&s, s.controls.get("group.3.master").unwrap()), None);
        assert!(!s.gate.is_armed());
    }
}
