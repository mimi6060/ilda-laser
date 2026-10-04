//! Laser studio: a live laser show controller that runs on the Mac, with
//! its UI in the browser. Pick a shape or text, animate it, make it react
//! to music from the microphone, save looks as scenes and run them as a
//! playlist - all with a live preview, with or without a laser attached.
//!
//! One engine thread renders a frame 60 times a second from the shared
//! state and hands it to the preview and (when one is configured) to the
//! laser output. The HTTP handlers in `web.rs` only ever edit that shared
//! state.

mod applog;
mod artnet;
mod audience;
mod audio;
mod autosave;
mod beat;
mod chases;
mod checklist;
mod colorcal;
mod autoshow;
mod controls;
mod crossings;
mod cues;
mod cuelist;
mod devices;
mod dmx;
mod datafile;
mod dwell;
mod engine;
mod evolving;
mod examples;
mod fans;
mod festival;
mod figure_import;
mod figures;
mod font;
mod generators;
mod grid;
mod history;
mod ilda;
mod ilda_library;
mod instance;
mod layers;
mod lfo;
mod interlock;
mod keep_awake;
mod live;
mod menubar;
mod midi;
mod netin;
mod notices;
mod external;
mod optimize;
mod output;
mod output_manager;
mod patterns;
mod ponk;
mod power;
mod presence;
mod presets;
mod running;
mod program;
mod project;
mod projection;
mod push;
mod safety;
#[cfg(test)]
mod safety_invariants;
mod safety_log;
mod showmode;
#[cfg(feature = "shownet")]
mod shownet;
mod scan;
mod scenes;
mod sdbank;
mod sheets;
mod tempo;
mod testpattern;
mod textures;
mod timecode;
mod timeline;
mod transition;
mod tunnels;
mod usercues;
mod venue;
mod watchdog;
mod zones;
mod web;
mod wifi;

#[cfg(test)]
mod test_support;

use anyhow::{Context, Result};
use clap::Parser;
use engine::{Animator, AudioFeatures, Calibration, Settings};
use interlock::{ArmGate, EStop};
use output::{Output, OutputStage};
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
    /// Laser DAC to drive: an id from `--list-devices` (Helios USB, Ether
    /// Dream, IDN/HeliosPRO), or "auto" for the first one found. Without
    /// it, the output remembered in RÉGLAGES › Sorties is reconnected, or
    /// the studio runs in preview only. For this run only: it never
    /// replaces the remembered output. The laser starts disarmed.
    #[arg(long)]
    device: Option<String>,
    /// Send the frames over PONK (UDP) to MadMapper/MadLaser, which drives
    /// the ShowNET (T-300). Alone: MadMapper on this Mac (127.0.0.1:5583);
    /// or `ip:port` / `host:port`. Off by default; the laser still starts
    /// disarmed and only empty frames go out until it is armed.
    #[arg(long, value_name = "HOST:PORT", num_args = 0..=1, default_missing_value = ponk::DEFAULT_TARGET,
          value_parser = ponk::parse_target, conflicts_with_all = ["device", "test_output", "midi_test"])]
    ponk: Option<std::net::SocketAddr>,
    /// Drive a ShowNET in Art-Net mode (T-301): the box plays the files of
    /// its SD card (RÉGLAGES › Carte SD ShowNET), the studio sends the
    /// bank number of the cue on show and the intensity. An IPv4 address,
    /// optionally `:port` (6454). Profile, universe and DMX address come
    /// from RÉGLAGES › Sorties. Off by default; starts disarmed, intensity
    /// 0 and pattern 0 until armed.
    #[arg(long, value_name = "IP[:PORT]", value_parser = artnet::parse_target,
          conflicts_with_all = ["device", "ponk", "test_output", "list_devices", "midi_test"])]
    shownet_artnet: Option<std::net::SocketAddr>,
    /// Points per second sent to the laser (default: the one in
    /// outputs.json, else 30000).
    #[arg(long)]
    pps: Option<u32>,
    /// Port for the browser UI.
    #[arg(long, default_value_t = 8080)]
    port: u16,
    /// Where scenes and calibration are saved.
    #[arg(long, default_value = "studio-data")]
    data_dir: PathBuf,
    /// Scan for laser DACs (Helios on USB, Ether Dream and IDN on the
    /// network), print their ids for --device and exit. Opens no output.
    #[arg(long, conflicts_with_all = ["device", "ponk", "test_output"])]
    list_devices: bool,
    /// Testing only (T-303): the DACs a scan "finds", as a JSON list of
    /// `{kind, id, name}`. No scan ever touches the network or USB then.
    #[arg(long, hide = true, value_name = "JSON", value_parser = devices::parse_test_devices, conflicts_with = "device")]
    test_devices: Option<devices::TestDevices>,
    /// Print every control id (for MIDI/OSC mapping) as Markdown and exit.
    #[arg(long)]
    list_controls: bool,
    /// Don't open any MIDI port (tests, e2e, or a second instance that
    /// must not grab the controller).
    #[arg(long)]
    no_midi: bool,
    /// Listen for OSC (T-013) for this run, whatever RÉGLAGES says:
    /// `ip:port`, an IP or a port. Alone: 127.0.0.1:8000. Nothing received
    /// over the network can arm the laser.
    #[arg(long, value_name = "IP:PORT", num_args = 0..=1, default_missing_value = "127.0.0.1:8000",
          value_parser = |s: &str| netin::parse_cli_addr(s, netin::DEFAULT_OSC_PORT))]
    osc: Option<std::net::SocketAddr>,
    /// Listen for Art-Net (T-013) for this run, like --osc. Alone:
    /// 127.0.0.1:6454. Receive only: the studio never sends Art-Net.
    #[arg(long, value_name = "IP:PORT", num_args = 0..=1, default_missing_value = "127.0.0.1:6454",
          value_parser = |s: &str| netin::parse_cli_addr(s, netin::artnet::PORT))]
    artnet: Option<std::net::SocketAddr>,
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
    /// Testing only (T-258): enforce the pre-show checklist as if a real
    /// output were configured (never needs one: preview or --test-output).
    #[arg(long, hide = true)]
    test_checklist: bool,
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
    /// Testing only: a fake sleep blocker, so a test that arms or plays
    /// never touches the Mac's power settings (T-412). Implied by
    /// --test-hooks and --test-output.
    #[arg(long, hide = true)]
    test_keep_awake: bool,
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
    /// Testing only (T-358): shut down cleanly, as on Ctrl+C, once the
    /// process with this PID is no longer our parent (the test harness
    /// died) or the data directory is gone, so an interrupted test run
    /// leaves no studio behind. Also given by the studio to its `--decode`
    /// children (T-359), which exit at once when it is gone.
    #[arg(long, hide = true, value_name = "PID", conflicts_with = "device")]
    exit_with_parent: Option<u32>,
    /// Application mode (the macOS app, T-413): data in
    /// ~/Library/Application Support/Laser Studio unless --data-dir is
    /// given, a free port from 8080 up unless --port is given, always on
    /// 127.0.0.1, and the UI opens in the default browser. A second launch
    /// on the same data opens the running studio's page instead. The laser
    /// starts disarmed, as always.
    #[arg(long)]
    app: bool,
    /// With --app: don't open the browser (tests).
    #[arg(long, hide = true, requires = "app", conflicts_with = "device")]
    no_open: bool,
    /// Set by `resolve_app_mode`: the data directory is the application's
    /// default one (for the startup line of the log).
    #[arg(skip)]
    app_default_data_dir: bool,
}

/// The port the application tries first (T-413), then the next ones.
const APP_PORT: u16 = 8080;
/// How many ports after `APP_PORT` are tried before any free one: a
/// stable address keeps the page's own settings (stored per address).
const APP_PORT_TRIES: u16 = 20;

/// The application's data directory under `home` (T-413): the same files
/// as `studio-data/`, in the place macOS apps keep theirs.
fn app_data_dir(home: Option<&std::ffi::OsStr>) -> Result<PathBuf> {
    let home = home.filter(|h| !h.is_empty()).context("HOME n'est pas défini : impossible de trouver le dossier Application Support")?;
    Ok(PathBuf::from(home).join("Library").join("Application Support").join("Laser Studio"))
}

/// A port free on 127.0.0.1: `preferred` or one of the next ones, else
/// any the system gives. Checked by binding it (released at once; the web
/// server binds it again a moment later).
fn pick_port(preferred: u16, tries: u16) -> Result<u16> {
    let free = |port: u16| std::net::TcpListener::bind(("127.0.0.1", port)).ok().and_then(|l| l.local_addr().ok()).map(|a| a.port());
    (0..tries)
        .filter_map(|i| preferred.checked_add(i))
        .find_map(free)
        .or_else(|| free(0))
        .context("aucun port libre sur 127.0.0.1")
}

/// `--app`: the application's data directory and a free port, unless the
/// command line names them. Nothing else changes; the CLI without `--app`
/// behaves exactly as before.
fn resolve_app_mode(cli: &mut Cli, explicit_data_dir: bool, explicit_port: bool, home: Option<&std::ffi::OsStr>) -> Result<()> {
    if !cli.app {
        return Ok(());
    }
    if !explicit_data_dir {
        cli.data_dir = app_data_dir(home)?;
        cli.app_default_data_dir = true;
    }
    if !explicit_port {
        cli.port = pick_port(APP_PORT, APP_PORT_TRIES)?;
    }
    Ok(())
}

/// The command line as the binary sees it. Started from a `.app` bundle
/// (Finder, Dock, `open`) with no argument, it is the application:
/// `--app` is added. Any other start is unchanged, so the decoder child
/// (`--decode …`) and every CLI use keep their meaning. Old macOS versions
/// pass a `-psn_…` process serial number: dropped.
fn launch_args(args: Vec<std::ffi::OsString>, exe: Option<&std::path::Path>) -> Vec<std::ffi::OsString> {
    let mut args: Vec<_> = args.into_iter().enumerate().filter(|(i, a)| *i == 0 || !a.to_string_lossy().starts_with("-psn_")).map(|(_, a)| a).collect();
    let in_bundle = exe.is_some_and(|exe| {
        let mut up = exe.ancestors().skip(1);
        let (macos, contents, bundle) = (up.next(), up.next(), up.next());
        macos.and_then(|p| p.file_name()) == Some("MacOS".as_ref())
            && contents.and_then(|p| p.file_name()) == Some("Contents".as_ref())
            && bundle.and_then(|p| p.extension()) == Some("app".as_ref())
    });
    if in_bundle && args.len() <= 1 {
        args.push("--app".into());
    }
    args
}

/// Parses the command line, then applies `--app` (T-413).
fn parse_cli() -> Result<Cli> {
    use clap::{CommandFactory, FromArgMatches};
    let args = launch_args(std::env::args_os().collect(), std::env::current_exe().ok().as_deref());
    let matches = Cli::command().get_matches_from(args);
    let mut cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if cli.decode.is_none() && !cli.list_devices && !cli.list_controls {
        let explicit = |id: &str| matches.value_source(id) == Some(clap::parser::ValueSource::CommandLine);
        resolve_app_mode(&mut cli, explicit("data_dir"), explicit("port"), std::env::var_os("HOME").as_deref())?;
    }
    Ok(cli)
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
    /// `Output::kind` ("dac", "ponk", "test"), for the UI's output badge.
    pub output_kind: Option<&'static str>,
    pub output_error: Option<String>,
    /// The application log on disk (T-403): its state for the UI, the
    /// tail and the diagnostic report.
    pub applog: Arc<applog::AppLog>,
    pub pps: u32,
    pub scenes: SceneStore,
    pub playlist: Option<Playlist>,
    pub presets: Vec<presets::Preset>,
    pub controls: controls::ControlRegistry,
    /// Cue-grid page shown in the UI and on MIDI grids (0-based, in `grid.view()`).
    pub cue_page: usize,
    /// Id of the newest playing cue, for highlighting and LED feedback.
    pub active_cue: Option<String>,
    /// Bumped each time `settings` changes (look panel, cue, scene,
    /// playlist, look controls), so the page reloads a look it didn't make.
    pub settings_rev: u64,
    /// Playing cues and the grid's trigger settings (cues.rs).
    pub deck: cues::CueDeck,
    /// The cue grid's size and pages (grid.rs, pages.json).
    pub grid: grid::CueGrid,
    /// Saved cue lists (cuelist.rs, T-410, `cuelists.json`).
    pub cue_lists: cuelist::CueListStore,
    /// The loaded cue list and where it is. Memory only: every start is
    /// at *Début*, nothing loaded.
    pub cue_list: cuelist::ListPlayer,
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
    /// User scan presets (scan_presets.json, T-415).
    pub scan_presets: scan::ScanPresetStore,
    /// Festival palettes on show and perceived balance (T-130,
    /// festival.json; the palette on show is memory only).
    pub festival: festival::Festival,
    /// MIDI controllers (midi/mod.rs); not part of any saved look.
    pub midi: midi::MidiState,
    /// OSC and Art-Net inputs (netin/, netin.json); never in a look or a project.
    pub netin: netin::NetInState,
    /// The local gate and per-source scopes for external inputs (T-408):
    /// the gate is memory only and closed at start; scopes in external.json.
    pub external: external::External,
    /// Master LFO modulators (lfo.rs, lfos.json), applied every frame on
    /// top of the stored values.
    pub lfos: lfo::LfoStore,
    /// Audio routes (audio/routes.rs, audio_routes.json): analysis values
    /// and events shaped onto controls, every frame, on the same copies.
    pub routes: audio::routes::RouteStore,
    /// Audio event triggers (audio/triggers.rs, audio_triggers.json): rules
    /// that press cues on a kick, a drop, a break… Their switch is memory
    /// only and off at start; they never arm.
    pub triggers: audio::triggers::TriggerStore,
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
    /// Projection zones and their geometric correction (projection.rs,
    /// projection.json). Machine settings, like the calibration.
    pub projection: projection::ProjectionStore,
    /// What the audio guard did on the last frame (T-245).
    pub audio_guard: audio::safety::GuardStatus,
    /// What the static beam guard (dwell.rs) did on the last frame.
    pub dwell: dwell::DwellStatus,
    /// Evolving cues on show in the last frame: (animator id, where it is).
    /// Id 0 is the manual look.
    pub evolving: Vec<(u64, evolving::Progress)>,
    /// The timeline player (timeline.rs): its events play through the
    /// same layers and live stage as the cues. Never arms anything.
    pub timeline: timeline::Player,
    /// Saved shows, `studio-data/shows/`.
    pub shows: timeline::ShowStore,
    /// Incoming timecode (timecode/, T-168): settings in `timecode.json`,
    /// TC-IN in memory only (off at start). Drives the timeline only.
    pub timecode: timecode::TimecodeState,
    /// The operator's figures (figures.rs, `studio-data/figures/`), also
    /// the cues of the « Figures » page.
    pub figures: figures::FigureStore,
    /// The operator's own cues (usercues.rs, T-405, `user_cues.json`):
    /// looks stored into grid cells, also in `presets`.
    pub user_cues: usercues::UserCueStore,
    /// The user's songs, `studio-data/media/audio/` (T-161).
    pub media: Arc<audio::media::MediaStore>,
    /// « Générer un show depuis le morceau » (autoshow.rs, T-167): the
    /// job runs on its own thread, with its own lock.
    pub autoshow: Arc<autoshow::AutoShow>,
    /// The user's ILDA files, `studio-data/media/ilda/` (T-011), read on
    /// their own thread at start-up (`ilda_library::spawn_load`).
    pub ilda: Arc<ilda_library::IldaLibrary>,
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
    /// Test pattern mode (testpattern.rs, T-404): memory only, off at start.
    pub test_pattern: testpattern::TestPatternState,
    /// Prevent sleep during a show (keep_awake.rs): setting and status.
    pub keep_awake: keep_awake::KeepAwakeState,
    /// Show mode (showmode.rs, T-283): editing routes refused with 423.
    /// Kept in `prefs.json`; never arms anything.
    pub show_mode: showmode::ShowMode,
    /// Damaged data files moved to `quarantine/` this run (datafile.rs,
    /// T-402), shown until acknowledged. In memory only.
    pub data_warnings: Vec<datafile::DataWarning>,
    /// The pre-show checklist (checklist.rs, T-258): the list is saved,
    /// what was ticked lives for this run only.
    pub checklist: checklist::Checklist,
    /// Audience scanning mode (audience.rs, T-255). In memory only: every
    /// start is « Au-dessus du public ».
    pub audience: audience::AudienceState,
    /// `--test-hooks`: allows `POST /api/test/stall`.
    pub test_hooks: bool,
    /// The menu-bar entries in application mode (menubar.rs, T-413), for
    /// `POST /api/test/menu`; None otherwise.
    pub menu: Option<Arc<menubar::MenuActions>>,
    /// A pending simulated stall (ms): the engine sleeps this long while
    /// holding the lock, the worst case for the watchdog.
    pub test_stall_ms: u64,
    /// Who last changed each control, and when (T-423, « En cours »).
    /// Memory only.
    pub touches: std::collections::HashMap<String, controls::Touch>,
    /// The DACs the last scan found (devices.rs). The scan never opens one.
    pub devices: Arc<devices::DeviceList>,
    /// Choosing, connecting and reconnecting the output (T-401). Its own
    /// lock: taken under this one, never the other way round.
    pub output_mgr: Arc<output_manager::OutputManager>,
    /// The ShowNET SD card bank (sdbank.rs, T-302, `sdbank.json`): which
    /// cue is which card file. A machine file, not part of a project.
    pub sdbank: sdbank::SdBankStore,
    /// The ShowNET SD card box over Art-Net (artnet.rs, T-301,
    /// `shownet_artnet.json`), shared with the output backend.
    pub shownet_dmx: Arc<artnet::ShowNetDmxStore>,
    /// The connected output's own report (`Output::status`), last frame.
    pub output_status: Option<serde_json::Value>,
    /// Whether a ShowNET output's link is the Wi-Fi (T-472): a warning in
    /// the top bar, nothing else.
    pub wifi: Arc<wifi::WifiWatch>,
    /// The newest frame and page state for the pushed routes (push.rs,
    /// T-411): the engine drops each frame there and never waits.
    pub push: Arc<push::Hub>,
    /// The venue and its projectors (venue.rs, T-277, `venue.json`): one
    /// output stage per projector. Machine settings, like the outputs.
    pub venue: venue::VenueStore,
    /// The outputs the engine has installed for the venue's other
    /// projectors, by projector id (the main one is `output_name`).
    pub projector_outputs: std::collections::BTreeMap<String, venue::Installed>,
    /// The other projectors' last frames (`GET /api/frames`).
    pub projector_frames: Vec<venue::ProjectorFrame>,
    /// Notification centre (T-432): events without a request, in memory.
    pub notices: notices::Notices,
    /// Undo / redo of edits (history.rs, T-285): in memory, 50 levels,
    /// emptied when another project opens.
    pub history: history::EditHistory,
}

