//! Test helpers shared by unit tests across modules.

use crate::engine::{AudioFeatures, Calibration, Settings};
use crate::scenes::SceneStore;
use crate::{controls, cues, layers, lfo, live, midi, presets, tempo, timeline, Shared};
use std::time::Instant;

static SCRATCH_DIRS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// T-483: a data folder of its own for one test: process id plus a
/// counter, so neither the tests of this binary (they run in parallel)
/// nor other `cargo test` runs on the same machine (other worktrees)
/// ever share it. Not created: it only appears once a test writes there.
///
/// It replaces the fixed `$TMPDIR/laser-studio-test-unused/` that every
/// `shared()` used to load its LFOs, audio routes, scenes, palettes and
/// scan presets from. A test that saved one (an LFO on `master.size`)
/// leaked it into every later `shared()`, in this run or in another
/// one, and four tests then failed "under load" (more runs at once).
pub fn scratch_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("laser-studio-test-{}-{}", std::process::id(), SCRATCH_DIRS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)))
}

/// T-486: the preset catalogue, the control registry built from it and
/// the default cue grid resolved on it, as `main` builds them at startup:
/// built once per test binary (they depend on nothing but the code) and
/// cloned for each `shared()`, so a test that adds a preset or a control
/// still only changes its own copy. Building them was a tenth of the
/// safety fuzz's time.
fn catalog() -> (Vec<presets::Preset>, controls::ControlRegistry, crate::grid::CueGrid) {
    static BUILT: std::sync::OnceLock<(Vec<presets::Preset>, controls::ControlRegistry, crate::grid::CueGrid)> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let presets = presets::catalog();
            let controls = controls::ControlRegistry::build(&presets);
            let mut grid = crate::grid::CueGrid::default();
            grid.resolve(&presets);
            (presets, controls, grid)
        })
        .clone()
}

