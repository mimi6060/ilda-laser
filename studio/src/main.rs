//! Laser studio: a live laser show controller that runs on the Mac, with
//! its UI in the browser. Pick a shape or text, animate it, make it react
//! to music from the microphone, save looks as scenes and run them as a
//! playlist - all with a live preview, with or without a laser attached.
//!
//! One engine thread renders a frame 60 times a second from the shared
//! state and hands it to the preview and (when one is configured) to the
//! laser output. The HTTP handlers in `web.rs` only ever edit that shared
//! state.

mod ableton_link;
mod abstracts;
mod about;
mod agenda;
mod applog;
mod artnet;
mod audience;
mod autopilot;
mod audio;
mod autosave;
mod backup;
mod beat;
mod chases;
mod checklist;
mod colorcal;
mod command;
mod autoshow;
mod controls;
mod crossings;
mod cues;
mod cuelist;
mod devices;
mod dmx;
mod citp;
mod dmx_out;
mod datafile;
mod formats;
mod dwell;
mod engine;
mod evolving;
mod examples;
mod desk;
mod executors;
mod fans;
mod fixed_rate;
mod fx;
mod festival;
mod figure_import;
mod figures;
mod fonts;
mod generators;
mod graphics;
mod grid;
mod history;
mod ilda;
mod ilda_library;
mod instance;
mod layers;
mod levels;
mod lfo;
mod interlock;
mod help;
mod keep_awake;
mod latency;
mod live;
mod macros;
mod menubar;
mod midi;
mod mpe;
mod netin;
mod notices;
mod onboarding;
mod operators;
mod external;
mod optimize;
mod outdoor;
mod output;
mod output_manager;
mod patterns;
mod perf;
#[cfg(test)]
mod perf_budget;
mod pools;
mod ponk;
mod power;
mod presence;
mod prepared;
mod presets;
mod profiles;
mod running;
mod program;
mod project;
mod projection;
mod push;
mod safety;
#[cfg(test)]
mod safety_invariants;
mod safety_log;
mod safetylock;
mod safetysheet;
mod showlist;
mod showmode;
mod sites;
mod lspack;
#[cfg(feature = "shownet")]
mod shownet;
mod speed;
mod spread;
mod scan;
mod script;
mod scenes;
mod record;
mod remote;
mod standby;
mod sdbank;
mod sheets;
mod psn;
mod targets;
mod tempo;
mod take;
mod templates;
mod testpattern;
mod textures;
mod timecode;
mod timeline;
mod timeline_csv;
mod transition;
mod tunnels;
mod usercues;
mod versions;
mod venue;
mod watchdog;
mod zonechase;
mod zones;
mod zonetrace;
mod web;
mod wifi;
mod wire;

#[cfg(test)]
mod test_support;

use anyhow::{Context, Result};
use clap::Parser;
use engine::{Animator, AudioFeatures, Calibration, Settings};
use interlock::{ArmGate, EStop};
use output::{Output, OutputStage};
use patterns::Point;
use scenes::SceneStore;
use std::collections::{BTreeMap, HashMap};
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
    /// Address the tablet remote (T-428) listens on once the operator
    /// allows it from RÉGLAGES: every interface by default. The browser
    /// UI itself always stays on 127.0.0.1.
    #[arg(long, value_name = "IP", default_value = "0.0.0.0")]
    remote_bind: std::net::IpAddr,
    /// Address the backup machine's sync listener (T-456) binds when this
    /// studio is the main machine: every interface by default (or the one
    /// chosen in RÉGLAGES); any other value always wins (tests: 127.0.0.1).
    #[arg(long, value_name = "IP", default_value = "0.0.0.0")]
    standby_bind: std::net::IpAddr,
    /// Testing only (T-456): the folder of the outputs' exclusive claims,
    /// shared by two test studios that must never drive the same output.
    /// Default: the system's temporary folder (real outputs), or the data
    /// folder (test outputs).
    #[arg(long, hide = true, value_name = "DIR")]
    dac_lock_dir: Option<PathBuf>,
    /// Where scenes and calibration are saved.
    #[arg(long, default_value = "studio-data")]
    data_dir: PathBuf,
    /// Scan for laser DACs (Helios and LaserCube on USB, Ether Dream, IDN
    /// and LaserCube on the network), print their ids for --device and exit. Opens no output.
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
    if exe.is_some_and(in_app_bundle) && args.len() <= 1 {
        args.push("--app".into());
    }
    args
}