impl Shared {
    /// Arm request, after catching up with the e-stop latch, the watchdog
    /// and the heartbeats.
    pub fn request_arm(&mut self, src: interlock::ArmSource) -> Result<(), Vec<String>> {
        self.sync_data_warnings();
        self.sync_safety(Instant::now());
        self.sync_audience();
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
        self.sync_data_warnings();
        self.sync_safety(Instant::now());
        self.sync_audience();
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
            self.gate.watchdog_trip();
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

    /// Collects the data-file warnings recorded since the last call (shows,
    /// figures and preferences are read at run time too) and closes the
    /// `data_ack` interlock while a safety file's warning is unacknowledged.
    pub fn sync_data_warnings(&mut self) {
        let fresh = datafile::take_pending(&self.project.data_dir.join(datafile::QUARANTINE));
        for w in &fresh {
            self.gate.log().record("data_quarantine", Some("system"), serde_json::json!({ "file": w.file, "moved_to": w.moved_to, "safety": w.safety }));
        }
        self.data_warnings.extend(fresh);
        if self.data_warnings.iter().any(|w| w.safety) {
            let clear = self.data_warnings.iter().all(|w| !w.safety || w.acknowledged);
            self.gate.register(interlock::DATA_ACK, "Fichier de sécurité abîmé : à valider", clear);
        }
    }

    /// Brings the notification centre's conditions up to date (T-432):
    /// file writes reported since the last call, the autosave status and
    /// the audio input. Called with the state (HTTP threads), constant time.
    pub fn sync_notices(&mut self) {
        notices::drain_writes(&mut self.notices);
        let autosave = self.project.autosave.error.as_ref().map(|e| (notices::Level::Warning, format!("échec de la sauvegarde automatique : {e}")));
        self.notices.condition("autosave", "Projet", autosave);
        let audio = self.audio_in.status();
        let level = match audio.state {
            audio::CaptureState::DeviceLost => Some(notices::Level::Warning),
            audio::CaptureState::PermissionDenied | audio::CaptureState::Error => Some(notices::Level::Error),
            _ => None,
        };
        let text = if audio.message.is_empty() { "entrée audio indisponible".to_string() } else { audio.message };
        self.notices.condition("audio", "Audio", level.map(|l| (l, text)));
    }

    /// The operator has seen the warning of `file` (or of every file).
    /// Nothing is restored. Returns how many warnings were acknowledged.
    pub fn ack_data_warnings(&mut self, file: Option<&str>) -> usize {
        self.sync_data_warnings();
        let mut n = 0;
        for w in self.data_warnings.iter_mut().filter(|w| !w.acknowledged && file.is_none_or(|f| f == w.file)) {
            w.acknowledged = true;
            n += 1;
        }
        if n > 0 {
            self.gate.log().record("data_ack", Some("ui"), serde_json::json!({ "file": file, "count": n }));
        }
        self.sync_data_warnings();
        n
    }

    /// Brings the `checklist` interlock in line with the checklist. Only
    /// registered when a real output is configured; never arms.
    ///
    /// Also the ShowNET part (T-472), from the connected output and the
    /// projector sheet: the ShowNET items, the `hw_stop` interlock
    /// (registered from the first ShowNET output on) and the Wi-Fi watch.
    /// Call it after any change of output, sheet or checklist.
    pub fn sync_checklist(&mut self) {
        // The main output, else any other projector's (T-277): one ShowNET
        // anywhere brings its items and `hw_stop`.
        let kind = checklist::OutputKind::of(self.output_kind, self.outputs.active().projector.dac_model).or_else(|| {
            self.projector_outputs.iter().find_map(|(id, o)| checklist::OutputKind::of(Some(o.kind), self.outputs.config_for(id).projector.dac_model))
        });
        if self.checklist.set_output(kind) {
            self.gate.log().record("checklist_reset", Some("system"), serde_json::json!({ "reason": "shownet" }));
        }
        if self.checklist.enforced() {
            self.gate.register(interlock::CHECKLIST, checklist::LABEL_FR, self.checklist.satisfied());
        }
        if kind.is_some() || self.gate.has_interlock(interlock::HW_STOP) {
            self.gate.register(interlock::HW_STOP, interlock::HW_STOP_LABEL, self.checklist.hw_stop_ok());
        }
        // A PONK id holds an address literal: parsed, never resolved.
        let receiver = self
            .output_mgr
            .selected()
            .and_then(|id| id.strip_prefix("ponk:").or_else(|| id.strip_prefix(artnet::ID_PREFIX))?.parse::<std::net::SocketAddr>().ok())
            .map(|a| a.ip());
        self.wifi.set_target(kind.map(|_| wifi::Target::for_receiver(receiver)));
        // The audience mode follows the checklist and the sheet (T-255).
        self.sync_audience();
    }

    /// The conditions to unlock audience scanning, as they stand (T-255).
    pub fn audience_conditions(&self) -> Vec<audience::Condition> {
        let safety = self.safety.get();
        let of = |projector: &power::ProjectorInfo| {
            audience::conditions(&audience::Inputs { safety: &safety, projector, checklist: &self.checklist, hold_to_run: self.presence.settings.hold_to_run })
        };
        let mut conds = of(&self.outputs.active().projector);
        // Every other projector that drives an output must meet them too
        // (T-277): its own sheet is checked, the rest is shared.
        for p in self.venue.get().extras().filter(|p| p.output.is_some()) {
            for (c, other) in conds.iter_mut().zip(of(&self.outputs.config_for(&p.id).projector)) {
                if c.ok && !other.ok {
                    c.ok = false;
                    c.detail_fr = format!("{} : {}", p.name, other.detail_fr);
                }
            }
        }
        conds
    }

    /// The `projectors` interlock (T-277): registered once the venue has a
    /// projector besides the main one that drives an output, closed only
    /// while every such projector has its output installed. Never arms.
    pub fn sync_projectors(&mut self) {
        let needed: Vec<(String, String, String)> = self.venue.get().extras().filter_map(|p| Some((p.name.clone(), p.id.clone(), p.output.clone()?))).collect();
        if needed.is_empty() && !self.gate.has_interlock(interlock::PROJECTORS) {
            return;
        }
        match venue::missing_outputs(&needed, &self.projector_outputs) {
            Some(label) => self.gate.register(interlock::PROJECTORS, &label, false),
            None => self.gate.register(interlock::PROJECTORS, "Sorties des projecteurs", true),
        }
    }

    /// Asks the output manager for every venue projector's output, and to
    /// close the outputs of projectors gone or changed (T-277). The output
    /// the main projector drives is never opened twice. Never arms; the
    /// caller made sure the laser is disarmed when anything changes.
    pub fn connect_projectors(&mut self, src: interlock::ArmSource) {
        let main = self.output_mgr.selected();
        let venue = self.venue.get().clone();
        for p in venue.extras() {
            if let Err(e) = self.outputs.ensure(&p.id) {
                log::warn!("{e:#}");
            }
            let output = p.output.clone().filter(|o| main.as_deref() != Some(o.as_str()));
            if self.projector_outputs.get(&p.id).is_some_and(|i| Some(&i.output) != output.as_ref()) {
                self.projector_outputs.remove(&p.id);
            }
            self.output_mgr.select_projector(&p.id, output, src);
        }
        for link in self.output_mgr.projector_links() {
            if venue.projector(&link.projector).is_none() || link.projector == power::MAIN_OUTPUT {
                self.projector_outputs.remove(&link.projector);
                self.output_mgr.select_projector(&link.projector, None, src);
            }
        }
        self.sync_projectors();
    }

    /// `POST /api/audience {mode: "scanning", phrase}`: unlocks audience
    /// scanning (disarmed, every condition met, the phrase typed), logged
    /// with every condition. Never arms anything.
    pub fn unlock_audience_scanning(&mut self, phrase: &str, src: interlock::ArmSource) -> Result<(), audience::UnlockError> {
        // Only the e-stop latch: no wall-clock read here, so a simulated
        // clock (the safety invariants) stays the only time base.
        self.gate.sync_estop(&self.estop);
        let conds = self.audience_conditions();
        let mut detail = self.audience.unlock(&conds, phrase, self.gate.is_armed())?;
        detail["operator"] = serde_json::json!(self.checklist.view()["operator"]);
        log::warn!("Balayage public déverrouillé ({})", src.label_fr());
        self.gate.log().record("audience_mode", Some(src.id()), detail);
        Ok(())
    }

    /// Back to « Au-dessus du public » (tightening: always allowed, even
    /// armed). `reason` goes to the safety log. No-op when already there.
    pub fn end_audience_scanning(&mut self, reason: &str, src: interlock::ArmSource, missing: &[String]) {
        if self.audience.lock() {
            log::warn!("Balayage public terminé : {reason}");
            self.gate.log().record("audience_mode", Some(src.id()), serde_json::json!({ "mode": "above", "reason": reason, "missing": missing }));
        }
    }

    /// Keeps the audience mode and the `audience_zone` interlock in line
    /// (T-255), at every frame and after every change that matters.
    /// Audience scanning ends on an e-stop, on any disarm the operator did
    /// not ask for, and as soon as a condition stops being met. With a
    /// real output (checklist enforced), arming waits for a Public zone or
    /// a full horizon. Never arms.
    pub fn sync_audience(&mut self) {
        if let Some(unlocked_at) = self.audience.unlocked_at() {
            let disarm = self.gate.last_disarm().filter(|(reason, at)| *at > unlocked_at && **reason != interlock::DisarmReason::User).map(|(reason, _)| reason.id());
            if self.estop.is_latched() {
                self.end_audience_scanning("estop", interlock::ArmSource::System, &[]);
            } else if let Some(reason) = disarm {
                self.end_audience_scanning(&format!("disarm:{reason}"), interlock::ArmSource::System, &[]);
            } else {
                let missing: Vec<String> = self.audience_conditions().into_iter().filter(|c| !c.ok).map(|c| c.label_fr.to_string()).collect();
                if !missing.is_empty() {
                    self.end_audience_scanning("condition", interlock::ArmSource::System, &missing);
                }
            }
        }
        if self.checklist.enforced() {
            let ok = audience::protected(&self.safety.get());
            if !self.gate.has_interlock(interlock::AUDIENCE_ZONE) || self.gate.interlock_ok(interlock::AUDIENCE_ZONE) != ok {
                self.gate.register(interlock::AUDIENCE_ZONE, audience::INTERLOCK_LABEL, ok);
            }
        }
    }

    /// The safety settings the output stage applies now: Public zones
    /// blanked outside audience scanning (audience.rs).
    pub fn effective_safety(&self) -> safety::SafetySettings {
        audience::effective(&self.safety.get(), self.audience.is_scanning())
    }

    /// Trips the emergency stop and records it in the gate. Audience
    /// scanning ends with it (T-255).
    pub fn emergency_stop(&mut self, src: interlock::ArmSource) {
        self.estop.trip(src);
        // Fades end and waiting cues are dropped at once (T-158 / T-159).
        self.deck.cut_transitions();
        // The cue list pauses; it never starts again by itself (T-410).
        self.cue_list.halt();
        self.gate.sync_estop(&self.estop);
        // External inputs lose the hand until the operator reopens them,
        // and so does the audio automation (T-240).
        external::sync(self);
        audio::triggers::sync_estop(self);
        // Timecode restarts nothing until TC-IN is armed again (T-168).
        timecode::sync(self);
        self.sync_audience();
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
    let cli = parse_cli()?;
    if let Some(name) = &cli.decode {
        // The decoder child (audio/isolate.rs): no panic hook, no output,
        // nothing but one file read and a reply on stdout. It never opens
        // the application log (T-403).
        env_logger::init();
        // T-359: never outlive the studio, nor the time limit.
        let timeout = cli.test_decode_timeout_ms.map_or(audio::isolate::DECODE_TIMEOUT, Duration::from_millis);
        spawn_decode_watch(cli.exit_with_parent.unwrap_or_else(std::os::unix::process::parent_id), cli.data_dir.clone(), timeout);
        let want = if cli.decode_peaks { audio::isolate::Want::Peaks } else { audio::isolate::Want::Samples };
        std::process::exit(audio::isolate::child_main(&cli.data_dir, name, want, cli.test_hooks));
    }
    if cli.list_devices {
        // A scan only: nothing is connected, armed or sent.
        let found = match &cli.test_devices {
            Some(fake) => fake.0.clone(),
            None => devices::scan_dacs(devices::Scope { usb: true })?,
        };
        print!("{}", devices::print_list(&found));
        return Ok(());
    }
    if cli.list_controls {
        print!("{}", controls::markdown(&controls::ControlRegistry::build(&presets::catalog())));
        return Ok(());
    }
    // The macOS application with its menu-bar item (T-413): AppKit owns
    // the main thread, the studio runs unchanged on another one.
    #[cfg(all(feature = "macos-app", target_os = "macos"))]
    if cli.app {
        return menubar::native::run(move |ready| run_studio(cli, Some(ready)));
    }
    run_studio(cli, None)
}

/// The studio: everything `main` runs once the command line is read, until
/// a clean shutdown. `on_ready` receives what the menu-bar item needs once
/// the engine runs (application mode with the `macos-app` feature).
fn run_studio(cli: Cli, on_ready: Option<Box<dyn FnOnce(menubar::Ready) + Send>>) -> Result<()> {
    let made = std::fs::create_dir_all(&cli.data_dir);
    // The application log (T-403): terminal as before, plus the file.
    let app_log = applog::init(&cli.data_dir, &applog::LogSettings::load(&cli.data_dir));
    made.with_context(|| format!("failed to create {}", cli.data_dir.display()))?;
    // One studio per data folder (T-412), before any output is opened. Held
    // until `main` returns, after the output is closed.
    let instance_lock = match instance::acquire(&cli.data_dir, cli.port) {
        Ok(lock) => Some(lock),
        Err(instance::LockError::Held(Some(info))) if cli.app => {
            // A second launch of the application: show the running one.
            let url = format!("http://127.0.0.1:{}/", info.port);
            println!("Laser Studio tourne déjà (PID {}) : {url}", info.pid);
            log::info!("Laser Studio tourne déjà (PID {}) : ouverture de son interface", info.pid);
            if !cli.no_open {
                menubar::open_in_browser(&url)?;
            }
            return Ok(());
        }
        Err(e @ instance::LockError::Held(_)) => {
            eprintln!("{}", e.message(&cli.data_dir));
            std::process::exit(2);
        }
        Err(e) => {
            // Some network volumes have no advisory locks: warn, don't refuse.
            log::warn!("{} : démarrage sans verrou d'instance", e.message(&cli.data_dir));
            None
        }
    };

    let mut state = startup_state(&cli);
    // The output named on the command line opens now, as before (a failure
    // stops the studio); otherwise the remembered one is connected by the
    // output manager once it runs (T-401). Either way: disarmed.
    let output: Option<Box<dyn Output>> = match startup_output_id(&cli) {
        Some(id) => {
            let out = state.output_mgr.open_at_start(&id).inspect_err(|e| log::error!("sortie laser : {e:#}"))?;
            match out.kind() {
                "ponk" => println!("Laser output: {} (disarmed: empty PONK frames until armed).", out.name()),
                artnet::KIND => println!("Laser output: {} (disarmed: intensity 0 and pattern 0 until armed; the box plays its SD card).", out.name()),
                "test" => println!("--test-output: fake output logged to {} (no laser).", cli.test_output.as_ref().map(|p| p.display().to_string()).unwrap_or_default()),
                _ => println!("Laser output: {}", out.name()),
            }
            Some(out)
        }
        None => {
            match &state.outputs.connection().selected {
                Some(id) => println!("Remembered output {id}: connecting in the background (disarmed)."),
                None => println!("No output chosen: preview only (pick one in RÉGLAGES › Sorties, or start with --device)."),
            }
            None
        }
    };
    note_output(&mut state, output.as_deref());
    state.applog = Arc::clone(&app_log);
    log::info!("{}", startup_line(&cli, state.output_name.as_deref()));
    let safety_log = state.gate.log().clone();
    safety_log.record(
        "app_start",
        Some("system"),
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "output": state.output_name.clone().unwrap_or_else(|| "aperçu seul".into()),
            // A lock left by a run that did not end cleanly (T-412).
            "previous_pid": instance_lock.as_ref().and_then(|l| l.previous.as_ref()).map(|i| i.pid),
        }),
    );
    let (estop, health) = (Arc::clone(&state.estop), Arc::clone(&state.health));
    let sim = state.midi.sim.clone();
    let (netin_hub, netin_tx) = (Arc::clone(&state.netin.hub), state.netin.sender());
    let audio_hub = Arc::clone(&state.audio_in);
    let (song_hub, media) = (Arc::clone(&state.song), Arc::clone(&state.media));
    if let Some(kill) = output.as_ref().and_then(|o| o.kill_switch()) {
        estop.set_kill_switch(kill);
    }
    let output_mgr = Arc::clone(&state.output_mgr);
    let wifi_watch = Arc::clone(&state.wifi);
    let remembered = state.outputs.connection().selected.filter(|_| output.is_none());
    let shared = Arc::new(Mutex::new(state));
    let running = Arc::new(AtomicBool::new(true));
    let url = format!("http://127.0.0.1:{}/", cli.port);
    if cli.app {
        // The menu-bar entries (T-413); also `POST /api/test/menu`.
        let actions = Arc::new(menubar::MenuActions { estop: Arc::clone(&estop), running: Arc::clone(&running), url: url.clone(), open_browser: !cli.no_open });
        shared.lock().unwrap().menu = Some(Arc::clone(&actions));
        if let Some(on_ready) = on_ready {
            on_ready(menubar::Ready { shared: Arc::clone(&shared), actions });
        }
    }
    // The ILDA files can be big: read beside everything else, their cues
    // appear once parsed.
    ilda_library::spawn_load(&shared);
    // Opens, switches and reconnects outputs off the engine thread (T-401).
    let output_thread = output_mgr.spawn(Arc::clone(&running));
    // Reads the route to a ShowNET output (T-472); a warning only.
    let _wifi_thread = wifi_watch.spawn(Arc::clone(&running));
    if let Some(id) = remembered {
        output_mgr.select(Some(id), interlock::ArmSource::System);
    }
    // The venue's other projectors (T-277): their outputs too, disarmed.
    shared.lock().unwrap().connect_projectors(interlock::ArmSource::System);

