//! Recording what plays, or a whole timeline, as an ILDA animation (T-442).
//!
//! Two sources, one file format (ILDA format 5, `ilda::write_format5`):
//!
//! - **Live** (`RecordSource::Live`): `n` seconds of what the engine
//!   computes (cues, layers, master modifiers, modulators, audio reaction).
//!   At every deadline `k / fps` the engine thread hands the frame it just
//!   computed to the `Recorder` (`due` then `offer`): at most one frame a
//!   tick, so engine jitter never skips nor doubles an image, and the count
//!   is exactly `frame_count(seconds, fps)`. The engine only copies a frame
//!   when one is due; everything else happens on an I/O thread.
//! - **Timeline** (`RecordSource::Timeline`): a show rendered offline on the
//!   I/O thread with a simulated clock (`render_timeline`), its own
//!   `Player` and its own animators: never the live ones, never `Shared`.
//!   The music is not analysed: audio reaction is neutral. Same show, same
//!   settings: the same file, byte for byte.
//!
//! **Safety.** A recorded file is meant to play later on real hardware (the
//! ShowNET SD card, other software) without the studio. So it carries the
//! frames *after* the output safety chain (T-355, `ilda_library::bake_chain`):
//! calibration, the output's colour rendering, beam horizon, zones (Public
//! zones blanked), strobe limiter, static beam guard (as if armed), power
//! caps of the chosen output. The live capture takes the frame before that
//! chain (after the mixer, master stage and calibration) and the I/O thread
//! runs the very same chain functions with their own state over the whole
//! sequence, looped, so the engine's limiter and guard are never touched.
//! Never raw looks. The projector that plays the file has its own stages,
//! and its hardware stop stays the real safety: the dialog says so.
//!
//! Recording never arms, never disarms and never changes what is sent: it
//! works disarmed (it captures the computed frame, the preview's). An
//! emergency stop does not interrupt it (the output is cut, the engine keeps
//! computing); the user can cancel it. A cancelled recording leaves no file.
//!
//! Files go to the ILDA media library (`<data-dir>/media/ilda/`, T-011),
//! written aside then renamed (`IldaLibrary::import`), under a validated
//! name: no path ever leaves that folder. Only our own engine is a source:
//! there is no path here that reads another editor's format.

use crate::controls::ControlRegistry;
use crate::engine::{Animator, AudioFeatures, BeatClock};
use crate::ilda;
use crate::ilda_library::{self, ExportSafety, IldaLibrary};
use crate::layers::{self, Mixer};
use crate::live::{self, LiveModifiers, LiveState, Palette};
use crate::patterns::Point;
use crate::presets::Preset;
use crate::timeline::{self, Player, Show, TimeBase};
use crate::Shared;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

pub const MIN_SECONDS: f32 = 0.1;
pub const MAX_SECONDS: f32 = 600.0;
pub const DEFAULT_SECONDS: f32 = 10.0;
pub const MIN_FPS: u8 = 10;
pub const MAX_FPS: u8 = 60;
pub const DEFAULT_FPS: u8 = 25;
pub const DEFAULT_NAME: &str = "enregistrement";
/// Memory bound: points of all the images held at once (a 16 MB file).
/// A recording that goes beyond fails with a message.
pub const MAX_POINTS: usize = 2_000_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct RecordRequest {
    pub source: RecordSource,
    /// Live only; a timeline records its range.
    pub seconds: f32,
    pub fps: u8,
    /// The file name without `.ild` (ignored with `sd_number`).
    pub name: String,
    /// `001.ild`-`229.ild` for the ShowNET SD card (`ilda::shownet_sd_name`).
    pub sd_number: Option<u32>,
    /// The output whose safety chain is baked in; none: `main`.
    pub output: Option<String>,
}

