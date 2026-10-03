//! Laser studio: a live laser show controller that runs on the Mac, with
//! its UI in the browser. Pick a shape or text, animate it, make it react
//! to music from the microphone, save looks as scenes and run them as a
//! playlist - all with a live preview, with or without a laser attached.
//!
//! One engine thread renders a frame 60 times a second from the shared
//! state and hands it to the preview and (when one is configured) to the
//! laser output. The HTTP handlers in `web.rs` only ever edit that shared
//! state.

mod audio;
mod beat;
mod controls;
mod cues;
mod engine;
mod evolving;
mod fans;
mod figure_import;
mod figures;
mod font;
mod generators;
mod layers;
mod lfo;
mod interlock;
mod live;
mod midi;
mod output;
mod patterns;
mod power;
mod presence;
mod presets;
mod project;
mod safety;
mod scenes;
mod sheets;
mod tempo;
mod timeline;
mod tunnels;
mod watchdog;
mod zones;
mod web;

#[cfg(test)]
mod test_support;

use anyhow::{Context, Result};
use clap::Parser;
use engine::{Animator, AudioFeatures, Calibration, Settings};
use interlock::{ArmGate, EStop};
use output::{DacOutput, FileLogOutput, Output, OutputStage};
use patterns::Point;
use scenes::SceneStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "laser-studio", version)]
struct Cli {
    /// Laser DAC to drive: an id from `ilda-laser discover`, or "auto" for
    /// the first one found. Without it, the studio runs in preview-only
    /// mode.
    #[arg(long)]
    device: Option<String>,
    /// Points per second sent to the laser.
    #[arg(long, default_value_t = 30_000)]
    pps: u32,
    /// Port for the browser UI.
    #[arg(long, default_value_t = 8080)]
    port: u16,
    /// Where scenes and calibration are saved.
    #[arg(long, default_value = "studio-data")]
    data_dir: PathBuf,
    /// Print every control id (for MIDI/OSC mapping) as Markdown and exit.
    #[arg(long)]
    list_controls: bool,
    /// Don't open any MIDI port (tests, e2e, or a second instance that
    /// must not grab the controller).
    #[arg(long)]
    no_midi: bool,
    /// Audio input the studio listens to: a name from
    /// `GET /api/audio/devices`, "default" (the Mac's input) or "none" (no
    /// capture). Overrides audio.json for this run.
    #[arg(long)]
    audio_device: Option<String>,
    /// Don't open any audio input (tests, e2e, a second instance). The
    /// browser source (`POST /api/audio`) still works.
    #[arg(long)]
    no_audio: bool,
    /// Testing only: add an interlock that is never satisfied, so arming
    /// is always refused (e2e tests of the refusal message).
    #[arg(long, hide = true)]
    test_interlock: bool,
    /// Testing only (e2e, T-209): plug in a simulated APC40 mkII
    /// (« Test APC40 mkII ») and enable `POST /api/midi/inject` and
    /// `GET /api/midi/sent`. Needs --no-midi (no real MIDI port is ever
    /// opened) and refuses --device (preview only).
    #[arg(long, hide = true, requires = "no_midi", conflicts_with = "device")]
    midi_test: bool,
    /// Testing only: a fake output that logs its calls to this file (no
    /// laser), for the shutdown subprocess test.
    #[arg(long, hide = true, conflicts_with = "device")]
    test_output: Option<PathBuf>,
    /// Testing only: enable `POST /api/test/stall` (simulated engine stall
    /// for the watchdog e2e test).
    #[arg(long, hide = true)]
    test_hooks: bool,
    /// Internal (T-298): decode this song of `<data-dir>/media/audio/` and
    /// write the result on stdout, then exit. The studio runs itself this
    /// way so a decoder crash can't stop it (audio/isolate.rs).
    #[arg(long, hide = true, value_name = "FILE")]
    decode: Option<String>,
    /// With --decode: send the waveform only, not the samples.
    #[arg(long, hide = true, requires = "decode")]
    decode_peaks: bool,
    /// Testing only: the decoding time limit in ms (default 120 s).
    #[arg(long, hide = true, requires = "test_hooks")]
    test_decode_timeout_ms: Option<u64>,
}

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