    // A panic anywhere: cut the output first (the DAC kill switch, no lock),
    // then let the default report run and every loop wind down. The engine's
    // output stage also blanks and disarms as its thread unwinds. The one
    // exception is a contained audio decoder thread (only used if the
    // decoder child can't be started): its panic is an import error.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new({
        let (estop, health, running, app_log) = (Arc::clone(&estop), Arc::clone(&health), Arc::clone(&running), Arc::clone(&app_log));
        move |info| {
            if !audio::isolate::panic_is_contained() {
                watchdog::on_panic(&estop, &health, &running);
            }
            // After the output is cut; one queued line, never a wait.
            app_log.panic(info);
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
    if let Some(parent) = cli.exit_with_parent {
        spawn_orphan_watch(parent, cli.data_dir.clone(), Arc::clone(&estop), Arc::clone(&running));
    }

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

    // OSC / Art-Net listeners: idle (no socket) until switched on.
    let netin_threads = netin::listener::spawn(netin_hub, netin_tx, Arc::clone(&running));

    let audio_threads = if cli.no_audio {
        println!("--no-audio: no audio input opened (the browser source still works), no song played (the timeline runs on the system clock).");
        Vec::new()
    } else {
        let mut threads = audio::worker::spawn(audio_hub, Arc::clone(&running));
        threads.push(audio::playback::spawn(song_hub, media, Arc::clone(&running)));
        threads
    };

    let addr = format!("127.0.0.1:{}", cli.port);
    println!("Studio: open {url} in your browser - Ctrl+C to quit");
    if cli.app && !cli.no_open {
        // The application opens its page once the server answers.
        let addr = addr.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while std::net::TcpStream::connect(&addr).is_err() {
                if Instant::now() > deadline {
                    return log::warn!("Interface non ouverte : le serveur ne répond pas sur {addr}");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if let Err(e) = menubar::open_in_browser(&url) {
                log::warn!("Ouverture de l'interface impossible : {e:#}");
            }
        });
    }
    let autosave_thread = autosave::spawn(Arc::clone(&shared), Arc::clone(&running));
    // Keep the Mac awake while armed or playing (T-412); never the real
    // assertion in tests.
    let blocker: Box<dyn keep_awake::SleepBlocker> = if cli.test_keep_awake || cli.test_hooks || cli.test_output.is_some() {
        Box::new(keep_awake::FakeBlocker::default())
    } else {
        keep_awake::system_blocker()
    };
    let keep_awake_thread = keep_awake::spawn(Arc::clone(&shared), Arc::clone(&running), blocker);
    let served = web::run(&addr, shared, estop, cli.data_dir.join("calibration.json"), Arc::clone(&running));

    running.store(false, Ordering::SeqCst);
    // No audio decoder child left behind (T-359).
    audio::isolate::stop_children();
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
    // The output manager stops within 100 ms, unless an open is under way
    // (a DAC lookup takes ~1.5 s): never wait for that.
    let deadline = Instant::now() + Duration::from_millis(300);
    while output_thread.as_ref().is_some_and(|t| !t.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    keep_awake_thread.join().ok(); // lets go of the sleep assertion
    // The autosave thread writes what is unsaved, then stops; a stuck disk
    // must not hold the exit.
    let deadline = Instant::now() + AUTOSAVE_SHUTDOWN_WAIT;
    while !autosave_thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(midi_thread) = midi_thread {
        midi_thread.join().ok(); // lets it switch the APC LEDs off
    }
    for t in netin_threads {
        t.join().ok(); // within one receive timeout (100 ms)
    }
    // The audio threads close the input within ~50 ms; a device stuck in
    // CoreAudio must not hold the exit.
    let deadline = Instant::now() + AUDIO_SHUTDOWN_WAIT;
    while audio_threads.iter().any(|t| !t.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    safety_log.record("app_stop", Some("system"), serde_json::json!({}));
    if !safety_log.flush(Duration::from_millis(500)) {
        log::warn!("safety log: last events not written before exit");
    }
    match &served {
        Ok(()) => log::info!("Arrêt du studio"),
        Err(e) => log::error!("Arrêt du studio sur erreur : {e:#}"),
    }
    if !app_log.flush(Duration::from_millis(500)) {
        eprintln!("Journal de l'application : dernières lignes non écrites avant l'arrêt");
    }
    drop(instance_lock);
    served
}

/// T-358: why a test studio started with `--exit-with-parent` must stop,
/// if it must: the harness that started it is gone (we were re-parented,
/// e.g. to launchd) or its temporary data directory was deleted.
fn orphan_reason(parent: u32, current_parent: u32, data_dir_exists: bool) -> Option<&'static str> {
    if current_parent != parent {
        Some("le processus parent a disparu")
    } else if !data_dir_exists {
        Some("le dossier de données a disparu")
    } else {
        None
    }
}

/// T-358: `--exit-with-parent`: checks four times a second, and on an
/// orphan takes the Ctrl+C path (cut, then the engine disarms, sends dark
/// frames and closes the output). Never started for a normal launch.
fn spawn_orphan_watch(parent: u32, data_dir: PathBuf, estop: Arc<EStop>, running: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            if let Some(why) = orphan_reason(parent, std::os::unix::process::parent_id(), data_dir.is_dir()) {
                log::warn!("Studio de test : {why}, arrêt");
                estop.kill_output();
                running.store(false, Ordering::SeqCst);
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

/// T-359: the `--decode` child's watch: checks four times a second and
/// ends the process at once (there is no output to close) when the studio
/// `parent` is gone or the time limit has passed.
fn spawn_decode_watch(parent: u32, data_dir: PathBuf, timeout: Duration) {
    let started = Instant::now();
    std::thread::spawn(move || loop {
        let orphan = orphan_reason(parent, std::os::unix::process::parent_id(), data_dir.is_dir());
        if let Some(why) = audio::isolate::child_must_exit(orphan, started.elapsed(), timeout) {
            eprintln!("--decode: {why}, arrêt");
            std::process::exit(3);
        }
        std::thread::sleep(Duration::from_millis(250));
    });
}

/// The startup line of the application log: version, system, output and
/// options, without any path (the data directory is « par défaut » or
/// « personnalisé ») nor network address.
fn startup_line(cli: &Cli, output: Option<&str>) -> String {
    let mut options = vec![format!("port {}", cli.port), format!("pps {}", cli.pps.map_or_else(|| "par défaut".to_string(), |p| p.to_string()))];
    for (on, name) in [
        (cli.no_midi, "--no-midi"),
        (cli.no_audio, "--no-audio"),
        (cli.midi_test, "--midi-test"),
        (cli.test_hooks, "--test-hooks"),
        (cli.test_interlock, "--test-interlock"),
        (cli.test_checklist, "--test-checklist"),
        (cli.test_output.is_some(), "--test-output"),
        (cli.audio_device.is_some(), "--audio-device"),
        (cli.osc.is_some(), "--osc"),
        (cli.artnet.is_some(), "--artnet"),
        (cli.app, "--app"),
    ] {
        if on {
            options.push(name.to_string());
        }
    }
    let data_dir = if cli.app_default_data_dir || cli.data_dir.as_path() == std::path::Path::new("studio-data") { "par défaut" } else { "personnalisé" };
    format!(
        "Démarrage de Laser Studio {} ({} {}, {}) ; sortie : {} ; options : {} ; dossier de données {data_dir}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        applog::os_version(),
        std::env::consts::ARCH,
        output.unwrap_or("aperçu seul"),
        options.join(", "),
    )
}

/// How long `main` waits for the engine to close the output at shutdown.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
const AUDIO_SHUTDOWN_WAIT: Duration = Duration::from_millis(500);
const AUTOSAVE_SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

/// The state the studio starts with. Whatever the command line and the
/// files in the data directory say, it is disarmed (reason « Démarrage »):
/// arming is not part of any saved state, and there is no option for it.
fn startup_state(cli: &Cli) -> Shared {
    let mut gate = ArmGate::default();
    // Arming needs a connected output (T-401). The one the command line
    // names is opened by `main` right after (or the studio stops), so it
    // counts already: the safety log then starts without a stray line.
    // The engine keeps the interlock in line with the stage afterwards.
    gate.register(interlock::OUTPUT, interlock::OUTPUT_LABEL, startup_output_id(cli).is_some());
    if cli.test_interlock {
        gate.register(interlock::TEST, "Verrou de test (--test-interlock)", false);
    }
    // No page has beaten yet: arming waits for the UI (T-252).
    gate.register(interlock::UI_ALIVE, "Aucune interface ouverte (battement perdu)", false);
    // The safety log (T-259), after the interlocks' starting states.
    gate.set_log(safety_log::SafetyLog::start(cli.data_dir.join("logs")));

    let sim = cli.midi_test.then(|| {
        let sim = midi::testing::SimMidi::default();
        sim.plug(midi::testing::TEST_MK2_PORT, midi::testing::FakeApc::new(midi::Model::Apc40Mk2));
        sim
    });

    let presets = presets::catalog();
    let controls = controls::ControlRegistry::build(&presets);
    let lfos = lfo::LfoStore::load_or_create(cli.data_dir.join("lfos.json"), &controls);
    let routes = audio::routes::RouteStore::load_or_create(cli.data_dir.join("audio_routes.json"), &controls);
    // Caps and the remembered connection (outputs.json), then the output
    // manager. Its thread is started by `main` only: building the state
    // never opens, scans or connects anything.
    let outputs = power::OutputStore::load_or_create(cli.data_dir.join("outputs.json"));
    let prefs = outputs.connection();
    let pps = cli.pps.unwrap_or(prefs.pps);
    let shownet_dmx = Arc::new(artnet::ShowNetDmxStore::load(cli.data_dir.join("shownet_artnet.json")));
    let output_mgr = Arc::new(output_backend(cli, &prefs, pps, &shownet_dmx));
    let epoch = Instant::now();
    let estop = Arc::new(EStop::default());
    let netin_cli = netin::CliOverride { osc: cli.osc, artnet: cli.artnet };
    let mut state = Shared {
        settings: Settings::default(),
        calibration: web::load_calibration(&cli.data_dir.join("calibration.json")),
        audio: AudioFeatures::default(),
        audio_at: Instant::now(),
        audio_in: Arc::new(audio::AudioHub::load(&cli.data_dir, epoch, !cli.no_audio, cli.audio_device.as_deref())),
        gate,
        netin: netin::NetInState::new(netin::NetHub::load(cli.data_dir.join("netin.json"), netin_cli, Arc::clone(&estop))),
        external: external::External::load(cli.data_dir.join("external.json")),
        estop,
        frame: Vec::new(),
        output_lit: 0,
        output_name: None,
        output_kind: None,
        output_error: None,
        applog: Default::default(),
        pps,
        scenes: SceneStore::load_or_create(cli.data_dir.join("scenes.json")),
        sdbank: sdbank::SdBankStore::load(cli.data_dir.join("sdbank.json")),
        shownet_dmx,
        output_status: None,
        playlist: None,
        controls,
        presets,
        cue_page: 0,
        active_cue: None,
        settings_rev: 0,
        deck: cues::CueDeck::load(cli.data_dir.join("grid.json")),
        grid: grid::CueGrid::load(cli.data_dir.join("pages.json")),
        cue_lists: cuelist::CueListStore::load_or_create(cli.data_dir.join("cuelists.json")),
        cue_list: Default::default(),
        look_on: true,
        tempo: tempo::TempoClock::default(),
        epoch,
        // Held effects (« Noir », « Figer », strobe flash) never survive a restart.
        live: {
            let mut live: live::LiveModifiers = load_json(&cli.data_dir.join("live.json"));
            live.release_momentary();
            live
        },
        live_dirty: false,
        palettes: live::PaletteStore::load_or_create(cli.data_dir.join("palettes.json")),
        scan_presets: scan::ScanPresetStore::load_or_create(cli.data_dir.join("scan_presets.json")),
        festival: festival::Festival::load(festival::path(&cli.data_dir)),
        midi: {
            let mut m = midi::MidiState::new(!cli.no_midi || cli.midi_test, midi::profile::ProfileStore::load(cli.data_dir.join("midi")));
            m.sim = sim.clone();
            m
        },
        lfos,
        routes,
        triggers: audio::triggers::TriggerStore::load(cli.data_dir.join("audio_triggers.json")),
        mixer: {
            let mut m: layers::Mixer = load_json(&cli.data_dir.join("layers.json"));
            m.sanitize();
            m
        },
        mixer_dirty: false,
        mix: layers::MixReport::default(),
        safety: safety::SafetyStore::load_or_create(cli.data_dir.join("safety.json")),
        strobe: safety::StrobeStatus::default(),
        outputs,
        projection: projection::ProjectionStore::load_or_create(cli.data_dir.join("projection.json")),
        audio_guard: audio::safety::GuardStatus::default(),
        dwell: dwell::DwellStatus::default(),
        evolving: Vec::new(),
        timeline: timeline::Player::default(),
        shows: timeline::ShowStore::new(cli.data_dir.join("shows")),
        timecode: timecode::TimecodeState::load(cli.data_dir.join("timecode.json")),
        figures: figures::FigureStore::load(cli.data_dir.join("figures")),
        user_cues: usercues::UserCueStore::load_or_create(cli.data_dir.join("user_cues.json")),
        media: Arc::new(audio::media::MediaStore::new(&cli.data_dir).isolated(audio::isolate::Isolation::child(
            &cli.data_dir,
            cli.test_hooks,
            cli.test_decode_timeout_ms.map_or(audio::isolate::DECODE_TIMEOUT, Duration::from_millis),
        ))),
        autoshow: Default::default(),
        ilda: Arc::new(ilda_library::IldaLibrary::new(&cli.data_dir)),
        song: Arc::new(audio::playback::SongHub::new(epoch, !cli.no_audio)),
        song_sync: Default::default(),
        presence: presence::Presence::load(cli.data_dir.join("presence.json")),
        health: Arc::new(watchdog::EngineHealth::default()),
        project: project::ProjectState::load(&cli.data_dir),
        test_pattern: Default::default(),
        keep_awake: keep_awake::KeepAwakeState::load(&cli.data_dir),
        show_mode: showmode::ShowMode::load(&cli.data_dir),
        data_warnings: Vec::new(),
        checklist: checklist::Checklist::load(cli.data_dir.join("checklist.json"), checklist_enforced(cli)),
        audience: Default::default(),
        test_hooks: cli.test_hooks,
        menu: None,
        test_stall_ms: 0,
        touches: Default::default(),
        devices: Arc::clone(output_mgr.devices()),
        output_mgr,
        // Simulated (no command run) whenever a test flag is given.
        wifi: Arc::new(wifi::WifiWatch::new(cli.test_hooks || cli.test_devices.is_some() || cli.test_output.is_some())),
        push: Default::default(),
        venue: venue::VenueStore::load_or_create(cli.data_dir.join("venue.json")),
        projector_outputs: Default::default(),
        projector_frames: Vec::new(),
        notices: Default::default(),
        history: Default::default(),
    };
    usercues::sync_presets(&mut state);
    figures::refresh(&mut state);
    if cli.test_hooks {
        state.project.autosave.allow_test_delays();
    }
    // First start: the existing data becomes a « Sans titre » project.
    project::startup(&mut state);
    // Every show is read once, so a damaged one is set aside now (T-402).
    state.shows.list();
    // Damaged files found while loading: the banner, and `data_ack` closed
    // for a safety file. The gate is disarmed whatever happens here.
    state.sync_data_warnings();
    // A real output: arming waits for the pre-show checklist (T-258).
    state.sync_checklist();
    // Projectors that drive an output: arming waits for them (T-277).
    state.sync_projectors();
    state
}

/// The pre-show checklist gates arming when a real output is configured
/// (a DAC, or PONK towards MadMapper and the ShowNET), or in tests with
/// `--test-checklist`. Pure preview and the fake `--test-output` don't.
fn checklist_enforced(cli: &Cli) -> bool {
    cli.device.is_some() || cli.ponk.is_some() || cli.shownet_artnet.is_some() || cli.test_checklist
}

/// The output manager and what it can open: the fake backend whenever a
/// test flag is given (`--test-devices`, `--test-output`), so a test can
/// never open a real DAC; the real one (laser-dac, PONK) otherwise.
fn output_backend(cli: &Cli, prefs: &output_manager::OutputPrefs, pps: u32, shownet_dmx: &Arc<artnet::ShowNetDmxStore>) -> output_manager::OutputManager {
    use output_manager::{FakeBackend, OutputManager, RealBackend};
    if cli.test_devices.is_some() || cli.test_output.is_some() {
        let fake = cli.test_devices.as_ref().map(|d| d.0.clone()).unwrap_or_default();
        let mut backend = FakeBackend::new(fake.clone(), cli.test_output.clone(), cli.ponk, cli.data_dir.clone());
        backend.artnet = Arc::clone(shownet_dmx);
        let dropped = Arc::clone(&backend.dropped);
        let list = Arc::new(devices::DeviceList::fixed(fake));
        return OutputManager::new(Arc::new(backend), list, prefs, pps).with_test_drop(dropped);
    }
    let backend = Arc::new(RealBackend {
        ponk: cli.ponk.unwrap_or_else(|| ponk::DEFAULT_TARGET.parse().expect("valid default PONK target")),
        data_dir: cli.data_dir.clone(),
        artnet: Arc::clone(shownet_dmx),
    });
    let scanner: devices::Scanner = {
        let backend = Arc::clone(&backend);
        Arc::new(move |scope| output_manager::OutputBackend::scan(&*backend, scope))
    };
    let list = Arc::new(devices::DeviceList::new(scanner, devices::Scope { usb: true }));
    OutputManager::new(backend, list, prefs, pps)
}

/// The output the command line names, as an output manager id.
fn startup_output_id(cli: &Cli) -> Option<String> {
    if let Some(target) = cli.ponk {
        return Some(format!("ponk:{target}"));
    }
    if let Some(device) = &cli.device {
        return Some(device.clone());
    }
    if let Some(target) = cli.shownet_artnet {
        return Some(format!("{}{target}", artnet::ID_PREFIX));
    }
    cli.test_output.as_ref().map(|_| output_manager::TEST_ID.to_string())
}

/// The output opened at start-up, before the engine takes it: its name in
/// the header, the `output` interlock closed, USB left out of the scans
/// while a DAC is driven.
fn note_output(state: &mut Shared, output: Option<&dyn Output>) {
    state.output_name = output.map(|o| o.name().to_string());
    state.output_kind = output.map(|o| o.kind());
    state.devices.set_driving_dac(output.is_some_and(|o| o.kind() == "dac"));
    state.gate.set_output_ready(output.is_some());
    // PONK at start-up: the ShowNET items and `hw_stop` (T-472).
    state.sync_checklist();
}

/// Installs the output the manager hands over (T-401), on the engine
/// thread between two frames. The gate is disarmed first when it was
/// armed (a switch or a reconnection never keeps the laser armed), then
/// the old output gets its dark frames and closes, then the new one is
/// installed disarmed with its kill switch in the e-stop.
fn apply_output_change(shared: &Mutex<Shared>, stage: &mut OutputStage, estop: &EStop, mgr: &output_manager::OutputManager, change: output_manager::StageChange) {
    use output_manager::ChangeKind;
    let source = if change.kind == ChangeKind::Reconnected { interlock::ArmSource::System } else { change.source };
    {
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        if s.gate.is_armed() {
            s.gate.disarm(interlock::DisarmReason::OutputChanged, source);
        }
        s.gate.set_output_ready(false);
    }
    estop.replace_kill_switch(None);
    close_aside(stage.replace(change.output));
    estop.replace_kill_switch(stage.kill_switch());
    let described = stage.describe();
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    let action = match change.kind {
        ChangeKind::Connected => "connect",
        ChangeKind::Disconnected => "disconnect",
        ChangeKind::Reconnected => "reconnect",
    };
    s.gate.log().record("output", Some(source.id()), serde_json::json!({ "action": action, "id": change.id, "name": described.as_ref().map(|d| d.0.clone()) }));
    s.output_name = described.as_ref().map(|d| d.0.clone());
    s.output_kind = described.as_ref().map(|d| d.1);
    s.output_error = None;
    s.output_status = None;
    s.devices.set_driving_dac(described.as_ref().is_some_and(|d| d.1 == "dac"));
    // A real output (a DAC, PONK to MadMapper, the ShowNET over Art-Net):
    // the pre-show checklist gates arming from now on, as with --device
    // or --ponk (T-258).
    if described.as_ref().is_some_and(|d| matches!(d.1, "dac" | "ponk" | artnet::KIND)) && !s.checklist.enforced() {
        s.checklist.enforce();
    }
    // And the ShowNET items and `hw_stop` follow the new output (T-472).
    s.sync_checklist();
    // Arming waits until this is the output the operator wants.
    let ok = described.is_some() && mgr.is_current(change.gen);
    s.gate.set_output_ready(ok);
}

/// The installed output stopped answering: disarm (« Sortie perdue »),
/// open the `output` interlock, close it, and let the manager retry.
fn output_lost(shared: &Mutex<Shared>, stage: &mut OutputStage, estop: &EStop, mgr: &output_manager::OutputManager, gen: u64, error: String) {
    {
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        s.gate.disarm(interlock::DisarmReason::OutputLost, interlock::ArmSource::System);
        s.gate.set_output_ready(false);
        s.gate.log().record("output", Some("system"), serde_json::json!({ "action": "lost", "name": s.output_name, "error": error }));
        s.output_error = Some(error.clone());
        s.output_status = None;
    }
    estop.replace_kill_switch(None);
    close_aside(stage.replace(None));
    mgr.report_lost(gen, error);
}

/// Closes an output the stage has disarmed and blanked, on a thread of
/// its own: dropping a DAC session waits for its worker, which must never
/// hold up the next frame.
fn close_aside(old: Option<Box<dyn Output>>) {
    if let Some(old) = old {
        let spawned = std::thread::Builder::new().name("output-close".into()).spawn(move || drop(old));
        if let Err(e) = spawned {
            log::warn!("closing the old output aside failed: {e}");
        }
    }
}

/// One more projector's output stage (T-277): its own output, strobe
/// limiter, static-beam guard and safety-log tracker. The main projector's
/// are the `Engine`'s own fields.
struct ProjectorStage {
    id: String,
    stage: OutputStage,
    limiter: safety::StrobeLimiter,
    dwell_guard: dwell::DwellState,
    strobe_log: safety_log::LimiterTracker,
    /// The hand-over of its installed output, for loss reports.
    gen: u64,
}

impl ProjectorStage {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            stage: OutputStage::new(None),
            limiter: safety::StrobeLimiter::default(),
            dwell_guard: dwell::DwellState::default(),
            strobe_log: safety_log::LimiterTracker::new("strobe"),
            gen: 0,
        }
    }
}

/// What one more projector gets this frame, read under the lock (T-277).
struct ProjectorPlan {
    id: String,
    /// The output it is meant to drive (its projection zones follow it).
    output: Option<String>,
    caps: power::OutputLimits,
    color: colorcal::ColorCalibration,
    /// Its points per frame (T-171).
    frame_limits: output::FrameLimits,
    /// For an Art-Net output (T-301).
    cue_signal: Option<output::CueSignal>,
}

/// `apply_output_change` for a venue projector's output (T-277): the gate
/// is disarmed first when armed (any output change disarms every
/// projector), the old output gets its dark frames and closes, the new one
/// is installed disarmed with its kill switch in the e-stop. The
/// `projectors` interlock closes only once it is the output wanted.
fn apply_projector_change(shared: &Mutex<Shared>, extras: &mut Vec<ProjectorStage>, estop: &EStop, mgr: &output_manager::OutputManager, pid: &str, change: output_manager::StageChange) {
    use output_manager::ChangeKind;
    let source = if change.kind == ChangeKind::Reconnected { interlock::ArmSource::System } else { change.source };
    {
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        if s.gate.is_armed() {
            s.gate.disarm(interlock::DisarmReason::OutputChanged, source);
        }
        s.projector_outputs.remove(pid);
        s.sync_projectors();
    }
    let i = match extras.iter().position(|p| p.id == pid) {
        Some(i) => i,
        None => {
            extras.push(ProjectorStage::new(pid));
            extras.len() - 1
        }
    };
    let st = &mut extras[i];
    estop.replace_projector_kill_switch(pid, None);
    close_aside(st.stage.replace(change.output));
    estop.replace_projector_kill_switch(pid, st.stage.kill_switch());
    st.gen = change.gen;
    let described = st.stage.describe();
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    let action = match change.kind {
        ChangeKind::Connected => "connect",
        ChangeKind::Disconnected => "disconnect",
        ChangeKind::Reconnected => "reconnect",
    };
    s.gate.log().record("output", Some(source.id()), serde_json::json!({ "action": action, "projector": pid, "id": change.id, "name": described.as_ref().map(|d| d.0.clone()) }));
    if let (Some((name, kind)), Some(id)) = (described, change.id) {
        if mgr.is_current_projector(pid, change.gen) {
            s.projector_outputs.insert(pid.to_string(), venue::Installed { output: id, name, kind });
        }
        // A real output on any projector: the pre-show checklist gates
        // arming from now on (T-258).
        if matches!(kind, "dac" | "ponk" | artnet::KIND) && !s.checklist.enforced() {
            s.checklist.enforce();
        }
    }
    s.sync_checklist();
    s.sync_projectors();
}

/// A venue projector's output stopped answering (T-277): every projector
/// disarms (« Sortie perdue »), the `projectors` interlock opens until it
/// is back, and the manager retries it.
fn projector_lost(shared: &Mutex<Shared>, st: &mut ProjectorStage, estop: &EStop, mgr: &output_manager::OutputManager, error: String) {
    {
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        s.gate.disarm(interlock::DisarmReason::OutputLost, interlock::ArmSource::System);
        let name = s.projector_outputs.remove(&st.id).map(|i| i.name);
        s.gate.log().record("output", Some("system"), serde_json::json!({ "action": "lost", "projector": st.id, "name": name, "error": error }));
        s.sync_projectors();
    }
    estop.replace_projector_kill_switch(&st.id, None);
    close_aside(st.stage.replace(None));
    mgr.report_lost_projector(&st.id, st.gen, error);
}

/// The safety stage of one projector's frame, in its order (see
/// `Engine::tick`): frame size, colour rendering, strobe limiter and
/// horizon, static-beam guard, power caps. Every projector runs it with
/// its own limiter, guard, colour and caps (T-277).
#[allow(clippy::too_many_arguments)]
fn safety_chain(
    frame: Vec<Point>,
    frame_limits: output::FrameLimits,
    max_points: Option<usize>,
    color: &colorcal::ColorCalibration,
    t: f64,
    safety_cfg: &safety::SafetySettings,
    limiter: &mut safety::StrobeLimiter,
    dwell_guard: &mut dwell::DwellState,
    armed: bool,
    caps: &power::OutputLimits,
) -> Vec<Point> {
    // Points per frame of the output (T-171: its own range, and the
    // Helios USB frame size, T-303): thinned or completed here, before
    // the safety stage (T-353), so zones, horizon, strobe limiter, static
    // beam guard and caps all apply to the frame that really goes out.
    // Nothing after the safety stage adds light.
    let mut frame = output::fit_frame_limits(frame, frame_limits, max_points);
    // The output's colour rendering (T-414): curve, white balance and
    // colour delay, on the frame that goes out, *before* the safety
    // stage. Channels stay within 0..1, a dark point stays dark (the
    // delay never colours travel), and the zones, horizon, safety
    // gains, limiter, guard and caps all apply after it. Identity:
    // the frame is untouched.
    color.apply(&mut frame);
    // Strobe limiter and beam horizon (T-101): on the finished,
    // calibrated frame, so no layer, live move, LFO, gate or flash can
    // get past it; the preview shows the limited frame too.
    let frame = safety::apply(frame, t, safety_cfg, limiter);
    // Static beam guard (T-256): on the frame that really goes out
    // (after zones and the limiter), so a figure shrunk to a point, a
    // frozen frame or a held beam is dimmed whatever made it.
    // Strict below the horizon and in the Public zones (T-255).
    let area = dwell::StrictArea::new(safety_cfg.horizon.y, &safety_cfg.zones);
    let mut frame = dwell_guard.apply(frame, t, &safety_cfg.dwell, &area, armed);
    // Power caps of the output (T-254), after everything else in the
    // safety stage: they can only reduce, and a lowered cap applies
    // from this tick. The preview shows the capped frame too.
    power::cap(&mut frame, caps);
    frame
}

fn run_engine(
    shared: Arc<Mutex<Shared>>,
    output: Option<Box<dyn Output>>,
    running: Arc<AtomicBool>,
    live_path: PathBuf,
    layers_path: PathBuf,
) {
    let mut engine = Engine::new(&shared, output, live_path, layers_path);
    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        engine.tick(&shared, now);
        std::thread::sleep(FRAME_INTERVAL.saturating_sub(now.elapsed()));
    }
    engine.shutdown(&shared);
}

/// The engine thread's own state between frames. One `tick` is one frame;
/// `run_engine` calls it at 60 fps, and the safety invariant tests
/// (safety_invariants.rs) call it on a simulated clock, so they drive the
/// very same pipeline.
struct Engine {
    /// One animator per playing cue instance (0 = the manual look), so
    /// every cue keeps its own motion and a restart starts it over.
    animators: HashMap<u64, Animator>,
    live_state: live::LiveState,
    /// Rotation state of timeline events with their own modifiers.
    event_live: HashMap<u64, live::LiveState>,
    /// Rotation state of user cues stored with master modifiers (T-405).
    cue_live: HashMap<u64, live::LiveState>,
    /// « Figer » held (T-142): the (seconds, beats) time stood still at,
    /// and the content beat of each timeline event met while frozen.
    frozen: Option<(f64, f64)>,
    frozen_events: HashMap<u64, f64>,
    frames_since_save: u32,
    last: Instant,
    /// If this thread panics, unwinding drops the stage: dark frame + disarm.
    stage: OutputStage,
    limiter: safety::StrobeLimiter,
    /// Strobe limiter engagements go to the safety log, at most one line a
    /// second (T-259); nothing is allocated on the frames that write none.
    safety_log: safety_log::SafetyLog,
    strobe_log: safety_log::LimiterTracker,
    audio_guard: audio::safety::AudioGuard,
    dwell_guard: dwell::DwellState,
    live_path: PathBuf,
    layers_path: PathBuf,
    /// Outputs connected, switched or found again from the UI (T-401).
    output_mgr: Arc<output_manager::OutputManager>,
    /// The hand-over of the installed output, for loss reports.
    output_gen: u64,
    /// The venue's other projectors (T-277), each with its own stage.
    extras: Vec<ProjectorStage>,
}

impl Engine {
    fn new(shared: &Mutex<Shared>, output: Option<Box<dyn Output>>, live_path: PathBuf, layers_path: PathBuf) -> Self {
        let (safety_log, output_mgr) = {
            let s = shared.lock().unwrap();
            (s.gate.log().clone(), Arc::clone(&s.output_mgr))
        };
        let output_gen = output_mgr.current_gen();
        Self {
            animators: HashMap::new(),
            live_state: live::LiveState::default(),
            event_live: HashMap::new(),
            cue_live: HashMap::new(),
            frozen: None,
            frozen_events: HashMap::new(),
            frames_since_save: 0,
            last: Instant::now(),
            stage: OutputStage::new(output),
            limiter: safety::StrobeLimiter::default(),
            safety_log,
            strobe_log: safety_log::LimiterTracker::new("strobe"),
            audio_guard: audio::safety::AudioGuard::default(),
            dwell_guard: dwell::DwellState::default(),
            live_path,
            layers_path,
            output_mgr,
            output_gen,
            extras: Vec::new(),
        }
    }

