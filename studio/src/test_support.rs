//! Test helpers shared by unit tests across modules.

use crate::engine::{AudioFeatures, Calibration, Settings};
use crate::scenes::SceneStore;
use crate::{controls, live, midi, presets, tempo, Shared};
use std::time::Instant;

/// A fresh `Shared` like the one `main` builds, with scenes stored in a
/// path that is never written unless a test saves a scene.
pub fn shared() -> Shared {
    let presets = presets::catalog();
    Shared {
        settings: Settings::default(),
        calibration: Calibration::default(),
        audio: AudioFeatures::default(),
        audio_at: Instant::now(),
        armed: false,
        frame: Vec::new(),
        output_name: None,
        output_error: None,
        pps: 30_000,
        scenes: SceneStore::load_or_create(std::env::temp_dir().join("laser-studio-test-unused/scenes.json")),
        playlist: None,
        controls: controls::ControlRegistry::build(&presets),
        presets,
        cue_page: 0,
        active_cue: None,
        tempo: tempo::TempoClock::default(),
        epoch: Instant::now(),
        live: live::LiveModifiers::default(),
        live_dirty: false,
        midi: midi::MidiState::new(false, midi::profile::ProfileStore::in_memory()),
    }
}
