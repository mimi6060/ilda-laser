//! Laser studio: a live laser show controller that runs on the Mac, with
//! its UI in the browser. Pick a shape or text, animate it, make it react
//! to music from the microphone, save looks as scenes and run them as a
//! playlist - all with a live preview, with or without a laser attached.
//!
//! One engine thread renders a frame 60 times a second from the shared
//! state and hands it to the preview and (when one is configured) to the
//! laser output. The HTTP handlers in `web.rs` only ever edit that shared
//! state.

mod beat;
mod controls;
mod cues;
mod engine;
mod font;
mod generators;
mod lfo;
mod interlock;
mod live;
mod midi;
mod output;
mod patterns;
mod presets;
mod scenes;
mod tempo;
mod web;

#[cfg(test)]
mod test_support;

use anyhow::{Context, Result};
use clap::Parser;
use engine::{Animator, AudioFeatures, Calibration, Settings};
use interlock::{ArmGate, EStop};
use output::{DacOutput, Output, OutputStage};
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
    /// Testing only: add an interlock that is never satisfied, so arming
    /// is always refused (e2e tests of the refusal message).
    #[arg(long, hide = true)]
    test_interlock: bool,
}

/// Audio features older than this are treated as silence (the browser tab
/// was closed or the mic stopped).
const AUDIO_STALE: Duration = Duration::from_millis(500);

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

pub struct Shared {
    pub settings: Settings,
    pub calibration: Calibration,
    pub audio: AudioFeatures,
    pub audio_at: Instant,
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
}

impl Shared {
    /// Arm request, after catching up with the e-stop latch.
    pub fn request_arm(&mut self, src: interlock::ArmSource) -> Result<(), Vec<String>> {
        self.gate.sync_estop(&self.estop);
        self.gate.request_arm(src)
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
}

pub struct Playlist {
    pub index: usize,
    pub started: Instant,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    if cli.list_controls {
        print!("{}", controls::markdown(&controls::ControlRegistry::build(&presets::catalog())));
        return Ok(());
    }

    std::fs::create_dir_all(&cli.data_dir)
        .with_context(|| format!("failed to create {}", cli.data_dir.display()))?;
    let calibration_path = cli.data_dir.join("calibration.json");
    let live_path = cli.data_dir.join("live.json");

    let output: Option<Box<dyn Output>> = match &cli.device {
        Some(device) => {
            let out = DacOutput::open(device, cli.pps)?;
            println!("Laser output: {}", out.name());
            Some(Box::new(out))
        }
        None => {
            println!("No --device given: preview only (no laser output).");
            None
        }
    };

    let estop = Arc::new(EStop::default());
    if let Some(kill) = output.as_ref().and_then(|o| o.kill_switch()) {
        estop.set_kill_switch(kill);
    }
    let mut gate = ArmGate::default();
    if cli.test_interlock {
        gate.register(interlock::TEST, "Verrou de test (--test-interlock)", false);
    }

    let presets = presets::catalog();
    let controls = controls::ControlRegistry::build(&presets);
    let lfos = lfo::LfoStore::load_or_create(cli.data_dir.join("lfos.json"), &controls);
    let shared = Arc::new(Mutex::new(Shared {
        settings: Settings::default(),
        calibration: web::load_calibration(&calibration_path),
        audio: AudioFeatures::default(),
        audio_at: Instant::now(),
        gate,
        estop: Arc::clone(&estop),
        frame: Vec::new(),
        output_lit: 0,
        output_name: output.as_ref().map(|o| o.name().to_string()),
        output_error: None,
        pps: cli.pps,
        scenes: SceneStore::load_or_create(cli.data_dir.join("scenes.json")),
        playlist: None,
        controls,
        presets,
        cue_page: 0,
        active_cue: None,
        deck: cues::CueDeck::load(cli.data_dir.join("grid.json")),
        look_on: true,
        tempo: tempo::TempoClock::default(),
        epoch: Instant::now(),
        live: load_json(&live_path),
        live_dirty: false,
        palettes: live::PaletteStore::load_or_create(cli.data_dir.join("palettes.json")),
        midi: midi::MidiState::new(!cli.no_midi, midi::profile::ProfileStore::load(cli.data_dir.join("midi"))),
        lfos,
    }));

    let running = Arc::new(AtomicBool::new(true));
    ctrlc::set_handler({
        let running = Arc::clone(&running);
        move || running.store(false, Ordering::SeqCst)
    })
    .context("failed to install Ctrl+C handler")?;

