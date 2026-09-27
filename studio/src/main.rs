//! Laser studio: a live laser show controller that runs on the Mac, with
//! its UI in the browser. Pick a shape or text, animate it, make it react
//! to music from the microphone, save looks as scenes and run them as a
//! playlist - all with a live preview, with or without a laser attached.
//!
//! One engine thread renders a frame 60 times a second from the shared
//! state and hands it to the preview and (when one is configured) to the
//! laser output. The HTTP handlers in `web.rs` only ever edit that shared
//! state.

mod controls;
mod engine;
mod font;
mod generators;
mod live;
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
use output::{DacOutput, Output};
use patterns::Point;
use scenes::SceneStore;
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
    /// Laser emission requested by the UI. Always starts off.
    pub armed: bool,
    pub frame: Vec<Point>,
    pub output_name: Option<String>,
    pub output_error: Option<String>,
    pub pps: u32,
    pub scenes: SceneStore,
    pub playlist: Option<Playlist>,
    pub presets: Vec<presets::Preset>,
    pub controls: controls::ControlRegistry,
    /// Cue-grid page shown in the UI and on MIDI grids (0-based category index).
    pub cue_page: usize,
    /// Id of the last cue played, for highlighting and LED feedback.
    pub active_cue: Option<String>,
    /// The single tempo clock (see tempo.rs); times are seconds since `epoch`.
    pub tempo: tempo::TempoClock,
    pub epoch: Instant,
    /// Master live modifiers (live.rs), saved to live.json when changed.
    pub live: live::LiveModifiers,
    pub live_dirty: bool,
    /// User colour palettes (palettes.json).
    pub palettes: live::PaletteStore,
}

impl Shared {
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

    let presets = presets::catalog();
    let shared = Arc::new(Mutex::new(Shared {
        settings: Settings::default(),
        calibration: web::load_calibration(&calibration_path),
        audio: AudioFeatures::default(),
        audio_at: Instant::now(),
        armed: false,
        frame: Vec::new(),
        output_name: output.as_ref().map(|o| o.name().to_string()),
        output_error: None,
        pps: cli.pps,
        scenes: SceneStore::load_or_create(cli.data_dir.join("scenes.json")),
        playlist: None,
        controls: controls::ControlRegistry::build(&presets),
        presets,
        cue_page: 0,
        active_cue: None,
        tempo: tempo::TempoClock::default(),
        epoch: Instant::now(),
        live: load_json(&live_path),
        live_dirty: false,
        palettes: live::PaletteStore::load_or_create(cli.data_dir.join("palettes.json")),
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

    let addr = format!("127.0.0.1:{}", cli.port);
    println!("Studio: open http://{addr}/ in your browser - Ctrl+C to quit");
    web::run(&addr, shared, calibration_path, Arc::clone(&running))?;

    running.store(false, Ordering::SeqCst);
    engine.join().ok();
    Ok(())
}

fn run_engine(shared: Arc<Mutex<Shared>>, mut output: Option<Box<dyn Output>>, running: Arc<AtomicBool>, live_path: PathBuf) {
    let mut animator = Animator::default();
    let mut live_state = live::LiveState::default();
    let mut frames_since_save = 0u32;
    let mut last = Instant::now();
    let mut output_armed = false;

    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        let (settings, calibration, audio, armed, live, bpm, beats_per_bar, user_palettes) = {
            let mut s = shared.lock().unwrap();
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
            live_state.set_clock(t, s.tempo.beat_at(t));
            (s.settings.clone(), s.calibration, audio, s.armed, s.live.clone(), s.tempo.bpm, s.tempo.beats_per_bar, s.palettes.list().to_vec())
        };

        live_state.advance(&live, dt, bpm, beats_per_bar);
        let look = animator.render(&settings, audio, dt * live.speed.clamp(0.0, 4.0));
        let frame: Vec<Point> = live::apply(&look, &live, &live_state, &user_palettes)
            .into_iter()
            .map(|p| {
                let (x, y) = calibration.apply(p.x, p.y);
                Point { x, y, ..p }
            })
            .collect();

        if let Some(out) = output.as_mut() {
            if armed != output_armed {
                let result = out.set_armed(armed);
                output_armed = armed;
                shared.lock().unwrap().output_error = result.err().map(|e| e.to_string());
            }
            out.send(&frame);
        }
        shared.lock().unwrap().frame = frame;

        std::thread::sleep(FRAME_INTERVAL.saturating_sub(now.elapsed()));
    }

    if let Some(out) = output.as_mut() {
        let _ = out.set_armed(false);
    }
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
    s.settings = s.scenes.list()[playlist.index].settings.clone();
}
