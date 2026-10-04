//! The ILDA media library (T-011): the user's own `.ild` files in
//! `studio-data/media/ilda/`, each one a cue of the « ILDA » page, and the
//! export of a look as an ILDA format 5 file for the ShowNET SD card.
//!
//! Only files the user has the rights to (their own creations, content
//! bought under a licence that allows it) belong here: the UI says so at
//! the import. They stay on this Mac: never in git, never in a project
//! file (a cue only names its file). Parsing is `ilda.rs` (T-001); it runs
//! on the import request's own thread (`web.rs`, beside the single HTTP
//! worker) or on the start-up loader thread, never on the engine thread.
//!
//! An export bakes the output safety chain into the file (T-355,
//! `export_look`): the ShowNET plays it alone, without the studio.
//!
//! An imported file is played as it is: its points skip `densify` (the
//! file's author already placed its dwell and blanking), but they go
//! through the normal cue path and the whole safety chain like any other
//! look: layers and point budget, live modifiers, calibration, zones,
//! strobe limiter, static-beam guard, power caps, then the arm gate.
//! Nothing here can arm the laser.
//!
//! The engine reads parsed files from a process-wide registry by file
//! name (`clip`): a `Settings` value stays small and serializable (`file`
//! and `fps`), the points are shared, never copied per frame.

use crate::colorcal::ColorCalibration;
use crate::datafile;
use crate::dwell::{DwellState, StrictArea};
use crate::engine::{Animator, AudioFeatures, BeatClock, Calibration, Content, Settings};
use crate::ilda::{self, IldaFrame};
use crate::patterns::Point;
use crate::power::{self, OutputLimits};
use crate::presets::Preset;
use crate::safety::{self, SafetySettings, StrobeLimiter};
use crate::timeline::valid_show_name;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

/// The cue page imported files appear on (after the « Figures » page).
pub const CATEGORY: &str = "ILDA";
/// Cue id prefix: `ilda:<file>`.
pub const ID_PREFIX: &str = "ilda:";
/// The only extension kept in the library.
pub const EXTENSION: &str = "ild";
/// Images per second of a new cue (an ILDA file carries no rate).
pub const DEFAULT_FPS: f32 = 25.0;
pub const MIN_FPS: f32 = 1.0;
pub const MAX_FPS: f32 = 100.0;
/// Most images in one export (two minutes at 30 images per second).
pub const MAX_EXPORT_FRAMES: usize = 3_600;

pub fn default_fps() -> f32 {
    DEFAULT_FPS
}

/// A rate within `MIN_FPS..=MAX_FPS` (not finite → the default).
pub fn clamp_fps(fps: f32) -> f32 {
    if fps.is_finite() {
        fps.clamp(MIN_FPS, MAX_FPS)
    } else {
        DEFAULT_FPS
    }
}

/// A parsed file, shared by every cue that plays it.
#[derive(Debug)]
pub struct Clip {
    /// Never empty; every image has at least one point (`ilda::read`).
    pub frames: Vec<Vec<Point>>,
    /// Changes whenever the file is (re)loaded, so cached pictures of its
    /// cue are drawn again.
    pub stamp: u64,
}

impl Clip {
    pub fn points(&self) -> usize {
        self.frames.iter().map(Vec::len).sum()
    }

    /// The image shown `t` seconds after the start at `fps`, looping.
    pub fn frame_at(&self, t: f64, fps: f32) -> &[Point] {
        let n = self.frames.len().max(1) as u64;
        // Capped so a very long run can't overflow the cast.
        let step = (t.max(0.0) * clamp_fps(fps) as f64).floor().min(1e15) as u64;
        self.frames.get((step % n) as usize).map_or(&[], Vec::as_slice)
    }
}

/// Parsed files by name. One library per process (the studio has one data
/// directory); tests use distinct file names.
static CLIPS: LazyLock<RwLock<HashMap<String, Arc<Clip>>>> = LazyLock::new(Default::default);
static NEXT_STAMP: AtomicU64 = AtomicU64::new(1);

/// The parsed file `file`, if the library has it (still loading, removed
/// or damaged: `None`, and the cue draws nothing).
pub fn clip(file: &str) -> Option<Arc<Clip>> {
    CLIPS.read().unwrap_or_else(|e| e.into_inner()).get(file).cloned()
}

fn register(file: &str, frames: Vec<IldaFrame>) -> Arc<Clip> {
    let clip = Arc::new(Clip {
        frames: frames.into_iter().map(|f| f.points).collect(),
        stamp: NEXT_STAMP.fetch_add(1, Ordering::Relaxed),
    });
    CLIPS.write().unwrap_or_else(|e| e.into_inner()).insert(file.to_string(), Arc::clone(&clip));
    clip
}