pub struct Shared {
    pub settings: Settings,
    pub calibration: Calibration,
    /// The last `POST /api/audio` (the browser source) and when it came.
    pub audio: AudioFeatures,
    pub audio_at: Instant,
    /// Native capture (audio/mod.rs): config, status and the analysis
    /// snapshot, shared with the audio threads (their own short locks, never
    /// this one).
    pub audio_in: Arc<audio::AudioHub>,
    /// The only place the armed state changes (interlock.rs). Always
    /// starts disarmed, reason « Démarrage ».
    pub gate: ArmGate,
    /// Latched emergency stop, shared lock-free with the HTTP fast path
    /// and the engine's output stage.
    pub estop: Arc<EStop>,
    /// The computed frame, shown by the preview even while disarmed.
    pub frame: Vec<Point>,
    /// Lit points in the frame actually sent to the output (0 when disarmed).
    pub output_lit: usize,
    pub output_name: Option<String>,
    pub output_error: Option<String>,
    pub pps: u32,
    pub scenes: SceneStore,
    pub playlist: Option<Playlist>,
    pub presets: Vec<presets::Preset>,
    pub controls: controls::ControlRegistry,
    /// Cue-grid page shown in the UI and on MIDI grids (0-based category index).
    pub cue_page: usize,
    /// Id of the newest playing cue, for highlighting and LED feedback.
    pub active_cue: Option<String>,
    /// Bumped each time `settings` changes (look panel, cue, scene,
    /// playlist, look controls), so the page reloads a look it didn't make.
    pub settings_rev: u64,
    /// Playing cues and the grid's trigger settings (cues.rs).
    pub deck: cues::CueDeck,
    /// Whether `settings` is shown when no cue plays: on for a scene, the
    /// playlist or a look edited by hand; off once the last cue is stopped.
    pub look_on: bool,
    /// The single tempo clock (see tempo.rs); times are seconds since `epoch`.
    pub tempo: tempo::TempoClock,
    pub epoch: Instant,
    /// Master live modifiers (live.rs), saved to live.json when changed.
    pub live: live::LiveModifiers,
    pub live_dirty: bool,
    /// User colour palettes (palettes.json).
    pub palettes: live::PaletteStore,
    /// MIDI controllers (midi/mod.rs); not part of any saved look.
    pub midi: midi::MidiState,
    /// Master LFO modulators (lfo.rs, lfos.json), applied every frame on
    /// top of the stored values.
    pub lfos: lfo::LfoStore,
    /// Audio routes (audio/routes.rs, audio_routes.json): analysis values
    /// and events shaped onto controls, every frame, on the same copies.
    pub routes: audio::routes::RouteStore,
    /// The four cue layers and the point budget (layers.rs), saved to
    /// layers.json when changed.
    pub mixer: layers::Mixer,
    pub mixer_dirty: bool,
    /// What the point budget did to the last frame.
    pub mix: layers::MixReport,
    /// Strobe limiter and beam horizon settings (safety.rs, safety.json).
    pub safety: safety::SafetyStore,
    /// What the strobe limiter and horizon did on the last frame.
    pub strobe: safety::StrobeStatus,
    /// Per-output power caps and projector sheets (power.rs,
    /// outputs.json). Machine settings: never in a look or a project.
    pub outputs: power::OutputStore,
    /// Evolving cues on show in the last frame: (animator id, where it is).
    /// Id 0 is the manual look.
    pub evolving: Vec<(u64, evolving::Progress)>,
    /// The timeline player (timeline.rs): its events play through the
    /// same layers and live stage as the cues. Never arms anything.
    pub timeline: timeline::Player,
    /// Saved shows, `studio-data/shows/`.
    pub shows: timeline::ShowStore,
    /// The operator's figures (figures.rs, `studio-data/figures/`), also
    /// the cues of the « Figures » page.
    pub figures: figures::FigureStore,
    /// The user's songs, `studio-data/media/audio/` (T-161).
    pub media: Arc<audio::media::MediaStore>,
    /// The show's song: shared with the song-playback thread and the
    /// output callback (their own short locks, never this one).
    pub song: Arc<audio::playback::SongHub>,
    /// Engine side of the song clock (audio/playback.rs).
    pub song_sync: audio::playback::SongSync,
    /// UI heartbeats and hold-to-run (presence.rs, presence.json).
    pub presence: presence::Presence,
    /// Engine ticks, read lock-free by the watchdog (watchdog.rs).
    pub health: Arc<watchdog::EngineHealth>,
    /// The open project (project.rs, T-286). Never holds calibration,
    /// safety or arming.
    pub project: project::ProjectState,
    /// `--test-hooks`: allows `POST /api/test/stall`.
    pub test_hooks: bool,
    /// A pending simulated stall (ms): the engine sleeps this long while
    /// holding the lock, the worst case for the watchdog.
    pub test_stall_ms: u64,
}

impl Shared {
    /// Arm request, after catching up with the e-stop latch, the watchdog
    /// and the heartbeats.
    pub fn request_arm(&mut self, src: interlock::ArmSource) -> Result<(), Vec<String>> {
        self.sync_safety(Instant::now());
        self.gate.request_arm(src)?;
        self.health.note_armed();
        Ok(())
    }

    /// The only way MIDI can arm (see `midi::engine::frame`): the MIDI
    /// engine has checked its opt-in, the option is re-checked here, and
    /// the gate still applies every interlock and the e-stop.
    pub fn request_arm_midi_opt_in(&mut self) -> Result<(), Vec<String>> {
        if !self.midi.store.devices.safety.allow_arm {
            return Err(vec!["L'armement depuis le MIDI est désactivé".into()]);
        }
        self.sync_safety(Instant::now());
        self.gate.request_arm_midi_opt_in()?;
        self.health.note_armed();
        Ok(())
    }

    /// Brings the gate in line with everything that can disarm from
    /// outside it: the e-stop latch, a watchdog trip, operator presence.
    /// Returns whether hold-to-run lets the output emit.
    pub fn sync_safety(&mut self, now: Instant) -> bool {
        self.gate.sync_estop(&self.estop);
        if self.health.take_trip() {
            self.gate.disarm(interlock::DisarmReason::EngineStall, interlock::ArmSource::System);
        }
        self.sync_presence(now)
    }