/// Whether `exe` is the binary of a `.app` bundle (`X.app/Contents/MacOS/x`).
pub fn in_app_bundle(exe: &std::path::Path) -> bool {
    let mut up = exe.ancestors().skip(1);
    let (macos, contents, bundle) = (up.next(), up.next(), up.next());
    macos.and_then(|p| p.file_name()) == Some("MacOS".as_ref())
        && contents.and_then(|p| p.file_name()) == Some("Contents".as_ref())
        && bundle.and_then(|p| p.extension()) == Some("app".as_ref())
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
    /// Where the *Décalage de sortie* is saved (latency.rs, T-246);
    /// `None` in tests.
    pub latency_path: Option<PathBuf>,
    /// Speed groups A..D and the *Cadence* (speed.rs, T-419), saved to
    /// `speed.json` (`speed_path`, `None` in tests) when changed.
    pub speed: tempo::SpeedGroups,
    pub speed_dirty: bool,
    pub speed_path: Option<PathBuf>,
    /// Ableton Link (ableton_link.rs, T-154): off at every start.
    pub link: ableton_link::LinkState,
    pub epoch: Instant,
    /// Tests only: the simulated « now » `now_s` reads, set by the test
    /// rigs that drive the engine on a simulated clock (wall-clock time
    /// would make two identical simulated sessions differ).
    #[cfg(test)]
    pub sim_now: Option<Instant>,
    /// Master live modifiers (live.rs), saved to live.json when changed.
    pub live: live::LiveModifiers,
    pub live_dirty: bool,
    /// The cue level's working copies (levels.rs, T-144), by preset id,
    /// for the cues on show. Memory only: a cue starts from its cell's.
    pub cue_mods: HashMap<String, live::LiveModifiers>,
    /// User colour palettes (palettes.json).
    pub palettes: live::PaletteStore,
    /// User scan presets (scan_presets.json, T-415).
    pub scan_presets: scan::ScanPresetStore,
    /// Referenced presets (pools.json, T-422): position, size, colour and
    /// motion presets cues and timeline events point to.
    pub pools: pools::PoolStore,
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
    /// The FX grid (fx.rs, fx.json, T-146): effects played on top of the
    /// cues, on the same copies, after the LFOs. What plays is memory only.
    pub fx: fx::FxStore,
    /// Audio event triggers (audio/triggers.rs, audio_triggers.json): rules
    /// that press cues on a kick, a drop, a break… Their switch is memory
    /// only and off at start; they never arm.
    pub triggers: audio::triggers::TriggerStore,
    /// Pilote automatique (autopilot.rs, autopilot.json, T-148): presses a
    /// cue every N bars. Its switch is memory only and off at start; it
    /// never arms.
    pub autopilot: autopilot::AutopilotStore,
    /// Mirror targets (targets.rs, targets.json, T-109): calibrated points
    /// of the room. Machine settings, like the calibration.
    pub targets: targets::TargetStore,
    /// Target tracking over PosiStageNet (psn.rs, psn.json, T-457): moves
    /// the mobile targets. Machine settings; off by default.
    pub psn: psn::PsnState,
    /// The four cue layers and the point budget (layers.rs), saved to
    /// layers.json when changed.
    pub mixer: layers::Mixer,
    pub mixer_dirty: bool,
    /// A group master (T-420, `group.<n>.master`) moved: the engine writes
    /// projection.json at most once a second, like `mixer_dirty`.
    pub groups_dirty: bool,
    /// Where each zone chaser stands (T-170), written by the engine every
    /// frame for the page.
    pub zone_chase: zonechase::ChaseNow,
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
    /// The main output's fixed frame rate (T-463) on the last frame, None
    /// when off.
    pub fixed_rate: Option<fixed_rate::FixedRateStatus>,
    /// Evolving cues on show in the last frame: (animator id, where it is).
    /// Id 0 is the manual look.
    pub evolving: Vec<(u64, evolving::Progress)>,
    /// The timeline player (timeline.rs): its events play through the
    /// same layers and live stage as the cues. Never arms anything.
    pub timeline: timeline::Player,
    /// The live take recorder (take.rs, T-169): observes the cues and
    /// master moves while the timeline plays. Reads only, never arms.
    pub take: take::Recorder,
    /// Saved shows, `studio-data/shows/`.
    pub shows: timeline::ShowStore,
    /// Timeline templates (templates.rs, T-165), `studio-data/templates/`:
    /// the user's library, not part of a project.
    pub templates: templates::TemplateStore,
    /// Incoming timecode (timecode/, T-168): settings in `timecode.json`,
    /// TC-IN in memory only (off at start). Drives the timeline only.
    pub timecode: timecode::TimecodeState,
    /// The operator's figures (figures.rs, `studio-data/figures/`), also
    /// the cues of the « Figures » page.
    pub figures: figures::FigureStore,
    /// The operator's 3D objects (wire.rs, T-462, `studio-data/objects/`),
    /// also the cues of the « Objets 3D » page.
    pub objects: wire::WireStore,
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
    /// Tracing the zones for verification (zonetrace.rs, T-447): memory
    /// only, off at start.
    pub zone_trace: zonetrace::ZoneTrace,
    /// Prevent sleep during a show (keep_awake.rs): setting and status.
    pub keep_awake: keep_awake::KeepAwakeState,
    /// First launch (onboarding.rs, T-437): whether the tour was seen.
    pub onboarding: onboarding::OnboardingStore,
    /// Show mode (showmode.rs, T-283): editing routes refused with 423.
    /// Kept in `prefs.json`; never arms anything.
    pub show_mode: showmode::ShowMode,
    /// Safety lock (safetylock.rs, T-261): Safety-class routes refuse
    /// loosening edits with 423 while it is on. The code's hash is in
    /// `safety-lock.json`.
    pub safety_lock: safetylock::SafetyLock,
    /// Damaged data files moved to `quarantine/` this run (datafile.rs,
    /// T-402), shown until acknowledged. In memory only.
    pub data_warnings: Vec<datafile::DataWarning>,
    /// The pre-show checklist (checklist.rs, T-258): the list is saved,
    /// what was ticked lives for this run only.
    pub checklist: checklist::Checklist,
    /// Safety profiles per venue (profiles.rs, T-260): the active one is a
    /// label over the settings in force; loading one disarms.
    pub profiles: profiles::ProfileStore,
    /// Site profiles (sites.rs, T-289): calibration, safety and projection
    /// zones of a venue; the active one is a label, loading one disarms.
    pub sites: sites::SiteStore,
    /// Audience scanning mode (audience.rs, T-255). In memory only: every
    /// start is « Au-dessus du public ».
    pub audience: audience::AudienceState,
    /// « Ciel coupé » (T-262, outdoor.rs): every point above the horizon
    /// blanked on every projector. In memory only: off at every start.
    pub sky_off: bool,
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
    /// Recording what plays, or a timeline, as an ILDA animation (record.rs,
    /// T-442). Never saved; it never arms nor changes what is sent.
    pub recorder: record::Recorder,
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
    /// The engine's timings over the last 10 s (perf.rs, T-448): a copy
    /// written in the critical section that ends every frame.
    pub perf: perf::PerfReport,
    /// Notification centre (T-432): events without a request, in memory.
    pub notices: notices::Notices,
    /// Undo / redo of edits (history.rs, T-285): in memory, 50 levels,
    /// emptied when another project opens.
    pub history: history::EditHistory,
    /// « Préparer » (T-274): a cue loaded blind, with its own animator,
    /// rendered only by `GET /api/preview-frame`. The engine never reads
    /// it; only « Envoyer » plays it, as a cue press. Memory only.
    pub prepared: prepared::PreviewSlot,
    /// The executor bank (executors.rs, T-418, `executors.json`): the
    /// assignments are content (projects, history); what the hands hold
    /// is memory only.
    pub executors: executors::Executors,
    /// The custom desk (desk.rs, T-427, `desks.json`): tabs of buttons,
    /// faders and XY pads the operator lays out. Content (projects,
    /// history); playing it goes through the control registry.
    pub desks: desk::Desks,
    /// Macros (macros.rs, T-425, `macros.json`): the list is content
    /// (projects, history); what runs is memory only.
    pub macros: macros::Macros,
    /// Sandboxed scripts (script/, T-458, `scripts.json`): the list is
    /// content (projects, history); runs and their trust are machine and
    /// memory state (`scripts_trust.json`).
    pub scripts: script::Scripts,
    /// The tablet remote (remote.rs, T-428): off at every start; only
    /// its port is saved (`remote.json`).
    pub remote: remote::Remote,
    /// The synchronised backup machine (standby.rs, T-456, `standby.json`):
    /// Autonome by default, no listener; a backup in standby never arms
    /// and holds no output.
    pub standby: standby::Standby,
    /// The show player (showlist.rs, T-444, `showlist.json`): the list is
    /// content (projects, history); what plays is memory only.
    pub showlist: showlist::ShowListRunner,
    /// The agenda (agenda.rs, T-455, `agenda.json`): the machine's, not
    /// the project's; its own thread starts the playbacks. Never arms.
    pub agenda: agenda::AgendaRunner,
    /// The DMX output (T-453, dmx_out.rs): haze and simple lights, settings
    /// in `dmx_out.json`, never in a project; off by default.
    pub dmx_out: dmx_out::DmxOutState,
    /// Cue names and pictures for a lighting console (T-454, citp/):
    /// settings in `citp.json`, never in a project; off by default.
    pub citp: citp::CitpState,
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
        // A backup unreachable needs a confirmation MIDI cannot give.
        standby::arm_check(self, false).map_err(|e| vec![e])?;
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
        // The copy to a second location (T-450): unavailable, or low space.
        let storage = self.project.backup.status();
        self.notices.condition("backup", "Projet", storage.error.map(|e| (notices::Level::Warning, e)));
        self.notices.condition("disk_space", "Projet", storage.low_space.map(|e| (notices::Level::Warning, e)));
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
        if kind.is_some() {
            self.wifi.set_target(Some(wifi::Target::for_receiver(receiver)));
        } else {
            // A LaserCube on the network (T-496): the route to its address,
            // from the last scan or from an id in the IP form.
            let cube = self.output_mgr.selected().filter(|id| id.starts_with(devices::LASERCUBE_NETWORK_PREFIX)).and_then(|id| {
                self.output_mgr.devices().devices().into_iter().find(|d| d.id == id).and_then(|d| d.ip).or_else(|| devices::lasercube_id_ip(&id))
            });
            self.wifi.set_target_for(cube.filter(|ip| !ip.is_loopback() && !ip.is_unspecified()).map(wifi::Target::Host), "lasercube");
        }
        // The audience mode follows the checklist and the sheet (T-255).
        self.sync_audience();
    }

    /// The conditions to unlock audience scanning, as they stand (T-255).
    pub fn audience_conditions(&self) -> Vec<audience::Condition> {
        let safety = self.safety.get();
        let of = |projector: &power::ProjectorInfo| {
            audience::conditions(&audience::Inputs { safety: &safety, projector, checklist: &self.checklist, hold_to_run: self.presence.settings.hold_to_run, venue_allows: self.profiles.audience_allowed() })
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
        // Outdoor mode (T-262): no arming without the DGTA reference.
        let outdoor_ok = self.safety.get().outdoor.authorized();
        if self.gate.interlock_ok(interlock::OUTDOOR_AUTH) != outdoor_ok {
            self.gate.register(interlock::OUTDOOR_AUTH, interlock::OUTDOOR_AUTH_LABEL, outdoor_ok);
        }
        if self.checklist.enforced() {
            let ok = audience::protected(&self.safety.get());
            if !self.gate.has_interlock(interlock::AUDIENCE_ZONE) || self.gate.interlock_ok(interlock::AUDIENCE_ZONE) != ok {
                self.gate.register(interlock::AUDIENCE_ZONE, audience::INTERLOCK_LABEL, ok);
            }
        }
    }

    /// The safety settings the output stage applies now: Public zones
    /// blanked outside audience scanning (audience.rs), « Ciel coupé »
    /// (T-262) from this session.
    pub fn effective_safety(&self) -> safety::SafetySettings {
        let mut cfg = audience::effective(&self.safety.get(), self.audience.is_scanning());
        cfg.outdoor.sky_off = self.sky_off;
        cfg
    }

    /// « Ciel coupé » on or off (T-262). Cutting is always allowed, from
    /// any source (it only darkens, from the next frame); restoring is
    /// the operator's explicit gesture (UI, keyboard, local API). Never
    /// arms nor disarms. Returns false when refused.
    pub fn set_sky_off(&mut self, on: bool, src: interlock::ArmSource) -> bool {
        if !on && !profiles::operator_source(src) {
            return false;
        }
        // Restoring the sky loosens: refused under the safety lock (T-261).
        if !on && self.sky_off && safetylock::check(self, std::time::Instant::now()) {
            return false;
        }
        if self.sky_off != on {
            self.sky_off = on;
            log::warn!("Ciel coupé : {}", if on { "activé" } else { "rétabli" });
            self.gate.log().record("sky_off", Some(src.id()), serde_json::json!({ "on": on }));
        }
        true
    }

    /// Trips the emergency stop and records it in the gate. Audience
    /// scanning ends with it (T-255).
    pub fn emergency_stop(&mut self, src: interlock::ArmSource) {
        self.estop.trip(src);
        // No macro step runs after the stop (T-425).
        self.macros.player.stop_all();
        // Script runs are dropped, their triggers held (T-458).
        script::stop_all(self);
        // The show list stops chaining until *Lancer* (T-444).
        self.showlist.halt();
        // The zone trace ends at once (T-447); starting it again takes a click.
        zonetrace::stop(self, "estop");
        // A phone's alignment draft (T-460) is dropped: the saved zone.
        remote::align_end(self, "estop");
        // Fades end and waiting cues are dropped at once (T-158 / T-159).
        self.deck.cut_transitions();
        // A live take ends here, keeping what it captured (T-169).
        self.take.finish(take::EndReason::EStop);
        // The cue list pauses; it never starts again by itself (T-410).
        self.cue_list.halt();
        self.gate.sync_estop(&self.estop);
        // External inputs lose the hand until the operator reopens them,
        // and so does the audio automation (T-240).
        external::sync(self);
        audio::triggers::sync_estop(self);
        // The autopilot too (T-148); the reset doesn't switch it back on.
        autopilot::sync_stops(self);
        // Timecode restarts nothing until TC-IN is armed again (T-168).
        timecode::sync(self);
        // The haze button lets go; the DMX output sends zeros (T-453).
        dmx_out::estop(self);
        self.sync_audience();
    }

    /// Seconds since startup: the time base of the tempo clock.
    pub fn now_s(&self) -> f64 {
        #[cfg(test)]
        if let Some(now) = self.sim_now {
            return now.saturating_duration_since(self.epoch).as_secs_f64();
        }
        self.epoch.elapsed().as_secs_f64()
    }

    /// Where the speed groups (T-419) are measured from now: the effects'
    /// master beat, the effects' seconds and the bar length.
    pub fn speed_clock(&self) -> (f64, f64, u8) {
        let t = self.now_s();
        (self.tempo.effect_beat_at(t), self.tempo.effect_t(t), self.tempo.beats_per_bar)
    }

    /// The speed group a look plays in (T-419): its cell's (a cue of the
    /// deck), else its layer's, else the master clock.
    pub fn look_speed_group(&self, layer: u8, id: u64) -> u8 {
        let slot = self.deck.find(id).and_then(|a| self.deck.slots.get(&a.cue)).and_then(|sl| sl.speed_group);
        speed::resolve(slot, self.mixer.layer(layer).speed_group)
    }

    /// `speed.json`, when the speed groups changed (T-419); queued for
    /// the writer thread (T-482), the engine saves it.
    pub fn save_speed(&mut self) {
        if !std::mem::take(&mut self.speed_dirty) {
            return;
        }
        if let Some(path) = &self.speed_path {
            datafile::save_json_later(path, &self.speed);
        }
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
        // The effects' beat, like the engine's frame (T-246).
        timeline::Clock { t, beat: self.tempo.effect_beat_at(t), bpm: self.tempo.bpm, beats_per_bar: self.tempo.beats_per_bar }
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

    // Format versions (T-288), before any store reads its file: a data
    // folder written by a newer studio is refused, untouched; older files
    // are brought to the current format, their originals kept.
    if let Err(e) = formats::prepare_data_dir(&cli.data_dir) {
        eprintln!("{e:#}");
        log::error!("{e:#}");
        std::process::exit(2);
    }
    // T-448: one line on the data folder and its disk.
    log::info!("{}", perf::startup_check(&cli.data_dir));
    let mut state = startup_state(&cli);
    // The output named on the command line opens now, as before (a failure
    // stops the studio); otherwise the remembered one is connected by the
    // output manager once it runs (T-401). Either way: disarmed.
    // A backup machine in standby (T-456) opens no output at all.
    let in_standby = state.standby.is_standby();
    if in_standby {
        println!("Machine de secours en veille : aucune sortie ouverte, armement refusé.");
        standby::sync_gate(&mut state);
    }
    // A main machine whose backup took over while it was away opens
    // nothing and steps down (T-456): it never takes anything back.
    let hold_outputs = in_standby || standby::startup_check(&mut state);
    if hold_outputs && !in_standby {
        println!("Une autre machine est principale : aucune sortie ouverte au démarrage.");
    }
    let output: Option<Box<dyn Output>> = match startup_output_id(&cli).filter(|_| !hold_outputs) {
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
    let (clock_rx, clock_status) = (state.midi.clock.out.take_receiver(), Arc::clone(&state.midi.clock.out.status));
    let (netin_hub, netin_tx) = (Arc::clone(&state.netin.hub), state.netin.sender());
    let psn_hub = Arc::clone(&state.psn.hub);
    // OSC feedback (T-416): the queue the engine fills and "osc-out" empties.
    let osc_out_rx = state.netin.feedback.take_receiver();
    // Timecode output (T-417): the snapshots the engine queues for "tc-out".
    let (tc_out_rx, tc_out_status) = (state.timecode.out.take_receiver(), Arc::clone(&state.timecode.out.status));
    // DMX output (T-453): the universes the engine queues for "dmx-out".
    let (dmx_out_rx, dmx_out_status) = (state.dmx_out.take_receiver(), Arc::clone(&state.dmx_out.status));
    let audio_hub = Arc::clone(&state.audio_in);
    let (song_hub, media) = (Arc::clone(&state.song), Arc::clone(&state.media));
    if let Some(kill) = output.as_ref().and_then(|o| o.kill_switch()) {
        estop.set_kill_switch(kill);
    }
    let output_mgr = Arc::clone(&state.output_mgr);
    let wifi_watch = Arc::clone(&state.wifi);
    let remembered = state.outputs.connection().selected.filter(|_| output.is_none() && !hold_outputs);
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
    if !hold_outputs {
        shared.lock().unwrap().connect_projectors(interlock::ArmSource::System);
    }
    // The backup machine (T-456): its listener or its worker, per role.
    if let Err(e) = standby::apply_role(&shared) {
        log::error!("Machine de secours : {e}");
    }

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

    // MTC (T-417) and the MIDI clock (T-207) go out through the MIDI
    // backend in use (the simulator with --midi-test), each on a port the
    // operator picks; none with --no-midi.
    let midi_opener = midi::clock::opener(sim.clone(), cli.no_midi);
    let tc_out_thread = tc_out_rx.and_then(|rx| timecode::out::spawn(rx, tc_out_status, midi_opener.clone(), Arc::clone(&running)));
    // Idle (no socket) until switched on; reads the e-stop latch itself.
    let dmx_out_thread = dmx_out_rx.and_then(|rx| dmx_out::spawn(rx, dmx_out_status, Arc::clone(&estop), Arc::clone(&running)));
    // CITP (T-454): no socket until switched on; only reads the grid.
    let citp_hub = Arc::clone(&shared.lock().unwrap().citp.hub);
    let citp_thread = citp::server::spawn(Arc::clone(&shared), citp_hub, Arc::clone(&running));
    let clock_thread = clock_rx.and_then(|rx| midi::clock::spawn(rx, clock_status, midi_opener, Arc::clone(&running)));
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
    let mut netin_threads = netin::listener::spawn(Arc::clone(&netin_hub), netin_tx, Arc::clone(&running));
    // OSC feedback: idle (no socket) until switched on and a destination exists.
    netin_threads.extend(osc_out_rx.and_then(|rx| netin::feedback::spawn(rx, netin_hub, Arc::clone(&running))));
    // PosiStageNet (T-457): idle (no socket) until switched on.
    netin_threads.extend(psn::spawn(psn_hub, Arc::clone(&running)));

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
    // The copy to a second location (T-450): its own thread, fed by saves.
    let backup_hub = Arc::clone(&shared.lock().unwrap().project.backup);
    let backup_thread = backup::spawn(Arc::clone(&backup_hub));
    // Keep the Mac awake while armed or playing (T-412); never the real
    // assertion in tests.
    let blocker: Box<dyn keep_awake::SleepBlocker> = if cli.test_keep_awake || cli.test_hooks || cli.test_output.is_some() {
        Box::new(keep_awake::FakeBlocker::default())
    } else {
        keep_awake::system_blocker()
    };
    let keep_awake_thread = keep_awake::spawn(Arc::clone(&shared), Arc::clone(&running), blocker);
    // The agenda (T-455): starts playbacks at their slots, never arms.
    let agenda_thread = agenda::spawn(Arc::clone(&shared), Arc::clone(&running));
    let standby_shared = Arc::clone(&shared);
    let served = web::run(&addr, shared, estop, cli.data_dir.join("calibration.json"), Arc::clone(&running));

    running.store(false, Ordering::SeqCst);
    // The sync listener closes, the backup stops following (T-456).
    standby::stop(&standby_shared);
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
    agenda_thread.join().ok();
    // The autosave thread writes what is unsaved, then stops; a stuck disk
    // must not hold the exit.
    let deadline = Instant::now() + AUTOSAVE_SHUTDOWN_WAIT;
    while !autosave_thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    // Then the copy writes what it was given (that last autosave too); a
    // pulled-out key or a slow network folder must not hold the exit.
    backup_hub.shutdown();
    let deadline = Instant::now() + AUTOSAVE_SHUTDOWN_WAIT;
    while !backup_thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(midi_thread) = midi_thread {
        midi_thread.join().ok(); // lets it switch the APC LEDs off
    }
    if let Some(t) = clock_thread {
        t.join().ok(); // sends Stop on its way out
    }
    for t in netin_threads {
        t.join().ok(); // within one receive timeout (100 ms)
    }
    if let Some(t) = tc_out_thread {
        t.join().ok(); // within one poll (20 ms); the stream simply stops
    }
    if let Some(t) = dmx_out_thread {
        t.join().ok(); // within one poll (10 ms), after its frames of zeros
    }
    if let Some(t) = citp_thread {
        t.join().ok(); // within one poll (50 ms); its sockets close
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
    // First start ever (T-260): an empty data directory (the instance lock
    // and the logs aside). Read before anything is written.
    let fresh_install = std::fs::read_dir(&cli.data_dir)
        .map(|entries| entries.flatten().all(|e| matches!(e.file_name().to_str(), Some(instance::LOCK_FILE) | Some("logs"))))
        .unwrap_or(true);
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
    let fx = fx::FxStore::load_or_create(cli.data_dir.join("fx.json"), &controls);
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
        recorder: record::Recorder::default(),
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
        tempo: {
            // The *Décalage de sortie* (T-246), clamped on load.
            let mut tempo = tempo::TempoClock::default();
            let _ = tempo.set_output_offset_ms(latency::load(&latency::path(&cli.data_dir)).output_offset_ms as f64);
            tempo
        },
        latency_path: Some(latency::path(&cli.data_dir)),
        speed: load_json::<tempo::SpeedGroups>(&cli.data_dir.join("speed.json")).sanitized(),
        speed_dirty: false,
        speed_path: Some(cli.data_dir.join("speed.json")),
        link: Default::default(),
        epoch,
        #[cfg(test)]
        sim_now: None,
        // Held effects (« Noir », « Figer », strobe flash) never survive a restart.
        live: {
            let mut live: live::LiveModifiers = load_json(&cli.data_dir.join("live.json"));
            live.release_momentary();
            live
        },
        live_dirty: false,
        cue_mods: HashMap::new(),
        palettes: live::PaletteStore::load_or_create(cli.data_dir.join("palettes.json")),
        scan_presets: scan::ScanPresetStore::load_or_create(cli.data_dir.join("scan_presets.json")),
        pools: pools::PoolStore::load_or_create(cli.data_dir.join("pools.json")),
        festival: festival::Festival::load(festival::path(&cli.data_dir)),
        midi: {
            let mut m = midi::MidiState::new(!cli.no_midi || cli.midi_test, midi::profile::ProfileStore::load(cli.data_dir.join("midi")));
            m.sim = sim.clone();
            m
        },
        lfos,
        routes,
        fx,
        triggers: audio::triggers::TriggerStore::load(cli.data_dir.join("audio_triggers.json")),
        autopilot: autopilot::AutopilotStore::load(cli.data_dir.join("autopilot.json")),
        targets: targets::TargetStore::load(cli.data_dir.join("targets.json")),
        psn: psn::PsnState::load(cli.data_dir.join("psn.json")),
        mixer: {
            let mut m: layers::Mixer = load_json(&cli.data_dir.join("layers.json"));
            m.sanitize();
            m
        },
        mixer_dirty: false,
        groups_dirty: false,
        zone_chase: Default::default(),
        mix: layers::MixReport::default(),
        safety: safety::SafetyStore::load_or_create(cli.data_dir.join("safety.json")),
        strobe: safety::StrobeStatus::default(),
        outputs,
        projection: projection::ProjectionStore::load_or_create(cli.data_dir.join("projection.json")),
        audio_guard: audio::safety::GuardStatus::default(),
        dwell: dwell::DwellStatus::default(),
        fixed_rate: None,
        evolving: Vec::new(),
        timeline: timeline::Player::default(),
        take: take::Recorder::default(),
        shows: timeline::ShowStore::new(cli.data_dir.join("shows")),
        templates: templates::TemplateStore::new(cli.data_dir.join("templates")),
        timecode: timecode::TimecodeState::load(cli.data_dir.join("timecode.json")),
        figures: figures::FigureStore::load(cli.data_dir.join("figures")),
        objects: wire::WireStore::load(cli.data_dir.join("objects")),
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
        zone_trace: Default::default(),
        keep_awake: keep_awake::KeepAwakeState::load(&cli.data_dir),
        onboarding: onboarding::OnboardingStore::load(&cli.data_dir),
        show_mode: showmode::ShowMode::load(&cli.data_dir),
        safety_lock: safetylock::SafetyLock::load(&cli.data_dir),
        data_warnings: Vec::new(),
        checklist: checklist::Checklist::load(cli.data_dir.join("checklist.json"), checklist_enforced(cli)),
        profiles: profiles::ProfileStore::load(&cli.data_dir),
        sites: sites::SiteStore::load(&cli.data_dir),
        audience: Default::default(),
        sky_off: false,
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
        perf: Default::default(),
        notices: Default::default(),
        history: Default::default(),
        prepared: Default::default(),
        executors: executors::Executors::load(cli.data_dir.join("executors.json")),
        desks: desk::Desks::load(cli.data_dir.join("desks.json")),
        macros: macros::Macros::load(cli.data_dir.join("macros.json")),
        scripts: script::Scripts::load(&cli.data_dir),
        showlist: showlist::ShowListRunner::load(cli.data_dir.join("showlist.json")),
        agenda: agenda::AgendaRunner::load(cli.data_dir.join("agenda.json")),
        remote: remote::Remote::load(cli.data_dir.join("remote.json"), cli.remote_bind),
        standby: standby::Standby::load(cli.data_dir.join("standby.json"), cli.standby_bind),
        dmx_out: dmx_out::DmxOutState::load(cli.data_dir.join("dmx_out.json")),
        citp: citp::CitpState::load(cli.data_dir.join("citp.json")),
    };
    usercues::sync_presets(&mut state);
    let objects = state.objects.list().iter().map(wire::preset_of).collect();
    wire::set_presets(&mut state, objects);
    figures::refresh(&mut state);
    // Macro lines checked against the registry they will run on (T-425).
    macros::revalidate(&mut state);
    // Scripts too (T-458): a refused line switches its script off.
    script::revalidate(&mut state);
    // The show list's times, read by its worker (T-444).
    showlist::refresh(&mut state);
    if cli.test_hooks {
        state.project.autosave.allow_test_delays();
    }
    // First start: the existing data becomes a « Sans titre » project.
    project::startup(&mut state);
    // Safety profiles (T-260): existing settings kept as « Réglages
    // actuels », a fresh install on « Aperçu sûr ». Never arms.
    profiles::startup(&mut state, &cli.data_dir, fresh_install);
    // Site profiles (T-289): the settings in force, with calibration.json,
    // become « Par défaut » on the first start. Never arms.
    sites::startup(&mut state, &cli.data_dir);
    safetylock::startup(&mut state);
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
        let locks = cli.dac_lock_dir.clone().unwrap_or_else(|| cli.data_dir.join("output-locks"));
        return OutputManager::new(Arc::new(backend), list, prefs, pps).with_test_drop(dropped).with_lock_dir(locks);
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
    // Two studios on this Mac never drive the same output (T-456).
    let locks = cli.dac_lock_dir.clone().unwrap_or_else(|| std::env::temp_dir().join("laser-studio-output-locks"));
    OutputManager::new(backend, list, prefs, pps).with_lock_dir(locks)
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
    /// Its fixed frame rate between frames (T-463).
    fixed: fixed_rate::FixedRateState,
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
            fixed: fixed_rate::FixedRateState::default(),
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
    /// Its fixed frame rate, if on (T-463).
    fixed: Option<fixed_rate::FixedTarget>,
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

/// A fixed frame rate (T-463) as the safety chain applies it: the
/// output's target, the points its seam adds between frames, and its
/// state between frames.
struct FixedChain<'a> {
    target: fixed_rate::FixedTarget,
    seam: usize,
    state: &'a mut fixed_rate::FixedRateState,
}

/// Points of one frame the engine sends at a fixed rate: the target less
/// the output's seam, within the DAC's frame size (a Helios USB), at
/// least 2.
fn fixed_frame_points(target: &fixed_rate::FixedTarget, seam: usize, max_points: Option<usize>) -> usize {
    let n = target.points.saturating_sub(seam).max(2);
    max_points.map_or(n, |m| n.min(m.max(2)))
}

/// The safety stage of one projector's frame, in its order (see
/// `Engine::tick`): frame size, colour rendering, strobe limiter and
/// horizon, static-beam guard, power caps, the limiter's check on the
/// output, then (fixed rate only) dark points up to the frame size. Every
/// projector runs it with its own limiter, guard, colour and caps (T-277).
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
    fixed: Option<FixedChain>,
) -> Vec<Point> {
    // Points per frame of the output (T-171: its own range, and the
    // Helios USB frame size, T-303): thinned or completed here, before
    // the safety stage (T-353), so zones, horizon, strobe limiter, static
    // beam guard and caps all apply to the frame that really goes out.
    // At a fixed rate (T-463), resampled to exactly its frame size less a
    // reserve for the zone mask instead. Nothing after the safety stage
    // adds light.
    let fixed = fixed.map(|f| {
        let n = fixed_frame_points(&f.target, f.seam, max_points);
        let reserve = f.state.reserve(&f.target, n);
        (f, n, reserve)
    });
    let (mut frame, shaped) = match &fixed {
        Some((f, n, reserve)) => {
            let (out, report) = fixed_rate::shape(&frame, *n, *reserve, f.target.max_step, safety_cfg.dwell.sanitized().grid as usize);
            (out, Some(report))
        }
        None => (output::fit_frame_limits(frame, frame_limits, max_points), None),
    };
    let shaped_len = frame.len();
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
    // The limiter's last word (T-360): it measures the frame that really
    // goes out. The guard and the caps run after its hold (the frame it
    // puts back must be guarded and capped too), and dimming can make
    // light alternate as well; a flash out that would exceed the burst
    // stays dark. It only darkens, so everything above still holds.
    limiter.finish(&mut frame, t, safety_cfg);
    // Fixed rate (T-463): dark points held where the frame ends, up to
    // its size; nothing removed, nothing lit. A frame the zone mask made
    // longer than that goes out as it is, counted.
    if let (Some((f, n, _)), Some(report)) = (fixed, shaped) {
        f.state.after_safety(report, shaped_len, frame.len(), n);
        frame = fixed_rate::top_up(frame, n);
    }
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
    /// The cue and layer levels (T-144): rotation state of each cue
    /// instance's own level and of each layer, and the « Lissage » filters.
    cue_level_live: HashMap<u64, live::LiveState>,
    layer_live: [live::LiveState; layers::LAYER_COUNT],
    smoothers: levels::Smoothers,
    /// « Figer » held (T-142): the (seconds, beats) time stood still at,
    /// and the content beat of each timeline event met while frozen.
    frozen: Option<(f64, f64)>,
    frozen_events: HashMap<u64, f64>,
    /// The speed group each look was last drawn in (T-419).
    look_groups: HashMap<u64, u8>,
    /// Spread looks (T-421): delay memories and per-zone animators.
    spread: spread::SpreadState,
    frames_since_save: u32,
    last: Instant,
    /// If this thread panics, unwinding drops the stage: dark frame + disarm.
    stage: OutputStage,
    /// The main output's fixed frame rate between frames (T-463).
    fixed: fixed_rate::FixedRateState,
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
    /// Stage timings (T-448); `None` only in the budget test's run
    /// without measuring. Allocated once here, nothing per frame.
    perf: Option<perf::PerfMonitor>,
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
            cue_level_live: HashMap::new(),
            layer_live: Default::default(),
            smoothers: Default::default(),
            frozen: None,
            frozen_events: HashMap::new(),
            look_groups: HashMap::new(),
            spread: Default::default(),
            frames_since_save: 0,
            last: Instant::now(),
            stage: OutputStage::new(output),
            fixed: fixed_rate::FixedRateState::default(),
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
            perf: Some(perf::PerfMonitor::new()),
        }
    }

    /// One frame at `now`: render, safety stage, gate, output.
    fn tick(&mut self, shared: &Mutex<Shared>, now: Instant) {
        // T-448: wall-clock stamps (never `now`, which tests simulate);
        // the lock is not held for any of it.
        let measuring = self.perf.is_some();
        let started = measuring.then(Instant::now);
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

        let record_due;
        let (looks, (transition_fx, exec_gains), show_cues, (starts, look_groups, layer_groups, speed, speed_off), (cue_masters, levels, part_mods, zone_live), clock, mixer, calibration, audio, armed, hold_ok, live, user_palettes, estop, health, t, safety_cfg, (caps, frame_limits, fixed), color, pattern, routing, cue_signal, plans, (festival_palette, balance), (look_scan, show_scan), trace) = {
            let mut s = shared.lock().unwrap();
            // E-stop latch, watchdog trip, heartbeats and hold-to-run, then
            // the audience mode, which ends with any of them (T-255).
            let hold_ok = s.sync_safety(now);
            s.sync_audience();
            // The zone trace (T-447) ends on its time, an e-stop, a disarm
            // or its page gone, before this frame's output is chosen.
            zonetrace::sync(&mut s, now);
            // A phone's alignment draft (T-460) ends with the e-stop, its
            // remote lost or quiet, show mode, or the zone edited.
            remote::align_sync(&mut s, now);
            // A main machine whose backup stopped polling disarms itself
            // before the backup may take over (T-456).
            standby::check_lease(&mut s, now);
            s.health.tick(s.gate.is_armed());
            if s.test_stall_ms > 0 {
                // `--test-hooks` only: a stall while holding the lock.
                let stall = Duration::from_millis(std::mem::take(&mut s.test_stall_ms));
                std::thread::sleep(stall);
            }
            self.frames_since_save += 1;
            if (s.live_dirty || s.mixer_dirty || s.groups_dirty || s.fx.dirty || s.speed_dirty) && self.frames_since_save >= 60 {
                // At most once a second, so MIDI faders don't hammer the
                // disk; written by the writer thread, never here (T-482).
                save_dirty(&mut s, &self.live_path, &self.layers_path);
                self.frames_since_save = 0;
            }
            advance_playlist(&mut s);
            // MIDI faders/encoders: at most one write per control per frame.
            midi::engine::frame(&mut s, now);
            // MIDI clock (T-207): a lost clock gives the tempo back; the
            // output thread gets the tempo (never waits).
            midi::clock::frame(&mut s, now);
            // Ableton Link (T-154): only when the operator turned it on.
            ableton_link::frame(&mut s, now);
            // OSC and Art-Net: what the listeners queued, then signal loss.
            netin::frame(&mut s, now);
            // Tracking (T-457): the mobile targets move, or go dark.
            psn::frame(&mut s, now);
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
            // Temps 1 (T-235): the suggested "1" and the phrases, shown in
            // any source; the bar moves by itself only under Tempo auto.
            let downbeat = s.audio_in.fresh_downbeat(now);
            s.tempo.apply_downbeat(downbeat, t);
            // Macros (T-425): the steps due now, on the tempo clock, before
            // the deck so a cue they launch starts in this frame. The e-stop
            // and a disarm end them first.
            let macro_beat = s.tempo.beat_at(t);
            macros::frame(&mut s, t, macro_beat);
            // Scripts (T-458): the runs finished in their thread since the
            // last frame apply their lines (never waited for), then the
            // beat and bar triggers post new runs. Stops first.
            script::frame(&mut s, t, &audio);
            // Pilote automatique (T-148): on the bar it is due, one cue
            // pressed on the boundary, before the deck. E-stop or disarm:
            // switched off. It never arms.
            autopilot::frame(&mut s, t);
            // Quantised launches due now start, finished transitions end;
            // with the e-stop latched, every transition is cut (T-158/159).
            controls::deck_frame(&mut s, t);
            // The executor bank (T-418): each playing cue's level from the
            // faders and buttons that hold it.
            executors::frame(&mut s);
            // *Décalage de sortie* (T-246): what is rendered reads the beat
            // ahead by the output latency. Only the content: the gate, the
            // e-stop, the safety chain and the strobe limiter below run on
            // this very frame, on real time.
            let clock = engine::BeatClock { beat: s.tempo.effect_beat_at(t), bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar };
            // Speed groups (T-419): a multiple waiting for its bar lands;
            // every group's beat is a function of this one clock. The
            // effects' seconds (`effect_t`) for the free BPMs.
            let speed_off = s.tempo.effect_t(t) - t;
            if s.speed.frame(clock.beat, t + speed_off) {
                s.speed_dirty = true;
            }
            let speed = s.speed.clone();
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
            // « Lissage » (T-144): the operator's moves glide, before the
            // modulators move the copy (an LFO is never smoothed).
            live = self.smoothers.master.step(&live, &s.mixer.smoothing, dt);
            let time_share = s.routes.routing().time_share();
            // Each modulator counts the beats of its speed group (T-419).
            // One spread over a zone group (T-421) moves each zone's copy
            // of the master modifiers instead, below.
            let (zoned_mods, global_mods) = lfo::split_zoned(s.lfos.list(), s.projection.get());
            lfo::modulate_grouped(&global_mods, &s.controls, &mut settings, &mut live, mod_t, &|g| speed.beat(g, mod_beat, mod_t + speed_off), time_share);
            // The FX grid (T-146), on top of the LFOs, on the same copies.
            // An e-stop ends every effect (they don't come back after the
            // reset). *Cue sélectionné*: the master-level changes go to the
            // newest cue's own modifiers instead (its look is `settings`).
            if s.estop.is_latched() && s.fx.any_playing() {
                s.fx.stop_all();
            }
            let fx_groups = std::array::from_fn(|g| (speed.beat(g as u8, mod_beat, mod_t + speed_off), speed.bpm(g as u8, clock.bpm) / clock.bpm.max(1e-9)));
            let fx_clock = fx::FxClock { t, beat: clock.beat, mod_t, mod_beat, bpm: clock.bpm, beats_per_bar: clock.beats_per_bar, groups: Some(fx_groups) };
            s.fx.frame(&fx_clock);
            let fx_cue = match (s.fx.grid().dest, s.deck.primary()) {
                (fx::FxDest::Cue, Some(top)) => {
                    let mut m = usercues::master_of(&s, &top.cue).unwrap_or_default();
                    s.fx.apply(&s.controls, &mut settings, &mut m, &fx_clock);
                    Some((top.id, m))
                }
                _ => {
                    s.fx.apply(&s.controls, &mut settings, &mut live, &fx_clock);
                    None
                }
            };
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
            // Spread modulators (T-421): each zone's copy of the master
            // modifiers, from this one, the modulator at the zone's phase.
            // Like every modulation: clamped, a brightness only dims.
            let zone_live: Vec<(u8, live::LiveModifiers)> = if zoned_mods.is_empty() {
                Vec::new()
            } else {
                let cfg = s.projection.get();
                cfg.zones
                    .iter()
                    .map(|z| {
                        let mut copy = live.clone();
                        lfo::modulate_zone(&zoned_mods, z.id, cfg, &s.controls, &mut copy, mod_t, &|g| speed.beat(g, mod_beat, mod_t + speed_off), time_share);
                        (z.id, copy)
                    })
                    .collect()
            };
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
            // The zone trace (T-447) does the same, and wins over a pattern.
            let trace = zonetrace::hold(&mut s);
            let show_cues = timeline_cues(&mut s, t, &clock);
            // Executors (T-418): a layer flashed or blacked while a button
            // is held, on the copy the mixer draws with.
            let mut mixer = s.mixer.clone();
            executors::layer_overrides(&s, &mut mixer);
            let exec_gains = executors::gains(&s);
            // Scan presets (T-415): the optimiser settings each look is
            // traced with (event > cell > figure > the output's default;
            // « Auto » = `mixer.optimizer`), before the mixer and the
            // whole safety chain.
            let look_scan: Vec<optimize::OptimizeParams> = looks.iter().map(|(_, id, look)| scan::for_look(&s, *id, look)).collect();
            let show_scan: Vec<optimize::OptimizeParams> = show_cues.iter().map(|(cue, look)| scan::for_event(&s, cue, look)).collect();
            // Launch beats, so a cue's beat-synced motion counts from its start.
            let starts: HashMap<u64, f64> = s.deck.active.iter().map(|a| (a.id, a.started_beat)).collect();
            // Speed groups (T-419): each look's (cell > layer > default),
            // and each layer's for its own modifiers.
            let look_groups: Vec<u8> = looks.iter().map(|(layer, id, _)| s.look_speed_group(*layer, *id)).collect();
            let layer_groups: [u8; layers::LAYER_COUNT] = std::array::from_fn(|i| speed::resolve(None, s.mixer.layer(i as u8 + 1).speed_group));
            // User cues stored with master modifiers (T-405): theirs, per cue.
            let mut cue_masters: HashMap<u64, live::LiveModifiers> = s
                .deck
                .active
                .iter()
                .chain(s.deck.ending.iter().map(|e| &e.cue))
                .filter_map(|a| Some((a.id, usercues::master_of(&s, &a.cue)?)))
                .collect();
            cue_masters.extend(fx_cue);
            // Per-part modifiers of the figure cues on show (T-441): each
            // cell's stored ones, moved by the part modulators (copies, like
            // every modulation), per cue instance. Empty without parts.
            let part_mods = {
                let on_show: Vec<&cues::ActiveCue> = s.deck.active.iter().chain(s.deck.ending.iter().map(|e| &e.cue)).collect();
                let mut by_cue: lfo::PartMods = on_show
                    .iter()
                    .filter_map(|a| {
                        let stored = &s.deck.slots.get(&a.cue).filter(|slot| !slot.part_modifiers.is_empty())?.part_modifiers;
                        // In range whatever the cell holds, like the cue level's.
                        Some((a.cue.clone(), stored.iter().map(|(id, m)| (*id, levels::for_stage(m))).collect()))
                    })
                    .collect();
                if s.lfos.list().iter().any(|m| m.part.is_some()) {
                    let is_figure = |cue: &str| cue.starts_with(figures::ID_PREFIX) && on_show.iter().any(|a| a.cue == cue);
                    lfo::modulate_parts(s.lfos.list(), &mut by_cue, &is_figure, mod_t, &|g| speed.beat(g, mod_beat, mod_t + speed_off), time_share);
                }
                on_show.iter().filter_map(|a| Some((a.id, by_cue.get(&a.cue)?.clone()))).collect::<HashMap<u64, BTreeMap<u8, live::LiveModifiers>>>()
            };
            // The cue and layer levels (T-144): working copies follow the
            // deck, then this frame's values, smoothed.
            levels::frame(&mut s);
            let levels = self.smoothers.frame(&s, dt);
            let armed = s.gate.is_armed();
            let mut routing = Routing::read(&s, &looks, &show_cues);
            // A zone group flashed or blacked by an executor (T-418), held.
            executors::group_overrides(&s, &mut routing.config);
            // Zone chasers (T-170): each look's zones narrowed to the active
            // group(s) on the tempo clock (frozen with « Figer »), steps
            // bounded by the strobe limit. Routing only: the mixer, group
            // masters, geometry, calibration and safety stage follow.
            let chase_beat = self.frozen.map_or(clock.beat, |(_, beat)| beat);
            let chase = zonechase::ChaseNow::at(&routing.config, chase_beat, clock.bpm, s.safety.get().strobe_max_hz);
            routing.chase(&chase);
            s.zone_chase = chase;
            let caps = s.outputs.active_limits();
            // The ShowNET SD card over Art-Net (T-301) plays the cue's bank
            // number itself: it needs the cue, not only the points.
            let cue_signal = (s.output_kind == Some(artnet::KIND)).then(|| {
                let timeline_cue = show_cues.iter().rev().find_map(|(c, _)| match &c.source {
                    timeline::EventSource::Cue { id } => Some(id.as_str()),
                    timeline::EventSource::Look { .. } | timeline::EventSource::Dmx { .. } => None,
                });
                // The cue on show after transitions (T-158) and the beat
                // mode; a launch still waiting (T-159) is not on show.
                artnet::cue_signal(&s, s.deck.on_show(t), &live, timeline_cue, pattern.is_some() || trace.is_some(), &caps)
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
                            timeline::EventSource::Look { .. } | timeline::EventSource::Dmx { .. } => None,
                        });
                        artnet::cue_signal(&s, s.deck.on_show(t), &live, timeline_cue, pattern.is_some() || trace.is_some(), &cfg.limits)
                    });
                    let fixed = cfg.scan.fixed_target(s.pps, s.mixer.optimizer.lit_step);
                    ProjectorPlan { id: p.id.clone(), output: p.output.clone(), caps: cfg.limits, color: cfg.color, frame_limits: cfg.scan.frame_limits(), fixed, cue_signal }
                })
                .collect();
            let festival = (s.festival.colors(), s.festival.balance);
            // An ILDA recording (T-442) wants this frame: copied below,
            // before the safety chain; nothing else changes.
            // The projector it records (T-478): its frame, not the main one's.
            record_due = s.recorder.due(t).then(|| s.recorder.output().to_string());
            (looks, (transition_fx, exec_gains), show_cues, (starts, look_groups, layer_groups, speed, speed_off), (cue_masters, levels, part_mods, zone_live), clock, mixer, s.calibration, audio, armed, hold_ok, live, s.palettes.list().to_vec(), Arc::clone(&s.estop), Arc::clone(&s.health), t, s.effective_safety(), (caps, s.outputs.active().scan.frame_limits(), s.outputs.active().scan.fixed_target(s.pps, s.mixer.optimizer.lit_step)), s.outputs.active_color(), pattern, routing, cue_signal, plans, festival, (look_scan, show_scan), trace)
        };

        // « Figer »: no time passes for the animators and the rotations,
        // and the looks see the beat it was pressed on.
        let (live_dt, frozen) = if self.frozen.is_some() { (0.0, true) } else { (dt, false) };
        let render_clock = match self.frozen {
            Some((_, beat)) => engine::BeatClock { beat, ..clock },
            None => clock,
        };
        self.live_state.advance(&live, live_dt, clock.bpm, clock.beats_per_bar);
        // Speed groups (T-419): every group's clock this frame, and while
        // « Figer » is held the (seconds, beat) each one stood still at.
        // The master stage, the gate and the safety chain keep real time.
        let group_clocks = speed.clocks(&render_clock, self.frozen.map_or(t, |(ft, _)| ft) + speed_off);
        let group_frozen: [Option<(f64, f64)>; speed::SPEED_GROUPS + 1] =
            std::array::from_fn(|g| self.frozen.map(|(ft, fb)| (ft, speed.beat(g as u8, fb, ft + speed_off))));
        // The live clock of a group: frozen looks keep their beat, the
        // rotations and colours of a level read the group's real beat.
        let live_clocks = speed.clocks(&clock, t + speed_off);
        // Spread looks (T-421): an e-stop (the blackout is one), a « Noir »
        // or a disarm empties every delay memory, whatever stands in for the
        // looks this frame; looks no longer playing lose theirs.
        self.spread.frame(estop.is_latched() || live.black, armed, |id| {
            looks.iter().zip(&routing.look_spreads).any(|((_, i, _), sp)| *i == id && sp.is_some()) || show_cues.iter().zip(&routing.show_spreads).any(|((c, _), sp)| c.instance == id && sp.is_some())
        });
        let (mut frame, mut extra_frames, mix, evolving) = match (trace, pattern) {
            // The zone trace (T-447): the outlines, already in output
            // coordinates (no look, no live stage, no calibration), at
            // their fixed low level; the whole safety stage and the gate
            // below apply to it. Every projector of the venue shows it.
            (Some(only), _) => {
                let frame = zones::trace_frame(&safety_cfg.zones, safety_cfg.horizon, only);
                (frame.clone(), vec![frame; plans.len()], layers::MixReport::default(), Vec::new())
            }
            // The pattern alone, densified and calibrated, without live
            // modifiers (exact geometry). The cue animators are left as
            // they are, so the cues carry on where they were afterwards.
            // While aligning a projection zone, through that zone's geometry.
            // Every projector of the venue shows it (T-277).
            (None, Some((p, brightness))) => {
                let frame = testpattern::output_frame(p, brightness, routing.align.as_ref(), &calibration);
                (frame.clone(), vec![frame; plans.len()], layers::MixReport::default(), Vec::new())
            }
            (None, None) => {
                let anim_dt = live_dt * live.speed.clamp(0.0, 4.0);
                let (animators, event_live, cue_live, frozen_events) = (&mut self.animators, &mut self.event_live, &mut self.cue_live, &mut self.frozen_events);
                let frozen_at = self.frozen;
                // The manual look (id 0) keeps its animator while a cue
                // hides it (T-360): a flash released goes back to the look
                // where it was, instead of a fresh start each time (an
                // evolving look would wait on its first frame for the next
                // beat after every release, never moving on).
                let look_hidden = looks.iter().any(|(_, i, _)| *i != 0);
                // Cue → layer → master (T-144): the layers' rotations move
                // once a frame, every look of a layer shares them.
                let (cue_level_live, layer_live) = (&mut self.cue_level_live, &mut self.layer_live);
                let layer_clocks = layer_groups.map(|g| live_clocks[g as usize]);
                let layer_frozen = layer_groups.map(|g| group_frozen[g as usize]);
                levels.advance_layers(layer_live, t, &layer_clocks, &layer_frozen, live_dt);
                cue_level_live.retain(|id, _| levels.cues.contains_key(id));
                let layer_live = &*layer_live;
                animators.retain(|id, _| looks.iter().any(|(_, i, _)| i == id) || show_cues.iter().any(|(c, _)| c.instance == *id) || (*id == 0 && look_hidden));
                event_live.retain(|id, _| show_cues.iter().any(|(c, _)| c.instance == *id));
                cue_live.retain(|id, _| cue_masters.contains_key(id));
                let spread_state = &mut self.spread;
                let spread_clock = (t, clock.beat, clock.bpm, clock.beats_per_bar);
                // Timeline events first (under the cues played by hand on the same
                // layer), each on its own content clock; paused or held ones freeze.
                let mut rendered: Vec<projection::Routed> = show_cues
                    .iter()
                    .zip(&routing.show_zones)
                    .zip(&show_scan)
                    .zip(&routing.show_spreads)
                    .flat_map(|((((cue, settings), zones), scan), spread)| {
                        // An event met mid-way (a seek, a timecode jump, T-409)
                        // starts with the motion it would have had playing from
                        // its start: its first step is all the time elapsed.
                        let fresh = !animators.contains_key(&cue.instance);
                        let animator = animators.entry(cue.instance).or_insert_with(|| Animator::starting_at(0.0));
                        let content_beat = if frozen { *frozen_events.entry(cue.instance).or_insert(cue.content_beat) } else { cue.content_beat };
                        let content_clock = engine::BeatClock { beat: content_beat, ..clock };
                        let layer_speed = levels.speed(None, cue.layer);
                        let event_dt = match (fresh, cue.frozen || frozen) {
                            (true, _) => cue.elapsed_s as f32 * live.speed.clamp(0.0, 4.0) * layer_speed,
                            (false, true) => 0.0,
                            (false, false) => anim_dt * layer_speed,
                        };
                        animator.set_optimizer(scan);
                        animator.set_palette(festival_palette);
                        animator.set_point_budget(mixer.point_budget);
                        let points = animator.render(settings, audio, event_dt, &content_clock);
                        // A spread (T-421): the zones of its group drawn on
                        // their own, late or at their own speed, from the raw
                        // content; every stage below runs on each of them.
                        let plan = spread::plan(spread.as_ref(), &routing.config, zones);
                        let variants = match spread {
                            Some(sp) if !plan.is_empty() => spread_state.variants(cue.instance, sp, &plan, &points, spread_clock, |a, _, k| {
                                a.set_optimizer(scan);
                                a.set_palette(festival_palette);
                                a.set_point_budget(mixer.point_budget);
                                a.render(settings, audio, event_dt * k, &engine::BeatClock { beat: content_beat * k as f64, ..clock })
                            }),
                            _ => Vec::new(),
                        };
                        let own = cue.modifiers != live::LiveModifiers::default();
                        if own {
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
                        }
                        // Its layer's modifiers (T-144), the event's own colour first.
                        let coloured = cue.modifiers.color != live::ColorOverride::Normal;
                        let stage = |mut points: Vec<Point>| {
                            if let Some(st) = event_live.get(&cue.instance).filter(|_| own) {
                                points = live::apply(&points, &cue.modifiers, st, &user_palettes);
                            }
                            levels.layer_stage(cue.layer, points, coloured, layer_live, &user_palettes)
                        };
                        let rest = spread::rest(&routing.config, zones, &plan);
                        let mut out = vec![(cue.layer, stage(points), rest)];
                        out.extend(variants.into_iter().map(|(zone, raw)| (cue.layer, stage(raw), vec![zone])));
                        out
                    })
                    .collect();
                // Muted layers keep animating, so they come back in motion.
                let look_groups_seen = &mut self.look_groups;
                look_groups_seen.retain(|id, _| looks.iter().any(|(_, i, _)| i == id));
                let mut cue_looks: Vec<transition::Rendered<(u8, Vec<u8>)>> = looks.iter().zip(&routing.look_zones).zip(&look_scan).zip(&look_groups).zip(&routing.look_spreads).flat_map(|(((((layer, id, settings), zones), scan), &group), spread)| {
                    // A look moved to another speed group starts over on
                    // that group's beat line (a phase jump, never a freeze).
                    if look_groups_seen.insert(*id, group).is_some_and(|before| before != group) {
                        animators.remove(id);
                    }
                    let (render_clock, clock, frozen_at) = (group_clocks[group as usize], live_clocks[group as usize], group_frozen[group as usize]);
                    let animator = animators.entry(*id).or_insert_with(|| match starts.get(id).map(|&beat| speed.start_beat(group, beat)) {
                        Some(Some(beat)) => Animator::starting_at(beat),
                        _ => Animator::default(),
                    });
                    animator.set_optimizer(scan);
                    animator.set_palette(festival_palette);
                    animator.set_point_budget(mixer.point_budget);
                    // A figure cue's parts (T-441), before everything else.
                    animator.set_parts(part_mods.get(id), &user_palettes);
                    let look_dt = anim_dt * levels.speed(Some(*id), *layer);
                    let points = animator.render(settings, audio, look_dt, &render_clock);
                    // A spread (T-421): the zones of its group drawn on their
                    // own, late or at their own speed, from the raw content;
                    // every stage below (and the transitions, the executors,
                    // the masters, the safety stage) runs on each of them.
                    let plan = spread::plan(spread.as_ref(), &routing.config, zones);
                    let variants = match spread {
                        Some(sp) if !plan.is_empty() => spread_state.variants(*id, sp, &plan, &points, (t, render_clock.beat, render_clock.bpm, render_clock.beats_per_bar), |a, start, k| {
                            a.set_optimizer(scan);
                            a.set_palette(festival_palette);
                            a.set_point_budget(mixer.point_budget);
                            a.set_parts(part_mods.get(id), &user_palettes);
                            let beat = start + (render_clock.beat - start) * k as f64;
                            a.render(settings, audio, look_dt * k, &engine::BeatClock { beat, ..render_clock })
                        }),
                        _ => Vec::new(),
                    };
                    // A user cue's own master modifiers (T-405), before the
                    // master stage, like a timeline event's.
                    let mut coloured = false;
                    if let Some(m) = cue_masters.get(id) {
                        let st = cue_live.entry(*id).or_default();
                        st.set_clock(t, clock.beat);
                        st.freeze_colours(frozen_at);
                        st.advance(m, live_dt, clock.bpm, clock.beats_per_bar);
                        coloured = m.color != live::ColorOverride::Normal;
                    }
                    // Then the cue level and the layer level (T-144), still
                    // before the transitions, the mixer, the master stage
                    // and the whole safety chain.
                    if let Some(m) = levels.cues.get(id) {
                        let st = cue_level_live.entry(*id).or_default();
                        st.set_clock(t, clock.beat);
                        st.freeze_colours(frozen_at);
                        st.advance(m, live_dt, clock.bpm, clock.beats_per_bar);
                        coloured |= m.color != live::ColorOverride::Normal;
                    }
                    let stage = |mut points: Vec<Point>| {
                        if let (Some(m), Some(st)) = (cue_masters.get(id), cue_live.get(id)) {
                            points = live::apply(&points, m, st, &user_palettes);
                        }
                        if let (Some(m), Some(st)) = (levels.cues.get(id), cue_level_live.get(id)) {
                            points = live::apply(&points, m, st, &user_palettes);
                        }
                        levels.layer_stage(*layer, points, coloured, layer_live, &user_palettes)
                    };
                    let rest = spread::rest(&routing.config, zones, &plan);
                    let mut out = vec![(*id, stage(points), (*layer, rest))];
                    out.extend(variants.into_iter().map(|(zone, raw)| (*id, stage(raw), (*layer, vec![zone]))));
                    out
                }).collect();
                // Transitions between cues (T-158): fades scale colours by
                // at most 1, a morph never lights travel (transition.rs);
                // the mixer, live stage, zones, calibration and the whole
                // safety stage still come after.
                transition::apply(&mut cue_looks, &transition_fx);
                // Executor levels (T-418): a gain of at most 1 on each
                // cue's colours, still before the mixer, the master stage
                // and the whole safety chain.
                for (id, points, _) in cue_looks.iter_mut() {
                    if let Some(&g) = exec_gains.get(id) {
                        transition::scale(points, g);
                    }
                }
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
                // A zone moved by a spread modulator (T-421) uses its own
                // copy of the master modifiers.
                let finish = |zone: u8, look: &[Point]| {
                    let live = zone_live.iter().find(|(z, _)| *z == zone).map_or(&live, |(_, l)| l);
                    let mut out = match routing.remotes.iter().find(|(z, _)| *z == zone) {
                        Some((_, remote)) => dmx::fixture::apply(look, remote, live, &self.live_state, &user_palettes, t),
                        None => live::apply(look, live, &self.live_state, &user_palettes),
                    };
                    balance.apply(&mut out);
                    out
                };
                // Each projector of the venue (T-277) draws its own zones
                // (`ProjectionConfig::zones_for`, the rule the exports use
                // too, T-478), from the same rendered looks, through the
                // same geometry and calibration. The mix report is the
                // main one's. A lone projector draws `active` of its output.
                let main = (power::MAIN_OUTPUT, routing.output.as_deref());
                let all: Vec<(&str, Option<&str>)> = std::iter::once(main).chain(plans.iter().map(|p| (p.id.as_str(), p.output.as_deref()))).collect();
                let draw = |me: usize| projection::render_zones(&rendered, &routing.config, routing.config.zones_for(all[me].0, &all), &mixer, finish, &calibration);
                let (frame, mix) = draw(0);
                let extra = (1..all.len()).map(|i| draw(i).0).collect();
                (frame, extra, mix, evolving)
            }
        };
        // « Noir » held (T-142): every projector's frame dark, a test
        // pattern's too (the master stage already darkened the looks).
        // Arming is untouched, and the safety stage still follows.
        if live.black {
            live::black_out(&mut frame);
            extra_frames.iter_mut().for_each(|f| live::black_out(f));
        }
        // An ILDA recording's copy (T-442): the mixed, master-staged and
        // calibrated frame, before the safety stage, which the recorder's
        // I/O thread runs itself with its own state (record.rs). Only when
        // an image is due; arming and the output are untouched.
        // The projector the recording is for (T-478): its own zones were
        // drawn above; one the engine does not draw this frame (it left
        // the venue) gets a dark image, never another projector's.
        let recorded = record_due.map(|target| {
            if target == power::MAIN_OUTPUT {
                frame.clone()
            } else {
                match plans.iter().position(|p| p.id == target) {
                    Some(i) => extra_frames[i].clone(),
                    None => vec![Point::blanked(0.0, 0.0)],
                }
            }
        });
        let safety_at = measuring.then(Instant::now);
        // The safety stage (`safety_chain`): frame size, colour rendering,
        // strobe limiter and horizon, static beam guard, power caps, the
        // limiter's check on the output.
        // Fixed rate (T-463): the output's seam is made constant first.
        let seam = self.stage.fixed_seam(fixed.is_some());
        if fixed.is_none() && self.fixed.is_on() {
            self.fixed.clear();
        }
        let chain = fixed.map(|target| FixedChain { target, seam, state: &mut self.fixed });
        let frame = safety_chain(frame, frame_limits, self.stage.max_points(), &color, t, &safety_cfg, &mut self.limiter, &mut self.dwell_guard, armed, &caps, chain);
        let output_at = measuring.then(Instant::now);

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
        // The frame's timings, recorded and summed up before the lock
        // below, which only copies the report (no allocation either way).
        let report = match (&mut self.perf, started, safety_at, output_at) {
            (Some(monitor), Some(start), Some(safety_at), Some(output_at)) => {
                let done = Instant::now();
                monitor.record(safety_at - start, output_at - safety_at, done - output_at, done - start, frame.len());
                Some(monitor.report(0))
            }
            _ => None,
        };
        // The rate measured on what went out (seam included).
        let fixed_status = fixed.map(|target| {
            let sent = self.stage.max_points().map_or(frame.len(), |m| frame.len().min(m));
            self.fixed.record(t, sent + seam, target.pps);
            self.fixed.status()
        });
        let mut s = shared.lock().unwrap();
        s.fixed_rate = fixed_status;
        if let Some(report) = report {
            s.perf = report;
        }
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
        if let Some(copy) = recorded {
            s.recorder.offer(copy, t);
        }
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
            // The outdoor cut (T-262) follows this projector's own mounting;
            // without one it cuts its whole field while the mode is on.
            let mut cfg = safety_cfg.clone();
            cfg.outdoor.projector = plan.id.clone();
            let seam = st.stage.fixed_seam(plan.fixed.is_some());
            if plan.fixed.is_none() && st.fixed.is_on() {
                st.fixed.clear();
            }
            let chain = plan.fixed.map(|target| FixedChain { target, seam, state: &mut st.fixed });
            let frame = safety_chain(frame, plan.frame_limits, st.stage.max_points(), &plan.color, t, &cfg, &mut st.limiter, &mut st.dwell_guard, armed, &plan.caps, chain);
            if let Some(target) = plan.fixed {
                let sent = st.stage.max_points().map_or(frame.len(), |m| frame.len().min(m));
                st.fixed.record(t, sent + seam, target.pps);
            }
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
        save_dirty(&mut shared.lock().unwrap_or_else(|e| e.into_inner()), &self.live_path, &self.layers_path);
        // Everything queued, these included, is on disk before the studio exits.
        datafile::flush();
    }
}