fn unregister(file: &str) {
    CLIPS.write().unwrap_or_else(|e| e.into_inner()).remove(file);
}

/// A file of the library, as listed by `GET /api/media/ilda`.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct IldaInfo {
    pub file: String,
    /// The cue that plays it.
    pub id: String,
    pub size: u64,
    pub frames: usize,
    pub points: usize,
}

/// What an import returns.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Imported {
    #[serde(flatten)]
    pub info: IldaInfo,
    /// The same bytes were already in the library under this name.
    pub existing: bool,
}

/// A library file name: `<stem>.ild`, the stem made of letters, digits,
/// spaces, `-` and `_` (1 to 64 characters, trimmed).
pub fn valid_file_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| ext == EXTENSION && valid_show_name(stem))
}

/// A safe library name for an uploaded file name: other characters become
/// `_`, the extension is lower-cased. Refused: anything but `.ild`.
pub fn safe_file_name(original: &str) -> Result<String> {
    // Only the last path component, whatever the browser sent.
    let base = original.rsplit(['/', '\\']).next().unwrap_or("");
    let Some((stem, ext)) = base.rsplit_once('.') else { bail!("extension manquante (fichier .ild attendu)") };
    if !ext.eq_ignore_ascii_case(EXTENSION) {
        bail!("extension .{ext} non gérée (fichier .ild attendu)");
    }
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).take(56).collect();
    let stem = stem.trim();
    let stem = if stem.is_empty() { "animation" } else { stem };
    Ok(format!("{stem}.{EXTENSION}"))
}

/// The cue id of a file.
pub fn cue_id(file: &str) -> String {
    format!("{ID_PREFIX}{file}")
}

/// A file as a cue of the « ILDA » page: drawn at its own size, at
/// `DEFAULT_FPS` (the look panel changes the rate).
pub fn preset_of(info: &IldaInfo) -> Preset {
    let name = info.file.strip_suffix(".ild").unwrap_or(&info.file).to_string();
    Preset {
        id: info.id.clone(),
        name,
        category: CATEGORY,
        settings: Settings { content: Content::Ilda { file: info.file.clone(), fps: DEFAULT_FPS }, scale: 1.0, ..Settings::default() },
    }
}

/// Rebuilds the « ILDA » cue page (and the grid controls for MIDI) from
/// the library. Called whenever the library changes.
pub fn refresh(s: &mut crate::Shared) {
    s.presets.retain(|p| p.category != CATEGORY);
    let cues: Vec<Preset> = s.ilda.list().iter().map(preset_of).collect();
    s.presets.extend(cues);
    crate::grid::refresh(s);
}

/// Reads the library on its own thread (big files take a while), then
/// shows its cues and any damaged file's warning. The page lists
/// « chargement… » meanwhile.
pub fn spawn_load(shared: &Arc<Mutex<crate::Shared>>) {
    let library = Arc::clone(&shared.lock().unwrap_or_else(|e| e.into_inner()).ilda);
    library.loading.store(true, Ordering::SeqCst);
    let shared = Arc::clone(shared);
    let spawned = std::thread::Builder::new().name("ilda-load".into()).spawn(move || {
        library.load_all();
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        refresh(&mut s);
        s.sync_data_warnings();
    });
    if let Err(e) = spawned {
        log::warn!("médiathèque ILDA non chargée : {e}");
    }
}

/// `studio-data/media/ilda/`, kept in memory once loaded.
pub struct IldaLibrary {
    dir: PathBuf,
    quarantine: PathBuf,
    files: Mutex<Vec<IldaInfo>>,
    loading: AtomicBool,
    /// One import or removal at a time (picking a free name, then writing).
    writing: Mutex<()>,
}