impl Default for RecordRequest {
    fn default() -> Self {
        Self { source: RecordSource::Live, seconds: DEFAULT_SECONDS, fps: DEFAULT_FPS, name: DEFAULT_NAME.into(), sd_number: None, output: None }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordSource {
    #[default]
    Live,
    /// A saved show (or the loaded one, with its edits), between two
    /// positions in seconds (default: the whole show).
    Timeline { show: String, from_s: Option<f64>, to_s: Option<f64> },
}

impl RecordRequest {
    /// The library file name: the SD card name, or `<name>.ild` with the
    /// project-name rules (letters, digits, spaces, `-`, `_`).
    pub fn file_name(&self) -> Result<String> {
        if let Some(n) = self.sd_number {
            return ilda::shownet_sd_name(n);
        }
        let name = self.name.trim();
        if !timeline::valid_show_name(name) {
            bail!("Nom de fichier invalide : lettres, chiffres, espaces, - et _ seulement (64 caractères au plus)");
        }
        Ok(format!("{name}.{}", ilda_library::EXTENSION))
    }

    pub fn check_fps(&self) -> Result<()> {
        if !(MIN_FPS..=MAX_FPS).contains(&self.fps) {
            bail!("Images par seconde : {MIN_FPS} à {MAX_FPS}");
        }
        Ok(())
    }
}

/// Checks a duration in seconds.
pub fn check_seconds(seconds: f64) -> Result<()> {
    if !seconds.is_finite() || !((MIN_SECONDS as f64 - 1e-6)..=(MAX_SECONDS as f64 + 1e-6)).contains(&seconds) {
        bail!("Durée : {} à {} s", MIN_SECONDS.to_string().replace('.', ","), MAX_SECONDS);
    }
    Ok(())
}

/// Images in `seconds` at `fps`: `round(seconds × fps)`, at least 1.
pub fn frame_count(seconds: f32, fps: u8) -> usize {
    frames_in(seconds as f64, fps)
}

fn frames_in(seconds: f64, fps: u8) -> usize {
    ((seconds * fps as f64).round() as usize).max(1)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    #[default]
    Idle,
    Recording,
    Writing,
    Done,
    Failed,
    Cancelled,
}

/// `GET /api/ilda/record`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RecordStatus {
    pub state: RecordState,
    /// "live" or "timeline".
    pub source: &'static str,
    pub frames: usize,
    pub total: usize,
    pub fps: u8,
    pub output: String,
    pub file: Option<String>,
    pub bytes: usize,
    pub lit_before: usize,
    pub lit_after: usize,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

/// What the I/O thread did.
#[derive(Clone, Debug)]
pub struct Finished {
    pub file: String,
    pub bytes: usize,
    pub frames: usize,
    pub lit_before: usize,
    pub lit_after: usize,
    pub warnings: Vec<String>,
}

/// In `Shared`, never saved: the recording in progress or the last one.
#[derive(Default)]
pub struct Recorder {
    status: RecordStatus,
    /// Bumped by every start and cancel: a late I/O thread reports nothing.
    gen: u64,
    start_s: f64,
    next: usize,
    points: usize,
    /// Live: the engine's copies to the I/O thread.
    tx: Option<mpsc::Sender<Vec<Point>>>,
    cancel: Arc<AtomicBool>,
    /// Timeline: images rendered so far on the I/O thread.
    progress: Option<Arc<AtomicUsize>>,
}

/// What a started recording hands to its I/O thread.
pub struct Ticket {
    pub gen: u64,
    pub cancel: Arc<AtomicBool>,
}

impl Recorder {
    /// Recording or writing: a second start is refused (409).
    pub fn busy(&self) -> bool {
        matches!(self.status.state, RecordState::Recording | RecordState::Writing)
    }

    fn begin(&mut self, source: &'static str, total: usize, fps: u8, output: &str) -> Result<Ticket> {
        if self.busy() {
            bail!("Un enregistrement est déjà en cours");
        }
        self.gen += 1;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.status = RecordStatus { state: RecordState::Recording, source, total, fps, output: output.into(), ..Default::default() };
        self.next = 0;
        self.points = 0;
        self.tx = None;
        self.progress = None;
        Ok(Ticket { gen: self.gen, cancel: Arc::clone(&self.cancel) })
    }

    /// Starts a live recording at `now_s` (`Shared::now_s`): the first image
    /// is the next frame the engine computes. The receiver goes to the I/O
    /// thread.
    pub fn start(&mut self, req: &RecordRequest, now_s: f64, output: &str) -> Result<(Ticket, mpsc::Receiver<Vec<Point>>)> {
        req.check_fps()?;
        check_seconds(req.seconds as f64)?;
        let total = frame_count(req.seconds, req.fps);
        let ticket = self.begin("live", total, req.fps, output)?;
        let (tx, rx) = mpsc::channel();
        self.tx = Some(tx);
        self.start_s = now_s;
        Ok((ticket, rx))
    }

    /// Starts an offline (timeline) recording: the I/O thread counts its
    /// images in the returned counter.
    pub fn start_offline(&mut self, total: usize, fps: u8, output: &str) -> Result<(Ticket, Arc<AtomicUsize>)> {
        let ticket = self.begin("timeline", total, fps, output)?;
        let progress = Arc::new(AtomicUsize::new(0));
        self.progress = Some(Arc::clone(&progress));
        Ok((ticket, progress))
    }

    /// Whether the engine should copy its frame at `now_s`: a live
    /// recording whose next deadline has come.
    pub fn due(&self, now_s: f64) -> bool {
        self.tx.is_some() && now_s + 1e-9 >= self.start_s + self.next as f64 / self.status.fps.max(1) as f64
    }

    /// The engine's frame at `now_s` (before the output safety chain, see
    /// the module doc). Taken only when due, one per call; the last image
    /// closes the channel and the I/O thread takes over.
    pub fn offer(&mut self, frame: Vec<Point>, now_s: f64) {
        if !self.due(now_s) {
            return;
        }
        self.points += frame.len().max(1);
        if self.points > MAX_POINTS {
            self.tx = None;
            self.status.state = RecordState::Failed;
            self.status.error = Some(format!("Trop de points pour un enregistrement ({MAX_POINTS} au plus) : raccourcissez la durée ou baissez les images par seconde"));
            self.gen += 1;
            return;
        }
        let Some(tx) = &self.tx else { return };
        if tx.send(frame).is_err() {
            // The I/O thread is gone: it reported its own failure.
            self.tx = None;
            return;
        }
        self.next += 1;
        self.status.frames = self.next;
        if self.next >= self.status.total {
            self.tx = None;
            self.status.state = RecordState::Writing;
        }
    }

    /// Cancels the recording in progress: nothing is written (a file the
    /// I/O thread finished meanwhile is removed by it). False when idle.
    pub fn cancel(&mut self) -> bool {
        if !self.busy() {
            return false;
        }
        self.cancel.store(true, Ordering::SeqCst);
        self.tx = None;
        self.gen += 1;
        self.status.state = RecordState::Cancelled;
        true
    }

    /// The I/O thread's report. False when it is stale (cancelled or
    /// replaced): the caller then removes the file it wrote.
    pub fn finish(&mut self, gen: u64, result: Result<Finished, String>) -> bool {
        if gen != self.gen || !self.busy() {
            return false;
        }
        self.tx = None;
        if let Some(p) = self.progress.take() {
            self.status.frames = p.load(Ordering::SeqCst);
        }
        match result {
            Ok(f) => {
                self.status.state = RecordState::Done;
                self.status.frames = f.frames;
                self.status.file = Some(f.file);
                self.status.bytes = f.bytes;
                self.status.lit_before = f.lit_before;
                self.status.lit_after = f.lit_after;
                self.status.warnings = f.warnings;
            }
            Err(e) => {
                self.status.state = RecordState::Failed;
                self.status.error = Some(e);
            }
        }
        true
    }

    /// Marks the recording as writing (a timeline rendered all its images).
    fn writing(&mut self, gen: u64) {
        if gen == self.gen && self.status.state == RecordState::Recording {
            self.status.state = RecordState::Writing;
        }
    }

    pub fn status(&self) -> RecordStatus {
        let mut st = self.status.clone();
        if let (Some(p), RecordState::Recording | RecordState::Writing) = (&self.progress, st.state) {
            st.frames = p.load(Ordering::SeqCst).min(st.total);
        }
        st
    }
}

/// A show to render offline, copied out of `Shared` under the lock.
pub struct TimelineJob {
    pub show: Show,
    pub presets: Vec<Preset>,
    pub palettes: Vec<Palette>,
    pub mixer: Mixer,
    pub bpm: f64,
    pub beats_per_bar: u8,
    /// Seconds from the show's start.
    pub from_s: f64,
    pub to_s: f64,
}

/// The show's length in seconds (a Temps show at `bpm`).
pub fn show_seconds(show: &Show, bpm: f64) -> f64 {
    to_seconds(show, show.end(), bpm)
}

fn to_seconds(show: &Show, units: f64, bpm: f64) -> f64 {
    match show.time_base {
        TimeBase::Seconds => units,
        TimeBase::Beats => units * 60.0 / bpm.max(1.0),
    }
}

fn to_units(show: &Show, seconds: f64, bpm: f64) -> f64 {
    match show.time_base {
        TimeBase::Seconds => seconds,
        TimeBase::Beats => seconds * bpm.max(1.0) / 60.0,
    }
}

/// Whether the show reacts to music: a song, or a look with audio
/// reaction. An export renders it neutral; the dialog says so.
pub fn uses_audio(show: &Show, presets: &[Preset]) -> bool {
    show.song().is_some()
        || show.tracks.iter().flat_map(|t| &t.events).any(|e| match &e.source {
            timeline::EventSource::Look { settings } => settings.audio.enabled,
            timeline::EventSource::Cue { id } => presets.iter().any(|p| &p.id == id && p.settings.audio.enabled),
        })
}

impl TimelineJob {
    /// Images to render at `fps`.
    pub fn frames(&self, fps: u8) -> usize {
        frames_in(self.to_s - self.from_s, fps)
    }
}

/// The show between `job.from_s` and `job.to_s`, image after image at
/// `fps`, on a simulated clock: its own player, animators and rotation
/// states, neutral audio, the layers mixed at the mixer's point budget
/// (layer dimmers and master modifiers neutral), then `calibration`. Only
/// the content: the output safety chain comes after (`bake_chain`).
/// Deterministic. `progress` counts the images; `cancel` stops early.
pub fn render_timeline(job: &TimelineJob, fps: u8, calibration: &crate::engine::Calibration, progress: &AtomicUsize, cancel: &AtomicBool) -> Result<Vec<Vec<Point>>> {
    let frames = job.frames(fps);
    let bpm = if job.bpm.is_finite() { job.bpm.clamp(20.0, 300.0) } else { 120.0 };
    let reg = ControlRegistry::build(&job.presets);
    let mixer = Mixer { point_budget: job.mixer.point_budget, optimizer: job.mixer.optimizer.clone(), ..Mixer::default() };
    let clock_at = |t: f64| timeline::Clock { t, beat: t * bpm / 60.0, bpm, beats_per_bar: job.beats_per_bar.max(1) };
    let mut player = Player::default();
    player.load(job.show.clone());
    let c0 = clock_at(0.0);
    player.play(&c0);
    player.seek(to_units(&job.show, job.from_s, bpm), &c0);
    let audio = AudioFeatures::default().neutral();
    let dt = 1.0 / fps as f32;
    let mut animators: HashMap<u64, Animator> = HashMap::new();
    let mut event_live: HashMap<u64, LiveState> = HashMap::new();
    let mut out = Vec::with_capacity(frames);
    let mut points = 0usize;
    for i in 0..frames {
        if cancel.load(Ordering::SeqCst) {
            bail!("Enregistrement annulé");
        }
        let t = i as f64 / fps as f64;
        let c = clock_at(t);
        let cues = player.frame(&c);
        animators.retain(|id, _| cues.iter().any(|q| q.instance == *id));
        event_live.retain(|id, _| cues.iter().any(|q| q.instance == *id));
        let mut rendered = Vec::with_capacity(cues.len());
        for mut cue in cues {
            let Some(mut look) = timeline::look_of(&cue, &job.presets) else { continue };
            timeline::apply_envelopes(&cue.envelopes, &reg, &mut look, &mut cue.modifiers);
            let fresh = !animators.contains_key(&cue.instance);
            let animator = animators.entry(cue.instance).or_insert_with(|| Animator::starting_at(0.0));
            animator.set_optimizer(&mixer.optimizer);
            let event_dt = match (fresh, cue.frozen) {
                (true, _) => cue.elapsed_s as f32,
                (false, true) => 0.0,
                (false, false) => dt,
            };
            let content_clock = BeatClock { beat: cue.content_beat, bpm, beats_per_bar: c.beats_per_bar };
            let mut pts = animator.render(&look, audio, event_dt, &content_clock);
            if cue.modifiers != LiveModifiers::default() {
                let fresh = !event_live.contains_key(&cue.instance);
                let st = event_live.entry(cue.instance).or_default();
                st.set_clock(t, c.beat);
                let live_dt = match (fresh, cue.frozen) {
                    (true, _) => cue.elapsed_s as f32,
                    (false, true) => 0.0,
                    (false, false) => dt,
                };
                st.advance(&cue.modifiers, live_dt, bpm, c.beats_per_bar);
                pts = live::apply(&pts, &cue.modifiers, st, &job.palettes);
            }
            rendered.push((cue.layer, pts));
        }
        let (mixed, _) = layers::mix(rendered, &mixer);
        let frame: Vec<Point> = mixed
            .into_iter()
            .map(|p| {
                let (x, y) = calibration.apply(p.x, p.y);
                Point { x, y, ..p }
            })
            .collect();
        points += frame.len().max(1);
        if points > MAX_POINTS {
            bail!("Trop de points pour un enregistrement ({MAX_POINTS} au plus) : raccourcissez la plage ou baissez les images par seconde");
        }
        out.push(frame);
        progress.store(i + 1, Ordering::SeqCst);
    }
    Ok(out)
}

/// The images, through the output safety chain, encoded. Refused when
/// nothing is lit (before or after the chain). A file over the ShowNET's
/// 8 MB is written, with a warning.
pub fn bake(raw: &[Vec<Point>], fps: u8, chain: &ExportSafety) -> Result<(Vec<u8>, usize, usize, Vec<String>)> {
    let lit_before = ilda_library::lit_points(raw);
    if lit_before == 0 {
        bail!("Rien d'allumé pendant l'enregistrement : aucun fichier écrit");
    }
    let out = bake_chain_for(raw, fps, chain)?;
    let lit_after = ilda_library::lit_points(&out);
    let bytes = ilda_library::encode(out)?;
    let mut warnings = Vec::new();
    if bytes.len() > ilda::SHOWNET_MAX_FILE_BYTES {
        warnings.push("Plus de 8 Mo : trop gros pour la carte SD du ShowNET".to_string());
    }
    Ok((bytes, lit_before, lit_after, warnings))
}

fn bake_chain_for(raw: &[Vec<Point>], fps: u8, chain: &ExportSafety) -> Result<Vec<Vec<Point>>> {
    ilda_library::bake_chain(raw, fps as f32, chain, "cet enregistrement")
}

/// What the I/O thread renders from.
pub enum Source {
    /// The engine's copies, `total` of them.
    Live { rx: mpsc::Receiver<Vec<Point>>, total: usize },
    Timeline { job: Box<TimelineJob>, progress: Arc<AtomicUsize> },
}

/// One recording for the I/O thread.
pub struct Job {
    pub ticket: Ticket,
    pub source: Source,
    pub fps: u8,
    pub chain: ExportSafety,
    pub file: String,
    pub library: Arc<IldaLibrary>,
}

/// Runs `job` on its own thread: collect or render, safety chain, encode,
/// write into the library, report to the `Recorder`. Never holds the
/// `Shared` lock while working; never touches the output.
pub fn spawn(shared: Arc<Mutex<Shared>>, job: Job) -> Result<()> {
    std::thread::Builder::new().name("ilda-record".into()).spawn(move || run(&shared, job))?;
    Ok(())
}

fn run(shared: &Mutex<Shared>, job: Job) {
    let gen = job.ticket.gen;
    let cancelled = || job.ticket.cancel.load(Ordering::SeqCst);
    let raw = match job.source {
        Source::Live { rx, total } => {
            let mut frames = Vec::with_capacity(total.min(4096));
            while frames.len() < total {
                match rx.recv() {
                    Ok(f) => frames.push(f),
                    // Cancelled or failed: the recorder already says so.
                    Err(_) => return,
                }
            }
            Ok(frames)
        }
        Source::Timeline { job: tl, progress } => {
            let out = render_timeline(&tl, job.fps, &job.chain.calibration, &progress, &job.ticket.cancel);
            shared.lock().unwrap_or_else(|e| e.into_inner()).recorder.writing(gen);
            out
        }
    };
    if cancelled() {
        return;
    }
    let result = raw.and_then(|raw| {
        let frames = raw.len();
        let (bytes, lit_before, lit_after, mut warnings) = bake(&raw, job.fps, &job.chain)?;
        drop(raw);
        if cancelled() {
            bail!("Enregistrement annulé");
        }
        let imported = job.library.import(&job.file, &bytes)?;
        if imported.info.file != job.file {
            warnings.push(format!("« {} » existait déjà : enregistré sous « {} »", job.file, imported.info.file));
        }
        Ok(Finished { file: imported.info.file, bytes: bytes.len(), frames, lit_before, lit_after, warnings })
    });
    let written = result.as_ref().ok().map(|f| f.file.clone());
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    let current = s.recorder.finish(gen, result.map_err(|e| format!("{e:#}")));
    match (current, written) {
        (true, Some(file)) => {
            log::info!("Enregistrement ILDA écrit : {file} (sécurité de la sortie « {} » incluse)", job.chain.output);
            ilda_library::refresh(&mut s);
        }
        // Cancelled while writing: no file stays behind.
        (false, Some(file)) => {
            drop(s);
            let _ = job.library.remove(&file);
            ilda_library::refresh(&mut shared.lock().unwrap_or_else(|e| e.into_inner()));
        }
        _ => {}
    }
}

/// `POST /api/ilda/record`: validates, copies what the I/O thread needs
/// under the lock, starts. Err((status, French message)).
pub fn start(shared: &Arc<Mutex<Shared>>, req: &RecordRequest) -> Result<RecordStatus, (u16, String)> {
    let bad = |e: anyhow::Error| (400, format!("{e:#}"));
    req.check_fps().map_err(bad)?;
    let file = req.file_name().map_err(bad)?;
    let output = req.output.clone().filter(|o| !o.is_empty()).unwrap_or_else(|| crate::power::MAIN_OUTPUT.to_string());
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    if s.recorder.busy() {
        return Err((409, "Un enregistrement est déjà en cours".into()));
    }
    let chain = ExportSafety::of(&s, &output).map_err(bad)?;
    let library = Arc::clone(&s.ilda);
    let (ticket, source) = match &req.source {
        RecordSource::Live => {
            let now = s.now_s();
            let (ticket, rx) = s.recorder.start(req, now, &chain.output).map_err(bad)?;
            (ticket, Source::Live { rx, total: frame_count(req.seconds, req.fps) })
        }
        RecordSource::Timeline { show, from_s, to_s } => {
            let name = show.trim();
            // The loaded show with its edits, else the saved one.
            let loaded = s.timeline.show.as_ref().filter(|sh| sh.name == name).cloned();
            let show = match loaded {
                Some(sh) => sh,
                None => s.shows.load(name).map_err(|e| (404, format!("{e:#}")))?,
            };
            let bpm = s.tempo.bpm;
            let length = show_seconds(&show, bpm);
            let from = from_s.unwrap_or(0.0);
            let to = to_s.unwrap_or(length).min(length);
            if !from.is_finite() || !to.is_finite() || from < 0.0 || to <= from {
                return Err((400, "Plage invalide : le début doit précéder la fin, dans le show".into()));
            }
            check_seconds(to - from).map_err(|_| (400, format!("Durée de la plage : {} à {} s", MIN_SECONDS.to_string().replace('.', ","), MAX_SECONDS)))?;
            let job = TimelineJob {
                show,
                presets: s.presets.clone(),
                palettes: s.palettes.list().to_vec(),
                mixer: s.mixer.clone(),
                bpm,
                beats_per_bar: s.tempo.beats_per_bar,
                from_s: from,
                to_s: to,
            };
            let total = job.frames(req.fps);
            let (ticket, progress) = s.recorder.start_offline(total, req.fps, &chain.output).map_err(bad)?;
            (ticket, Source::Timeline { job: Box::new(job), progress })
        }
    };
    let status = s.recorder.status();
    drop(s);
    let job = Job { ticket, source, fps: req.fps, chain, file, library };
    if let Err(e) = spawn(Arc::clone(shared), job) {
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        s.recorder.cancel();
        return Err((500, format!("pas de fil d'écriture : {e}")));
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Calibration, Content, Settings};
    use crate::power::OutputLimits;
    use crate::safety::SafetySettings;
    use crate::timeline::{EventSource, Track};
    use crate::zones::{Horizon, Zone, ZoneKind};

    fn open_chain() -> ExportSafety {
        ExportSafety {
            output: "main".into(),
            calibration: Calibration::default(),
            safety: SafetySettings::default(),
            limits: OutputLimits { max_power: 1.0, max_color: [1.0; 3] },
            color: Default::default(),
            frame_limits: Default::default(),
        }
    }

    fn square(h: f32, c: f32) -> Vec<Point> {
        vec![
            Point::blanked(-h, -h),
            Point::lit(-h, -h, c, c, c),
            Point::lit(h, -h, c, c, c),
            Point::lit(h, h, c, c, c),
            Point::lit(-h, h, c, c, c),
            Point::lit(-h, -h, c, c, c),
        ]
    }

    #[test]
    fn frame_count_rounds() {
        assert_eq!(frame_count(10.0, 25), 250);
        assert_eq!(frame_count(2.0, 25), 50);
        assert_eq!(frame_count(0.1, 25), 3); // 2.5 → 3
        assert_eq!(frame_count(0.1, 10), 1);
        assert_eq!(frame_count(600.0, 60), 36_000);
        assert_eq!(frame_count(0.5, 25), 13); // 12.5 → 13
    }

    #[test]
    fn invalid_parameters_and_names_are_refused() {
        let mut r = Recorder::default();
        let req = |f: &dyn Fn(&mut RecordRequest)| {
            let mut q = RecordRequest::default();
            f(&mut q);
            q
        };
        assert!(r.start(&req(&|q| q.fps = 9), 0.0, "main").is_err());
        assert!(r.start(&req(&|q| q.fps = 61), 0.0, "main").is_err());
        assert!(r.start(&req(&|q| q.seconds = 0.05), 0.0, "main").is_err());
        assert!(r.start(&req(&|q| q.seconds = 601.0), 0.0, "main").is_err());
        assert!(r.start(&req(&|q| q.seconds = f32::NAN), 0.0, "main").is_err());
        assert!(!r.busy());
        for bad in ["../evil", "a/b", "", "  ", "x.ild\0", "..", "nom:2", &"a".repeat(65)] {
            assert!(req(&|q| q.name = bad.into()).file_name().is_err(), "{bad:?}");
        }
        assert_eq!(req(&|q| q.name = " Mon show 2 ".into()).file_name().unwrap(), "Mon show 2.ild");
        assert_eq!(req(&|q| q.sd_number = Some(7)).file_name().unwrap(), "007.ild");
        assert!(req(&|q| q.sd_number = Some(0)).file_name().is_err());
        assert!(req(&|q| q.sd_number = Some(230)).file_name().is_err());
    }

    /// A simulated engine at ~60 fps with jitter and a stall: the
    /// sampler takes exactly round(s × fps) images, each a different
    /// engine frame, in order.
    #[test]
    fn the_sampler_takes_the_exact_count_without_doubles() {
        for fps in [10u8, 25, 30, 50, 60] {
            let mut r = Recorder::default();
            let req = RecordRequest { seconds: 2.0, fps, ..Default::default() };
            let (_t, rx) = r.start(&req, 100.0, "main").unwrap();
            let mut t = 100.0;
            let mut tick = 0u32;
            let mut seed = 12345u32;
            while r.busy() && r.status().state == RecordState::Recording {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let jitter = ((seed >> 16) % 1000) as f64 / 1000.0 * 0.008 - 0.004;
                // A 150 ms stall once.
                let step = if tick == 40 { 0.15 } else { 1.0 / 60.0 + jitter };
                if r.due(t) {
                    r.offer(vec![Point::lit(tick as f32 / 1000.0, 0.0, 1.0, 1.0, 1.0)], t);
                }
                t += step;
                tick += 1;
                assert!(tick < 10_000);
            }
            let got: Vec<f32> = rx.try_iter().map(|f| f[0].x).collect();
            assert_eq!(got.len(), frame_count(2.0, fps), "fps {fps}");
            assert!(got.windows(2).all(|w| w[1] > w[0]), "no engine frame twice (fps {fps})");
            assert_eq!(r.status().state, RecordState::Writing);
            // Not due any more: further frames are ignored.
            assert!(!r.due(t + 10.0));
        }
    }

    #[test]
    fn a_second_start_is_refused_and_cancel_frees_it() {
        let mut r = Recorder::default();
        let (t1, _rx) = r.start(&RecordRequest::default(), 0.0, "main").unwrap();
        assert!(r.start(&RecordRequest::default(), 0.0, "main").is_err());
        assert!(r.start_offline(10, 25, "main").is_err());
        assert!(r.cancel());
        assert!(t1.cancel.load(Ordering::SeqCst));
        assert_eq!(r.status().state, RecordState::Cancelled);
        // The cancelled run's late report is ignored.
        assert!(!r.finish(t1.gen, Ok(Finished { file: "x.ild".into(), bytes: 1, frames: 1, lit_before: 1, lit_after: 1, warnings: vec![] })));
        assert!(r.start(&RecordRequest::default(), 0.0, "main").is_ok());
    }

    #[test]
    fn the_memory_bound_fails_the_recording() {
        let mut r = Recorder::default();
        let req = RecordRequest { seconds: 600.0, fps: 60, ..Default::default() };
        let (_t, rx) = r.start(&req, 0.0, "main").unwrap();
        let big = vec![Point::lit(0.0, 0.0, 1.0, 1.0, 1.0); 60_000];
        let mut t = 0.0;
        while r.status().state == RecordState::Recording {
            r.offer(big.clone(), t);
            t += 1.0 / 60.0;
        }
        assert_eq!(r.status().state, RecordState::Failed);
        assert!(r.status().error.unwrap().contains("Trop de points"));
        drop(rx);
    }

    /// The written file reads back as the same images (after the 16-bit
    /// and 8-bit quantisation), and writing them again gives the same bytes.
    #[test]
    fn round_trip_is_exact() {
        let raw: Vec<Vec<Point>> = (0..50).map(|i| square(0.1 + i as f32 / 100.0, 0.8)).collect();
        let (bytes, _, _, warnings) = bake(&raw, 25, &open_chain()).unwrap();
        assert!(warnings.is_empty());
        let baked = bake_chain_for(&raw, 25, &open_chain()).unwrap();
        let back = ilda::read(&bytes).unwrap();
        assert_eq!(back.len(), 50);
        for (f, r) in back.iter().zip(&baked) {
            assert_eq!(f.points.len(), r.len());
            for (a, b) in f.points.iter().zip(r) {
                assert!((a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4);
                assert!((a.r - b.r).abs() <= 1.0 / 255.0 + 1e-6);
                assert_eq!(a.is_lit(), b.is_lit());
            }
        }
        let again = ilda_library::encode(back.into_iter().map(|f| f.points).collect()).unwrap();
        assert_eq!(again, bytes);
    }

    /// The file carries the safety chain: a point in a Blank zone, below
    /// the full horizon (the audience side) or over the power cap comes
    /// out dark or capped.
    #[test]
    fn recorded_frames_are_safety_processed() {
        let mut chain = open_chain();
        chain.safety.horizon = Horizon { y: -0.5, lines: true, ramp: 0.0, level: 0.0 };
        let blank = [[-1.0, 0.5], [-0.5, 0.5], [-0.5, 1.0], [-1.0, 1.0]];
        chain.safety.zones = vec![Zone { id: 1, name: "z".into(), kind: ZoneKind::Blank, points: blank.to_vec(), ..Default::default() }];
        chain.limits = OutputLimits { max_power: 0.5, max_color: [1.0; 3] };
        let frame = vec![
            Point::blanked(0.0, -0.8),
            // Below the horizon.
            Point::lit(0.0, -0.8, 1.0, 1.0, 1.0),
            Point::lit(0.1, -0.9, 1.0, 1.0, 1.0),
            Point::blanked(-0.8, 0.8),
            // In the Blank zone.
            Point::lit(-0.8, 0.8, 1.0, 1.0, 1.0),
            Point::lit(-0.7, 0.9, 1.0, 1.0, 1.0),
            Point::blanked(0.2, 0.0),
            // Allowed, but over the cap.
            Point::lit(0.2, 0.0, 1.0, 1.0, 1.0),
            Point::lit(0.3, 0.1, 1.0, 1.0, 1.0),
        ];
        let raw: Vec<Vec<Point>> = (0..25).map(|i| frame.iter().map(|p| Point { x: p.x + i as f32 * 1e-3, ..*p }).collect()).collect();
        let (bytes, before, after, _) = bake(&raw, 25, &chain).unwrap();
        assert!(after < before);
        let back = ilda::read(&bytes).unwrap();
        assert_eq!(back.len(), 25);
        let mut seen_allowed = false;
        for f in &back {
            assert_eq!(crate::zones::blank_violations(&f.points, &chain.safety.zones), 0, "{}", f.name);
            for p in f.points.iter().filter(|p| p.is_lit()) {
                assert!(p.y >= -0.5 - 1e-3, "lit below the horizon: {p:?}");
                assert!(p.r <= 0.5 + 1e-6 && p.g <= 0.5 + 1e-6 && p.b <= 0.5 + 1e-6, "over the cap: {p:?}");
                seen_allowed = true;
            }
        }
        assert!(seen_allowed, "the allowed part stays lit");
        // Without the chain: lit there, at full power.
        let (open, ..) = bake(&raw, 25, &open_chain()).unwrap();
        assert!(ilda::read(&open).unwrap().iter().flat_map(|f| &f.points).any(|p| p.is_lit() && p.y < -0.7 && p.r > 0.9));
    }

    #[test]
    fn nothing_lit_is_refused() {
        let raw = vec![vec![Point::blanked(0.0, 0.0)]; 10];
        assert!(bake(&raw, 25, &open_chain()).unwrap_err().to_string().contains("Rien d'allumé"));
    }

    fn show() -> Show {
        let look = |c: [u8; 3]| Settings { content: Content::Shape { shape: "circle".into() }, color: c, scale: 0.4, rotation_speed: 90.0, ..Settings::default() };
        let mut sh = Show::new_empty("essai", TimeBase::Seconds);
        sh.tracks = vec![Track {
            name: "Piste 1".into(),
            layer: 1,
            events: vec![
                timeline::Event { id: 1, start: 0.0, len: 1.0, source: EventSource::Look { settings: look([255, 0, 0]) }, ..Default::default() },
                timeline::Event { id: 2, start: 1.0, len: 1.0, source: EventSource::Look { settings: look([0, 0, 255]) }, ..Default::default() },
            ],
            ..Default::default()
        }];
        sh.sanitize();
        sh
    }

    fn job(show: Show) -> TimelineJob {
        let to_s = show_seconds(&show, 120.0);
        TimelineJob { show, presets: Vec::new(), palettes: Vec::new(), mixer: Mixer::default(), bpm: 120.0, beats_per_bar: 4, from_s: 0.0, to_s }
    }

    #[test]
    fn render_timeline_is_deterministic_and_follows_the_show() {
        let j = job(show());
        let (p, c) = (AtomicUsize::new(0), AtomicBool::new(false));
        let a = render_timeline(&j, 25, &Calibration::default(), &p, &c).unwrap();
        assert_eq!(a.len(), 50);
        assert_eq!(p.load(Ordering::SeqCst), 50);
        let b = render_timeline(&j, 25, &Calibration::default(), &AtomicUsize::new(0), &c).unwrap();
        assert_eq!(a, b, "same show, same frames");
        let (fa, ..) = bake(&a, 25, &open_chain()).unwrap();
        let (fb, ..) = bake(&b, 25, &open_chain()).unwrap();
        assert_eq!(fa, fb, "byte for byte");
        // Red first, blue in the second half.
        let red = |f: &Vec<Point>| f.iter().any(|p| p.r > 0.3 && p.b < 0.1);
        let blue = |f: &Vec<Point>| f.iter().any(|p| p.b > 0.3 && p.r < 0.1);
        assert!(red(&a[5]) && !blue(&a[5]));
        assert!(blue(&a[40]) && !red(&a[40]));
        // A range: the second second only.
        let mut j2 = job(show());
        j2.from_s = 1.0;
        let c2 = render_timeline(&j2, 25, &Calibration::default(), &AtomicUsize::new(0), &c).unwrap();
        assert_eq!(c2.len(), 25);
        assert!(blue(&c2[10]));
        // Cancel stops it.
        assert!(render_timeline(&j, 25, &Calibration::default(), &AtomicUsize::new(0), &AtomicBool::new(true)).is_err());
    }

    /// The live capture point: what the engine offers is the frame before
    /// the output safety chain, already calibrated; the horizon is applied
    /// by the bake, not before (so the engine's limiter is never shared).
    #[test]
    fn calibration_goes_into_the_timeline_render() {
        let j = job(show());
        let cal = Calibration { offset_x: 0.3, ..Calibration::default() };
        let plain = render_timeline(&j, 25, &Calibration::default(), &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap();
        let moved = render_timeline(&j, 25, &cal, &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap();
        let cx = |f: &Vec<Point>| f.iter().filter(|p| p.is_lit()).map(|p| p.x).sum::<f32>() / f.iter().filter(|p| p.is_lit()).count().max(1) as f32;
        assert!((cx(&moved[10]) - cx(&plain[10]) - 0.3).abs() < 0.05);
    }

    #[test]
    fn audio_use_is_detected() {
        let mut sh = show();
        assert!(!uses_audio(&sh, &[]));
        if let EventSource::Look { settings } = &mut sh.tracks[0].events[0].source {
            settings.audio.enabled = true;
        }
        assert!(uses_audio(&sh, &[]));
    }
}
