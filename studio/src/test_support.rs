//! Test helpers shared by unit tests across modules.

use crate::engine::{AudioFeatures, Calibration, Settings};
use crate::scenes::SceneStore;
use crate::{controls, cues, layers, lfo, live, midi, presets, tempo, timeline, Shared};
use std::time::Instant;

static ILDA_DIRS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A fresh `Shared` like the one `main` builds, with scenes stored in a
/// path that is never written unless a test saves a scene.
pub fn shared() -> Shared {
    let output_mgr = std::sync::Arc::new(crate::output_manager::OutputManager::new(
        std::sync::Arc::new(crate::output_manager::FakeBackend::new(Vec::new(), None, None, std::env::temp_dir().join("laser-studio-test-unused"))),
        std::sync::Arc::new(crate::devices::DeviceList::fixed(Vec::new())),
        &Default::default(),
        30_000,
    ));
    let presets = presets::catalog();
    let controls = controls::ControlRegistry::build(&presets);
    let mut grid = crate::grid::CueGrid::default();
    grid.resolve(&presets);
    let lfos = lfo::LfoStore::load_or_create(std::env::temp_dir().join("laser-studio-test-unused/lfos.json"), &controls);
    let routes = crate::audio::routes::RouteStore::load_or_create(std::env::temp_dir().join("laser-studio-test-unused/audio_routes.json"), &controls);
    // One latch for the gate and the network listeners' blackout path.
    let estop: std::sync::Arc<crate::interlock::EStop> = Default::default();
    Shared {
        settings: Settings::default(),
        calibration: Calibration::default(),
        audio: AudioFeatures::default(),
        audio_at: Instant::now(),
        audio_in: std::sync::Arc::new(crate::audio::AudioHub::in_memory(false)),
        gate: Default::default(),
        estop: std::sync::Arc::clone(&estop),
        frame: Vec::new(),
        output_lit: 0,
        output_name: None,
        output_kind: None,
        output_error: None,
        applog: Default::default(),
        pps: 30_000,
        scenes: SceneStore::load_or_create(std::env::temp_dir().join("laser-studio-test-unused/scenes.json")),
        sdbank: crate::sdbank::SdBankStore::in_memory(),
        playlist: None,
        controls,
        presets,
        cue_page: 0,
        active_cue: None,
        settings_rev: 0,
        deck: cues::CueDeck::default(),
        grid,
        look_on: true,
        tempo: tempo::TempoClock::default(),
        epoch: Instant::now(),
        live: live::LiveModifiers::default(),
        live_dirty: false,
        palettes: live::PaletteStore::load_or_create(std::env::temp_dir().join("laser-studio-test-unused/palettes.json")),
        midi: midi::MidiState::new(false, midi::profile::ProfileStore::in_memory()),
        netin: crate::netin::NetInState::new(crate::netin::NetHub::in_memory(estop)),
        external: crate::external::External::in_memory(),
        lfos,
        routes,
        mixer: layers::Mixer::default(),
        mixer_dirty: false,
        mix: layers::MixReport::default(),
        safety: crate::safety::SafetyStore::in_memory(),
        strobe: Default::default(),
        outputs: crate::power::OutputStore::in_memory(),
        projection: crate::projection::ProjectionStore::in_memory(),
        audio_guard: Default::default(),
        dwell: Default::default(),
        evolving: Vec::new(),
        timeline: timeline::Player::default(),
        figures: crate::figures::FigureStore::in_memory(),
        shows: timeline::ShowStore::new(std::env::temp_dir().join(format!("laser-studio-test-shows-{}", std::process::id())).join("shows")),
        media: std::sync::Arc::new(crate::audio::media::MediaStore::new(&std::env::temp_dir().join(format!("laser-studio-test-media-{}", std::process::id())))),
        // Its own folder per call: tests import in parallel.
        ilda: std::sync::Arc::new(crate::ilda_library::IldaLibrary::new(&std::env::temp_dir().join(format!(
            "laser-studio-test-ilda-{}-{}",
            std::process::id(),
            ILDA_DIRS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))),
        // No song-playback thread in unit tests: the timeline runs on the system clock.
        song: std::sync::Arc::new(crate::audio::playback::SongHub::new(Instant::now(), false)),
        song_sync: Default::default(),
        // Not enforced: tests that arm without a page keep working. Tests
        // of presence build an enforced one themselves.
        presence: Default::default(),
        health: Default::default(),
        project: crate::project::ProjectState::load(&std::env::temp_dir().join("laser-studio-test-unused")),
        test_pattern: Default::default(),
        keep_awake: crate::keep_awake::KeepAwakeState::in_memory(),
        show_mode: crate::showmode::ShowMode::in_memory(),
        data_warnings: Vec::new(),
        // Pure preview: the checklist never blocks unit tests.
        checklist: crate::checklist::Checklist::in_memory(false),
        audience: Default::default(),
        test_hooks: false,
        test_stall_ms: 0,
        // Never a real scan in unit tests, never a real output.
        devices: std::sync::Arc::clone(output_mgr.devices()),
        output_mgr,
        wifi: std::sync::Arc::new(crate::wifi::WifiWatch::new(true)),
    }
}