impl IldaLibrary {
    /// The library of a data directory. Nothing is read yet (`load_all`).
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("media").join("ilda"),
            quarantine: data_dir.join(datafile::QUARANTINE),
            files: Mutex::new(Vec::new()),
            loading: AtomicBool::new(false),
            writing: Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> Vec<IldaInfo> {
        self.files.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn loading(&self) -> bool {
        self.loading.load(Ordering::SeqCst)
    }

    /// Valid names of regular files (not symlinks) in the folder.
    fn names_on_disk(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut names: Vec<String> = entries
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name().to_str()?.to_string();
                let meta = e.path().symlink_metadata().ok()?;
                (valid_file_name(&name) && meta.file_type().is_file()).then_some(name)
            })
            .collect();
        names.sort();
        names
    }

    /// Reads every file of the folder. A file that can't be read or parsed
    /// is moved to `<data-dir>/quarantine` and reported (T-402) - left in
    /// place, its cue would play nothing with no word why.
    pub fn load_all(&self) {
        self.loading.store(true, Ordering::SeqCst);
        let mut files = Vec::new();
        for name in self.names_on_disk() {
            let path = self.dir.join(&name);
            match ilda::read_file(&path) {
                Ok(frames) => {
                    let size = std::fs::symlink_metadata(&path).map_or(0, |m| m.len());
                    let clip = register(&name, frames);
                    files.push(info(&name, size, &clip));
                }
                Err(e) => {
                    unregister(&name);
                    datafile::set_aside(&path, &self.quarantine, &format!("{e:#}"), false);
                }
            }
        }
        *self.files.lock().unwrap_or_else(|e| e.into_inner()) = files;
        self.loading.store(false, Ordering::SeqCst);
    }

    /// Adds a file. It is parsed first: a damaged, truncated or unknown
    /// file is refused and leaves nothing. Written aside (`.<name>.part`)
    /// then renamed. The same bytes imported again give the existing file;
    /// another file with the same name gets « nom 2 ».
    pub fn import(&self, original_name: &str, bytes: &[u8]) -> Result<Imported> {
        let name = safe_file_name(original_name)?;
        if bytes.len() as u64 > ilda::MAX_READ_BYTES {
            bail!("Fichier ILDA trop gros (max {} Mo)", ilda::MAX_READ_BYTES >> 20);
        }
        let frames = ilda::read(bytes)?;
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(&self.dir).with_context(|| format!("impossible de créer {}", self.dir.display()))?;
        let stem = name.strip_suffix(".ild").unwrap_or(&name);
        let mut n = 1;
        let (file, existing) = loop {
            let candidate = if n == 1 { name.clone() } else { format!("{stem} {n}.{EXTENSION}") };
            let path = self.dir.join(&candidate);
            match std::fs::symlink_metadata(&path) {
                Err(_) => break (candidate, false),
                Ok(m) if m.file_type().is_file() && m.len() == bytes.len() as u64 && std::fs::read(&path).ok().as_deref() == Some(bytes) => {
                    break (candidate, true)
                }
                Ok(_) => n += 1,
            }
            if n > 999 {
                bail!("trop de fichiers nommés {name}");
            }
        };
        if !existing {
            let tmp = self.dir.join(format!(".{file}.part"));
            let path = self.dir.join(&file);
            let written = std::fs::File::create(&tmp).and_then(|mut f| {
                f.write_all(bytes)?;
                f.sync_all()?;
                std::fs::rename(&tmp, &path)
            });
            if let Err(e) = written {
                let _ = std::fs::remove_file(&tmp);
                return Err(e).with_context(|| format!("impossible d'écrire {}", path.display()));
            }
        }
        let clip = register(&file, frames);
        let info = info(&file, bytes.len() as u64, &clip);
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        files.retain(|f| f.file != file);
        files.push(info.clone());
        files.sort_by(|a, b| a.file.cmp(&b.file));
        Ok(Imported { info, existing })
    }

    /// Deletes a file from the folder and the library. Its cues then play
    /// nothing (a cue in the grid keeps its name, for a re-import).
    pub fn remove(&self, file: &str) -> Result<()> {
        if !valid_file_name(file) {
            bail!("nom de fichier invalide : {file}");
        }
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        if !files.iter().any(|f| f.file == file) {
            bail!("fichier introuvable : {file}");
        }
        match std::fs::remove_file(self.dir.join(file)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).context("échec de la suppression du fichier"),
            _ => {}
        }
        files.retain(|f| f.file != file);
        unregister(file);
        Ok(())
    }
}

fn info(file: &str, size: u64, clip: &Clip) -> IldaInfo {
    IldaInfo { file: file.to_string(), id: cue_id(file), size, frames: clip.frames.len(), points: clip.points() }
}

/// The output safety chain an export bakes into its file (T-355). The
/// ShowNET plays an SD card file alone, without the studio: whatever the
/// studio would have done to the light on its way out has to be in the
/// file already. Copied out of `Shared` under the lock, used without it.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSafety {
    /// The output whose caps apply (`outputs.json`).
    pub output: String,
    pub calibration: Calibration,
    /// Zones, horizon, colour calibration, strobe limiter, static beam guard.
    pub safety: SafetySettings,
    /// Power caps of `output`.
    pub limits: OutputLimits,
    /// Colour rendering of `output` (T-414), before the safety stage.
    pub color: ColorCalibration,
}