    /// Operator presence (T-252). The last page gone: held flashes end and
    /// the laser disarms with reason « Interface perdue »; no page at all
    /// blocks arming. Hold-to-run released too long disarms too. Nothing
    /// here can arm.
    pub fn sync_presence(&mut self, now: Instant) -> bool {
        if !self.presence.enforced {
            return true;
        }
        let v = self.presence.update(now, self.gate.is_armed());
        if v.ui_lost {
            // The known cue-modes issue: a flash held in a page that died.
            controls::release_held(self);
        }
        if !v.ui_alive && self.gate.is_armed() {
            self.gate.disarm(interlock::DisarmReason::UiLost, interlock::ArmSource::System);
        }
        self.gate.set_interlock(interlock::UI_ALIVE, v.ui_alive);
        if v.hold_expired && self.gate.is_armed() {
            self.gate.disarm(interlock::DisarmReason::HoldReleased, interlock::ArmSource::System);
        }
        v.hold_ok
    }

    /// Trips the emergency stop and records it in the gate.
    pub fn emergency_stop(&mut self, src: interlock::ArmSource) {
        self.estop.trip(src);
        self.gate.sync_estop(&self.estop);
    }

    /// Seconds since startup: the time base of the tempo clock.
    pub fn now_s(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// *Nouveau morceau*: the estimator forgets its history and guide, the
    /// clock its follower history and guide (its BPM and phase stay).
    pub fn tempo_new_track(&mut self) {
        self.audio_in.new_track();
        self.audio_in.set_guide(None);
        self.tempo.new_track();
    }

    /// The tempo clock now, for the timeline player.
    pub fn timeline_clock(&self) -> timeline::Clock {
        let t = self.now_s();
        timeline::Clock { t, beat: self.tempo.beat_at(t), bpm: self.tempo.bpm, beats_per_bar: self.tempo.beats_per_bar }
    }
}

pub struct Playlist {
    pub index: usize,
    pub started: Instant,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    if let Some(name) = &cli.decode {
        // The decoder child (audio/isolate.rs): no panic hook, no output,
        // nothing but one file read and a reply on stdout.
        let want = if cli.decode_peaks { audio::isolate::Want::Peaks } else { audio::isolate::Want::Samples };
        std::process::exit(audio::isolate::child_main(&cli.data_dir, name, want, cli.test_hooks));
    }
    if cli.list_controls {
        print!("{}", controls::markdown(&controls::ControlRegistry::build(&presets::catalog())));
        return Ok(());
    }

    std::fs::create_dir_all(&cli.data_dir)
        .with_context(|| format!("failed to create {}", cli.data_dir.display()))?;

    let output: Option<Box<dyn Output>> = match (&cli.device, &cli.test_output) {
        (Some(device), _) => {
            let out = DacOutput::open(device, cli.pps)?;
            println!("Laser output: {}", out.name());
            Some(Box::new(out))
        }
        (None, Some(path)) => {
            println!("--test-output: fake output logged to {} (no laser).", path.display());
            Some(Box::new(FileLogOutput::create(path)?))
        }
        (None, None) => {
            println!("No --device given: preview only (no laser output).");
            None
        }
    };

    let state = startup_state(&cli, output.as_deref());
    let (estop, health) = (Arc::clone(&state.estop), Arc::clone(&state.health));
    let sim = state.midi.sim.clone();
    let audio_hub = Arc::clone(&state.audio_in);
    let (song_hub, media) = (Arc::clone(&state.song), Arc::clone(&state.media));
    if let Some(kill) = output.as_ref().and_then(|o| o.kill_switch()) {
        estop.set_kill_switch(kill);
    }
    let shared = Arc::new(Mutex::new(state));
    let running = Arc::new(AtomicBool::new(true));