    let engine = std::thread::spawn({
        let shared = Arc::clone(&shared);
        let running = Arc::clone(&running);
        move || run_engine(shared, output, running, live_path)
    });

    let midi_thread = if cli.no_midi {
        println!("--no-midi: MIDI disabled.");
        None
    } else {
        midi::worker::spawn(Arc::clone(&shared), Arc::clone(&running))
    };

    let addr = format!("127.0.0.1:{}", cli.port);
    println!("Studio: open http://{addr}/ in your browser - Ctrl+C to quit");
    web::run(&addr, shared, estop, calibration_path, Arc::clone(&running))?;

    running.store(false, Ordering::SeqCst);
    engine.join().ok();
    if let Some(midi_thread) = midi_thread {
        midi_thread.join().ok(); // lets it switch the APC LEDs off
    }
    Ok(())
}

fn run_engine(shared: Arc<Mutex<Shared>>, output: Option<Box<dyn Output>>, running: Arc<AtomicBool>, live_path: PathBuf) {
    // One animator per playing cue instance (0 = the manual look), so
    // every cue keeps its own motion and a restart starts it over.
    let mut animators: HashMap<u64, Animator> = HashMap::new();
    let mut live_state = live::LiveState::default();
    let mut frames_since_save = 0u32;
    let mut last = Instant::now();
    let mut stage = OutputStage::new(output);

    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        let (looks, starts, clock, calibration, audio, armed, live, user_palettes, estop) = {
            let mut s = shared.lock().unwrap();
            let shared_state = &mut *s;
            shared_state.gate.sync_estop(&shared_state.estop);
            frames_since_save += 1;
            if s.live_dirty && frames_since_save >= 60 {
                // At most once a second, so MIDI faders don't hammer the disk.
                save_json(&live_path, &s.live);
                s.live_dirty = false;
                frames_since_save = 0;
            }
            advance_playlist(&mut s);
            let audio = if s.audio_at.elapsed() < AUDIO_STALE {
                s.audio
            } else {
                AudioFeatures { beat: s.audio.beat, ..Default::default() }
            };
            let t = s.now_s();
            let clock = engine::BeatClock { beat: s.tempo.beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar };
            live_state.set_clock(t, clock.beat);
            // LFOs move copies: the stored values stay the operator's base.
            let (mut settings, mut live) = (s.settings.clone(), s.live.clone());
            lfo::modulate(s.lfos.list(), &s.controls, &mut settings, &mut live, t, clock.beat);
            let looks = cues::looks(&s.deck, &settings, s.look_on);
            // Launch beats, so a cue's beat-synced motion counts from its start.
            let starts: HashMap<u64, f64> = s.deck.active.iter().map(|a| (a.id, a.started_beat)).collect();
            (looks, starts, clock, s.calibration, audio, s.gate.is_armed(), live, s.palettes.list().to_vec(), Arc::clone(&s.estop))
        };

        live_state.advance(&live, dt, clock.bpm, clock.beats_per_bar);
        let anim_dt = dt * live.speed.clamp(0.0, 4.0);
        animators.retain(|id, _| looks.iter().any(|(i, _)| i == id));
        let rendered = looks
            .iter()
            .map(|(id, settings)| {
                let animator = animators.entry(*id).or_insert_with(|| match starts.get(id) {
                    Some(&beat) => Animator::starting_at(beat),
                    None => Animator::default(),
                });
                animator.render(settings, audio, anim_dt, &clock)
            })
            .collect();
        let look = engine::join_looks(rendered);
        let frame: Vec<Point> = live::apply(&look, &live, &live_state, &user_palettes)
            .into_iter()
            .map(|p| {
                let (x, y) = calibration.apply(p.x, p.y);
                Point { x, y, ..p }
            })
            .collect();

        // Last stage: the gate. `armed` was read under the lock at the top
        // of the frame; the e-stop latch is re-read here, lock-free.
        let emitted = stage.emit(&frame, armed, &estop);
        let mut s = shared.lock().unwrap();
        if let Some(error) = emitted.arm_change {
            s.output_error = error;
        }
        s.output_lit = emitted.lit;
        s.frame = frame;
        drop(s);

        std::thread::sleep(FRAME_INTERVAL.saturating_sub(now.elapsed()));
    }

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
    } else {
        // Only flashes can play over the playlist (a latched cue stops it):
        // the next scene waits for them to end.
        s.deck.parked = Some(next);
    }
}