impl ExportSafety {
    /// The chain of output `output`. Unknown: refused, never a looser default.
    /// A file outlives the session: it never bakes audience scanning in,
    /// its Public zones are blanked (T-255).
    pub fn of(s: &crate::Shared, output: &str) -> Result<Self> {
        let Some(o) = s.outputs.get(output) else { bail!("sortie inconnue : {output}") };
        Ok(Self { output: o.id.clone(), calibration: s.calibration, safety: crate::audience::effective(&s.safety.get(), false), limits: o.limits, color: o.color })
    }
}

/// An exported file and what the safety chain did to it.
#[derive(Clone, Debug)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub frames: usize,
    /// Lit points before and after the safety chain (all images).
    pub lit_before: usize,
    pub lit_after: usize,
}

/// Laps of the images run through the chain before the one written, so
/// the strobe limiter and the static beam guard are in the state they
/// reach when the ShowNET loops the file: at least a whole burst and its
/// pause (see `export_look`).
fn warm_up_laps(frames: usize, fps: f32, cfg: &SafetySettings) -> usize {
    let lap_s = frames as f32 / fps;
    let need = cfg.strobe_burst_s.max(0.0) + cfg.strobe_cooldown_s.max(0.0) + 0.5;
    ((need / lap_s).ceil() as usize).max(1)
}

/// Rounds every channel down to the 8-bit step the file stores, so the
/// written value is never above what the chain let out (the writer itself
/// rounds to the nearest step).
fn floor_to_file_steps(frame: &mut [Point]) {
    let down = |v: f32| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0 + 1e-4).floor() / 255.0 } else { 0.0 };
    for p in frame {
        (p.r, p.g, p.b) = (down(p.r), down(p.g), down(p.b));
    }
}