    // A panic anywhere: cut the output first (the DAC kill switch, no lock),
    // then let the default report run and every loop wind down. The engine's
    // output stage also blanks and disarms as its thread unwinds. The one
    // exception is a contained audio decoder thread (only used if the
    // decoder child can't be started): its panic is an import error.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new({
        let (estop, health, running) = (Arc::clone(&estop), Arc::clone(&health), Arc::clone(&running));
        move |info| {
            if !audio::isolate::panic_is_contained() {
                watchdog::on_panic(&estop, &health, &running);
            }
            default_hook(info);
        }
    }));

    // Ctrl+C and SIGTERM: cut the output at once, then shut down cleanly
    // (the engine disarms and sends dark frames before closing the output).
    ctrlc::set_handler({
        let (estop, running) = (Arc::clone(&estop), Arc::clone(&running));
        move || {
            estop.kill_output();
            running.store(false, Ordering::SeqCst);
        }
    })
    .context("failed to install Ctrl+C handler")?;

    let engine = std::thread::spawn({
        let shared = Arc::clone(&shared);
        let running = Arc::clone(&running);
        let (live_path, layers_path) = (cli.data_dir.join("live.json"), cli.data_dir.join("layers.json"));
        move || run_engine(shared, output, running, live_path, layers_path)
    });
    let watchdog = watchdog::spawn(Arc::clone(&health), Arc::clone(&estop), Arc::clone(&running));

    let midi_thread = if let Some(sim) = sim {
        println!("--midi-test: simulated APC40 mkII only, MIDI injection enabled (tests only).");
        midi::worker::spawn_on(sim, Arc::clone(&shared), Arc::clone(&running))
    } else if cli.no_midi {
        println!("--no-midi: MIDI disabled.");
        None
    } else {
        midi::worker::spawn(Arc::clone(&shared), Arc::clone(&running))
    };

    let audio_threads = if cli.no_audio {
        println!("--no-audio: no audio input opened (the browser source still works), no song played (the timeline runs on the system clock).");
        Vec::new()
    } else {
        let mut threads = audio::worker::spawn(audio_hub, Arc::clone(&running));
        threads.push(audio::playback::spawn(song_hub, media, Arc::clone(&running)));
        threads
    };

    let addr = format!("127.0.0.1:{}", cli.port);
    println!("Studio: open http://{addr}/ in your browser - Ctrl+C to quit");
    let served = web::run(&addr, shared, estop, cli.data_dir.join("calibration.json"), Arc::clone(&running));

    running.store(false, Ordering::SeqCst);
    // The engine closes the output; don't wait forever if it is stuck (the
    // kill switch has already cut a real DAC).
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    while !engine.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if engine.is_finished() {
        engine.join().ok();
    } else {
        log::error!("engine thread did not stop in {SHUTDOWN_WAIT:?}: exiting anyway");
    }
    watchdog.join().ok();
    if let Some(midi_thread) = midi_thread {
        midi_thread.join().ok(); // lets it switch the APC LEDs off
    }
    // The audio threads close the input within ~50 ms; a device stuck in
    // CoreAudio must not hold the exit.
    let deadline = Instant::now() + AUDIO_SHUTDOWN_WAIT;
    while audio_threads.iter().any(|t| !t.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    served
}

/// How long `main` waits for the engine to close the output at shutdown.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
const AUDIO_SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

/// The state the studio starts with. Whatever the command line and the
/// files in the data directory say, it is disarmed (reason « Démarrage »):
/// arming is not part of any saved state, and there is no option for it.
fn startup_state(cli: &Cli, output: Option<&dyn Output>) -> Shared {
    let mut gate = ArmGate::default();
    if cli.test_interlock {
        gate.register(interlock::TEST, "Verrou de test (--test-interlock)", false);
    }
    // No page has beaten yet: arming waits for the UI (T-252).
    gate.register(interlock::UI_ALIVE, "Aucune interface ouverte (battement perdu)", false);

    let sim = cli.midi_test.then(|| {
        let sim = midi::testing::SimMidi::default();
        sim.plug(midi::testing::TEST_MK2_PORT, midi::testing::FakeApc::new(midi::Model::Apc40Mk2));
        sim
    });

    let presets = presets::catalog();
    let controls = controls::ControlRegistry::build(&presets);
    let lfos = lfo::LfoStore::load_or_create(cli.data_dir.join("lfos.json"), &controls);
    let routes = audio::routes::RouteStore::load_or_create(cli.data_dir.join("audio_routes.json"), &controls);
    let epoch = Instant::now();
    let mut state = Shared {
        settings: Settings::default(),
        calibration: web::load_calibration(&cli.data_dir.join("calibration.json")),
        audio: AudioFeatures::default(),
        audio_at: Instant::now(),
        audio_in: Arc::new(audio::AudioHub::load(&cli.data_dir, epoch, !cli.no_audio, cli.audio_device.as_deref())),
        gate,
        estop: Arc::new(EStop::default()),
        frame: Vec::new(),
        output_lit: 0,
        output_name: output.map(|o| o.name().to_string()),
        output_error: None,
        pps: cli.pps,
        scenes: SceneStore::load_or_create(cli.data_dir.join("scenes.json")),
        playlist: None,
        controls,
        presets,
        cue_page: 0,
        active_cue: None,
        settings_rev: 0,
        deck: cues::CueDeck::load(cli.data_dir.join("grid.json")),
        look_on: true,
        tempo: tempo::TempoClock::default(),
        epoch,
        live: load_json(&cli.data_dir.join("live.json")),
        live_dirty: false,
        palettes: live::PaletteStore::load_or_create(cli.data_dir.join("palettes.json")),
        midi: {
            let mut m = midi::MidiState::new(!cli.no_midi || cli.midi_test, midi::profile::ProfileStore::load(cli.data_dir.join("midi")));
            m.sim = sim.clone();
            m
        },
        lfos,
        routes,
        mixer: {
            let mut m: layers::Mixer = load_json(&cli.data_dir.join("layers.json"));
            m.sanitize();
            m
        },
        mixer_dirty: false,
        mix: layers::MixReport::default(),
        safety: safety::SafetyStore::load_or_create(cli.data_dir.join("safety.json")),
        strobe: safety::StrobeStatus::default(),
        outputs: power::OutputStore::load_or_create(cli.data_dir.join("outputs.json")),
        evolving: Vec::new(),
        timeline: timeline::Player::default(),
        shows: timeline::ShowStore::new(cli.data_dir.join("shows")),
        figures: figures::FigureStore::load(cli.data_dir.join("figures")),
        media: Arc::new(audio::media::MediaStore::new(&cli.data_dir).isolated(audio::isolate::Isolation::child(
            &cli.data_dir,
            cli.test_hooks,
            cli.test_decode_timeout_ms.map_or(audio::isolate::DECODE_TIMEOUT, Duration::from_millis),
        ))),
        song: Arc::new(audio::playback::SongHub::new(epoch, !cli.no_audio)),
        song_sync: Default::default(),
        presence: presence::Presence::load(cli.data_dir.join("presence.json")),
        health: Arc::new(watchdog::EngineHealth::default()),
        project: project::ProjectState::load(&cli.data_dir),
        test_hooks: cli.test_hooks,
        test_stall_ms: 0,
    };
    figures::refresh(&mut state);
    // First start: the existing data becomes a « Sans titre » project.
    project::startup(&mut state);
    state
}