    /// One frame at `now`: render, safety stage, gate, output.
    fn tick(&mut self, shared: &Mutex<Shared>, now: Instant) {
        // A new output from the manager's thread: installed between two
        // frames, never opened here.
        while let Some(change) = self.output_mgr.engine_take() {
            let estop = Arc::clone(&shared.lock().unwrap_or_else(|e| e.into_inner()).estop);
            match change.projector.clone() {
                // A venue projector's output (T-277).
                Some(pid) => apply_projector_change(shared, &mut self.extras, &estop, &self.output_mgr, &pid, change),
                None => {
                    self.output_gen = change.gen;
                    apply_output_change(shared, &mut self.stage, &estop, &self.output_mgr, change);
                }
            }
        }
        let dt = now.saturating_duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;

        let (looks, transition_fx, show_cues, starts, cue_masters, clock, mixer, calibration, audio, armed, hold_ok, live, user_palettes, estop, health, t, safety_cfg, (caps, frame_limits), color, pattern, routing, cue_signal, plans, (festival_palette, balance), (look_scan, show_scan)) = {
            let mut s = shared.lock().unwrap();
            // E-stop latch, watchdog trip, heartbeats and hold-to-run, then
            // the audience mode, which ends with any of them (T-255).
            let hold_ok = s.sync_safety(now);
            s.sync_audience();
            s.health.tick(s.gate.is_armed());
            if s.test_stall_ms > 0 {
                // `--test-hooks` only: a stall while holding the lock.
                let stall = Duration::from_millis(std::mem::take(&mut s.test_stall_ms));
                std::thread::sleep(stall);
            }
            self.frames_since_save += 1;
            if (s.live_dirty || s.mixer_dirty) && self.frames_since_save >= 60 {
                // At most once a second, so MIDI faders don't hammer the disk.
                if s.live_dirty {
                    save_json(&self.live_path, &s.live);
                }
                if s.mixer_dirty {
                    save_json(&self.layers_path, &s.mixer);
                }
                s.live_dirty = false;
                s.mixer_dirty = false;
                self.frames_since_save = 0;
            }
            advance_playlist(&mut s);
            // MIDI faders/encoders: at most one write per control per frame.
            midi::engine::frame(&mut s, now);
            // OSC and Art-Net: what the listeners queued, then signal loss.
            netin::frame(&mut s, now);
            // Native capture, else the browser's features, else silence
            // (falling, not jumping): one snapshot for the whole frame.
            let (audio, active) = s.audio_in.frame(s.audio, s.audio_at, now);
            // Audio safety (T-245): freshness, silence, source changes and
            // the flash cap. It only shapes copies; arming is never touched.
            let (source_on, audio_safety) = s.audio_in.reaction_config();
            let reaction = self.audio_guard.frame(&audio, active, source_on, &audio_safety, dt);
            // `Shared::now_s` taken at `now`, so a simulated clock (tests)
            // drives everything.
            let t = now.saturating_duration_since(s.epoch).as_secs_f64();
            // Tempo auto (T-234): the clock follows the detection's proposal
            // on its own terms. Only the one clock; arming untouched.
            if s.tempo.source == tempo::TempoSource::Audio {
                let est = s.audio_in.fresh_tempo(now).unwrap_or_default();
                s.tempo.apply_detection(&est, t);
            }
            // Quantised launches due now start, finished transitions end;
            // with the e-stop latched, every transition is cut (T-158/159).
            controls::deck_frame(&mut s, t);
            let clock = engine::BeatClock { beat: s.tempo.beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar };
            self.live_state.set_clock(t, clock.beat);
            // « Figer » (T-142): while held, animation, LFOs and colour
            // effects read the time it was pressed at. The strobe, the
            // limiter and the static-beam guard keep the real clock.
            if !s.live.freeze {
                self.frozen = None;
                self.frozen_events.clear();
            } else if self.frozen.is_none() {
                self.frozen = Some((t, clock.beat));
            }
            let (mod_t, mod_beat) = self.frozen.unwrap_or((t, clock.beat));
            self.live_state.freeze_colours(self.frozen);
            // A festival palette waiting for its « one » lands (T-130).
            s.festival.fire_due(clock.beat);
            // Audio triggers (T-240): this frame's events press cues before
            // the looks are read. Off, stale audio or e-stop: nothing.
            // They never arm; their black is a gain on the master copy.
            audio::triggers::frame(&mut s, &audio, active, reaction.guard.source_changed, t, &clock, audio_safety.max_flash_hz);
            // LFOs, then audio routes, move copies: the stored values stay
            // the operator's base. The Temps ↔ Audio crossfader shares them.
            // Silence with *Look calme*: that scene stands in for the manual
            // look (a copy; a missing scene keeps the look).
            let calm = match (&audio_safety.silence_action, reaction.silent) {
                (audio::safety::SilenceAction::CalmLook(name), true) => s.scenes.get(name).map(|sc| sc.settings.clone()),
                _ => None,
            };
            let (mut settings, mut live) = (calm.unwrap_or_else(|| s.settings.clone()), s.live.clone());
            let time_share = s.routes.routing().time_share();
            lfo::modulate_scaled(s.lfos.list(), &s.controls, &mut settings, &mut live, mod_t, mod_beat, time_share);
            let beat_len_s = (60.0 / clock.bpm.max(1.0)) as f32;
            if s.routes.apply(&audio, dt, beat_len_s, &reaction.guard, &mut settings, &mut live) {
                lfo::recolor_live(&mut live);
            }
            // Silence with *Noir*: the master copy fades to black. Not the
            // transport blackout: nothing to release, arming untouched.
            live.brightness = live.brightness.clamp(0.0, 1.0) * reaction.gain;
            // A trigger's momentary black (T-240): the same copy, not the
            // transport blackout.
            live.brightness *= s.triggers.engine.gain();
            // DMX signal lost with *Fondu au noir* / *Noir immédiat* (T-406):
            // the same copy goes dark, before the safety stage. No disarm.
            live.brightness *= s.netin.gain.clamp(0.0, 1.0);
            self.audio_guard.set_flash_limited(s.routes.flash_limited());
            s.audio_guard = self.audio_guard.status();
            // The looks' own reaction sees the beat counter capped too.
            let audio = reaction.visual;
            // The cue list (T-410): on its own clock, paused by the e-stop.
            // Its steps join the looks under the cues played by hand, its
            // fades the transitions: same layers, same safety chain.
            cuelist::frame(&mut s, dt as f64, clock.beat, &audio);
            let mut looks = s.cue_list.looks();
            looks.extend(cues::layered_looks(&s.deck, &settings, s.look_on));
            // Fades and morphs between cues (T-158): applied to the
            // rendered looks, before the mixer and the safety chain.
            let mut transition_fx = s.deck.transition_fx(t);
            transition_fx.extend(s.cue_list.fx());
            // A test pattern (T-404) holds the timeline paused and stands
            // in for the whole look; the cues wait in the deck.
            let pattern = testpattern::hold_timeline(&mut s);
            let show_cues = timeline_cues(&mut s, t, &clock);
            let mixer = s.mixer.clone();
            // Scan presets (T-415): the optimiser settings each look is
            // traced with (event > cell > figure > the output's default;
            // « Auto » = `mixer.optimizer`), before the mixer and the
            // whole safety chain.
            let look_scan: Vec<optimize::OptimizeParams> = looks.iter().map(|(_, id, look)| scan::for_look(&s, *id, look)).collect();
            let show_scan: Vec<optimize::OptimizeParams> = show_cues.iter().map(|(cue, look)| scan::for_event(&s, cue, look)).collect();
            // Launch beats, so a cue's beat-synced motion counts from its start.
            let starts: HashMap<u64, f64> = s.deck.active.iter().map(|a| (a.id, a.started_beat)).collect();
            // User cues stored with master modifiers (T-405): theirs, per cue.
            let cue_masters: HashMap<u64, live::LiveModifiers> = s
                .deck
                .active
                .iter()
                .chain(s.deck.ending.iter().map(|e| &e.cue))
                .filter_map(|a| Some((a.id, usercues::master_of(&s, &a.cue)?)))
                .collect();
            let armed = s.gate.is_armed();
            let routing = Routing::read(&s, &looks, &show_cues);
            let caps = s.outputs.active_limits();
            // The ShowNET SD card over Art-Net (T-301) plays the cue's bank
            // number itself: it needs the cue, not only the points.
            let cue_signal = (s.output_kind == Some(artnet::KIND)).then(|| {
                let timeline_cue = show_cues.iter().rev().find_map(|(c, _)| match &c.source {
                    timeline::EventSource::Cue { id } => Some(id.as_str()),
                    timeline::EventSource::Look { .. } => None,
                });
                // The cue on show after transitions (T-158) and the beat
                // mode; a launch still waiting (T-159) is not on show.
                artnet::cue_signal(&s, s.deck.on_show(t), &live, timeline_cue, pattern.is_some(), &caps)
            });
            // The venue's other projectors (T-277): each its own caps and
            // colour rendering (`outputs.json` under its id; the cautious
            // defaults while it has none), and its cue signal if its
            // output plays the SD card.
            let plans: Vec<ProjectorPlan> = s
                .venue
                .get()
                .extras()
                .map(|p| {
                    let cfg = s.outputs.config_for(&p.id);
                    let cue_signal = s.projector_outputs.get(&p.id).filter(|o| o.kind == artnet::KIND).map(|_| {
                        let timeline_cue = show_cues.iter().rev().find_map(|(c, _)| match &c.source {
                            timeline::EventSource::Cue { id } => Some(id.as_str()),
                            timeline::EventSource::Look { .. } => None,
                        });
                        artnet::cue_signal(&s, s.deck.on_show(t), &live, timeline_cue, pattern.is_some(), &cfg.limits)
                    });
                    ProjectorPlan { id: p.id.clone(), output: p.output.clone(), caps: cfg.limits, color: cfg.color, frame_limits: cfg.scan.frame_limits(), cue_signal }
                })
                .collect();
            let festival = (s.festival.colors(), s.festival.balance);
            (looks, transition_fx, show_cues, starts, cue_masters, clock, mixer, s.calibration, audio, armed, hold_ok, live, s.palettes.list().to_vec(), Arc::clone(&s.estop), Arc::clone(&s.health), t, s.effective_safety(), (caps, s.outputs.active().scan.frame_limits()), s.outputs.active_color(), pattern, routing, cue_signal, plans, festival, (look_scan, show_scan))
        };

        // « Figer »: no time passes for the animators and the rotations,
        // and the looks see the beat it was pressed on.
        let (live_dt, frozen) = if self.frozen.is_some() { (0.0, true) } else { (dt, false) };
        let render_clock = match self.frozen {
            Some((_, beat)) => engine::BeatClock { beat, ..clock },
            None => clock,
        };
        self.live_state.advance(&live, live_dt, clock.bpm, clock.beats_per_bar);
        let (mut frame, mut extra_frames, mix, evolving) = match pattern {
            // The pattern alone, densified and calibrated, without live
            // modifiers (exact geometry). The cue animators are left as
            // they are, so the cues carry on where they were afterwards.
            // While aligning a projection zone, through that zone's geometry.
            // Every projector of the venue shows it (T-277).
            Some((p, brightness)) => {
                let frame = testpattern::output_frame(p, brightness, routing.align.as_ref(), &calibration);
                (frame.clone(), vec![frame; plans.len()], layers::MixReport::default(), Vec::new())
            }
            None => {
                let anim_dt = live_dt * live.speed.clamp(0.0, 4.0);
                let (animators, event_live, cue_live, frozen_events) = (&mut self.animators, &mut self.event_live, &mut self.cue_live, &mut self.frozen_events);
                let frozen_at = self.frozen;
                animators.retain(|id, _| looks.iter().any(|(_, i, _)| i == id) || show_cues.iter().any(|(c, _)| c.instance == *id));
                event_live.retain(|id, _| show_cues.iter().any(|(c, _)| c.instance == *id));
                cue_live.retain(|id, _| cue_masters.contains_key(id));
                // Timeline events first (under the cues played by hand on the same
                // layer), each on its own content clock; paused or held ones freeze.
                let mut rendered: Vec<projection::Routed> = show_cues
                    .iter()
                    .zip(&routing.show_zones)
                    .zip(&show_scan)
                    .map(|(((cue, settings), zones), scan)| {
                        // An event met mid-way (a seek, a timecode jump, T-409)
                        // starts with the motion it would have had playing from
                        // its start: its first step is all the time elapsed.
                        let fresh = !animators.contains_key(&cue.instance);
                        let animator = animators.entry(cue.instance).or_insert_with(|| Animator::starting_at(0.0));
                        let content_beat = if frozen { *frozen_events.entry(cue.instance).or_insert(cue.content_beat) } else { cue.content_beat };
                        let content_clock = engine::BeatClock { beat: content_beat, ..clock };
                        let event_dt = match (fresh, cue.frozen || frozen) {
                            (true, _) => cue.elapsed_s as f32 * live.speed.clamp(0.0, 4.0),
                            (false, true) => 0.0,
                            (false, false) => anim_dt,
                        };
                        animator.set_optimizer(scan);
                        animator.set_palette(festival_palette);
                        let mut points = animator.render(settings, audio, event_dt, &content_clock);
                        if cue.modifiers != live::LiveModifiers::default() {
                            let fresh = !event_live.contains_key(&cue.instance);
                            let st = event_live.entry(cue.instance).or_default();
                            st.set_clock(t, clock.beat);
                            st.freeze_colours(frozen_at);
                            let live_dt = match (fresh, cue.frozen || frozen) {
                                (true, _) => cue.elapsed_s as f32,
                                (false, true) => 0.0,
                                (false, false) => dt,
                            };
                            st.advance(&cue.modifiers, live_dt, clock.bpm, clock.beats_per_bar);
                            points = live::apply(&points, &cue.modifiers, st, &user_palettes);
                        }
                        (cue.layer, points, zones.clone())
                    })
                    .collect();
                // Muted layers keep animating, so they come back in motion.
                let mut cue_looks: Vec<transition::Rendered<(u8, Vec<u8>)>> = looks.iter().zip(&routing.look_zones).zip(&look_scan).map(|(((layer, id, settings), zones), scan)| {
                    let animator = animators.entry(*id).or_insert_with(|| match starts.get(id) {
                        Some(&beat) => Animator::starting_at(beat),
                        None => Animator::default(),
                    });
                    animator.set_optimizer(scan);
                    animator.set_palette(festival_palette);
                    let mut points = animator.render(settings, audio, anim_dt, &render_clock);
                    // A user cue's own master modifiers (T-405), before the
                    // master stage, like a timeline event's.
                    if let Some(m) = cue_masters.get(id) {
                        let st = cue_live.entry(*id).or_default();
                        st.set_clock(t, clock.beat);
                        st.freeze_colours(frozen_at);
                        st.advance(m, live_dt, clock.bpm, clock.beats_per_bar);
                        points = live::apply(&points, m, st, &user_palettes);
                    }
                    (*id, points, (*layer, zones.clone()))
                }).collect();
                // Transitions between cues (T-158): fades scale colours by
                // at most 1, a morph never lights travel (transition.rs);
                // the mixer, live stage, zones, calibration and the whole
                // safety stage still come after.
                transition::apply(&mut cue_looks, &transition_fx);
                rendered.extend(cue_looks.into_iter().map(|(_, points, (layer, zones))| (layer, points, zones)));
                let evolving: Vec<(u64, evolving::Progress)> =
                    looks.iter().filter_map(|(_, id, _)| Some((*id, animators.get(id)?.progress()?.clone()))).collect();
                // Per projection zone (T-012): layers 1 → 4 with their
                // dimmers within the zone's share of the point budget, the
                // master live controls, then the zone's geometry and the
                // calibration (both clamp to -1..1). Zones are joined by
                // blanked travel; one neutral zone is the plain path.
                // A zone patched as a DMX fixture (T-407) gets its remote on
                // top of the master controls, still before the geometry,
                // the calibration and the safety stage.
                // Perceived balance (T-130) on the content, with the master
                // stage: gains of at most 1, before the calibration, the
                // output's colour calibration and the safety stage.
                let finish = |zone: u8, look: &[Point]| {
                    let mut out = match routing.remotes.iter().find(|(z, _)| *z == zone) {
                        Some((_, remote)) => dmx::fixture::apply(look, remote, &live, &self.live_state, &user_palettes, t),
                        None => live::apply(look, &live, &self.live_state, &user_palettes),
                    };
                    balance.apply(&mut out);
                    out
                };
                if plans.is_empty() {
                    let (frame, mix) = projection::render(rendered, &routing.config, routing.output.as_deref(), &mixer, finish, &calibration);
                    (frame, Vec::new(), mix, evolving)
                } else {
                    // A venue with several projectors (T-277): each draws
                    // its own zones (`ProjectionConfig::active_on`), from
                    // the same rendered looks, through the same geometry
                    // and calibration. The mix report is the main one's.
                    let main = (power::MAIN_OUTPUT, routing.output.as_deref());
                    let all: Vec<(&str, Option<&str>)> = std::iter::once(main).chain(plans.iter().map(|p| (p.id.as_str(), p.output.as_deref()))).collect();
                    let draw = |me: usize| {
                        let others: Vec<(&str, Option<&str>)> = all.iter().enumerate().filter(|(i, _)| *i != me).map(|(_, o)| *o).collect();
                        let zones = routing.config.active_on(all[me].0, all[me].1, &others);
                        projection::render_zones(&rendered, &routing.config, zones, &mixer, finish, &calibration)
                    };
                    let (frame, mix) = draw(0);
                    let extra = (1..all.len()).map(|i| draw(i).0).collect();
                    (frame, extra, mix, evolving)
                }
            }
        };
        // « Noir » held (T-142): every projector's frame dark, a test
        // pattern's too (the master stage already darkened the looks).
        // Arming is untouched, and the safety stage still follows.
        if live.black {
            live::black_out(&mut frame);
            extra_frames.iter_mut().for_each(|f| live::black_out(f));
        }
        // The safety stage (`safety_chain`): frame size, colour rendering,
        // strobe limiter and horizon, static beam guard, power caps.
        let frame = safety_chain(frame, frame_limits, self.stage.max_points(), &color, t, &safety_cfg, &mut self.limiter, &mut self.dwell_guard, armed, &caps);

        // Last stage: the gate. `armed` was read under the lock at the top
        // of the frame; the e-stop latch and a watchdog trip (this frame
        // stalled) are re-read here, lock-free.
        if let Some(signal) = &cue_signal {
            self.stage.cue_signal(signal);
        }
        let emitted = self.stage.emit(&frame, armed && !health.is_tripped(), hold_ok, &estop);
        let output_status = self.stage.status();
        // An output that stopped answering (T-401): disarm, close, retry.
        if let Some(error) = self.stage.fault() {
            output_lost(shared, &mut self.stage, &estop, &self.output_mgr, self.output_gen, error);
        }
        let strobe = self.limiter.status();
        self.strobe_log.update(strobe.active, now, &self.safety_log, || serde_json::json!({ "rate_hz": strobe.rate_hz, "burst_s": strobe.burst_s }));
        let projector_frames = self.emit_projectors(shared, plans, extra_frames, now, t, &safety_cfg, armed && !health.is_tripped(), hold_ok, &estop);
        let mut s = shared.lock().unwrap();
        if let Some(error) = emitted.arm_change {
            if let Some(e) = &error {
                log::error!("sortie laser : {e}");
            } else if s.output_error.is_some() {
                log::info!("sortie laser : de nouveau sans erreur");
            }
            s.output_error = error;
        }
        s.output_lit = emitted.lit;
        s.output_status = output_status;
        // The pages' binary frames (T-411): a swap in the hub, no network
        // write, no wait on a client. The flags say what the gate is now.
        let estop_latched = s.estop.is_latched();
        let flags = [
            (s.gate.is_armed() && !estop_latched, push::flags::ARMED),
            (estop_latched, push::flags::ESTOP),
            (emitted.lit > 0, push::flags::LIT),
            (s.health.engine_ok(), push::flags::ENGINE_OK),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .fold(0, |acc, (_, bit)| acc | bit);
        s.push.publish_frame(&frame, flags);
        s.frame = frame;
        s.mix = mix;
        s.strobe = strobe;
        s.dwell = self.dwell_guard.status().clone();
        s.evolving = evolving;
        s.projector_frames = projector_frames;
    }

    /// The venue's other projectors (T-277), after the main one: each frame
    /// through its own safety chain and its own gate. The arm state is the
    /// main one's (one arm, one e-stop); the e-stop latch is re-read by
    /// each stage. A stage whose projector left the venue stays dark until
    /// the manager takes its output away, then goes.
    #[allow(clippy::too_many_arguments)]
    fn emit_projectors(
        &mut self,
        shared: &Mutex<Shared>,
        plans: Vec<ProjectorPlan>,
        frames: Vec<Vec<Point>>,
        now: Instant,
        t: f64,
        safety_cfg: &safety::SafetySettings,
        armed: bool,
        hold_ok: bool,
        estop: &EStop,
    ) -> Vec<venue::ProjectorFrame> {
        self.extras.retain(|st| st.stage.describe().is_some() || plans.iter().any(|p| p.id == st.id));
        for st in self.extras.iter_mut().filter(|st| !plans.iter().any(|p| p.id == st.id)) {
            st.stage.emit(&[Point::blanked(0.0, 0.0)], false, false, estop);
        }
        let mut out = Vec::with_capacity(plans.len());
        for (plan, frame) in plans.into_iter().zip(frames) {
            let i = match self.extras.iter().position(|st| st.id == plan.id) {
                Some(i) => i,
                None => {
                    self.extras.push(ProjectorStage::new(&plan.id));
                    self.extras.len() - 1
                }
            };
            let st = &mut self.extras[i];
            let frame = safety_chain(frame, plan.frame_limits, st.stage.max_points(), &plan.color, t, safety_cfg, &mut st.limiter, &mut st.dwell_guard, armed, &plan.caps);
            if let Some(signal) = &plan.cue_signal {
                st.stage.cue_signal(signal);
            }
            let emitted = st.stage.emit(&frame, armed, hold_ok, estop);
            if let Some(Some(e)) = &emitted.arm_change {
                log::error!("sortie du projecteur {} : {e}", st.id);
            }
            if let Some(error) = st.stage.fault() {
                projector_lost(shared, st, estop, &self.output_mgr, error);
            }
            let strobe = st.limiter.status();
            let id = st.id.clone();
            st.strobe_log.update(strobe.active, now, &self.safety_log, || serde_json::json!({ "projector": id, "rate_hz": strobe.rate_hz, "burst_s": strobe.burst_s }));
            out.push(venue::ProjectorFrame { id: plan.id, points: frame, lit: emitted.lit });
        }
        out
    }

    /// Clean shutdown: disarm the gate (recorded), then the output goes
    /// dark and closes. A poisoned lock still holds the gate.
    fn shutdown(mut self, shared: &Mutex<Shared>) {
        shared.lock().unwrap_or_else(|e| e.into_inner()).gate.disarm(interlock::DisarmReason::Shutdown, interlock::ArmSource::System);
        self.stage.shutdown();
        for st in &mut self.extras {
            st.stage.shutdown();
        }
        // The last second of live and layer moves joins the working copy (else
        // the next start would see them only in the autosave, T-287).
        let s = shared.lock().unwrap_or_else(|e| e.into_inner());
        if s.live_dirty {
            save_json(&self.live_path, &s.live);
        }
        if s.mixer_dirty {
            save_json(&self.layers_path, &s.mixer);
        }
    }
}

/// A file directly in the data directory, or its defaults; a damaged one
/// is moved to the quarantine folder and reported (datafile.rs, T-402).
pub fn load_json<T: serde::de::DeserializeOwned + Default>(path: &std::path::Path) -> T {
    datafile::load_or_default(path, &datafile::beside(path), false)
}

pub use datafile::save_json;

/// The timeline's events for this frame, with their looks. An emergency
/// stop halts the timeline (it never resumes by itself), and so its song;
/// otherwise the song tells the timeline where it is (T-161) and its
/// events join the cues, through the same mix, live stage and gate.
/// Where this frame's looks go (T-012), read under the lock: each look's
/// projection zones (empty = the default zones), the zone settings, the
/// connected output and the zone a test pattern is aligned through.
struct Routing {
    look_zones: Vec<Vec<u8>>,
    show_zones: Vec<Vec<u8>>,
    config: projection::ProjectionConfig,
    output: Option<String>,
    align: Option<projection::Geometry>,
    /// Each zone's DMX remote this frame (T-407); none: the master only.
    remotes: Vec<(u8, dmx::personality::ZoneRemote)>,
}

impl Routing {
    fn read(s: &Shared, looks: &[(u8, u64, Settings)], show_cues: &[(timeline::TimelineCue, Settings)]) -> Self {
        let of_cue = |cue: &str| s.deck.slot(cue).zones;
        let look_zones = looks
            .iter()
            .map(|(_, id, _)| s.deck.find(*id).map(|a| of_cue(&a.cue)).unwrap_or_default())
            .collect();
        let show_zones = show_cues
            .iter()
            .map(|(cue, _)| match &cue.source {
                timeline::EventSource::Cue { id } => of_cue(id),
                timeline::EventSource::Look { .. } => Vec::new(),
            })
            .collect();
        Self {
            look_zones,
            show_zones,
            config: s.projection.get().clone(),
            output: s.output_mgr.prefs().selected,
            align: s.projection.align_geometry(),
            remotes: s.netin.fixtures.remotes.clone(),
        }
    }
}

fn timeline_cues(s: &mut Shared, t: f64, clock: &engine::BeatClock) -> Vec<(timeline::TimelineCue, Settings)> {
    let c = timeline::Clock { t, beat: clock.beat, bpm: clock.bpm, beats_per_bar: clock.beats_per_bar };
    if s.estop.is_latched() {
        s.timeline.halt(&c);
    }
    let song = Arc::clone(&s.song);
    s.song_sync.sync(&song, &mut s.timeline, &c);
    // Incoming timecode (T-168) has the last word on the playhead.
    timecode::frame(s, &c);
    // A cue event plays the cue's grid program too (T-157).
    let with_program = |cue: &timeline::TimelineCue, look: Settings| match &cue.source {
        timeline::EventSource::Cue { id } => s.deck.with_program(id, look),
        timeline::EventSource::Look { .. } => look,
    };
    s.timeline
        .frame(&c)
        .into_iter()
        .filter_map(|mut cue| {
            let mut look = with_program(&cue, timeline::look_of(&cue, &s.presets)?);
            // Envelopes (T-163): on these per-frame copies only, before
            // the master live stage and the safety chain.
            timeline::apply_envelopes(&cue.envelopes, &s.controls, &mut look, &mut cue.modifiers);
            Some((cue, look))
        })
        .collect()
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

    /// T-413: the application keeps its data where macOS apps do.
    #[test]
    fn the_app_data_dir_is_in_application_support() {
        let dir = app_data_dir(Some("/Users/someone".as_ref())).unwrap();
        assert_eq!(dir, PathBuf::from("/Users/someone/Library/Application Support/Laser Studio"));
        assert!(app_data_dir(None).is_err());
        assert!(app_data_dir(Some("".as_ref())).is_err());
    }

    /// T-413: a busy preferred port is skipped; a free one is kept.
    #[test]
    fn the_app_picks_a_free_port() {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = busy.local_addr().unwrap().port();
        let got = pick_port(taken, 1).unwrap();
        assert_ne!(got, taken);
        assert_ne!(got, 0);
        std::net::TcpListener::bind(("127.0.0.1", got)).expect("the port given is free");
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(pick_port(free, 1).unwrap(), free);
        drop(busy);
    }

    /// T-413: `--app` fills in the data directory and the port unless the
    /// command line names them; without `--app` nothing changes.
    #[test]
    fn app_mode_resolves_only_what_the_command_line_leaves_out() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["laser-studio"][..], args].concat()).unwrap();
        let home = Some("/Users/x".as_ref());

