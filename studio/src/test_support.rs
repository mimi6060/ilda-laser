//! Test helpers shared by unit tests across modules.

use crate::engine::{AudioFeatures, Calibration, Settings};
use crate::scenes::SceneStore;
use crate::{controls, cues, layers, lfo, live, midi, presets, tempo, timeline, Shared};
use std::time::Instant;

static ILDA_DIRS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A fresh `Shared` like the one `main` builds, with scenes stored in a
/// path that is never written unless a test saves a scene.
pub fn shared() -> Shared {
    // One Art-Net box setting (T-301) for the backend and the handlers.
    let shownet_dmx = std::sync::Arc::new(crate::artnet::ShowNetDmxStore::in_memory());
    let mut backend = crate::output_manager::FakeBackend::new(Vec::new(), None, None, std::env::temp_dir().join("laser-studio-test-unused"));
    backend.artnet = std::sync::Arc::clone(&shownet_dmx);
    let output_mgr = std::sync::Arc::new(crate::output_manager::OutputManager::new(
        std::sync::Arc::new(backend),
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
        shownet_dmx,
        output_status: None,
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
        user_cues: crate::usercues::UserCueStore::in_memory(),
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
        touches: Default::default(),
        // Never a real scan in unit tests, never a real output.
        devices: std::sync::Arc::clone(output_mgr.devices()),
        output_mgr,
        wifi: std::sync::Arc::new(crate::wifi::WifiWatch::new(true)),
    }
}

/// T-358: how much longer than its own limit a guarded test may run, from
/// `LASER_STUDIO_TEST_TIME_SCALE` (a slow or busy machine: `4`; `0` turns
/// every guard off, e.g. under a debugger). Default 1.
fn time_scale() -> f64 {
    std::env::var("LASER_STUDIO_TEST_TIME_SCALE").ok().and_then(|v| v.parse::<f64>().ok()).filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(1.0)
}

/// Ends the whole test binary, with a clear message, if it is still alive
/// after its limit. Dropping it (the test finished, passed or panicked)
/// switches it off.
#[must_use = "the guard only watches while it is alive"]
pub struct TimeGuard {
    done: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl Drop for TimeGuard {
    fn drop(&mut self) {
        let (lock, cv) = &*self.done;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

/// T-358: a time limit for a long test or one that waits on a thread. The
/// Rust test harness has none, so a hang once ran for hours: past `limit`
/// (times `LASER_STUDIO_TEST_TIME_SCALE`) the test binary prints which test
/// hung and exits with a failure instead of waiting forever.
pub fn time_guard(what: &str, limit: std::time::Duration) -> TimeGuard {
    let done = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let scale = time_scale();
    if scale > 0.0 {
        let limit = limit.mul_f64(scale);
        let (what, done) = (what.to_string(), std::sync::Arc::clone(&done));
        std::thread::spawn(move || {
            let (lock, cv) = &*done;
            let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (finished, _) = cv.wait_timeout_while(guard, limit, |finished| !*finished).unwrap_or_else(|e| e.into_inner());
            if !*finished {
                // Straight to stderr: `eprintln!` would be captured by the
                // test harness, and lost with the process.
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr(),
                    "\n*** T-358: test « {what} » still running after {limit:?}: hung? Stopping the test binary. \
                     (LASER_STUDIO_TEST_TIME_SCALE=4 gives a busy machine more time, 0 turns the limits off) ***\n"
                );
                std::process::exit(101);
            }
        });
    }
    TimeGuard { done }
}

/// T-358: the whole unit-test binary normally runs in about a minute; a
/// hang in a test without a guard of its own still stops it after this.
pub const BINARY_LIMIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Starts the catch-all limit of the whole test binary: its thread
    /// outlives this test and ends the process only if the run is still
    /// going after `BINARY_LIMIT`.
    #[test]
    fn the_test_binary_has_a_time_limit() {
        std::mem::forget(time_guard("the whole unit-test binary", BINARY_LIMIT));
    }

    #[test]
    fn a_guard_dropped_in_time_does_nothing() {
        let guard = time_guard("a quick test", Duration::from_millis(50));
        drop(guard);
        std::thread::sleep(Duration::from_millis(100));
        // Still here: the process was not stopped.
    }
}