fn run_engine(
    shared: Arc<Mutex<Shared>>,
    output: Option<Box<dyn Output>>,
    running: Arc<AtomicBool>,
    live_path: PathBuf,
    layers_path: PathBuf,
) {
    // One animator per playing cue instance (0 = the manual look), so
    // every cue keeps its own motion and a restart starts it over.
    let mut animators: HashMap<u64, Animator> = HashMap::new();
    let mut live_state = live::LiveState::default();
    // Rotation state of timeline events with their own modifiers.
    let mut event_live: HashMap<u64, live::LiveState> = HashMap::new();
    let mut frames_since_save = 0u32;
    let mut last = Instant::now();
    // If this thread panics, unwinding drops the stage: dark frame + disarm.
    let mut stage = OutputStage::new(output);
    let mut limiter = safety::StrobeLimiter::default();

    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        let (looks, show_cues, starts, clock, mixer, calibration, audio, armed, hold_ok, live, user_palettes, estop, health, t, safety_cfg, caps) = {
            let mut s = shared.lock().unwrap();
            // E-stop latch, watchdog trip, heartbeats and hold-to-run.
            let hold_ok = s.sync_safety(now);
            s.health.tick(s.gate.is_armed());
            if s.test_stall_ms > 0 {
                // `--test-hooks` only: a stall while holding the lock.
                let stall = Duration::from_millis(std::mem::take(&mut s.test_stall_ms));
                std::thread::sleep(stall);
            }
            frames_since_save += 1;
            if (s.live_dirty || s.mixer_dirty) && frames_since_save >= 60 {
                // At most once a second, so MIDI faders don't hammer the disk.
                if s.live_dirty {
                    save_json(&live_path, &s.live);
                }
                if s.mixer_dirty {
                    save_json(&layers_path, &s.mixer);
                }
                s.live_dirty = false;
                s.mixer_dirty = false;
                frames_since_save = 0;
            }
            advance_playlist(&mut s);
            // MIDI faders/encoders: at most one write per control per frame.
            midi::engine::frame(&mut s, now);
            // Native capture, else the browser's features, else silence
            // (falling, not jumping): one snapshot for the whole frame.
            let (audio, _) = s.audio_in.frame(s.audio, s.audio_at, now);
            let t = s.now_s();
            // Tempo auto (T-234): the clock follows the detection's proposal
            // on its own terms. Only the one clock; arming untouched.
            if s.tempo.source == tempo::TempoSource::Audio {
                let est = s.audio_in.fresh_tempo(now).unwrap_or_default();
                s.tempo.apply_detection(&est, t);
            }
            let clock = engine::BeatClock { beat: s.tempo.beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar };
            live_state.set_clock(t, clock.beat);
            // LFOs, then audio routes, move copies: the stored values stay
            // the operator's base. The Temps ↔ Audio crossfader shares them.
            let (mut settings, mut live) = (s.settings.clone(), s.live.clone());
            let time_share = s.routes.routing().time_share();
            lfo::modulate_scaled(s.lfos.list(), &s.controls, &mut settings, &mut live, t, clock.beat, time_share);
            let beat_len_s = (60.0 / clock.bpm.max(1.0)) as f32;
            if s.routes.apply(&audio, dt, beat_len_s, &mut settings, &mut live) {
                lfo::recolor_live(&mut live);
            }
            let looks = cues::layered_looks(&s.deck, &settings, s.look_on);
            let show_cues = timeline_cues(&mut s, t, &clock);
            let mixer = s.mixer.clone();
            // Launch beats, so a cue's beat-synced motion counts from its start.
            let starts: HashMap<u64, f64> = s.deck.active.iter().map(|a| (a.id, a.started_beat)).collect();
            let armed = s.gate.is_armed();
            (looks, show_cues, starts, clock, mixer, s.calibration, audio, armed, hold_ok, live, s.palettes.list().to_vec(), Arc::clone(&s.estop), Arc::clone(&s.health), t, s.safety.get(), s.outputs.active_limits())
        };

        live_state.advance(&live, dt, clock.bpm, clock.beats_per_bar);
        let anim_dt = dt * live.speed.clamp(0.0, 4.0);
        animators.retain(|id, _| looks.iter().any(|(_, i, _)| i == id) || show_cues.iter().any(|(c, _)| c.instance == *id));
        event_live.retain(|id, _| show_cues.iter().any(|(c, _)| c.instance == *id));
        // Timeline events first (under the cues played by hand on the same
        // layer), each on its own content clock; paused or held ones freeze.
        let mut rendered: Vec<(u8, Vec<Point>)> = show_cues
            .iter()
            .map(|(cue, settings)| {
                let animator = animators.entry(cue.instance).or_insert_with(|| Animator::starting_at(0.0));
                let content_clock = engine::BeatClock { beat: cue.content_beat, ..clock };
                let event_dt = if cue.frozen { 0.0 } else { anim_dt };
                let mut points = animator.render(settings, audio, event_dt, &content_clock);
                if cue.modifiers != live::LiveModifiers::default() {
                    let st = event_live.entry(cue.instance).or_default();
                    st.set_clock(t, clock.beat);
                    st.advance(&cue.modifiers, if cue.frozen { 0.0 } else { dt }, clock.bpm, clock.beats_per_bar);
                    points = live::apply(&points, &cue.modifiers, st, &user_palettes);
                }
                (cue.layer, points)
            })
            .collect();
        // Muted layers keep animating, so they come back in motion.
        rendered.extend(looks.iter().map(|(layer, id, settings)| {
            let animator = animators.entry(*id).or_insert_with(|| match starts.get(id) {
                Some(&beat) => Animator::starting_at(beat),
                None => Animator::default(),
            });
            (*layer, animator.render(settings, audio, anim_dt, &clock))
        }));
        let evolving: Vec<(u64, evolving::Progress)> =
            looks.iter().filter_map(|(_, id, _)| Some((*id, animators.get(id)?.progress()?.clone()))).collect();
        // Layers 1 → 4 with their dimmers, within the point budget.
        let (look, mix) = layers::mix(rendered, &mixer);
        let frame: Vec<Point> = live::apply(&look, &live, &live_state, &user_palettes)
            .into_iter()
            .map(|p| {
                let (x, y) = calibration.apply(p.x, p.y);
                Point { x, y, ..p }
            })
            .collect();
        // Strobe limiter and beam horizon (T-101): on the finished,
        // calibrated frame, so no layer, live move, LFO, gate or flash can
        // get past it; the preview shows the limited frame too.
        let mut frame = safety::apply(frame, t, &safety_cfg, &mut limiter);
        // Power caps of the output (T-254), after everything else in the
        // safety stage: they can only reduce, and a lowered cap applies
        // from this tick. The preview shows the capped frame too.
        power::cap(&mut frame, &caps);

        // Last stage: the gate. `armed` was read under the lock at the top
        // of the frame; the e-stop latch and a watchdog trip (this frame
        // stalled) are re-read here, lock-free.
        let emitted = stage.emit(&frame, armed && !health.is_tripped(), hold_ok, &estop);
        let mut s = shared.lock().unwrap();
        if let Some(error) = emitted.arm_change {
            s.output_error = error;
        }
        s.output_lit = emitted.lit;
        s.frame = frame;
        s.mix = mix;
        s.strobe = limiter.status();
        s.evolving = evolving;
        drop(s);

        std::thread::sleep(FRAME_INTERVAL.saturating_sub(now.elapsed()));
    }

    // Clean shutdown: disarm the gate (recorded), then the output goes dark
    // and closes. A poisoned lock still holds the gate.
    shared.lock().unwrap_or_else(|e| e.into_inner()).gate.disarm(interlock::DisarmReason::Shutdown, interlock::ArmSource::System);
    stage.shutdown();
}

