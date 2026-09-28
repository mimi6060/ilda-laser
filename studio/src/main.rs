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
mod evolving;
mod fans;
mod font;
mod generators;
mod layers;
mod lfo;
mod interlock;
mod live;
mod midi;
mod output;
mod patterns;
mod presets;
mod safety;
mod scenes;
mod tempo;
mod timeline;
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
    /// Testing only (e2e, T-209): plug in a simulated APC40 mkII
    /// (« Test APC40 mkII ») and enable `POST /api/midi/inject` and
    /// `GET /api/midi/sent`. Needs --no-midi (no real MIDI port is ever
    /// opened) and refuses --device (preview only).
    #[arg(long, hide = true, requires = "no_midi", conflicts_with = "device")]
    midi_test: bool,
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
    /// Evolving cues on show in the last frame: (animator id, where it is).
    /// Id 0 is the manual look.
    pub evolving: Vec<(u64, evolving::Progress)>,
    /// The timeline player (timeline.rs): its events play through the
    /// same layers and live stage as the cues. Never arms anything.
    pub timeline: timeline::Player,
    /// Saved shows, `studio-data/shows/`.
    pub shows: timeline::ShowStore,
}

impl Shared {
    /// Arm request, after catching up with the e-stop latch.
    pub fn request_arm(&mut self, src: interlock::ArmSource) -> Result<(), Vec<String>> {
        self.gate.sync_estop(&self.estop);
        self.gate.request_arm(src)
    }

    /// The only way MIDI can arm (see `midi::engine::frame`): the MIDI
    /// engine has checked its opt-in, the option is re-checked here, and
    /// the gate still applies every interlock and the e-stop.
    pub fn request_arm_midi_opt_in(&mut self) -> Result<(), Vec<String>> {
        if !self.midi.store.devices.safety.allow_arm {
            return Err(vec!["L'armement depuis le MIDI est désactivé".into()]);
        }
        self.gate.sync_estop(&self.estop);
        self.gate.request_arm_midi_opt_in()
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
    if cli.list_controls {
        print!("{}", controls::markdown(&controls::ControlRegistry::build(&presets::catalog())));
        return Ok(());
    }

    std::fs::create_dir_all(&cli.data_dir)
        .with_context(|| format!("failed to create {}", cli.data_dir.display()))?;
    let calibration_path = cli.data_dir.join("calibration.json");
    let live_path = cli.data_dir.join("live.json");
    let layers_path = cli.data_dir.join("layers.json");

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

    let sim = cli.midi_test.then(|| {
        let sim = midi::testing::SimMidi::default();
        sim.plug(midi::testing::TEST_MK2_PORT, midi::testing::FakeApc::new(midi::Model::Apc40Mk2));
        sim
    });

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
        settings_rev: 0,
        deck: cues::CueDeck::load(cli.data_dir.join("grid.json")),
        look_on: true,
        tempo: tempo::TempoClock::default(),
        epoch: Instant::now(),
        live: load_json(&live_path),
        live_dirty: false,
        palettes: live::PaletteStore::load_or_create(cli.data_dir.join("palettes.json")),
        midi: {
            let mut m = midi::MidiState::new(!cli.no_midi || cli.midi_test, midi::profile::ProfileStore::load(cli.data_dir.join("midi")));
            m.sim = sim.clone();
            m
        },
        lfos,
        mixer: {
            let mut m: layers::Mixer = load_json(&layers_path);
            m.sanitize();
            m
        },
        mixer_dirty: false,
        mix: layers::MixReport::default(),
        safety: safety::SafetyStore::load_or_create(cli.data_dir.join("safety.json")),
        strobe: safety::StrobeStatus::default(),
        evolving: Vec::new(),
        timeline: timeline::Player::default(),
        shows: timeline::ShowStore::new(cli.data_dir.join("shows")),
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
        move || run_engine(shared, output, running, live_path, layers_path)
    });

    let midi_thread = if let Some(sim) = sim {
        println!("--midi-test: simulated APC40 mkII only, MIDI injection enabled (tests only).");
        midi::worker::spawn_on(sim, Arc::clone(&shared), Arc::clone(&running))
    } else if cli.no_midi {
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
    let mut stage = OutputStage::new(output);
    let mut limiter = safety::StrobeLimiter::default();

    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        let (looks, show_cues, starts, clock, mixer, calibration, audio, armed, live, user_palettes, estop, t, safety_cfg) = {
            let mut s = shared.lock().unwrap();
            let shared_state = &mut *s;
            shared_state.gate.sync_estop(&shared_state.estop);
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
            let looks = cues::layered_looks(&s.deck, &settings, s.look_on);
            let show_cues = timeline_cues(&mut s, t, &clock);
            let mixer = s.mixer.clone();
            // Launch beats, so a cue's beat-synced motion counts from its start.
            let starts: HashMap<u64, f64> = s.deck.active.iter().map(|a| (a.id, a.started_beat)).collect();
            (looks, show_cues, starts, clock, mixer, s.calibration, audio, s.gate.is_armed(), live, s.palettes.list().to_vec(), Arc::clone(&s.estop), t, s.safety.get())
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
        let frame = safety::apply(frame, t, &safety_cfg, &mut limiter);

        // Last stage: the gate. `armed` was read under the lock at the top
        // of the frame; the e-stop latch is re-read here, lock-free.
        let emitted = stage.emit(&frame, armed, &estop);
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
/// stop halts the timeline (it never resumes by itself); otherwise its
/// events join the cues, through the same mix, live stage and gate.
fn timeline_cues(s: &mut Shared, t: f64, clock: &engine::BeatClock) -> Vec<(timeline::TimelineCue, Settings)> {
    let c = timeline::Clock { t, beat: clock.beat, bpm: clock.bpm, beats_per_bar: clock.beats_per_bar };
    if s.estop.is_latched() {
        s.timeline.halt(&c);
    }
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
}