/// Queues the stores the engine saves (live, speed groups, layers, group
/// masters, FX grid) that changed, and marks them saved. Serialised here,
/// under the lock; written by `datafile`'s writer thread (T-482), so a
/// slow disk never holds a frame.
fn save_dirty(s: &mut Shared, live_path: &std::path::Path, layers_path: &std::path::Path) {
    if std::mem::take(&mut s.live_dirty) {
        datafile::save_json_later(live_path, &s.live);
    }
    s.save_speed();
    if std::mem::take(&mut s.mixer_dirty) {
        datafile::save_json_later(layers_path, &s.mixer);
    }
    if std::mem::take(&mut s.groups_dirty) {
        s.projection.save_later();
    }
    if s.fx.dirty {
        s.fx.save_later();
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
    /// The zone chaser tied to each look's cue (T-170), same order.
    look_chasers: Vec<Option<usize>>,
    show_chasers: Vec<Option<usize>>,
    /// Each look's spread over a zone group (T-421), same order: the
    /// cell's; an event's own, else its cue's cell's.
    look_spreads: Vec<Option<spread::Spread>>,
    show_spreads: Vec<Option<spread::Spread>>,
    config: projection::ProjectionConfig,
    output: Option<String>,
    align: Option<projection::Geometry>,
    /// Each zone's DMX remote this frame (T-407); none: the master only.
    remotes: Vec<(u8, dmx::personality::ZoneRemote)>,
}

impl Routing {
    fn read(s: &Shared, looks: &[(u8, u64, Settings)], show_cues: &[(timeline::TimelineCue, Settings)]) -> Self {
        let of_cue = |cue: &str| {
            let slot = s.deck.slot(cue);
            (slot.zones, slot.zone_chaser)
        };
        let (look_zones, look_chasers) = looks
            .iter()
            .map(|(_, id, _)| s.deck.find(*id).map(|a| of_cue(&a.cue)).unwrap_or_default())
            .unzip();
        let (show_zones, show_chasers) = show_cues
            .iter()
            .map(|(cue, _)| match &cue.source {
                timeline::EventSource::Cue { id } => of_cue(id),
                timeline::EventSource::Look { .. } | timeline::EventSource::Dmx { .. } => (Vec::new(), None),
            })
            .unzip();
        let look_spreads = looks.iter().map(|(_, id, _)| s.deck.find(*id).and_then(|a| s.deck.slot(&a.cue).spread)).collect();
        let show_spreads = show_cues
            .iter()
            .map(|(cue, _)| match &cue.source {
                _ if cue.spread.is_some() => cue.spread.clone(),
                timeline::EventSource::Cue { id } => s.deck.slot(id).spread,
                timeline::EventSource::Look { .. } | timeline::EventSource::Dmx { .. } => None,
            })
            .collect();
        Self {
            look_zones,
            show_zones,
            look_chasers,
            show_chasers,
            look_spreads,
            show_spreads,
            config: s.projection.get().clone(),
            output: s.output_mgr.prefs().selected,
            // A phone's alignment draft (T-460) wins over « Aligner ».
            align: remote::align_geometry(s).or_else(|| s.projection.align_geometry()),
            remotes: s.netin.fixtures.remotes.clone(),
        }
    }
}

impl Routing {
    /// Every look's zones through the zone chasers of this frame (T-170).
    fn chase(&mut self, now: &zonechase::ChaseNow) {
        for (zones, own) in self.look_zones.iter_mut().zip(&self.look_chasers).chain(self.show_zones.iter_mut().zip(&self.show_chasers)) {
            *zones = now.route(&self.config, zones, *own);
        }
    }
}

fn timeline_cues(s: &mut Shared, t: f64, clock: &engine::BeatClock) -> Vec<(timeline::TimelineCue, Settings)> {
    let c = timeline::Clock { t, beat: clock.beat, bpm: clock.bpm, beats_per_bar: clock.beats_per_bar };
    if s.estop.is_latched() {
        s.timeline.halt(&c);
    }
    // The show list (T-444): at the end of a show, the next one (ready)
    // starts in this same frame, before its song is told.
    showlist::frame(s, &c);
    let song = Arc::clone(&s.song);
    s.song_sync.sync(&song, &mut s.timeline, &c);
    // Incoming timecode (T-168) has the last word on the playhead.
    timecode::frame(s, &c);
    let cues = s.timeline.frame(&c);
    // The live take (T-169) observes the deck and the masters, after the
    // playhead moved. Reads only.
    take::frame(s, &c);
    // The timecode output (T-417) reads the playhead it ended on.
    timecode::out::frame(s, &c);
    // The DMX output (T-453): its DMX tracks and the haze panel.
    dmx_out::frame(s, &c);
    // A cue event plays the cue's grid program too (T-157).
    let with_program = |cue: &timeline::TimelineCue, look: Settings| match &cue.source {
        timeline::EventSource::Cue { id } => s.deck.with_program(id, look),
        timeline::EventSource::Look { .. } | timeline::EventSource::Dmx { .. } => look,
    };
    cues
        .into_iter()
        .filter_map(|mut cue| {
            let mut look = with_program(&cue, timeline::look_of(&cue, &s.presets)?);
            // Referenced presets (T-422): every frame, so a preset changed
            // during playback moves the event at the next frame.
            if !cue.refs.is_empty() {
                cue.modifiers = pools::resolve(&cue.refs, &s.pools, &cue.modifiers);
            }
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
        let zone = zones::Zone { id: 1, name: "Public".into(), kind: zones::ZoneKind::Dim, level: 0.1, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 0.0], [-1.0, 0.0]], audience: true, ..Default::default() };
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
        // The venue's safety profile allows it (T-260), confirmed.
        let allow = profiles::SaveRequest { name: "Club".into(), venue: None, notes: None, audience_scan_allowed: Some(true) };
        profiles::save(&mut s, allow, true, interlock::ArmSource::Ui).unwrap();
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
        assert_eq!(s.unlock_audience_scanning(PHRASE, interlock::ArmSource::Ui).unwrap_err(), UnlockError::Missing(vec![s.audience_conditions().iter().find(|c| c.id == "checklist").unwrap().label_fr.to_string()]));

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

    /// T-444: the show list chains two shows through the engine's
    /// timeline step, the second one's events reach the frame, and neither
    /// the chaining nor an e-stop ever changes the arm state.
    #[test]
    fn the_show_list_chains_through_the_frame_without_touching_the_arm_state() {
        let mut s = test_support::shared();
        let cue = s.presets[0].id.clone();
        let dir = std::env::temp_dir().join(format!("laser-studio-main-showlist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        s.shows = timeline::ShowStore::new(dir.join("shows"));
        for (name, len) in [("Un", 1.0), ("Deux", 5.0)] {
            let mut show = one_event_show(&cue);
            show.name = name.into();
            show.tracks[0].events[0].len = len;
            s.shows.save(&show).unwrap();
        }
        let list = showlist::ShowList { items: ["Un", "Deux"].iter().map(|n| showlist::ShowListItem { show: n.to_string(), ..Default::default() }).collect(), loop_list: false };
        showlist::set(&mut s, list).unwrap();
        let clock = |t: f64| engine::BeatClock { beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 };
        for armed in [false, true] {
            if armed {
                controls::apply(&mut s, "transport.arm", controls::ControlInput::Value(1.0), false).unwrap();
            }
            let t0 = s.now_s();
            s.sim_now = Some(s.epoch + std::time::Duration::from_secs_f64(t0));
            showlist::play(&mut s, Some(0)).unwrap();
            let mut t = t0;
            let mut last_one = None;
            while t < t0 + 1.5 {
                s.sim_now = Some(s.epoch + std::time::Duration::from_secs_f64(t));
                let cues = timeline_cues(&mut s, t, &clock(t));
                assert_eq!(cues.len(), 1, "a frame without a show at {:.3} s", t - t0);
                if s.timeline.show.as_ref().unwrap().name == "Un" {
                    last_one = Some(t);
                }
                assert_eq!(s.gate.is_armed(), armed, "the list never changes the arm state");
                t += 1.0 / 60.0;
            }
            assert_eq!(s.timeline.show.as_ref().unwrap().name, "Deux");
            assert!(last_one.is_some_and(|l| l > t0 + 0.9));
            let gap = showlist::summary(&s).gap_ms.unwrap();
            assert!(gap < 100.0, "gap {gap} ms");
        }
        s.emergency_stop(interlock::ArmSource::Keyboard);
        let t = s.now_s() + 2.0;
        assert!(timeline_cues(&mut s, t, &clock(t)).is_empty());
        assert_eq!(s.showlist.transport, showlist::ListTransport::Stopped);
        assert!(!s.gate.is_armed());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-419: the *Cadence* scales the fades played live, never a
    /// timeline's: the same show at 50 % and 200 % gives the same gains.
    #[test]
    fn timeline_fades_ignore_the_rate_master() {
        let gains = |rate: f32| {
            let mut s = test_support::shared();
            let cue = s.presets[0].id.clone();
            let mut show = one_event_show(&cue);
            show.tracks[0].events[0].fade_in = timeline::Dur::Seconds(2.0);
            s.timeline.load(show);
            s.speed.set_rate(rate).unwrap();
            s.sim_now = Some(s.epoch);
            controls::timeline_play(&mut s).unwrap();
            (1..=5)
                .map(|k| {
                    let t = k as f64 * 0.4;
                    let cues = timeline_cues(&mut s, t, &engine::BeatClock { beat: t * 2.0, bpm: 120.0, beats_per_bar: 4 });
                    cues[0].0.gain
                })
                .collect::<Vec<f32>>()
        };
        let slow = gains(0.5);
        assert!(slow[0] < 1.0, "the fade is on: {slow:?}");
        assert_eq!(slow, gains(2.0));
    }

    /// T-419: the registry's speed controls drive the groups (external,
    /// read back for MIDI), NaN is refused, the *Cadence* is clamped, and
    /// `tempo.resync` brings the multiple groups back on the "one".
    #[test]
    fn speed_controls_drive_the_groups() {
        use controls::{apply, ControlInput};
        use speed::SpeedMode;
        let mut s = test_support::shared();
        for n in 1..=4 {
            for p in ["mult", "free", "bpm"] {
                assert!(s.controls.get(&format!("speed.{n}.{p}")).is_some_and(|d| d.external), "speed.{n}.{p}");
            }
        }
        assert!(s.controls.get("master.rate").is_some_and(|d| d.external));
        s.speed.change_on_bar = false;
        apply(&mut s, "speed.1.mult", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.speed.groups[0], SpeedMode::Multiple { mult: 0.5 });
        let desc = s.controls.get("speed.1.mult").unwrap().clone();
        assert_eq!(controls::current(&s, &desc), Some(serde_json::json!(1)));
        apply(&mut s, "speed.2.bpm", ControlInput::Value(98.0), true).unwrap();
        assert_eq!(s.speed.groups[1], SpeedMode::FreeBpm { bpm: 98.0 });
        apply(&mut s, "speed.2.free", ControlInput::Value(0.0), true).unwrap();
        assert_eq!(s.speed.groups[1], SpeedMode::Multiple { mult: 1.0 });
        apply(&mut s, "speed.3.free", ControlInput::Value(1.0), true).unwrap();
        assert_eq!(s.speed.groups[2], SpeedMode::FreeBpm { bpm: 120.0 });
        for bad in [f32::NAN, f32::INFINITY] {
            assert!(apply(&mut s, "master.rate", ControlInput::Value(bad), true).is_err());
            assert!(apply(&mut s, "speed.4.bpm", ControlInput::Value(bad), true).is_err());
        }
        apply(&mut s, "master.rate", ControlInput::Value(5.0), true).unwrap();
        assert_eq!(s.speed.rate(), 2.0);
        apply(&mut s, "master.rate", ControlInput::Norm(0.0), true).unwrap();
        assert_eq!(s.speed.rate(), 0.5);
        let rate = s.controls.get("master.rate").unwrap().clone();
        assert_eq!(controls::current(&s, &rate), Some(serde_json::json!(0.5)));
        assert_eq!(s.live, live::LiveModifiers::default(), "master.rate is not a live modifier");
        // Resync: the master's "1" is group A's "1" too.
        s.sim_now = Some(s.epoch + Duration::from_millis(2_300));
        apply(&mut s, "tempo.resync", ControlInput::Value(1.0), true).unwrap();
        let t = s.now_s();
        let (bar, g) = (s.tempo.beat_at(t), s.speed.beat(1, s.tempo.beat_at(t), t));
        assert_eq!((bar % 4.0, g % 4.0), (0.0, 0.0));
        assert!(!s.gate.is_armed(), "no speed control arms");
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

    /// A `Shared` with one Blank zone (id 1).
    fn with_a_zone(mut s: Shared) -> Shared {
        let zone = zones::Zone { id: 1, points: vec![[-0.2, -0.2], [0.2, -0.2], [0.2, 0.2], [-0.2, 0.2]], ..Default::default() };
        s.safety.set(safety::SafetySettings { zones: vec![zone], ..Default::default() }, true).unwrap();
        s
    }

    /// T-447: the zone trace pauses the timeline and leaves the deck alone,
    /// ends by itself after 60 s (simulated), and everything resumes.
    #[test]
    fn a_zone_trace_pauses_the_timeline_and_ends_after_60_s() {
        let mut s = with_a_zone(test_support::shared());
        assert!(controls::press_cue(&mut s, "tunnels-001", Some(ClickMode::Toggle), true));
        let cues_before: Vec<String> = s.deck.active.iter().map(|a| a.cue.clone()).collect();
        let cue = s.presets[0].id.clone();
        s.timeline.load(one_event_show(&cue));
        controls::timeline_play(&mut s).unwrap();
        let t0 = Instant::now();
        assert_eq!(zonetrace::start(&mut s, t0, Some(9), None), Err("zone inconnue : 9".into()));
        zonetrace::start(&mut s, t0, None, None).unwrap();
        assert!(!s.timeline.is_playing(), "paused under the trace");
        // Played again from elsewhere: held paused.
        controls::timeline_play(&mut s).unwrap();
        assert_eq!(zonetrace::hold(&mut s), Some(None));
        assert!(!s.timeline.is_playing());
        zonetrace::sync(&mut s, t0 + Duration::from_secs(zones::TRACE_SECONDS - 1));
        assert!(s.zone_trace.is_on());
        assert_eq!(zonetrace::view(&s, t0 + Duration::from_millis(16_500))["remaining_s"], 44);
        zonetrace::sync(&mut s, t0 + Duration::from_secs(zones::TRACE_SECONDS));
        assert!(!s.zone_trace.is_on(), "ends by itself");
        assert_eq!(zonetrace::hold(&mut s), None);
        assert!(s.timeline.is_playing(), "the timeline resumes");
        assert_eq!(s.deck.active.iter().map(|a| a.cue.clone()).collect::<Vec<_>>(), cues_before);
        assert!(!s.gate.is_armed());
        // One zone only, then that zone removed: the trace ends.
        zonetrace::start(&mut s, t0, Some(1), None).unwrap();
        s.safety.set(safety::SafetySettings::default(), true).unwrap();
        zonetrace::sync(&mut s, t0);
        assert!(!s.zone_trace.is_on());
    }

    /// T-447: the e-stop ends the trace at once and refuses a new one; a
    /// disarm while it runs ends it; starting it never arms.
    #[test]
    fn a_zone_trace_ends_on_the_estop_and_on_a_disarm_and_never_arms() {
        let mut s = with_a_zone(test_support::shared());
        let cue = s.presets[0].id.clone();
        s.timeline.load(one_event_show(&cue));
        controls::timeline_play(&mut s).unwrap();
        let t0 = Instant::now();
        zonetrace::start(&mut s, t0, None, None).unwrap();
        assert!(!s.gate.is_armed(), "never arms");
        s.emergency_stop(ArmSource::Keyboard);
        assert!(!s.zone_trace.is_on(), "Échap ends it at once");
        assert!(!s.timeline.is_playing(), "and nothing resumes");
        assert!(zonetrace::start(&mut s, t0, None, None).is_err(), "refused under the e-stop");
        s.gate.reset_estop(&s.estop.clone());
        // Armed by the operator, then disarmed: the trace ends.
        s.request_arm(ArmSource::Ui).unwrap();
        zonetrace::start(&mut s, t0, None, None).unwrap();
        zonetrace::sync(&mut s, t0);
        assert!(s.zone_trace.is_on() && s.gate.is_armed());
        s.gate.disarm(interlock::DisarmReason::User, ArmSource::Ui);
        zonetrace::sync(&mut s, t0);
        assert!(!s.zone_trace.is_on());
        // Started disarmed: it runs (preview only) and stays disarmed.
        zonetrace::start(&mut s, t0, None, None).unwrap();
        zonetrace::sync(&mut s, t0 + Duration::from_secs(1));
        assert!(s.zone_trace.is_on() && !s.gate.is_armed());
    }

    /// T-447: the page that started the trace stops beating: it ends.
    #[test]
    fn a_zone_trace_ends_when_its_page_is_gone() {
        let mut s = with_a_zone(with_presence(PresenceSettings::default()));
        let t0 = Instant::now();
        s.presence.beat("page", t0, true, false);
        s.presence.beat("other", t0, true, false);
        s.sync_safety(t0);
        zonetrace::start(&mut s, t0, None, Some("page")).unwrap();
        // The other page keeps beating, this one stops.
        for step in 1..=10u64 {
            let now = t0 + ms(step * 500);
            s.presence.beat("other", now, true, false);
            s.sync_safety(now);
            zonetrace::sync(&mut s, now);
        }
        assert!(!s.zone_trace.is_on(), "ended with its page");
    }

    /// T-447: nothing in the control registry (MIDI, DMX, OSC, timeline,
    /// macros) reaches the trace, and the route is a safety route (refused
    /// by the tablet remote).
    #[test]
    fn the_zone_trace_is_no_control_and_a_safety_route() {
        let s = test_support::shared();
        assert!(!s.controls.list().iter().any(|d| d.id.contains("zone_trace") || d.id.contains("safety.trace") || d.id.contains("zones.trace")));
        assert!(web::ROUTES.iter().any(|(m, p, c)| *m == "POST" && *p == "/api/safety/trace" && *c == web::RouteClass::Safety));
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
            let server = web::bind("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();
            let estop = Arc::clone(&shared.lock().unwrap().estop);
            let hub = Arc::clone(&shared.lock().unwrap().push);
            let web = std::thread::spawn({
                let (shared, running) = (Arc::clone(shared), Arc::clone(running));
                move || web::serve(server, shared, estop, crate::test_support::scratch_dir().join("calibration.json"), running)
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
        // Nothing of the connection is written at start; a fresh install
        // only gets the « Aperçu sûr » caps (T-260).
        let written: power::OutputsFile = load_json(&dir.join("outputs.json"));
        assert_eq!(written.connection, output_manager::OutputPrefs::default(), "no connection written at start");
        assert_eq!(s.outputs.active_limits().max_power, profiles::SAFE_PREVIEW_POWER);
        assert_eq!(s.profiles.active().unwrap().name, profiles::SAFE_PREVIEW);
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
        for name in ["presence.json", "live.json", "layers.json", "grid.json", "calibration.json", "scenes.json", "lfos.json", "audio_routes.json", "audio_triggers.json", "palettes.json", "fx.json"] {
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