pub fn load_json<T: serde::de::DeserializeOwned + Default>(path: &std::path::Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_json<T: serde::Serialize>(path: &std::path::Path, value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                log::warn!("failed to save {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("failed to serialize {}: {e}", path.display()),
    }
}

/// The timeline's events for this frame, with their looks. An emergency
/// stop halts the timeline (it never resumes by itself), and so its song;
/// otherwise the song tells the timeline where it is (T-161) and its
/// events join the cues, through the same mix, live stage and gate.
fn timeline_cues(s: &mut Shared, t: f64, clock: &engine::BeatClock) -> Vec<(timeline::TimelineCue, Settings)> {
    let c = timeline::Clock { t, beat: clock.beat, bpm: clock.bpm, beats_per_bar: clock.beats_per_bar };
    if s.estop.is_latched() {
        s.timeline.halt(&c);
    }
    let song = Arc::clone(&s.song);
    s.song_sync.sync(&song, &mut s.timeline, &c);
    s.timeline.frame(&c).into_iter().filter_map(|cue| timeline::look_of(&cue, &s.presets).map(|look| (cue, look))).collect()
}

fn advance_playlist(s: &mut Shared) {
    let count = s.scenes.list().len();
    let Some(playlist) = s.playlist.as_mut() else { return };
    if count == 0 {
        s.playlist = None;
        return;
    }
    let current = &s.scenes.list()[playlist.index % count];
    if playlist.started.elapsed().as_secs_f32() < current.duration_secs.max(0.5) {
        return;
    }
    playlist.index = (playlist.index + 1) % count;
    playlist.started = Instant::now();
    let next = s.scenes.list()[playlist.index].settings.clone();
    if s.deck.active.is_empty() {
        s.settings = next;
        s.settings_rev += 1;
    } else {
        // Only flashes can play over the playlist (a latched cue stops it):
        // the next scene waits for them to end.
        s.deck.parked = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenes::Scene;

    #[test]
    fn midi_test_needs_no_midi_and_refuses_a_device() {
        let parse = |args: &[&str]| Cli::try_parse_from(std::iter::once("laser-studio").chain(args.iter().copied()));
        assert!(parse(&["--midi-test"]).is_err(), "never next to the real CoreMIDI");
        assert!(parse(&["--no-midi", "--midi-test", "--device", "auto"]).is_err(), "preview only");
        let cli = parse(&["--no-midi", "--midi-test"]).unwrap();
        assert!(cli.midi_test && cli.no_midi);
        assert!(!parse(&[]).unwrap().midi_test, "off by default");
    }
    use crate::test_support;
    use std::time::Duration;

    #[test]
    fn the_playlist_moves_on_with_a_new_look_and_no_cue() {
        let dir = std::env::temp_dir().join(format!("laser-studio-playlist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = test_support::shared();
        s.scenes = SceneStore::load_or_create(dir.join("scenes.json"));
        for (name, scale) in [("a", 0.2), ("b", 0.4)] {
            let settings = Settings { scale, ..Default::default() };
            s.scenes.upsert(Scene { name: name.into(), settings, duration_secs: 1.0 }).unwrap();
        }
        let first = s.scenes.list()[0].settings.clone();
        controls::show_look(&mut s, first);
        s.playlist = Some(Playlist { index: 0, started: Instant::now() - Duration::from_secs(2) });
        let rev = s.settings_rev;
        advance_playlist(&mut s);
        assert_eq!(s.playlist.as_ref().map(|p| p.index), Some(1));
        assert_eq!(s.settings.scale, s.scenes.list()[1].settings.scale);
        assert!(s.settings_rev > rev, "the page must reload the look");
        assert_eq!(s.active_cue, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn one_event_show(cue: &str) -> timeline::Show {
        let event = timeline::Event { id: 1, start: 0.0, len: 60.0, source: timeline::EventSource::Cue { id: cue.into() }, ..Default::default() };
        timeline::Show { name: "t".into(), tracks: vec![timeline::Track { layer: 2, events: vec![event], ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn the_timeline_feeds_the_frame_and_an_estop_halts_it_without_arming() {
        let mut s = test_support::shared();
        let cue = s.presets[0].id.clone();
        s.timeline.load(one_event_show(&cue));
        controls::timeline_play(&mut s).unwrap();
        let clock = |t: f64| engine::BeatClock { beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 };
        let t = s.now_s();
        let cues = timeline_cues(&mut s, t, &clock(t));
        assert_eq!(cues.len(), 1);
        assert_eq!((cues[0].0.layer, cues[0].1.content.clone()), (2, s.presets[0].settings.content.clone()));
        assert!(!s.gate.is_armed(), "playing a show never arms");

        s.emergency_stop(interlock::ArmSource::Keyboard);
        assert!(timeline_cues(&mut s, t + 1.0, &clock(t + 1.0)).is_empty(), "Échap: nothing more from the timeline");
        assert!(!s.timeline.is_playing());
        assert!(timeline_cues(&mut s, t + 2.0, &clock(t + 2.0)).is_empty(), "and it doesn't come back by itself");
        assert!(controls::timeline_play(&mut s).is_err(), "refused while the e-stop is latched");
        s.gate.reset_estop(&s.estop.clone());
        controls::timeline_play(&mut s).unwrap();
        assert!(!s.gate.is_armed(), "resetting the e-stop and playing again doesn't arm");
    }

    use crate::cues::ClickMode;
    use crate::interlock::{ArmSource, UI_ALIVE};
    use crate::presence::{Presence, PresenceSettings};

    /// A `Shared` that enforces presence like the real studio.
    fn with_presence(settings: PresenceSettings) -> Shared {
        let mut s = test_support::shared();
        s.presence = Presence::new(settings, true);
        s.gate.register(UI_ALIVE, "Aucune interface ouverte", false);
        s
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn reason(s: &Shared) -> String {
        s.gate.status(&s.estop).last_disarm.unwrap().reason
    }

    #[test]
    fn no_page_no_arming() {
        let mut s = with_presence(PresenceSettings::default());
        assert!(s.request_arm(ArmSource::Ui).is_err(), "no heartbeat yet");
        s.presence.beat("page", Instant::now(), true, false);
        s.request_arm(ArmSource::Ui).unwrap();
        assert!(s.gate.is_armed());
    }

    #[test]
    fn a_lost_heartbeat_disarms_with_its_reason_and_releases_held_flashes() {
        let t0 = Instant::now();
        let mut s = with_presence(PresenceSettings::default());
        s.presence.beat("page", t0, true, false);
        s.sync_safety(t0);
        s.request_arm(ArmSource::Keyboard).unwrap();
        assert!(controls::press_cue(&mut s, "tunnels-001", Some(ClickMode::Flash), true));
        assert!(s.deck.active.iter().any(|a| a.held));
        s.sync_safety(t0 + ms(1_900));
        assert!(s.gate.is_armed(), "still within the timeout");
        s.sync_safety(t0 + ms(2_000));
        assert!(!s.gate.is_armed());
        assert_eq!(reason(&s), "ui_lost");
        assert_eq!(s.gate.status(&s.estop).last_disarm.unwrap().reason_fr, "Interface perdue");
        assert!(s.deck.active.is_empty(), "the held flash is released");
        assert_eq!(s.active_cue, None);
        // Nothing comes back by itself when a page returns.
        s.presence.beat("page", t0 + ms(3_000), true, false);
        s.sync_safety(t0 + ms(3_000));
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn a_latched_cue_survives_the_lost_page() {
        let t0 = Instant::now();
        let mut s = with_presence(PresenceSettings::default());
        s.presence.beat("page", t0, true, false);
        s.sync_safety(t0);
        assert!(controls::press_cue(&mut s, "tunnels-001", Some(ClickMode::Toggle), true));
        s.sync_safety(t0 + ms(5_000));
        assert_eq!(s.deck.active.len(), 1, "only held flashes are released");
    }

    #[test]
    fn two_pages_one_closed_stays_armed() {
        let t0 = Instant::now();
        let mut s = with_presence(PresenceSettings::default());
        s.presence.beat("a", t0, true, false);
        s.presence.beat("b", t0, true, false);
        s.request_arm(ArmSource::Ui).unwrap();
        s.presence.leave("a");
        for step in 1..=10 {
            let now = t0 + ms(step * 500);
            s.presence.beat("b", now, true, false);
            s.sync_safety(now);
        }
        assert!(s.gate.is_armed());
    }

    #[test]
    fn hold_to_run_blanks_then_disarms_after_the_release_limit() {
        let t0 = Instant::now();
        let mut s = with_presence(PresenceSettings { hold_to_run: true, ..Default::default() });
        s.presence.beat("page", t0, true, false);
        s.request_arm(ArmSource::Ui).unwrap();
        assert!(!s.sync_safety(t0), "armed but not held: black");
        s.presence.beat("page", t0 + ms(500), true, true);
        assert!(s.sync_safety(t0 + ms(500)), "held: emits");
        // Released at 1 s: disarmed 10 s later.
        for step in 2..=22 {
            let now = t0 + ms(step * 500);
            s.presence.beat("page", now, true, false);
            assert!(!s.sync_safety(now));
            assert_eq!(s.gate.is_armed(), step < 22, "at {} ms", step * 500);
        }
        assert_eq!(reason(&s), "hold_released");
    }

    #[test]
    fn a_watchdog_trip_disarms_with_engine_stall() {
        let mut s = test_support::shared();
        s.request_arm(ArmSource::Ui).unwrap();
        s.health.trip();
        s.sync_safety(Instant::now());
        assert!(!s.gate.is_armed());
        assert_eq!(reason(&s), "engine_stall");
        assert_eq!(s.gate.status(&s.estop).last_disarm.unwrap().reason_fr, "Moteur bloqué");
        assert!(!s.health.is_tripped(), "recorded once");
        s.request_arm(ArmSource::Ui).unwrap();
        assert!(s.gate.is_armed(), "the operator can re-arm");
    }

    #[test]
    fn presence_sync_never_arms() {
        let t0 = Instant::now();
        for hold_to_run in [false, true] {
            let mut s = with_presence(PresenceSettings { hold_to_run, ..Default::default() });
            for step in 0..40u64 {
                let now = t0 + ms(step * 250);
                if step % 3 != 0 {
                    s.presence.beat("page", now, step % 2 == 0, step % 5 == 0);
                }
                s.presence.set_midi_hold(step % 7 == 0);
                s.sync_safety(now);
                assert!(!s.gate.is_armed());
            }
        }
    }

    /// T-253: no command-line option and no file in the data directory can
    /// make the studio start armed.
    #[test]
    fn no_startup_path_is_armed() {
        use clap::CommandFactory;
        for arg in Cli::command().get_arguments() {
            let id = arg.get_id().as_str().to_lowercase();
            assert!(!id.contains("arm") && !id.contains("emit"), "unexpected option --{id}");
        }
        let dir = std::env::temp_dir().join(format!("laser-studio-startup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Files that try their luck with arming fields.
        for name in ["presence.json", "live.json", "layers.json", "grid.json", "calibration.json", "scenes.json", "lfos.json", "audio_routes.json", "palettes.json"] {
            std::fs::write(dir.join(name), r#"{"armed": true, "arm": true, "on": true, "hold_to_run": false}"#).unwrap();
        }
        let data_dir = dir.to_str().unwrap();
        let variants: [&[&str]; 4] = [
            &[],
            &["--test-interlock"],
            &["--test-hooks", "--pps", "12000"],
            &["--test-hooks", "--test-interlock", "--port", "0"],
        ];
        for extra in variants {
            let args = [&["laser-studio", "--no-midi", "--data-dir", data_dir][..], extra].concat();
            let cli = Cli::try_parse_from(&args).unwrap();
            let mut s = startup_state(&cli, None);
            assert!(!s.gate.is_armed(), "{args:?}");
            let status = s.gate.status(&s.estop);
            assert!(!status.armed);
            assert_eq!(status.last_disarm.unwrap().reason, "startup");
            assert!(s.presence.enforced);
            // Without a page, even a direct request is refused.
            assert!(s.request_arm(ArmSource::Keyboard).is_err(), "{args:?}");
        }
        assert!(Cli::try_parse_from(["laser-studio", "--device", "x", "--test-output", "/tmp/x"]).is_err(), "the fake output never goes with a real device");
        let _ = std::fs::remove_dir_all(dir);
    }
}