/// A fresh `Shared` like the one `main` builds. Its stores (scenes, LFOs,
/// audio routes...) live in a `scratch_dir` of its own: they start empty,
/// and what a test saves is seen by that test only.
pub fn shared() -> Shared {
    let data = scratch_dir();
    // One Art-Net box setting (T-301) for the backend and the handlers.
    let shownet_dmx = std::sync::Arc::new(crate::artnet::ShowNetDmxStore::in_memory());
    let mut backend = crate::output_manager::FakeBackend::new(Vec::new(), None, None, data.clone());
    backend.artnet = std::sync::Arc::clone(&shownet_dmx);
    let output_mgr = std::sync::Arc::new(crate::output_manager::OutputManager::new(
        std::sync::Arc::new(backend),
        std::sync::Arc::new(crate::devices::DeviceList::fixed(Vec::new())),
        &Default::default(),
        30_000,
    ));
    let (presets, controls, grid) = catalog();
    let lfos = lfo::LfoStore::load_or_create(data.join("lfos.json"), &controls);
    let routes = crate::audio::routes::RouteStore::load_or_create(data.join("audio_routes.json"), &controls);
    let fx = crate::fx::FxStore::in_memory(&controls);
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
        scenes: SceneStore::load_or_create(data.join("scenes.json")),
        sdbank: crate::sdbank::SdBankStore::in_memory(),
        recorder: crate::record::Recorder::default(),
        shownet_dmx,
        output_status: None,
        playlist: None,
        controls,
        presets,
        cue_page: 0,
        active_cue: None,
        settings_rev: 0,
        deck: cues::CueDeck::default(),
        cue_lists: crate::cuelist::CueListStore::in_memory(),
        cue_list: Default::default(),
        grid,
        look_on: true,
        tempo: tempo::TempoClock::default(),
        latency_path: None,
        speed: Default::default(),
        speed_dirty: false,
        speed_path: None,
        epoch: Instant::now(),
        sim_now: None,
        live: live::LiveModifiers::default(),
        live_dirty: false,
        cue_mods: Default::default(),
        palettes: live::PaletteStore::load_or_create(data.join("palettes.json")),
        scan_presets: crate::scan::ScanPresetStore::load_or_create(data.join("scan_presets.json")),
        pools: crate::pools::PoolStore::in_memory(),
        festival: Default::default(),
        midi: midi::MidiState::new(false, midi::profile::ProfileStore::in_memory()),
        netin: crate::netin::NetInState::new(crate::netin::NetHub::in_memory(estop)),
        external: crate::external::External::in_memory(),
        timecode: crate::timecode::TimecodeState::in_memory(),
        lfos,
        routes,
        fx,
        triggers: crate::audio::triggers::TriggerStore::in_memory(),
        autopilot: crate::autopilot::AutopilotStore::in_memory(),
        mixer: layers::Mixer::default(),
        mixer_dirty: false,
        groups_dirty: false,
        zone_chase: Default::default(),
        mix: layers::MixReport::default(),
        safety: crate::safety::SafetyStore::in_memory(),
        strobe: Default::default(),
        outputs: crate::power::OutputStore::in_memory(),
        projection: crate::projection::ProjectionStore::in_memory(),
        audio_guard: Default::default(),
        dwell: Default::default(),
        evolving: Vec::new(),
        timeline: timeline::Player::default(),
        take: crate::take::Recorder::default(),
        figures: crate::figures::FigureStore::in_memory(),
        user_cues: crate::usercues::UserCueStore::in_memory(),
        shows: timeline::ShowStore::new(data.join("shows")),
        templates: crate::templates::TemplateStore::new(data.join("templates")),
        media: std::sync::Arc::new(crate::audio::media::MediaStore::new(&data)),
        autoshow: Default::default(),
        // In the call's own folder like every store: tests import in parallel.
        ilda: std::sync::Arc::new(crate::ilda_library::IldaLibrary::new(&data)),
        // No song-playback thread in unit tests: the timeline runs on the system clock.
        song: std::sync::Arc::new(crate::audio::playback::SongHub::new(Instant::now(), false)),
        song_sync: Default::default(),
        // Not enforced: tests that arm without a page keep working. Tests
        // of presence build an enforced one themselves.
        presence: Default::default(),
        health: Default::default(),
        project: crate::project::ProjectState::load(&data),
        test_pattern: Default::default(),
        zone_trace: Default::default(),
        keep_awake: crate::keep_awake::KeepAwakeState::in_memory(),
        onboarding: crate::onboarding::OnboardingStore::in_memory(),
        show_mode: crate::showmode::ShowMode::in_memory(),
        safety_lock: crate::safetylock::SafetyLock::in_memory(),
        data_warnings: Vec::new(),
        // Pure preview: the checklist never blocks unit tests.
        checklist: crate::checklist::Checklist::in_memory(false),
        profiles: crate::profiles::ProfileStore::in_memory(),
        sites: crate::sites::SiteStore::in_memory(),
        audience: Default::default(),
        sky_off: false,
        test_hooks: false,
        menu: None,
        test_stall_ms: 0,
        touches: Default::default(),
        // Never a real scan in unit tests, never a real output.
        devices: std::sync::Arc::clone(output_mgr.devices()),
        output_mgr,
        wifi: std::sync::Arc::new(crate::wifi::WifiWatch::new(true)),
        push: Default::default(),
        venue: crate::venue::VenueStore::in_memory(),
        projector_outputs: Default::default(),
        projector_frames: Vec::new(),
        perf: Default::default(),
        notices: Default::default(),
        history: Default::default(),
        prepared: Default::default(),
        executors: crate::executors::Executors::in_memory(),
        desks: crate::desk::Desks::in_memory(),
        macros: crate::macros::Macros::in_memory(),
        showlist: crate::showlist::ShowListRunner::in_memory(),
        agenda: crate::agenda::AgendaRunner::in_memory(),
        remote: crate::remote::Remote::in_memory(),
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

/// T-486: a hang guard for a test made of many cases, run by one or more
/// workers: it fires when one case makes no progress for `limit`, not
/// when the whole test runs long. A total-time limit fired on sound runs
/// of a busy machine (other test binaries, other worktrees) and so could
/// not tell a slow run from a hang; this one catches a real hang in
/// `limit` whatever the machine's load, and names the stuck case.
///
/// A worker calls `begin` when it starts a case, `beat` whenever the case
/// moves on (each frame), `end` when it is done. Dropping the guard (the
/// test finished, passed or panicked) switches it off.
#[must_use = "the guard only watches while it is alive"]
pub struct ProgressGuard {
    inner: std::sync::Arc<ProgressInner>,
}

struct ProgressInner {
    start: Instant,
    /// Per worker: whether it runs a case, which one, and when it last
    /// moved (milliseconds since `start`).
    busy: Vec<std::sync::atomic::AtomicBool>,
    case: Vec<std::sync::atomic::AtomicU64>,
    last_ms: Vec<std::sync::atomic::AtomicU64>,
    done: (std::sync::Mutex<bool>, std::sync::Condvar),
}

impl ProgressInner {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

impl ProgressGuard {
    /// Worker `worker` starts case `case` (a seed).
    pub fn begin(&self, worker: usize, case: u64) {
        use std::sync::atomic::Ordering::SeqCst;
        let i = &self.inner;
        i.last_ms[worker].store(i.now_ms(), SeqCst);
        i.case[worker].store(case, SeqCst);
        i.busy[worker].store(true, SeqCst);
    }

    /// Worker `worker`'s case moved on.
    pub fn beat(&self, worker: usize) {
        let i = &self.inner;
        i.last_ms[worker].store(i.now_ms(), std::sync::atomic::Ordering::SeqCst);
    }

    /// Worker `worker` finished its case.
    pub fn end(&self, worker: usize) {
        self.inner.busy[worker].store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        let (lock, cv) = &self.inner.done;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

/// T-486: a `ProgressGuard` for `workers` workers that ends the test
/// binary, with a message naming the stuck case, when one case makes no
/// progress for `limit` (times `LASER_STUDIO_TEST_TIME_SCALE`; `0` turns
/// it off).
pub fn progress_guard(what: &str, workers: usize, limit: std::time::Duration) -> ProgressGuard {
    let scale = time_scale();
    let what = what.to_string();
    let on_stall = move |msg: String| {
        // Straight to stderr: `eprintln!` would be captured by the test
        // harness, and lost with the process.
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr(),
            "\n*** T-486: test « {what} »: {msg}: hung? Stopping the test binary. \
             (LASER_STUDIO_TEST_TIME_SCALE=4 gives a busy machine more time, 0 turns the limits off) ***\n"
        );
        std::process::exit(101);
    };
    progress_guard_with(workers, (scale > 0.0).then(|| limit.mul_f64(scale)), on_stall)
}

/// `progress_guard` with what to do on a stall (called once, with a
/// message naming the case) and an exact limit (`None`: never fires).
pub fn progress_guard_with(workers: usize, limit: Option<std::time::Duration>, on_stall: impl FnOnce(String) + Send + 'static) -> ProgressGuard {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
    let inner = std::sync::Arc::new(ProgressInner {
        start: Instant::now(),
        busy: (0..workers).map(|_| AtomicBool::new(false)).collect(),
        case: (0..workers).map(|_| AtomicU64::new(0)).collect(),
        last_ms: (0..workers).map(|_| AtomicU64::new(0)).collect(),
        done: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
    });
    if let Some(limit) = limit {
        let watched = std::sync::Arc::clone(&inner);
        // Looks a few times per limit: a stall is reported at most a
        // quarter of the limit late.
        let every = (limit / 4).clamp(std::time::Duration::from_millis(10), std::time::Duration::from_secs(1));
        let limit_ms = limit.as_millis() as u64;
        std::thread::spawn(move || {
            let (lock, cv) = &watched.done;
            let mut finished = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                finished = cv.wait_timeout(finished, every).unwrap_or_else(|e| e.into_inner()).0;
                if *finished {
                    return;
                }
                let now = watched.now_ms();
                let stuck = (0..workers).find(|&w| watched.busy[w].load(SeqCst) && now.saturating_sub(watched.last_ms[w].load(SeqCst)) > limit_ms);
                if let Some(w) = stuck {
                    let case = watched.case[w].load(SeqCst);
                    drop(finished);
                    on_stall(format!("case SAFETY_SEED={case} made no progress for {limit:?}"));
                    return;
                }
            }
        });
    }
    ProgressGuard { inner }
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

    /// T-483: what one test saves (an LFO on the master size, an audio
    /// route on the master brightness, a scene) never reaches another
    /// test's `shared()`. A shared folder once leaked such an LFO into
    /// four safety and state-stream tests, in this run and in other runs
    /// on the same machine.
    #[test]
    fn what_a_test_saves_never_reaches_another_shared() {
        let mut a = shared();
        let (lfo_path, routes_path) = (a.lfos.path().to_path_buf(), a.routes.path().to_path_buf());
        let lfo = lfo::Modulator { target: "master.size".into(), ..Default::default() };
        a.lfos.set(vec![lfo], &a.controls).unwrap();
        let mut routing = a.routes.routing().clone();
        routing.routes.push(crate::audio::routes::AudioRoute { target: "master.brightness".into(), ..Default::default() });
        a.routes.set(routing, &a.controls).unwrap();
        a.scenes.upsert(crate::scenes::Scene { name: "leak".into(), settings: Settings::default(), duration_secs: 4.0 }).unwrap();
        assert!(lfo_path.exists() && routes_path.exists(), "the stores were saved");

        let b = shared();
        assert_ne!(b.lfos.path().parent(), a.lfos.path().parent(), "a folder of its own");
        assert!(b.lfos.list().is_empty(), "an LFO leaked: {:?}", b.lfos.list());
        assert!(b.routes.routing().routes.is_empty(), "an audio route leaked");
        assert!(b.scenes.list().is_empty(), "a scene leaked");
        assert_ne!(scratch_dir(), scratch_dir());
        assert!(scratch_dir().starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(lfo_path.parent().unwrap());
    }

    #[test]
    fn a_guard_dropped_in_time_does_nothing() {
        let guard = time_guard("a quick test", Duration::from_millis(50));
        drop(guard);
        std::thread::sleep(Duration::from_millis(100));
        // Still here: the process was not stopped.
    }
}