/// A look as an ILDA format 5 file: `frames` images rendered `1/fps`
/// seconds apart at `bpm`, without music or live modifiers, then put
/// through the same output safety chain as the engine's frames (T-355):
/// calibration, the output's colour rendering (T-414), beam horizon,
/// zones and colour calibration, strobe
/// limiter, static beam guard (as if armed: its « off » profile never
/// applies), power caps of `chain.output`. The real functions, in the
/// engine's order (`main.rs`, `Engine::tick`).
///
/// The ShowNET loops the file: the stateful stages (strobe limiter, static
/// beam guard) first run over the images lap after lap, long enough for a
/// burst and its pause, and the lap written is the next one, so a strobe
/// that is too fast stays cut across the loop. Then a fresh limiter
/// watches the written lap looped: if it would still cut something, the
/// export is refused. All of this assumes the ShowNET plays the file at
/// `fps` images per second.
///
/// Refused when the look lights nothing, when the chain leaves nothing
/// lit, and when the file is too big for the ShowNET SD card.
pub fn export_look(settings: &Settings, frames: usize, fps: f32, bpm: f64, chain: &ExportSafety) -> Result<Exported> {
    if frames == 0 || frames > MAX_EXPORT_FRAMES {
        bail!("nombre d'images : 1 à {MAX_EXPORT_FRAMES}");
    }
    if !fps.is_finite() || !(MIN_FPS..=MAX_FPS).contains(&fps) {
        bail!("images par seconde : {MIN_FPS} à {MAX_FPS}");
    }
    let bpm = if bpm.is_finite() { bpm.clamp(20.0, 300.0) } else { 120.0 };
    let dt = 1.0 / fps;
    let mut animator = Animator::starting_at(0.0);
    let mut raw = Vec::with_capacity(frames);
    // The file's size so far (32-byte headers, 8-byte points): a look too
    // big for the card stops here, before it is all in memory. Zone edges
    // may add a few points: the written file is checked again.
    let mut size = 32;
    for i in 0..frames {
        let clock = BeatClock { beat: i as f64 * dt as f64 * bpm / 60.0, bpm, beats_per_bar: 4 };
        let points: Vec<Point> = animator
            .render(settings, AudioFeatures::default(), if i == 0 { 0.0 } else { dt }, &clock)
            .into_iter()
            .map(|p| {
                let (x, y) = chain.calibration.apply(p.x, p.y);
                Point { x, y, ..p }
            })
            .collect();
        size += 32 + 8 * points.len().max(1);
        ilda::check_shownet_size(size)?;
        raw.push(points);
    }
    let lit = |frames: &[Vec<Point>]| frames.iter().flatten().filter(|p| p.is_lit()).count();
    let lit_before = lit(&raw);
    if lit_before == 0 {
        bail!("Ce look n'allume rien : aucun fichier à exporter");
    }

    // The chain, lap after lap; the last lap is the file.
    let cfg = &chain.safety;
    let laps = warm_up_laps(frames, fps, cfg) + 1;
    let at = |lap: usize, i: usize| (lap * frames + i) as f64 * dt as f64;
    let mut limiter = StrobeLimiter::default();
    let mut dwell = DwellState::default();
    let area = StrictArea::new(cfg.horizon.y, &cfg.zones);
    let mut out = Vec::with_capacity(frames);
    for lap in 0..laps {
        for (i, points) in raw.iter().enumerate() {
            let mut frame = points.clone();
            chain.color.apply(&mut frame);
            let frame = safety::apply(frame, at(lap, i), cfg, &mut limiter);
            let mut frame = dwell.apply(frame, at(lap, i), &cfg.dwell, &area, true);
            power::cap(&mut frame, &chain.limits);
            floor_to_file_steps(&mut frame);
            if lap + 1 == laps {
                out.push(frame);
            }
        }
    }
    let lit_after = lit(&out);
    if lit_after == 0 {
        bail!(
            "La chaîne de sortie (couleurs de la sortie, zones, horizon, limiteur de strobe, garde anti-point fixe, plafonds de la sortie « {} ») coupe tout ce look : rien à exporter",
            chain.output
        );
    }
    // The file looped, seen by a limiter that knows nothing of it.
    let mut check = StrobeLimiter::default();
    for lap in 0..laps {
        for (i, frame) in out.iter().enumerate() {
            if check.would_cut(frame, at(lap, i), cfg) {
                bail!("Ce look clignote trop vite pour tourner en boucle sur le ShowNET (limiteur de strobe) : ralentissez le strobe ou changez la durée");
            }
        }
    }

    let images: Vec<IldaFrame> = out
        .into_iter()
        .enumerate()
        .map(|(i, points)| {
            let mut f = IldaFrame::new(&format!("LS{i:06}"), points);
            f.company = "LaserStd".into();
            f
        })
        .collect();
    let bytes = ilda::write_format5(&images)?;
    ilda::check_shownet_size(bytes.len())?;
    Ok(Exported { bytes, frames, lit_before, lit_after })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, AudioFeatures, BeatClock};

    /// A small animation written by our own writer (no third-party file):
    /// image i is a red square of half-size (i + 1) / 10.
    fn sample_file(frames: usize) -> Vec<u8> {
        let frames: Vec<IldaFrame> = (0..frames)
            .map(|i| {
                let h = (i + 1) as f32 / 10.0;
                let pts = vec![
                    Point::blanked(-h, -h),
                    Point::lit(-h, -h, 1.0, 0.0, 0.0),
                    Point::lit(h, -h, 1.0, 0.0, 0.0),
                    Point::lit(h, h, 1.0, 0.0, 0.0),
                    Point::lit(-h, h, 1.0, 0.0, 0.0),
                    Point::lit(-h, -h, 1.0, 0.0, 0.0),
                ];
                IldaFrame::new(&format!("f{i}"), pts)
            })
            .collect();
        ilda::write_format5(&frames).unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-ilda-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn names_are_confined_to_the_folder() {
        assert_eq!(safe_file_name("../../Mon Logo!.ILD").unwrap(), "Mon Logo_.ild");
        assert_eq!(safe_file_name("C:\\shows\\a.ild").unwrap(), "a.ild");
        assert_eq!(safe_file_name("???.ild").unwrap(), "___.ild");
        assert_eq!(safe_file_name(".ild").unwrap(), "animation.ild");
        assert!(safe_file_name("notes.txt").is_err());
        assert!(safe_file_name("noext").is_err());
        assert!(valid_file_name("a b-c_1.ild"));
        for bad in ["../a.ild", "a.ILD", ".a.ild.part", "a.ild ", "", "a/b.ild"] {
            assert!(!valid_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn import_parses_first_stores_and_registers() {
        let dir = temp_dir("import");
        let lib = IldaLibrary::new(&dir);
        // A damaged file is refused and leaves nothing.
        assert!(lib.import("cassé.ild", b"ILDA\0\0\0\x05junk").is_err());
        assert!(lib.import("vide.ild", b"").is_err());
        assert!(!lib.dir().join("cassé.ild").exists());
        assert!(lib.list().is_empty());

        let bytes = sample_file(3);
        let got = lib.import("libtest import.ild", &bytes).unwrap();
        assert_eq!(got.info.file, "libtest import.ild");
        assert_eq!(got.info.id, "ilda:libtest import.ild");
        assert_eq!((got.info.frames, got.info.points, got.existing), (3, 18, false));
        assert_eq!(std::fs::read(lib.dir().join("libtest import.ild")).unwrap(), bytes);
        let c = clip("libtest import.ild").unwrap();
        assert_eq!(c.frames.len(), 3);

        // Same bytes: the same file. Other bytes, same name: « nom 2 ».
        assert!(lib.import("libtest import.ild", &bytes).unwrap().existing);
        let other = lib.import("libtest import.ild", &sample_file(2)).unwrap();
        assert_eq!(other.info.file, "libtest import 2.ild");
        assert_eq!(lib.list().len(), 2);
        // No temporary file left behind.
        let names: Vec<_> = std::fs::read_dir(lib.dir()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 2, "{names:?}");

        lib.remove("libtest import 2.ild").unwrap();
        assert!(clip("libtest import 2.ild").is_none());
        assert!(!lib.dir().join("libtest import 2.ild").exists());
        assert!(lib.remove("libtest import 2.ild").is_err());
        assert!(lib.remove("../x.ild").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_quarantines_damaged_files_and_skips_others() {
        let dir = temp_dir("load");
        let media = dir.join("media").join("ilda");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("libtest bon.ild"), sample_file(2)).unwrap();
        std::fs::write(media.join("libtest abime.ild"), b"ILDA garbage").unwrap();
        std::fs::write(media.join("notes.txt"), b"not ours").unwrap();
        let lib = IldaLibrary::new(&dir);
        lib.load_all();
        assert!(!lib.loading());
        let list = lib.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].file, "libtest bon.ild");
        assert!(clip("libtest bon.ild").is_some());
        assert!(clip("libtest abime.ild").is_none());
        // The damaged file went to quarantine, with a warning.
        assert!(!media.join("libtest abime.ild").exists());
        let warnings = datafile::take_pending(&dir.join(datafile::QUARANTINE));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].file, "media/ilda/libtest abime.ild");
        assert!(!warnings[0].safety);
        assert!(media.join("notes.txt").exists(), "other files are left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frames_loop_at_the_cue_rate() {
        let c = Clip { frames: vec![vec![Point::blanked(0.0, 0.0)], vec![Point::blanked(1.0, 0.0)], vec![Point::blanked(0.5, 0.0)]], stamp: 0 };
        let x = |t: f64, fps: f32| c.frame_at(t, fps)[0].x;
        assert_eq!(x(0.0, 10.0), 0.0);
        assert_eq!(x(0.1, 10.0), 1.0);
        assert_eq!(x(0.25, 10.0), 0.5);
        assert_eq!(x(0.3, 10.0), 0.0, "loops");
        assert_eq!(x(-5.0, 10.0), 0.0);
        assert_eq!(x(1e300, f32::NAN), x(1e300, DEFAULT_FPS));
        assert_eq!(clamp_fps(0.0), MIN_FPS);
        assert_eq!(clamp_fps(1e9), MAX_FPS);
    }

    #[test]
    fn an_ilda_cue_plays_the_file_without_densify() {
        let dir = temp_dir("play");
        let lib = IldaLibrary::new(&dir);
        lib.import("libtest play.ild", &sample_file(4)).unwrap();
        let mut s = preset_of(&lib.list()[0]).settings;
        s.brightness = 1.0;
        if let Content::Ilda { fps, .. } = &mut s.content {
            *fps = 10.0;
        }
        let mut a = Animator::default();
        let clock = BeatClock::default();
        let first = a.render(&s, AudioFeatures::default(), 0.0, &clock);
        // Exactly the file's points (no densify, no corner dwell), scale 1.
        assert_eq!(first.len(), 6);
        assert!((first[2].x - 0.1).abs() < 1e-3 && (first[2].y + 0.1).abs() < 1e-3, "{first:?}");
        assert!(first[1].r > 0.99 && first[1].g == 0.0);
        assert!(!first[0].is_lit());
        // 0.25 s later at 10 images/s: image 2 (half-size 0.3).
        let later = a.render(&s, AudioFeatures::default(), 0.25, &clock);
        assert!((later[2].x - 0.3).abs() < 1e-3, "{later:?}");
        // Brightness and size apply on top.
        s.brightness = 0.5;
        s.scale = 0.5;
        let dim = a.render(&s, AudioFeatures::default(), 0.0, &clock);
        assert!((dim[2].x - 0.15).abs() < 1e-3 && (dim[1].r - 0.5).abs() < 1e-3, "{dim:?}");
        // A file not (or no longer) in the library draws nothing.
        s.content = Content::Ilda { file: "libtest absent.ild".into(), fps: 25.0 };
        assert!(a.render(&s, AudioFeatures::default(), 0.1, &clock).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ilda_content_round_trips_as_json_with_a_default_rate() {
        let c: Content = serde_json::from_str(r#"{"kind":"ilda","file":"a.ild"}"#).unwrap();
        assert_eq!(c, Content::Ilda { file: "a.ild".into(), fps: DEFAULT_FPS });
        let back: Content = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert!(c.same_drawing(&Content::Ilda { file: "a.ild".into(), fps: 5.0 }));
        assert!(!c.same_drawing(&Content::Ilda { file: "b.ild".into(), fps: 25.0 }));
    }

    #[test]
    fn export_is_a_valid_format5_file_read_back_by_the_reader() {
        let look = Settings { content: Content::Shape { shape: "square".into() }, rotation_speed: 90.0, brightness: 1.0, ..Settings::default() };
        let bytes = export_look(&look, 12, 30.0, 120.0, &open_chain()).unwrap().bytes;
        assert_eq!(bytes[7], 5, "format 5");
        let back = ilda::read(&bytes).unwrap();
        assert_eq!(back.len(), 12);
        assert_eq!(back[0].name, "LS000000");
        assert_eq!(back[11].company, "LaserStd");
        assert!(back.iter().all(|f| f.points.iter().any(|p| p.is_lit())));
        // It rotates: the first and last images differ.
        assert_ne!(back[0].points, back[11].points);
        assert!(export_look(&look, 0, 30.0, 120.0, &open_chain()).is_err());
        assert!(export_look(&look, MAX_EXPORT_FRAMES + 1, 30.0, 120.0, &open_chain()).is_err());
        assert!(export_look(&look, 10, 0.0, 120.0, &open_chain()).is_err());
        assert!(export_look(&look, 10, f32::NAN, 120.0, &open_chain()).is_err());
    }

    #[test]
    fn export_too_big_for_the_sd_card_is_refused() {
        let dir = temp_dir("big");
        let lib = IldaLibrary::new(&dir);
        let dense: Vec<Point> = (0..60_000).map(|i| Point::lit((i % 200) as f32 / 100.0 - 1.0, 0.0, 0.0, 1.0, 0.0)).collect();
        lib.import("libtest big.ild", &ilda::write_format5(&[IldaFrame::new("big", dense)]).unwrap()).unwrap();
        let look = Settings { content: Content::Ilda { file: "libtest big.ild".into(), fps: 25.0 }, ..Settings::default() };
        // 60 000 points × 8 bytes per image: 17 images pass 8 Mo.
        assert!(export_look(&look, 10, 25.0, 120.0, &open_chain()).is_ok());
        let e = export_look(&look, 40, 25.0, 120.0, &open_chain()).unwrap_err().to_string();
        assert!(e.contains("carte SD"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default safety settings with the caps wide open, so a test
    /// sees the look itself.
    fn open_chain() -> ExportSafety {
        ExportSafety {
            output: power::MAIN_OUTPUT.into(),
            calibration: Calibration::default(),
            safety: SafetySettings::default(),
            limits: OutputLimits { max_power: 1.0, max_color: [1.0; 3] },
            color: ColorCalibration::default(),
        }
    }

    fn zone(kind: crate::zones::ZoneKind, points: &[[f32; 2]]) -> crate::zones::Zone {
        crate::zones::Zone { id: 1, name: "z".into(), kind, level: 0.3, points: points.to_vec(), audience: false }
    }

    /// A big circle turning, lit all around the field.
    fn white_circle() -> Settings {
        Settings { content: Content::Shape { shape: "circle".into() }, rotation_speed: 90.0, brightness: 1.0, scale: 1.0, ..Settings::default() }
    }

    #[test]
    fn exported_files_have_nothing_lit_in_a_blank_zone_nor_above_the_caps() {
        let blank = zone(crate::zones::ZoneKind::Blank, &[[0.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, 1.0]]);
        let mut chain = open_chain();
        chain.safety.zones = vec![blank.clone()];
        chain.limits = OutputLimits { max_power: 0.4, max_color: [1.0, 0.5, 0.8] };
        let got = export_look(&white_circle(), 30, 30.0, 120.0, &chain).unwrap();
        assert!(got.lit_after > 0 && got.lit_after < got.lit_before, "{} → {}", got.lit_before, got.lit_after);
        let back = ilda::read(&got.bytes).unwrap();
        assert_eq!(back.len(), 30);
        for f in &back {
            assert_eq!(crate::zones::blank_violations(&f.points, &chain.safety.zones), 0, "{}", f.name);
            assert!(f.points.iter().any(|p| p.is_lit() && p.x < -0.1), "the left half still shows");
            for p in &f.points {
                // Read back from 8-bit values: never above the cap, not even by rounding.
                assert!(p.r <= 0.4 + 1e-6 && p.g <= 0.2 + 1e-6 && p.b <= 0.32 + 1e-6, "{p:?}");
            }
        }
        // Without the chain the same look is lit on the right and brighter.
        let open = ilda::read(&export_look(&white_circle(), 30, 30.0, 120.0, &open_chain()).unwrap().bytes).unwrap();
        assert!(open.iter().flat_map(|f| &f.points).any(|p| p.is_lit() && p.x > 0.5 && p.r.max(p.g).max(p.b) > 0.9));
    }

    #[test]
    fn calibration_and_horizon_are_in_the_file() {
        let mut chain = open_chain();
        chain.calibration = Calibration { offset_x: 0.5, scale_x: 0.25, scale_y: 0.25, ..Calibration::default() };
        let back = ilda::read(&export_look(&white_circle(), 5, 30.0, 120.0, &chain).unwrap().bytes).unwrap();
        for p in back.iter().flat_map(|f| &f.points).filter(|p| p.is_lit()) {
            assert!((0.2..=0.8).contains(&p.x) && p.y.abs() <= 0.3, "{p:?}");
        }
        // Full horizon at 0, lines included: nothing lit below it.
        let mut chain = open_chain();
        chain.safety.horizon = crate::zones::Horizon { y: 0.0, lines: true, ramp: 0.0, level: 0.0 };
        let back = ilda::read(&export_look(&white_circle(), 5, 30.0, 120.0, &chain).unwrap().bytes).unwrap();
        assert!(back.iter().flat_map(|f| &f.points).filter(|p| p.is_lit()).all(|p| p.y >= -1e-3));
        assert!(back.iter().flat_map(|f| &f.points).any(|p| p.is_lit()));
    }

    #[test]
    fn an_export_the_chain_cuts_entirely_is_refused() {
        let mut chain = open_chain();
        chain.safety.zones = vec![zone(crate::zones::ZoneKind::Blank, &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]])];
        let e = export_look(&white_circle(), 10, 30.0, 120.0, &chain).unwrap_err().to_string();
        assert!(e.contains("coupe tout"), "{e}");
        chain.safety.zones.clear();
        chain.limits.max_power = 0.0;
        assert!(export_look(&white_circle(), 10, 30.0, 120.0, &chain).unwrap_err().to_string().contains("coupe tout"));
        let dark = Settings { brightness: 0.0, ..white_circle() };
        assert!(export_look(&dark, 10, 30.0, 120.0, &open_chain()).unwrap_err().to_string().contains("n'allume rien"));
    }

    #[test]
    fn a_fast_strobe_is_cut_in_the_file_across_the_loop() {
        // 8 flashes per beat at 120 BPM: 16 Hz, far above the 4 Hz default.
        let strobe = Settings { strobe_div: 8.0, strobe_duty: 0.4, ..white_circle() };
        let fps = 50.0;
        let got = export_look(&strobe, 100, fps, 120.0, &open_chain()).unwrap();
        let back = ilda::read(&got.bytes).unwrap();
        // The file looped many times: lit / dark changes stay within the
        // limit (the limiter holds the light through the gaps).
        let lit: Vec<bool> = back.iter().map(|f| f.points.iter().any(|p| p.is_lit())).collect();
        let onsets = (0..lit.len() * 4).filter(|&i| lit[i % lit.len()] && !lit[(i + lit.len() - 1) % lit.len()]).count();
        let seconds = 4.0 * lit.len() as f32 / fps;
        assert!(onsets as f32 <= seconds * crate::safety::DEFAULT_MAX_HZ + 1.0, "{onsets} flashes in {seconds} s");
        // Without the limiter it would flash 16 times a second.
        let on = |i: usize| strobe.rhythm_gain(i as f64 / fps as f64 * 2.0) > 0.0;
        let raw = (1..100).filter(|&i| on(i) && !on(i - 1)).count();
        assert!(raw >= 25, "{raw}");
    }

    #[test]
    fn the_chain_of_an_unknown_output_is_refused() {
        let s = crate::test_support::shared();
        assert_eq!(ExportSafety::of(&s, power::MAIN_OUTPUT).unwrap().limits, s.outputs.active_limits());
        assert!(ExportSafety::of(&s, "ailleurs").is_err());
    }

    /// T-414: the file carries the output's colour rendering, before the
    /// safety stage and the caps: a balance at 0 on every colour leaves
    /// nothing to export.
    #[test]
    fn the_export_carries_the_output_colour_rendering() {
        let mut s = crate::test_support::shared();
        let color = ColorCalibration { white_balance: [0.0; 3], ..Default::default() };
        s.outputs.set_color(power::MAIN_OUTPUT, color, false, false).unwrap();
        assert_eq!(ExportSafety::of(&s, power::MAIN_OUTPUT).unwrap().color, color);
        assert!(export_look(&white_circle(), 2, 25.0, 120.0, &open_chain()).is_ok());
        let e = export_look(&white_circle(), 2, 25.0, 120.0, &ExportSafety { color, ..open_chain() }).unwrap_err().to_string();
        assert!(e.contains("couleurs de la sortie"), "{e}");
    }
}