        let mut cli = parse(&[]);
        resolve_app_mode(&mut cli, false, false, home).unwrap();
        assert_eq!((cli.data_dir.as_path(), cli.port, cli.app_default_data_dir), (std::path::Path::new("studio-data"), 8080, false), "CLI unchanged");

        let mut cli = parse(&["--app"]);
        resolve_app_mode(&mut cli, false, true, home).unwrap();
        assert_eq!(cli.data_dir, PathBuf::from("/Users/x/Library/Application Support/Laser Studio"));
        assert!(cli.app_default_data_dir);
        // (The free port itself: `the_app_picks_a_free_port`, never on 8080.)

        let mut cli = parse(&["--app", "--data-dir", "/tmp/d", "--port", "9123"]);
        resolve_app_mode(&mut cli, true, true, home).unwrap();
        assert_eq!((cli.data_dir.as_path(), cli.port, cli.app_default_data_dir), (std::path::Path::new("/tmp/d"), 9123, false));

        // The test options never go with a real laser, nor without --app.
        assert!(Cli::try_parse_from(["laser-studio", "--app", "--no-open", "--device", "auto"]).is_err());
        assert!(Cli::try_parse_from(["laser-studio", "--no-open"]).is_err());
        assert!(Cli::try_parse_from(["laser-studio", "--app", "--no-open"]).is_ok());
    }

    /// T-413: started from a bundle with no argument, the binary is the
    /// application; any other start keeps its arguments.
    #[test]
    fn a_bundle_launch_adds_app_and_nothing_else_does() {
        let args = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let bundle = std::path::Path::new("/Applications/Laser Studio.app/Contents/MacOS/laser-studio");
        let plain = std::path::Path::new("/Users/x/ilda-laser/target/release/laser-studio");
        assert_eq!(launch_args(args(&["ls"]), Some(bundle)), args(&["ls", "--app"]));
        assert_eq!(launch_args(args(&["ls", "-psn_0_12345"]), Some(bundle)), args(&["ls", "--app"]));
        assert_eq!(launch_args(args(&["ls"]), Some(plain)), args(&["ls"]));
        assert_eq!(launch_args(args(&["ls"]), None), args(&["ls"]));
        // The decoder child, run by the app on itself: untouched.
        let child = args(&["ls", "--decode", "a.mp3", "--data-dir", "/d"]);
        assert_eq!(launch_args(child.clone(), Some(bundle)), child);
        assert_eq!(launch_args(args(&["ls"]), Some(std::path::Path::new("/x/Contents/MacOS/ls"))), args(&["ls"]), "no .app");
    }

    /// T-413: the application starts disarmed, like the CLI, whatever the
    /// data directory holds.
    #[test]
    fn app_mode_starts_disarmed() {
        let dir = std::env::temp_dir().join(format!("laser-studio-appmode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cli = Cli::try_parse_from(["laser-studio", "--app", "--no-open", "--no-midi", "--no-audio", "--data-dir", dir.to_str().unwrap()]).unwrap();
        resolve_app_mode(&mut cli, true, false, None).unwrap();
        let s = startup_state(&cli);
        assert!(!s.gate.is_armed());
        assert!(!s.estop.is_latched());
        assert_eq!(s.gate.status(&s.estop).last_disarm.unwrap().reason_fr, "Démarrage");
        let model = menubar::current_model(&Mutex::new(s), &EStop::default());
        assert_eq!(model.status_line(), "Désarmé — Aperçu seulement");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// T-358: a test studio stops once its harness is gone or its data
    /// directory was deleted; the option is hidden and refuses --device.
    #[test]
    fn a_test_studio_stops_when_orphaned() {
        assert_eq!(orphan_reason(42, 42, true), None);
        assert!(orphan_reason(42, 1, true).is_some(), "re-parented to launchd");
        assert!(orphan_reason(42, 42, false).is_some(), "data directory deleted");
        let cli = Cli::try_parse_from(["laser-studio", "--exit-with-parent", "42"]).unwrap();
        assert_eq!(cli.exit_with_parent, Some(42));
        assert_eq!(Cli::try_parse_from(["laser-studio"]).unwrap().exit_with_parent, None, "a normal launch never watches");
        assert!(Cli::try_parse_from(["laser-studio", "--device", "x", "--exit-with-parent", "42"]).is_err());
        use clap::CommandFactory;
        assert!(Cli::command().get_arguments().any(|a| a.get_id() == "exit_with_parent" && a.is_hide_set()));
    }

    /// T-258: the checklist gates arming with a real output (DAC, PONK) or
    /// its simulation, never in pure preview nor with the fake output; the
    /// edited list is kept across restarts, the ticks are not.
    /// T-472: a ShowNET output refuses arming until the hardware stop is
    /// ticked, logs the refusal, and never arms by itself.
    #[test]
    fn a_shownet_output_waits_for_the_hardware_stop_item() {
        let mut s = test_support::shared();
        s.gate.set_log(safety_log::SafetyLog::default());
        s.output_kind = Some("test");
        s.gate.set_output_ready(true);
        s.sync_checklist();
        assert!(!s.gate.has_interlock(interlock::HW_STOP), "not a ShowNET: no hw_stop interlock");

        // The fake output declared ShowNET on the projector sheet.
        let sheet = s.outputs.active().projector.with_model(power::DacModel::ShowNet);
        s.outputs.set(power::MAIN_OUTPUT, None, Some(sheet), false, false).unwrap();
        s.sync_checklist();
        assert!(!s.gate.interlock_ok(interlock::HW_STOP));
        assert!(s.gate.blocking().contains(&interlock::HW_STOP_LABEL.to_string()));
        assert!(s.gate.request_arm(interlock::ArmSource::Ui).unwrap_err().contains(&interlock::HW_STOP_LABEL.to_string()));
        assert!(!s.gate.is_armed());

        s.checklist.check(checklist::HW_STOP_ITEM, true, None).unwrap();
        s.sync_checklist();
        assert!(s.gate.interlock_ok(interlock::HW_STOP));
        assert!(!s.gate.is_armed(), "ticking never arms");

        // Unticking while armed disarms on the spot.
        s.gate.register(interlock::UI_ALIVE, "ui", true);
        assert_eq!(s.gate.blocking(), Vec::<String>::new());
        s.gate.request_arm(interlock::ArmSource::Ui).unwrap();
        s.checklist.check(checklist::HW_STOP_ITEM, false, None).unwrap();
        s.sync_checklist();
        assert!(!s.gate.is_armed());
        assert_eq!(s.gate.status(&s.estop).last_disarm.unwrap().reason_fr, format!("Verrou : {}", interlock::HW_STOP_LABEL));

        // An Ether Dream / Helios / IDN ("dac") never gets the ShowNET items.
        s.output_kind = Some("dac");
        s.sync_checklist();
        assert!(s.gate.interlock_ok(interlock::HW_STOP), "held open only for a ShowNET");
        assert!(!s.checklist.visible_items().any(|i| i.only_for.is_some()));
        // PONK (MadMapper drives the ShowNET) always does.
        s.output_kind = Some("ponk");
        s.sync_checklist();
        assert!(!s.gate.interlock_ok(interlock::HW_STOP));
        assert_eq!(s.checklist.visible_items().filter(|i| i.only_for.is_some()).count(), 3);
    }

    /// A studio where every condition of audience scanning is met (T-255).
    fn ready_for_audience_scanning(log: safety_log::SafetyLog) -> Shared {
        let mut s = test_support::shared();
        s.gate.set_log(log);
        let zone = zones::Zone { id: 1, name: "Public".into(), kind: zones::ZoneKind::Dim, level: 0.1, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 0.0], [-1.0, 0.0]], audience: true };
        let mut cfg = safety::SafetySettings { zones: vec![zone], ..Default::default() };
        cfg.dwell.profile = dwell::DwellProfile::Strict;
        s.safety.set(cfg, true).unwrap();
        let sheet = power::ProjectorInfo { name: "P".into(), power_mw: [100.0; 3], divergence_mrad: 1.0, hw_scan_fail: Some(true), ..Default::default() };
        s.outputs.set(power::MAIN_OUTPUT, None, Some(sheet), false, false).unwrap();
        let mut list = s.checklist.config().clone();
        list.audience_scan = true;
        s.checklist.set_config(list, false).unwrap();
        s.checklist.set_operator("Op").unwrap();
        let ids: Vec<String> = s.checklist.visible_items().map(|i| i.id.clone()).collect();
        for id in ids {
            s.checklist.check(&id, true, Some("0.8")).unwrap();
        }
        s.checklist.complete().unwrap();
        s.presence.settings.hold_to_run = true;
        s
    }

    /// T-255: the mode ends on an e-stop (and stays ended after the reset),
    /// on a disarm the operator did not ask for, and when a condition stops
    /// being met; a disarm by the operator keeps it. Never arms.
    #[test]
    fn audience_scanning_ends_on_estop_unasked_disarm_or_a_lost_condition() {
        use crate::audience::{UnlockError, PHRASE};
        let (log, sink) = safety_log::testing::memory_log();
        let mut s = ready_for_audience_scanning(log.clone());
        assert!(!s.audience.is_scanning(), "every start is above the audience");
        s.request_arm(interlock::ArmSource::Ui).unwrap();
        assert_eq!(s.unlock_audience_scanning(PHRASE, interlock::ArmSource::Ui), Err(UnlockError::Armed));
        s.gate.disarm(interlock::DisarmReason::User, interlock::ArmSource::Ui);
        s.unlock_audience_scanning(PHRASE, interlock::ArmSource::Ui).unwrap();
        assert!(s.audience.is_scanning() && !s.gate.is_armed(), "unlocking never arms");
        let cfg = s.effective_safety();
        assert_eq!(cfg.zones[0].kind, zones::ZoneKind::Dim, "the Public zone keeps its attenuation");

        // The operator arms and disarms: still scanning.
        s.request_arm(interlock::ArmSource::Ui).unwrap();
        s.gate.disarm(interlock::DisarmReason::User, interlock::ArmSource::Ui);
        s.sync_audience();
        assert!(s.audience.is_scanning());

        // E-stop: back above, and the reset does not bring it back.
        s.emergency_stop(interlock::ArmSource::Keyboard);
        assert!(!s.audience.is_scanning());
        assert_eq!(s.effective_safety().zones[0].kind, zones::ZoneKind::Blank);
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        s.sync_audience();
        assert!(!s.audience.is_scanning());

        // A disarm nobody asked for (the watchdog).
        s.unlock_audience_scanning(PHRASE, interlock::ArmSource::Ui).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        s.gate.watchdog_trip();
        s.sync_audience();
        assert!(!s.audience.is_scanning());

        // A condition lost: the guard loosened, or the checklist restarted.
        s.unlock_audience_scanning(PHRASE, interlock::ArmSource::Ui).unwrap();
        let mut cfg = s.safety.get();
        cfg.dwell.profile = dwell::DwellProfile::Beams;
        s.safety.set(cfg, true).unwrap();
        s.sync_audience();
        assert!(!s.audience.is_scanning());
        let mut cfg = s.safety.get();
        cfg.dwell.profile = dwell::DwellProfile::Strict;
        s.safety.set(cfg, true).unwrap();
        s.unlock_audience_scanning(PHRASE, interlock::ArmSource::Ui).unwrap();
        s.checklist.restart();
        s.sync_audience();
        assert!(!s.audience.is_scanning());
        assert_eq!(s.unlock_audience_scanning(PHRASE, interlock::ArmSource::Ui).unwrap_err(), UnlockError::Missing(vec![s.audience_conditions()[2].label_fr.to_string()]));

        // Every change is in the safety log, the unlock with its conditions.
        log.flush(Duration::from_secs(2));
        let events = sink.events();
        let modes: Vec<&serde_json::Value> = events.iter().filter(|e| e.kind == "audience_mode").map(|e| &e.detail).collect();
        assert_eq!(modes.iter().filter(|d| d["mode"] == "scanning").count(), 4);
        assert!(modes.iter().any(|d| d["reason"] == "estop"));
        assert!(modes.iter().any(|d| d["reason"] == "disarm:engine_stall"));
        assert!(modes.iter().any(|d| d["reason"] == "condition"));
    }

    /// T-255: with a real output, arming waits until a Public zone or a
    /// full horizon says where the audience is. Pure preview: no interlock.
    #[test]
    fn a_real_output_refuses_arming_until_the_audience_is_located() {
        let mut s = test_support::shared();
        s.sync_checklist();
        assert!(!s.gate.has_interlock(interlock::AUDIENCE_ZONE), "preview: no audience interlock");
        s.checklist.enforce();
        s.sync_checklist();
        assert!(!s.gate.interlock_ok(interlock::AUDIENCE_ZONE));
        assert!(s.gate.blocking().contains(&audience::INTERLOCK_LABEL.to_string()));
        let mut cfg = s.safety.get();
        cfg.horizon = zones::Horizon { lines: true, level: 0.0, ..cfg.horizon };
        s.safety.set(cfg, true).unwrap();
        s.sync_audience();
        assert!(s.gate.interlock_ok(interlock::AUDIENCE_ZONE), "a full horizon at 0 % is enough");
        let mut cfg = s.safety.get();
        cfg.horizon.lines = false;
        cfg.zones.push(zones::Zone { name: "Public".into(), points: vec![[-1.0, -1.0], [1.0, -1.0], [0.0, -0.5]], audience: true, ..Default::default() });
        s.safety.set(cfg, true).unwrap();
        s.sync_audience();
        assert!(s.gate.interlock_ok(interlock::AUDIENCE_ZONE), "a Public zone too");
    }

    #[test]
    fn the_checklist_is_enforced_only_with_a_real_output() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["laser-studio"][..], args].concat()).unwrap();
        assert!(!checklist_enforced(&parse(&[])));
        assert!(!checklist_enforced(&parse(&["--test-output", "/tmp/x"])));
        assert!(checklist_enforced(&parse(&["--device", "auto"])));
        assert!(checklist_enforced(&parse(&["--ponk", "127.0.0.1:9"])));
        assert!(checklist_enforced(&parse(&["--test-checklist"])));
        assert!(checklist_enforced(&parse(&["--test-checklist", "--test-output", "/tmp/x"])));

        let dir = std::env::temp_dir().join(format!("laser-studio-checklist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let data_dir = dir.to_str().unwrap();
        let preview = startup_state(&parse(&["--no-midi", "--data-dir", data_dir]));
        assert!(preview.gate.interlock_ok(interlock::CHECKLIST));
        drop(preview);
        let mut s = startup_state(&parse(&["--no-midi", "--test-checklist", "--data-dir", data_dir]));
        assert!(!s.gate.interlock_ok(interlock::CHECKLIST));
        let mut cfg = s.checklist.config().clone();
        cfg.items.truncate(1);
        s.checklist.set_config(cfg, false).unwrap();
        s.checklist.set_operator("Op").unwrap();
        let id = s.checklist.config().items[0].id.clone();
        s.checklist.check(&id, true, None).unwrap();
        s.checklist.complete().unwrap();
        s.sync_checklist();
        assert!(s.gate.interlock_ok(interlock::CHECKLIST));
        assert!(!s.gate.is_armed(), "completing never arms");
        drop(s);
        let again = startup_state(&parse(&["--no-midi", "--test-checklist", "--data-dir", data_dir]));
        // The ShowNET and audience items are put back (T-472, T-255).
        assert_eq!(again.checklist.config().items.iter().filter(|i| i.only_for.is_none() && i.when != checklist::ItemWhen::AudienceScan).count(), 1, "the edited list is kept");
        assert!(!again.gate.interlock_ok(interlock::CHECKLIST), "the ticks are not");
        drop(again);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn midi_test_needs_no_midi_and_refuses_a_device() {
        let parse = |args: &[&str]| Cli::try_parse_from(std::iter::once("laser-studio").chain(args.iter().copied()));
        assert!(parse(&["--midi-test"]).is_err(), "never next to the real CoreMIDI");
        assert!(parse(&["--no-midi", "--midi-test", "--device", "auto"]).is_err(), "preview only");
        let cli = parse(&["--no-midi", "--midi-test"]).unwrap();
        assert!(cli.midi_test && cli.no_midi);
        assert!(!parse(&[]).unwrap().midi_test, "off by default");
    }

    /// T-013: OSC and Art-Net only on request, on localhost unless told.
    #[test]
    fn network_inputs_are_off_unless_asked_and_default_to_localhost() {
        let parse = |args: &[&str]| Cli::try_parse_from(std::iter::once("laser-studio").chain(args.iter().copied()));
        let none = parse(&[]).unwrap();
        assert!(none.osc.is_none() && none.artnet.is_none());
        let both = parse(&["--osc", "--artnet"]).unwrap();
        assert_eq!(both.osc, Some("127.0.0.1:8000".parse().unwrap()));
        assert_eq!(both.artnet, Some("127.0.0.1:6454".parse().unwrap()));
        assert_eq!(parse(&["--osc", "9000"]).unwrap().osc, Some("127.0.0.1:9000".parse().unwrap()));
        assert_eq!(parse(&["--artnet=10.0.0.5"]).unwrap().artnet, Some("10.0.0.5:6454".parse().unwrap()));
        assert!(parse(&["--osc", "224.0.0.1:9000"]).is_err(), "never a multicast group");
        assert!(parse(&["--artnet", "255.255.255.255"]).is_err(), "never broadcast");

        let dir = std::env::temp_dir().join(format!("laser-studio-netin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = startup_state(&parse(&["--no-midi", "--osc", "--data-dir", dir.to_str().unwrap()]).unwrap());
        assert!(s.netin.hub.effective().osc.enabled && !s.netin.hub.effective().artnet.enabled);
        assert!(!s.netin.hub.file().osc.enabled, "not saved");
        assert!(!s.gate.is_armed());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-300: PONK only on request, never next to a DAC or the fake output.
    #[test]
    fn ponk_is_opt_in_and_exclusive() {
        let parse = |args: &[&str]| Cli::try_parse_from(std::iter::once("laser-studio").chain(args.iter().copied()));
        assert!(parse(&[]).unwrap().ponk.is_none(), "off by default");
        assert_eq!(parse(&["--ponk"]).unwrap().ponk, Some("127.0.0.1:5583".parse().unwrap()), "alone: MadMapper on this Mac");
        assert_eq!(parse(&["--ponk", "127.0.0.1:6001", "--port", "0"]).unwrap().ponk, Some("127.0.0.1:6001".parse().unwrap()));
        assert_eq!(parse(&["--ponk", "--port", "0"]).unwrap().port, 0, "the next flag isn't taken as the address");
        assert!(parse(&["--ponk", "not an address"]).is_err());
        assert!(parse(&["--ponk", "--device", "auto"]).is_err(), "one output at a time");
        assert!(parse(&["--ponk", "127.0.0.1:6001", "--test-output", "/tmp/x"]).is_err());
        assert!(parse(&["--no-midi", "--midi-test", "--ponk"]).is_err(), "MIDI tests stay preview only");
    }

    /// T-301: the ShowNET SD card over Art-Net only on request, to one
    /// typed box, never next to another output; a real output, so the
    /// pre-show checklist gates arming.
    #[test]
    fn shownet_artnet_is_opt_in_and_exclusive() {
        let parse = |args: &[&str]| Cli::try_parse_from(std::iter::once("laser-studio").chain(args.iter().copied()));
        assert!(parse(&[]).unwrap().shownet_artnet.is_none(), "off by default");
        let cli = parse(&["--shownet-artnet", "127.0.0.1"]).unwrap();
        assert_eq!(cli.shownet_artnet, Some("127.0.0.1:6454".parse().unwrap()));
        assert_eq!(startup_output_id(&cli).as_deref(), Some("artnet:127.0.0.1:6454"));
        assert!(checklist_enforced(&cli));
        assert_eq!(parse(&["--shownet-artnet", "127.0.0.1:7000"]).unwrap().shownet_artnet, Some("127.0.0.1:7000".parse().unwrap()));
        for bad in ["255.255.255.255", "shownet.local", "192.168.1.255"] {
            assert!(parse(&["--shownet-artnet", bad]).is_err(), "{bad}");
        }
        for other in [&["--device", "auto"][..], &["--ponk"], &["--test-output", "/tmp/x"], &["--list-devices"]] {
            let args = [&["--shownet-artnet", "127.0.0.1"][..], other].concat();
            assert!(parse(&args).is_err(), "one output at a time: {other:?}");
        }
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

    /// T-163: an event's envelopes move the frame's copies of its look and
    /// modifiers (never the stored values), an envelope on a forbidden
    /// control does nothing, and the e-stop cuts them with the timeline.
    #[test]
    fn envelopes_move_copies_only_and_end_with_the_estop() {
        let mut s = test_support::shared();
        let cue = s.presets[0].id.clone();
        let mut show = one_event_show(&cue);
        let key = |value| timeline::Key { at: 0.0, value, ease: Default::default() };
        let env = |target: &str, value| timeline::Envelope { target: target.into(), mode: timeline::EnvMode::Absolute, keys: vec![key(value)] };
        show.tracks[0].events[0].envelopes = vec![env("look.size", 0.3), env("master.brightness", 0.4), env("transport.arm", 1.0), env("layer.2.dimmer", 1.0)];
        s.timeline.load(show);
        controls::timeline_play(&mut s).unwrap();
        let (live_before, settings_before) = (s.live.clone(), s.settings.clone());
        let clock = |t: f64| engine::BeatClock { beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 };
        let t = s.now_s();
        let cues = timeline_cues(&mut s, t, &clock(t));
        assert!((cues[0].1.scale - 0.3).abs() < 1e-6);
        assert!(cues[0].0.modifiers.brightness <= 0.4 + 1e-6);
        assert_eq!((s.live.clone(), s.settings.clone()), (live_before, settings_before), "the stored values stay the operator's");
        assert!(!s.gate.is_armed(), "an envelope on transport.arm arms nothing");
        s.emergency_stop(interlock::ArmSource::Keyboard);
        assert!(timeline_cues(&mut s, t + 0.5, &clock(t + 0.5)).is_empty(), "Échap: no envelope any more");
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

    /// T-259 acceptance: arm, launch a cue, Escape → three lines in that
    /// order, with their sources; a cue while disarmed is not logged.
    #[test]
    fn arm_cue_and_escape_are_logged_in_order() {
        let (log, sink) = safety_log::testing::memory_log();
        let mut s = test_support::shared();
        s.gate.set_log(log.clone());
        assert!(controls::press_cue_from(&mut s, "tunnels-001", None, true, Some("ui")));
        s.request_arm(ArmSource::Keyboard).unwrap();
        assert!(controls::press_cue_from(&mut s, "tunnels-002", Some(ClickMode::Toggle), true, Some("midi")));
        s.emergency_stop(ArmSource::Keyboard);
        log.flush(Duration::from_secs(2));
        let events = sink.events();
        let lines: Vec<(&str, Option<&str>)> = events.iter().map(|e| (e.kind.as_str(), e.source.as_deref())).collect();
        assert_eq!(lines, [("arm", Some("keyboard")), ("cue", Some("midi")), ("estop", Some("keyboard"))]);
        assert_eq!(events[1].detail["id"], "tunnels-002");
        assert!(!events[1].detail["name"].as_str().unwrap().is_empty());
    }

    #[test]
    fn a_watchdog_trip_and_a_lost_page_are_logged() {
        let (log, sink) = safety_log::testing::memory_log();
        let t0 = Instant::now();
        let mut s = with_presence(PresenceSettings::default());
        s.gate.set_log(log.clone());
        s.presence.beat("page", t0, true, false);
        s.request_arm(ArmSource::Ui).unwrap();
        s.health.trip();
        s.sync_safety(t0 + ms(100));
        s.request_arm(ArmSource::Ui).unwrap();
        s.sync_safety(t0 + ms(2_500));
        log.flush(Duration::from_secs(2));
        assert_eq!(sink.kinds(), ["presence", "arm", "watchdog", "arm", "disarm", "presence"]);
        assert_eq!(sink.events()[4].detail["reason"], "ui_lost");
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

    /// T-404: a pattern pauses the timeline and leaves the deck alone;
    /// leaving it gives back the same cues and the same timeline position.
    #[test]
    fn a_test_pattern_pauses_the_timeline_and_keeps_the_cues() {
        use testpattern::TestPattern;
        let mut s = test_support::shared();
        assert!(controls::press_cue(&mut s, "tunnels-001", Some(ClickMode::Toggle), true));
        let cues_before: Vec<String> = s.deck.active.iter().map(|a| a.cue.clone()).collect();
        let cue = s.presets[0].id.clone();
        s.timeline.load(one_event_show(&cue));
        controls::timeline_play(&mut s).unwrap();
        testpattern::select(&mut s, Some(TestPattern::Grid));
        assert!(!s.timeline.is_playing(), "paused under the pattern");
        let pos = s.timeline.position_at(&s.timeline_clock());
        std::thread::sleep(Duration::from_millis(30));
        // Played again from elsewhere: the engine holds it paused.
        controls::timeline_play(&mut s).unwrap();
        assert_eq!(testpattern::hold_timeline(&mut s), Some((TestPattern::Grid, 0.2)));
        assert!(!s.timeline.is_playing());
        assert!((s.timeline.position_at(&s.timeline_clock()) - pos).abs() < 0.02, "the position doesn't run");
        testpattern::select(&mut s, Some(TestPattern::ColorBars));
        testpattern::select(&mut s, None);
        assert!(s.timeline.is_playing(), "resumes with the look");
        assert!((s.timeline.position_at(&s.timeline_clock()) - pos).abs() < 0.02, "from where it was");
        assert_eq!(s.deck.active.iter().map(|a| a.cue.clone()).collect::<Vec<_>>(), cues_before);
        assert!(!s.gate.is_armed());
        assert_eq!(testpattern::hold_timeline(&mut s), None);
    }

    /// An e-stop under a pattern halts the timeline; leaving the pattern
    /// doesn't restart it, and nothing arms.
    #[test]
    fn leaving_a_pattern_after_an_estop_does_not_resume() {
        let mut s = test_support::shared();
        let cue = s.presets[0].id.clone();
        s.timeline.load(one_event_show(&cue));
        controls::timeline_play(&mut s).unwrap();
        testpattern::select(&mut s, Some(testpattern::TestPattern::CrossFrame));
        s.emergency_stop(ArmSource::Keyboard);
        let t = s.now_s();
        timeline_cues(&mut s, t, &engine::BeatClock { beat: 0.0, bpm: 120.0, beats_per_bar: 4 });
        testpattern::select(&mut s, None);
        assert!(!s.timeline.is_playing());
        assert!(!s.gate.is_armed());
    }

    /// The pattern mode is never part of the start-up state.
    #[test]
    fn no_pattern_at_startup() {
        let dir = std::env::temp_dir().join(format!("laser-studio-tp-startup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prefs.json"), r#"{"test_pattern":{"active":"grid"}}"#).unwrap();
        let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--data-dir", dir.to_str().unwrap()]).unwrap();
        let s = startup_state(&cli);
        assert_eq!(s.test_pattern.active, None);
        assert_eq!(s.test_pattern.brightness, testpattern::DEFAULT_BRIGHTNESS);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An output that notes when each frame left.
    struct TimedOutput(Arc<Mutex<Vec<Instant>>>);
    impl Output for TimedOutput {
        fn name(&self) -> &str {
            "chronométrée"
        }
        fn kind(&self) -> &'static str {
            "test"
        }
        fn set_armed(&mut self, _: bool) -> Result<()> {
            Ok(())
        }
        fn send(&mut self, _: &[Point]) {
            self.0.lock().unwrap().push(Instant::now());
        }
    }

    /// T-403: with the application log's writer stuck for good (a dead
    /// disk), the engine keeps its cadence over 300 frames while another
    /// thread floods the log and arms/disarms (both log under the lock):
    /// the mean frame interval stays that of the same engine with no
    /// logging at all (a debug build on a busy machine may not make
    /// 60 fps on its own, so the reference is measured, not assumed).
    #[test]
    fn the_engine_keeps_its_cadence_when_the_app_log_is_stuck() {
        // T-358: it joins the engine and flood threads; a hang fails fast.
        let _guard = test_support::time_guard("the_engine_keeps_its_cadence_when_the_app_log_is_stuck", Duration::from_secs(180));
        let app = applog::testing::install_stuck_global();
        for i in 0..2_000 {
            log::warn!("remplissage {i}");
        }
        assert!(app.status()["dropped"].as_u64().unwrap() > 0, "the queue is full");
        let (quiet, _) = engine_cadence(false);
        let (mean, worst) = engine_cadence(true);
        assert!(mean <= quiet + quiet / 4 + Duration::from_millis(1), "mean frame interval {mean:?}, {quiet:?} without logging");
        assert!(worst < Duration::from_millis(watchdog::STALL_MS as u64), "a frame took {worst:?}");
    }

    /// T-411: a page that opens the state stream and asks for binary frames
    /// but never reads them does not slow the engine: over 300 frames,
    /// with big states published all along, the cadence is that of the
    /// same engine without any page.
    #[test]
    fn a_push_reader_that_never_reads_does_not_slow_the_engine() {
        let _guard = test_support::time_guard("a_push_reader_that_never_reads_does_not_slow_the_engine", Duration::from_secs(180));
        let (quiet, _) = engine_cadence(false);
        let (mean, worst) = engine_cadence_with(|shared, running| {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();
            let estop = Arc::clone(&shared.lock().unwrap().estop);
            let hub = Arc::clone(&shared.lock().unwrap().push);
            let web = std::thread::spawn({
                let (shared, running) = (Arc::clone(shared), Arc::clone(running));
                move || web::serve(server, shared, estop, std::env::temp_dir().join("laser-studio-test-unused/calibration.json"), running)
            });
            // A reader with a tiny buffer that never reads: the stream and
            // twenty frame requests, all left unread.
            let stuck = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
            stuck.set_recv_buffer_size(4096).unwrap();
            stuck.connect(&addr.into()).unwrap();
            let mut stuck: std::net::TcpStream = stuck.into();
            std::io::Write::write_all(&mut stuck, b"GET /api/events HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let frames = std::net::TcpStream::connect(addr).unwrap();
            for _ in 0..20 {
                std::io::Write::write_all(&mut &frames, b"GET /api/frame.bin HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            }
            let flood = std::thread::spawn({
                let running = Arc::clone(running);
                move || {
                    let filler = "x".repeat(64 * 1024);
                    let mut n = 0u64;
                    while running.load(Ordering::SeqCst) {
                        n += 1;
                        hub.publish_state(serde_json::json!(n), |_| format!("{{\"n\":{n},\"f\":\"{filler}\"}}"));
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            });
            Box::new(move || {
                flood.join().unwrap();
                web.join().unwrap();
                drop((stuck, frames));
            })
        });
        assert!(mean <= quiet + quiet / 4 + Duration::from_millis(1), "mean frame interval {mean:?}, {quiet:?} without a page");
        assert!(worst < Duration::from_millis(watchdog::STALL_MS as u64), "a frame took {worst:?}");
    }

    type Teardown = Box<dyn FnOnce()>;

    /// Mean and worst interval of 300 engine frames, with or without a
    /// thread flooding the log.
    fn engine_cadence(flooded: bool) -> (Duration, Duration) {
        engine_cadence_with(|shared, running| {
            let flood = std::thread::spawn({
                let (shared, running) = (Arc::clone(shared), Arc::clone(running));
                move || {
                    while flooded && running.load(Ordering::SeqCst) {
                        log::info!("bruit");
                        {
                            let mut s = shared.lock().unwrap();
                            let _ = s.request_arm(ArmSource::Ui);
                            s.gate.disarm(interlock::DisarmReason::User, ArmSource::Ui);
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            });
            Box::new(move || flood.join().unwrap())
        })
    }

    /// Mean and worst interval of 300 engine frames, with `load` running
    /// beside the engine (its teardown runs once `running` is false).
    fn engine_cadence_with(load: impl FnOnce(&Arc<Mutex<Shared>>, &Arc<AtomicBool>) -> Teardown) -> (Duration, Duration) {
        static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let run = RUN.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("laser-studio-engine-cadence-{run}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shared = Arc::new(Mutex::new(test_support::shared()));
        let running = Arc::new(AtomicBool::new(true));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let teardown = load(&shared, &running);
        let engine = std::thread::spawn({
            let (shared, running, out) = (Arc::clone(&shared), Arc::clone(&running), TimedOutput(Arc::clone(&sent)));
            let (live, layers) = (dir.join("live.json"), dir.join("layers.json"));
            move || run_engine(shared, Some(Box::new(out)), running, live, layers)
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while sent.lock().unwrap().len() < 301 {
            assert!(Instant::now() < deadline, "the engine stopped: {} frames", sent.lock().unwrap().len());
            std::thread::sleep(Duration::from_millis(20));
        }
        running.store(false, Ordering::SeqCst);
        engine.join().unwrap();
        teardown();
        let times: Vec<Instant> = sent.lock().unwrap()[..301].to_vec();
        let mean = (times[300] - times[0]) / 300;
        let worst = times.windows(2).map(|w| w[1] - w[0]).max().unwrap();
        let _ = std::fs::remove_dir_all(dir);
        (mean, worst)
    }

    /// An output that records what it is asked, and its kill switch.
    #[derive(Clone, Default)]
    struct Recorder {
        calls: Arc<Mutex<Vec<&'static str>>>,
        kills: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Output for Recorder {
        fn name(&self) -> &str {
            "enregistreuse"
        }
        fn set_armed(&mut self, armed: bool) -> Result<()> {
            self.calls.lock().unwrap().push(if armed { "arm" } else { "disarm" });
            Ok(())
        }
        fn send(&mut self, points: &[Point]) {
            self.calls.lock().unwrap().push(if points.iter().any(|p| p.is_lit()) { "lit" } else { "dark" });
        }
        fn blank_now(&mut self) -> Result<()> {
            self.calls.lock().unwrap().push("blank");
            Ok(())
        }
        fn kill_switch(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
            let kills = Arc::clone(&self.kills);
            Some(Box::new(move || {
                kills.fetch_add(1, Ordering::SeqCst);
            }))
        }
    }
    impl Drop for Recorder {
        fn drop(&mut self) {
            self.calls.lock().unwrap().push("close");
        }
    }

    /// T-401: a switch while armed disarms the gate, gives the old output
    /// its dark frames and closes it before the new one is installed
    /// (disarmed), and the e-stop now cuts the new output.
    #[test]
    fn replacing_an_armed_output_blanks_and_closes_the_old_one_first() {
        let (old, new) = (Recorder::default(), Recorder::default());
        let shared = Mutex::new(test_support::shared());
        let (estop, mgr) = {
            let s = shared.lock().unwrap();
            (Arc::clone(&s.estop), Arc::clone(&s.output_mgr))
        };
        let mut stage = OutputStage::new(Some(Box::new(old.clone())));
        estop.set_kill_switch(old.kill_switch().unwrap());
        shared.lock().unwrap().request_arm(ArmSource::Ui).unwrap();
        stage.emit(&[Point::lit(0.0, 0.0, 1.0, 1.0, 1.0)], true, true, &estop);
        let change = output_manager::StageChange {
            output: Some(Box::new(new.clone())),
            kind: output_manager::ChangeKind::Connected,
            source: ArmSource::Ui,
            gen: 1,
            id: Some("b".into()),
            projector: None,
        };
        apply_output_change(&shared, &mut stage, &estop, &mgr, change);
        // Closed on a thread of its own, after the dark frames.
        let deadline = Instant::now() + Duration::from_secs(5);
        while old.calls.lock().unwrap().last() != Some(&"close") {
            assert!(Instant::now() < deadline, "never closed: {:?}", old.calls.lock().unwrap());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*old.calls.lock().unwrap(), ["arm", "lit", "disarm", "blank", "blank", "blank", "close"]);
        let s = shared.lock().unwrap();
        assert!(!s.gate.is_armed());
        assert_eq!(reason(&s), "output_changed");
        assert_eq!(s.output_name.as_deref(), Some("enregistreuse"));
        // A real output (a DAC) connected from the UI: the pre-show
        // checklist now gates arming, as with --device (T-258).
        assert!(s.checklist.enforced());
        assert!(!s.gate.interlock_ok(interlock::CHECKLIST));
        drop(s);
        assert!(new.calls.lock().unwrap().is_empty(), "installed, not armed, nothing sent yet");
        assert!(estop.has_kill_switch());
        estop.trip(ArmSource::Keyboard);
        assert_eq!((old.kills.load(Ordering::SeqCst), new.kills.load(Ordering::SeqCst)), (0, 1), "the e-stop cuts the new output");
        // Back to preview: no kill switch left.
        let none = output_manager::StageChange { output: None, kind: output_manager::ChangeKind::Disconnected, source: ArmSource::Ui, gen: 2, id: None, projector: None };
        apply_output_change(&shared, &mut stage, &estop, &mgr, none);
        assert!(!estop.has_kill_switch());
        let calls = new.calls.lock().unwrap().clone();
        assert_eq!(calls[..4], ["disarm", "blank", "blank", "blank"], "dark before it closes: {calls:?}");
        let s = shared.lock().unwrap();
        assert_eq!(s.output_name, None);
        assert!(!s.gate.interlock_ok(interlock::OUTPUT));
    }

    /// T-401 end to end on the engine: connect a fake device, arm, lose it,
    /// get it back. The loss disarms (« Sortie perdue ») and blocks arming;
    /// the reconnection never re-arms.
    #[test]
    fn a_lost_output_disarms_and_comes_back_disarmed() {
        // T-358: it joins the engine thread; a hang fails fast.
        let _guard = test_support::time_guard("a_lost_output_disarms_and_comes_back_disarmed", Duration::from_secs(180));
        let dir = std::env::temp_dir().join(format!("laser-studio-output-loss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let devices = r#"[{"kind":"etherdream","id":"etherdream:test","name":"Ether Dream test"}]"#;
        let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--no-audio", "--test-devices", devices, "--data-dir", dir.to_str().unwrap()]).unwrap();
        let mut state = startup_state(&cli);
        // No page in a unit test: presence not enforced.
        state.presence = presence::Presence::default();
        state.gate.set_interlock(UI_ALIVE, true);
        let mgr = Arc::clone(&state.output_mgr);
        mgr.devices().refresh();
        let shared = Arc::new(Mutex::new(state));
        let running = Arc::new(AtomicBool::new(true));
        let manager = mgr.spawn(Arc::clone(&running)).unwrap();
        let engine = std::thread::spawn({
            let (shared, running) = (Arc::clone(&shared), Arc::clone(&running));
            let (live, layers) = (dir.join("live.json"), dir.join("layers.json"));
            move || run_engine(shared, None, running, live, layers)
        });
        let wait = |what: &str, ok: &dyn Fn(&Shared) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ok(&shared.lock().unwrap()) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        assert_eq!(shared.lock().unwrap().request_arm(ArmSource::Ui).unwrap_err(), [interlock::OUTPUT_LABEL]);
        wait("the scan", &|s| s.output_mgr.knows("etherdream:test"));
        mgr.select(Some("etherdream:test".into()), ArmSource::Ui);
        wait("the connection", &|s| s.gate.interlock_ok(interlock::OUTPUT));
        assert_eq!(shared.lock().unwrap().output_name.as_deref(), Some("Ether Dream test"));
        shared.lock().unwrap().request_arm(ArmSource::Ui).unwrap();

        assert!(mgr.simulate_drop(true));
        wait("the loss", &|s| !s.gate.is_armed());
        {
            let mut s = shared.lock().unwrap();
            assert_eq!(reason(&s), "output_lost");
            assert_eq!(s.gate.status(&s.estop).last_disarm.unwrap().reason_fr, "Sortie perdue");
            assert_eq!(s.request_arm(ArmSource::Ui).unwrap_err(), [interlock::OUTPUT_LABEL]);
        }
        wait("the manager", &|s| s.output_mgr.state() == output_manager::LinkState::Lost);

        assert!(mgr.simulate_drop(false));
        wait("the reconnection", &|s| s.gate.interlock_ok(interlock::OUTPUT));
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.output_mgr.state(), output_manager::LinkState::Connected);
            assert!(!s.gate.is_armed(), "never re-armed by itself");
            assert_eq!(reason(&s), "output_lost", "the reason stays the loss");
        }
        running.store(false, Ordering::SeqCst);
        engine.join().unwrap();
        manager.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-401: an install without `outputs.json` starts in preview only,
    /// and arming says why it is refused.
    #[test]
    fn no_output_no_arming() {
        let dir = std::env::temp_dir().join(format!("laser-studio-no-output-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--no-audio", "--test-devices", "[]", "--data-dir", dir.to_str().unwrap()]).unwrap();
        let mut s = startup_state(&cli);
        s.presence.touch("page", Instant::now());
        assert_eq!(s.output_mgr.prefs(), output_manager::OutputPrefs::default());
        assert_eq!(s.output_mgr.state(), output_manager::LinkState::Available);
        assert_eq!(s.request_arm(ArmSource::Ui).unwrap_err(), [interlock::OUTPUT_LABEL]);
        assert_eq!(s.pps, 30_000);
        assert!(!dir.join("outputs.json").exists(), "nothing written at start");
        let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--pps", "45000", "--data-dir", dir.to_str().unwrap()]).unwrap();
        assert_eq!(startup_state(&cli).pps, 45_000);
        assert_eq!(startup_output_id(&cli), None);
        let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--ponk", "127.0.0.1:9", "--data-dir", dir.to_str().unwrap()]).unwrap();
        assert_eq!(startup_output_id(&cli).as_deref(), Some("ponk:127.0.0.1:9"));
        let _ = std::fs::remove_dir_all(dir);
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
        for name in ["presence.json", "live.json", "layers.json", "grid.json", "calibration.json", "scenes.json", "lfos.json", "audio_routes.json", "audio_triggers.json", "palettes.json"] {
            std::fs::write(dir.join(name), r#"{"armed": true, "arm": true, "on": true, "hold_to_run": false}"#).unwrap();
        }
        let data_dir = dir.to_str().unwrap();
        let variants: [&[&str]; 6] = [
            &[],
            &["--test-interlock"],
            &["--test-hooks", "--pps", "12000"],
            &["--test-hooks", "--test-interlock", "--port", "0"],
            &["--ponk", "127.0.0.1:9"],
            &["--shownet-artnet", "127.0.0.1:9"],
        ];
        for extra in variants {
            let args = [&["laser-studio", "--no-midi", "--data-dir", data_dir][..], extra].concat();
            let cli = Cli::try_parse_from(&args).unwrap();
            let mut s = startup_state(&cli);
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

    /// T-402: every damaged safety file starts disarmed with the strictest
    /// settings, and arming stays refused until the operator acknowledged it.
    #[test]
    fn a_damaged_safety_file_blocks_arming_until_acknowledged() {
        for rel in ["safety.json", "calibration.json", "projection.json", "outputs.json", "presence.json", "audio.json", "midi/devices.json"] {
            let (dir, _) = datafile::tests::with_damaged(&format!("startup-{}", rel.replace('/', "-")), rel);
            let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--no-audio", "--data-dir", dir.to_str().unwrap()]).unwrap();
            let mut s = startup_state(&cli);
            assert!(!s.gate.is_armed(), "{rel}");
            assert_eq!(s.data_warnings.len(), 1, "{rel}: {:?}", s.data_warnings);
            assert!(s.data_warnings[0].safety && s.data_warnings[0].file == rel);
            assert!(!s.gate.interlock_ok(interlock::DATA_ACK), "{rel}");
            s.presence.touch("page", Instant::now());
            let refused = s.request_arm(ArmSource::Ui).unwrap_err();
            assert!(refused.contains(&"Fichier de sécurité abîmé : à valider".to_string()), "{rel}: {refused:?}");
            // Never looser than the defaults.
            assert_eq!(s.safety.get(), if rel == "safety.json" { safety::SafetySettings::strictest() } else { safety::SafetySettings::default() });
            assert_eq!(s.outputs.active_limits(), power::OutputLimits::default());
            assert_eq!(s.projection.get(), &projection::ProjectionConfig::default());
            if rel == "midi/devices.json" {
                assert_eq!(s.midi.store.devices.safety, midi::safety::MidiSafety::strictest());
            }
            assert_eq!(s.ack_data_warnings(Some("autre.json")), 0);
            assert!(s.request_arm(ArmSource::Ui).is_err());
            assert_eq!(s.ack_data_warnings(Some(rel)), 1);
            assert!(s.gate.interlock_ok(interlock::DATA_ACK));
            // Nothing connected in this test: arming waits for an output too.
            assert_eq!(s.request_arm(ArmSource::Ui).unwrap_err(), [interlock::OUTPUT_LABEL]);
            note_output(&mut s, Some(&output::FileLogOutput::append(&dir.join("out.log")).unwrap()));
            s.request_arm(ArmSource::Ui).unwrap();
            s.gate.disarm(interlock::DisarmReason::User, ArmSource::Ui);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// T-402: other damaged files warn but never block arming; live and
    /// layers (saved by the engine) keep their quarantined copy.
    #[test]
    fn a_damaged_working_file_warns_without_blocking() {
        let (dir, _) = datafile::tests::with_damaged("startup-working", "live.json");
        std::fs::write(dir.join("layers.json"), datafile::tests::DAMAGED).unwrap();
        std::fs::write(dir.join("grid.json"), datafile::tests::DAMAGED).unwrap();
        let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--no-audio", "--data-dir", dir.to_str().unwrap()]).unwrap();
        let mut s = startup_state(&cli);
        let mut files: Vec<&str> = s.data_warnings.iter().map(|w| w.file.as_str()).collect();
        files.sort();
        assert_eq!(files, ["grid.json", "layers.json", "live.json"]);
        assert!(s.data_warnings.iter().all(|w| !w.safety && w.moved_to.is_some()));
        assert!(s.gate.interlock_ok(interlock::DATA_ACK));
        save_json(&dir.join("live.json"), &s.live);
        save_json(&dir.join("layers.json"), &s.mixer);
        for f in std::fs::read_dir(dir.join(datafile::QUARANTINE)).unwrap().flatten() {
            assert_eq!(std::fs::read(f.path()).unwrap(), datafile::tests::DAMAGED);
        }
        assert_eq!(s.ack_data_warnings(None), 3);
        assert!(s.data_warnings.iter().all(|w| w.acknowledged));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-402: a read-only data directory still starts; nothing could be
    /// moved, the warning says so, and the store never writes the file.
    #[test]
    fn a_read_only_data_dir_starts_and_keeps_the_damaged_file() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, path) = datafile::tests::with_damaged("startup-ro", "scenes.json");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let cli = Cli::try_parse_from(["laser-studio", "--no-midi", "--no-audio", "--data-dir", dir.to_str().unwrap()]).unwrap();
        let mut s = startup_state(&cli);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!s.gate.is_armed());
        let w = s.data_warnings.iter().find(|w| w.file == "scenes.json").expect("warned");
        assert_eq!(w.moved_to, None);
        assert!(w.message_fr().contains("impossible de le mettre de côté"));
        // Writable again: still memory only for this run.
        s.scenes.upsert(Scene { name: "x".into(), settings: Settings::default(), duration_secs: 1.0 }).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), datafile::tests::DAMAGED);
        let _ = std::fs::remove_dir_all(dir);
    }
}
