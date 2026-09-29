//! Project files (T-286): one `.lsproj` file holds a whole show - scenes
//! and playlist, cue-grid properties, timelines, default tempo, master
//! live modifiers, layers, LFOs, audio routes (T-153), user palettes, MIDI mappings and the
//! figure library (T-296) - as
//! pretty-printed, versioned JSON in `<data-dir>/projects/`.
//!
//! The data directory's own files (`scenes.json`, `grid.json`, `shows/`…)
//! stay the working copy the studio runs from; a project is a snapshot of
//! them. *Enregistrer* writes the snapshot, *Ouvrir* replaces them.
//!
//! What is **not** in a project, and never changed by opening one: the
//! calibration, the safety settings (strobe limiter, horizon, hold-to-run,
//! heartbeat), the MIDI safety options (`midi/devices.json`), the arming
//! state and the e-stop. They belong to the rig and the venue (the future
//! site profile, T-289), so a project made elsewhere can't loosen them.
//! Nothing here can arm the laser.
//!
//! Opening is atomic: the file is read and every section checked before
//! anything changes; on error the current state is untouched and the
//! reply says why, in French. Files are only ever read or written inside
//! `projects/`, whatever name or path is asked for or found in a file.
//! Writes are atomic (`.tmp`, fsync, rename) and happen on the HTTP worker
//! thread with the engine lock released: the lock is only held to copy
//! the state out, or to swap the checked state in.

use crate::audio::routes::AudioRouting;
use crate::cues::CueDeck;
use crate::figures::Figure;
use crate::layers::Mixer;
use crate::lfo::Modulator;
use crate::live::{LiveModifiers, Palette};
use crate::midi::profile::{valid_slug, Profile, ProfileStore};
use crate::scenes::Scene;
use crate::tempo::{MAX_BPM, MIN_BPM};
use crate::timeline::{valid_show_name, Show};
use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Current format. T-288 adds migrations from older ones.
pub const PROJECT_FORMAT: u32 = 1;
pub const EXTENSION: &str = "lsproj";
pub const UNTITLED: &str = "Sans titre";
const MAX_RECENT: usize = 10;
/// Larger files are refused before parsing.
const MAX_BYTES: u64 = 64 << 20;
const NAME_RULE: &str = "lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus";

/// Default tempo of a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TempoDefaults {
    pub bpm: f64,
    pub beats_per_bar: u8,
}

impl Default for TempoDefaults {
    fn default() -> Self {
        Self { bpm: 120.0, beats_per_bar: 4 }
    }
}

/// MIDI mappings: the user profiles (`midi/profiles/`). The MIDI safety
/// options and per-port choices stay with the rig.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiSection {
    pub profiles: BTreeMap<String, Profile>,
}

/// A whole project. Every section has a default, so a file from an older
/// version (or one missing a section) still opens; sections added by later
/// versions, and fields we don't know, are kept in `extra` and written
/// back on save.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub format_version: u32,
    pub app_version: String,
    /// UTC, ISO 8601.
    pub saved_at: String,
    pub name: String,
    pub scenes: Vec<Scene>,
    /// Scene names in playing order (today: the order of `scenes`).
    pub playlist: Vec<String>,
    /// Cue-grid properties (`grid.json`): click mode, multi, per-cue slots.
    pub grid: CueDeck,
    pub timelines: Vec<Show>,
    pub tempo: TempoDefaults,
    /// Master live modifiers.
    pub live: LiveModifiers,
    pub layers: Mixer,
    pub lfos: Vec<Modulator>,
    /// Audio routes and the *Temps ↔ Audio* crossfader. Missing in
    /// projects saved before T-153: they open with none.
    pub audio_routes: AudioRouting,
    pub palettes: Vec<Palette>,
    pub midi: MidiSection,
    /// The figure library (`figures/`). Missing in projects saved before
    /// T-296: they open with no figures.
    pub figures: Vec<Figure>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            format_version: PROJECT_FORMAT,
            app_version: String::new(),
            saved_at: String::new(),
            name: String::new(),
            scenes: Vec::new(),
            playlist: Vec::new(),
            grid: CueDeck::default(),
            timelines: Vec::new(),
            tempo: TempoDefaults::default(),
            live: LiveModifiers::default(),
            layers: Mixer::default(),
            lfos: Vec::new(),
            audio_routes: AudioRouting::default(),
            palettes: Vec::new(),
            midi: MidiSection::default(),
            figures: Vec::new(),
            extra: Map::new(),
        }
    }
}

impl Project {
    /// A hash of the sections (not the header, not `extra`): equal
    /// fingerprints = nothing to save.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let sections = Project { app_version: String::new(), saved_at: String::new(), name: String::new(), extra: Map::new(), ..self.clone() };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&sections).unwrap_or_default().hash(&mut h);
        h.finish()
    }
}

/// `<data-dir>/recent.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct RecentFile {
    /// File name (without extension) of the open project; `None` = *Sans titre*.
    current: Option<String>,
    /// Most recent first, at most `MAX_RECENT`.
    recent: Vec<String>,
}

/// The open project, part of `Shared`.
pub struct ProjectState {
    pub data_dir: PathBuf,
    /// `<data-dir>/projects`.
    pub dir: PathBuf,
    recent_path: PathBuf,
    /// Name (= file name without extension) of the open project, or `None`
    /// for an unsaved *Sans titre*.
    pub current: Option<String>,
    recent: Vec<String>,
    /// Unknown fields of the open project, written back on save.
    extra: Map<String, Value>,
    /// Fingerprint of the state as last opened or saved.
    saved: u64,
}

impl ProjectState {
    /// Reads `recent.json`; `startup` completes it once the stores are loaded.
    pub fn load(data_dir: &Path) -> Self {
        let recent_path = data_dir.join("recent.json");
        let file: RecentFile = crate::load_json(&recent_path);
        Self { data_dir: data_dir.to_path_buf(), dir: data_dir.join("projects"), recent_path, current: file.current, recent: file.recent, extra: Map::new(), saved: 0 }
    }

    fn remember(&mut self, name: &str) {
        self.current = Some(name.to_string());
        self.recent.retain(|n| n != name);
        self.recent.insert(0, name.to_string());
        self.recent.truncate(MAX_RECENT);
    }

    fn save_recent(&self) {
        let file = RecentFile { current: self.current.clone(), recent: self.recent.clone() };
        if let Err(e) = serde_json::to_vec_pretty(&file).map_err(anyhow::Error::from).and_then(|b| write_atomic(&self.recent_path, &b)) {
            log::warn!("failed to save {}: {e:#}", self.recent_path.display());
        }
    }

    fn path_of(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.{EXTENSION}"))
    }
}

/// Files of the working copy: if any exists on a first start (no
/// `recent.json` yet), they are imported into a *Sans titre* project.
const WORKING_FILES: [&str; 9] = ["scenes.json", "grid.json", "live.json", "layers.json", "lfos.json", "audio_routes.json", "palettes.json", "shows", "figures"];

/// Called once at startup, before the engine runs. First start with data
/// from before projects existed: it is copied into `projects/Sans
/// titre.lsproj` (the original files stay). Otherwise, the open project's
/// file is read to know whether the working copy differs from it.
pub fn startup(s: &mut Shared) {
    let data_dir = s.project.data_dir.clone();
    let first_start = !s.project.recent_path.exists();
    if first_start && WORKING_FILES.iter().any(|f| data_dir.join(f).exists()) {
        let name = (1..).map(|n| if n == 1 { UNTITLED.to_string() } else { format!("{UNTITLED} {n}") }).find(|n| !s.project.path_of(n).exists()).expect("some name is free");
        let mut p = snapshot(s);
        p.timelines = s.shows.load_all();
        p.name = name.clone();
        stamp(&mut p);
        match write_project(&s.project.path_of(&name), &p) {
            Ok(()) => {
                println!("Projet : données existantes importées dans {}", s.project.path_of(&name).display());
                s.project.remember(&name);
                s.project.saved = p.fingerprint();
            }
            Err(e) => log::warn!("failed to import the existing data into a project: {e:#}"),
        }
        s.project.save_recent();
        return;
    }
    if first_start {
        s.project.save_recent();
    }
    let current = s.project.current.clone().filter(|n| valid_name(n) && s.project.path_of(n).is_file());
    s.project.current = current.clone();
    s.project.saved = match current {
        Some(name) => match read(&s.project.path_of(&name)) {
            Ok(p) => {
                s.project.extra = p.extra.clone();
                p.fingerprint()
            }
            Err(e) => {
                log::warn!("project {name}: {e:#}");
                0
            }
        },
        None => {
            let mut p = snapshot(s);
            p.timelines = s.shows.load_all();
            p.fingerprint()
        }
    };
}

/// Resolves what the user typed or picked - a bare name, `name.lsproj`,
/// or a path - to a file directly inside `dir`. Anything else (another
/// folder, `..`, separators, odd characters) is refused.
pub fn resolve(dir: &Path, input: &str) -> Result<PathBuf> {
    let input = input.trim();
    let refused = || anyhow::anyhow!("chemin refusé : les projets sont enregistrés dans {} ({NAME_RULE})", dir.display());
    let path = Path::new(input);
    let file = path.file_name().and_then(|f| f.to_str()).ok_or_else(refused)?;
    if path.components().count() > 1 {
        // A full path: its folder must be the projects folder itself.
        let parent = path.parent().ok_or_else(refused)?;
        let same = match (parent.canonicalize(), dir.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            return Err(refused());
        }
    }
    let name = file.strip_suffix(&format!(".{EXTENSION}")).unwrap_or(file).trim();
    if !valid_name(name) {
        return Err(refused());
    }
    Ok(dir.join(format!("{name}.{EXTENSION}")))
}

/// Project names are file names: see `NAME_RULE`.
fn valid_name(name: &str) -> bool {
    valid_show_name(name)
}

fn name_of(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string()
}

/// Reads and checks a project file. Everything but the LFO and audio
/// route targets (which need the control registry, see `check_lfos`) is
/// checked here.
pub fn read(path: &Path) -> Result<Project> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => anyhow::anyhow!("projet introuvable : {}", name_of(path)),
        _ => anyhow::anyhow!("projet illisible : {e}"),
    })?;
    if !meta.is_file() {
        bail!("« {} » n'est pas un fichier projet", name_of(path));
    }
    if meta.len() > MAX_BYTES {
        bail!("projet trop gros ({} Mo au plus)", MAX_BYTES >> 20);
    }
    let text = std::fs::read_to_string(path).context("projet illisible")?;
    parse(&text)
}

/// Parses and checks a project from JSON text.
pub fn parse(text: &str) -> Result<Project> {
    let value: Value = serde_json::from_str(text).map_err(|e| anyhow::anyhow!("projet illisible (JSON invalide : {e})"))?;
    let Some(obj) = value.as_object() else { bail!("projet illisible : ce n'est pas un fichier projet") };
    match obj.get("format_version").and_then(Value::as_u64) {
        None => bail!("projet illisible : « format_version » manquant"),
        Some(0) => bail!("projet illisible : « format_version » invalide"),
        Some(v) if v > PROJECT_FORMAT as u64 => {
            bail!("projet créé par une version plus récente de Laser Studio (format {v}, cette version lit le format {PROJECT_FORMAT})")
        }
        Some(_) => {}
    }
    let mut p: Project = serde_json::from_value(value).map_err(|e| anyhow::anyhow!("projet illisible : {e}"))?;
    check(&mut p)?;
    Ok(p)
}

/// Checks every section and brings numbers into range. The scene order
/// follows the playlist.
fn check(p: &mut Project) -> Result<()> {
    let mut names = HashSet::new();
    for sc in &mut p.scenes {
        sc.name = sc.name.trim().to_string();
        if sc.name.is_empty() {
            bail!("scène sans nom");
        }
        if !names.insert(sc.name.clone()) {
            bail!("scène en double : « {} »", sc.name);
        }
        sc.duration_secs = if sc.duration_secs.is_finite() { sc.duration_secs.clamp(0.5, 3600.0) } else { 8.0 };
    }
    // Playlist order first, then any scene it doesn't list.
    let rank = |name: &str| p.playlist.iter().position(|n| n == name).unwrap_or(usize::MAX);
    let mut scenes = std::mem::take(&mut p.scenes);
    scenes.sort_by_key(|sc| rank(&sc.name));
    p.scenes = scenes;
    p.playlist = p.scenes.iter().map(|sc| sc.name.clone()).collect();

    p.grid.sanitize();

    let mut shows = HashSet::new();
    for show in &mut p.timelines {
        show.name = show.name.trim().to_string();
        if !valid_show_name(&show.name) {
            bail!("nom de timeline invalide : « {} » ({NAME_RULE})", show.name);
        }
        if !shows.insert(show.name.clone()) {
            bail!("timeline en double : « {} »", show.name);
        }
        show.sanitize();
    }
    p.timelines.sort_by(|a, b| a.name.cmp(&b.name));

    let mut figures = HashSet::new();
    for fig in &mut p.figures {
        fig.validate()?;
        if !figures.insert(fig.name.clone()) {
            bail!("figure en double : « {} »", fig.name);
        }
    }
    p.figures.sort_by(|a, b| a.name.cmp(&b.name));

    let bpm = if p.tempo.bpm.is_finite() { p.tempo.bpm } else { 120.0 };
    p.tempo = TempoDefaults { bpm: bpm.clamp(MIN_BPM, MAX_BPM), beats_per_bar: p.tempo.beats_per_bar.clamp(1, 16) };
    p.layers.sanitize();
    crate::live::validate_palettes(&p.palettes).map_err(|e| anyhow::anyhow!("palettes : {e}"))?;

    let builtin = ProfileStore::in_memory();
    for (slug, profile) in &p.midi.profiles {
        if !valid_slug(slug) || builtin.is_builtin(slug) {
            bail!("profil MIDI au nom invalide : « {slug} »");
        }
        // The same checks as a profile file on disk.
        Profile::parse(&serde_json::to_string(profile)?).map_err(|e| anyhow::anyhow!("profil MIDI « {slug} » : {e}"))?;
    }
    Ok(())
}

/// LFO and audio route targets must be controls that exist and can be
/// modulated; route sources must be known analysis values or events.
fn check_lfos(p: &Project, reg: &crate::controls::ControlRegistry) -> Result<()> {
    crate::lfo::validate(&p.lfos, reg).map_err(|e| anyhow::anyhow!("LFO : {e}"))?;
    crate::audio::routes::validate(&p.audio_routes, reg).map_err(|e| anyhow::anyhow!("liens audio : {e}"))
}

/// The project sections as they are now, without the timelines (on disk,
/// see `ShowStore::load_all`): called under the lock, so copies only.
pub fn snapshot(s: &Shared) -> Project {
    Project {
        scenes: s.scenes.list().to_vec(),
        playlist: s.scenes.list().iter().map(|sc| sc.name.clone()).collect(),
        grid: s.deck.clone(),
        tempo: TempoDefaults { bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar },
        live: s.live.clone(),
        layers: s.mixer.clone(),
        lfos: s.lfos.list().to_vec(),
        audio_routes: s.routes.routing().clone(),
        palettes: s.palettes.list().to_vec(),
        midi: MidiSection { profiles: s.midi.store.user_profiles().clone() },
        figures: s.figures.list().to_vec(),
        ..Project::default()
    }
}

/// Swaps a checked project into memory. Infallible, so the switch is all
/// or nothing. Transport stops (playlist, timeline); playing cues stay.
/// Never touches calibration, safety, presence, MIDI safety, the gate or
/// the e-stop.
fn apply(s: &mut Shared, p: &Project) {
    s.scenes.replace_in_memory(p.scenes.clone());
    s.playlist = None;
    s.deck.set_config(&p.grid);
    s.timeline.stop();
    let t = s.now_s();
    s.tempo.set_bpm_manual(p.tempo.bpm, t);
    s.tempo.beats_per_bar = p.tempo.beats_per_bar;
    s.live = p.live.clone();
    s.mixer = p.layers.clone();
    s.lfos.replace_in_memory(p.lfos.clone());
    s.routes.replace_in_memory(p.audio_routes.clone(), &s.controls);
    s.palettes.replace_in_memory(p.palettes.clone());
    s.midi.store.replace_user_in_memory(p.midi.profiles.clone());
    s.figures.replace_in_memory(p.figures.clone());
    crate::figures::refresh(s);
    // Saved below, not by the engine thread.
    s.live_dirty = false;
    s.mixer_dirty = false;
    s.settings_rev += 1;
}

/// Where the working copy lives, read under the lock.
struct WorkingPaths {
    scenes: PathBuf,
    grid: Option<PathBuf>,
    shows: PathBuf,
    lfos: PathBuf,
    routes: PathBuf,
    palettes: PathBuf,
    midi: Option<PathBuf>,
    figures: Option<PathBuf>,
    live: PathBuf,
    layers: PathBuf,
}

fn working_paths(s: &Shared, data_dir: &Path) -> WorkingPaths {
    WorkingPaths {
        scenes: s.scenes.path().to_path_buf(),
        grid: s.deck.path().map(Path::to_path_buf),
        shows: s.shows.dir().to_path_buf(),
        lfos: s.lfos.path().to_path_buf(),
        routes: s.routes.path().to_path_buf(),
        palettes: s.palettes.path().to_path_buf(),
        midi: s.midi.store.dir().map(|d| d.join("profiles")),
        figures: s.figures.dir().map(Path::to_path_buf),
        live: data_dir.join("live.json"),
        layers: data_dir.join("layers.json"),
    }
}

/// Writes the opened project into the working copy, lock released.
/// Returns the files that could not be written (the state in memory is
/// already the project's).
fn persist(w: &WorkingPaths, p: &Project) -> Vec<String> {
    let mut errors = Vec::new();
    let mut put = |path: &Path, bytes: Result<Vec<u8>, serde_json::Error>| {
        if let Err(e) = bytes.map_err(anyhow::Error::from).and_then(|b| write_atomic(path, &b)) {
            errors.push(format!("{} : {e:#}", path.display()));
        }
    };
    put(&w.scenes, serde_json::to_vec_pretty(&p.scenes));
    if let Some(grid) = &w.grid {
        put(grid, serde_json::to_vec_pretty(&p.grid));
    }
    put(&w.lfos, serde_json::to_vec_pretty(&p.lfos));
    put(&w.routes, serde_json::to_vec_pretty(&p.audio_routes));
    put(&w.palettes, serde_json::to_vec_pretty(&p.palettes));
    put(&w.live, serde_json::to_vec_pretty(&p.live));
    put(&w.layers, serde_json::to_vec_pretty(&p.layers));
    for show in &p.timelines {
        put(&w.shows.join(format!("{}.json", show.name)), serde_json::to_vec_pretty(show));
    }
    if let Some(dir) = &w.midi {
        for (slug, profile) in &p.midi.profiles {
            put(&dir.join(format!("{slug}.json")), serde_json::to_vec_pretty(profile));
        }
    }
    if let Some(dir) = &w.figures {
        for fig in &p.figures {
            put(&dir.join(format!("{}.json", fig.name)), serde_json::to_vec_pretty(fig));
        }
    }
    // The previous project's timelines, MIDI profiles and figures go: only plain
    // `<name>.json` files directly in those folders, never anything else.
    let keep_shows: HashSet<&str> = p.timelines.iter().map(|s| s.name.as_str()).collect();
    let keep_midi: HashSet<&str> = p.midi.profiles.keys().map(String::as_str).collect();
    let keep_figures: HashSet<&str> = p.figures.iter().map(|f| f.name.as_str()).collect();
    let mut prune = |dir: &Path, keep: &HashSet<&str>, valid: &dyn Fn(&str) -> bool| {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in entries.flatten().map(|e| e.path()) {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let is_json = path.extension().is_some_and(|x| x == "json") && path.is_file();
            if is_json && valid(stem) && !keep.contains(stem) {
                if let Err(e) = std::fs::remove_file(&path) {
                    errors.push(format!("{} : {e}", path.display()));
                }
            }
        }
    };
    prune(&w.shows, &keep_shows, &valid_show_name);
    if let Some(dir) = &w.midi {
        prune(dir, &keep_midi, &valid_slug);
    }
    if let Some(dir) = &w.figures {
        prune(dir, &keep_figures, &valid_show_name);
    }
    errors
}

/// Header fields for a save.
fn stamp(p: &mut Project) {
    p.format_version = PROJECT_FORMAT;
    p.app_version = env!("CARGO_PKG_VERSION").to_string();
    p.saved_at = utc_now();
}

fn write_project(path: &Path, p: &Project) -> Result<()> {
    let mut json = serde_json::to_vec_pretty(p).context("échec de la mise en forme du projet")?;
    json.push(b'\n');
    write_atomic(path, &json)
}

/// Atomic write: a hidden `.tmp` file in the same folder, flushed to disk,
/// then renamed over the target. A crash leaves either the old file or
/// the new one, never half of it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = write_tmp(path, bytes)?;
    commit(&tmp, path)
}

fn tmp_path(path: &Path) -> PathBuf {
    let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("fichier");
    path.with_file_name(format!(".{file}.tmp"))
}

fn write_tmp(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let dir = path.parent().context("chemin sans dossier")?;
    std::fs::create_dir_all(dir).with_context(|| format!("impossible de créer {}", dir.display()))?;
    let tmp = tmp_path(path);
    let mut f = std::fs::File::create(&tmp).with_context(|| format!("impossible d'écrire {}", tmp.display()))?;
    f.write_all(bytes).and_then(|()| f.sync_all()).with_context(|| format!("impossible d'écrire {}", tmp.display()))?;
    Ok(tmp)
}

fn commit(tmp: &Path, path: &Path) -> Result<()> {
    std::fs::rename(tmp, path).with_context(|| format!("impossible d'écrire {}", path.display()))?;
    // The rename itself reaches the disk with the folder.
    if let Some(dir) = path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Reply of `route`: JSON, or a status and a French message.
pub enum Reply {
    Json(Value),
    Text(u16, String),
}

#[derive(Deserialize)]
struct PathRequest {
    path: String,
}

/// `GET /api/project`, `POST /api/project/{new,open,save,save-as}`.
/// Runs on the HTTP worker thread; the lock is held only to copy the
/// state out or swap it in, never for file I/O.
pub fn route(shared: &Mutex<Shared>, post: bool, action: &str, body: &str) -> Option<Reply> {
    let path_req = || serde_json::from_str::<PathRequest>(body).map_err(|e| Reply::Text(400, format!("JSON invalide : {e}")));
    let reply = match (post, action) {
        (false, "") => return Some(Reply::Json(info(shared))),
        (true, "new") => open_project(shared, None),
        (true, "open") => match path_req() {
            Ok(req) => {
                let dir = shared.lock().unwrap().project.dir.clone();
                match resolve(&dir, &req.path) {
                    Ok(path) => open_project(shared, Some(&path)),
                    Err(e) => Err(Reply::Text(400, format!("{e:#}"))),
                }
            }
            Err(e) => Err(e),
        },
        (true, "save") => {
            let current = shared.lock().unwrap().project.current.clone();
            match current {
                Some(name) => save_project(shared, &name),
                None => Err(Reply::Text(409, "projet sans nom : utilisez « Enregistrer sous… »".into())),
            }
        }
        (true, "save-as") => match path_req() {
            Ok(req) => {
                let dir = shared.lock().unwrap().project.dir.clone();
                match resolve(&dir, &req.path) {
                    Ok(path) => save_project(shared, &name_of(&path)),
                    Err(e) => Err(Reply::Text(400, format!("{e:#}"))),
                }
            }
            Err(e) => Err(e),
        },
        _ => return None,
    };
    Some(match reply {
        Ok(warnings) => {
            let mut v = info(shared);
            v["warnings"] = json!(warnings);
            Reply::Json(v)
        }
        Err(r) => r,
    })
}

/// Opens `path`, or a new empty project with `None`. Atomic: nothing
/// changes unless the whole file is valid.
fn open_project(shared: &Mutex<Shared>, path: Option<&Path>) -> Result<Vec<String>, Reply> {
    let mut p = match path {
        Some(path) => read(path).map_err(|e| {
            let code = if path.exists() { 400 } else { 404 };
            Reply::Text(code, format!("{e:#}"))
        })?,
        None => Project::default(),
    };
    let name = path.map(name_of);
    p.name = name.clone().unwrap_or_else(|| UNTITLED.into());
    let w = {
        let mut s = shared.lock().unwrap();
        check_lfos(&p, &s.controls).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
        apply(&mut s, &p);
        let data_dir = s.project.data_dir.clone();
        working_paths(&s, &data_dir)
    };
    let warnings = persist(&w, &p);
    for e in &warnings {
        log::warn!("project open: failed to write {e}");
    }
    let mut s = shared.lock().unwrap();
    let mut now = snapshot(&s);
    now.timelines = p.timelines.clone();
    s.project.saved = now.fingerprint();
    s.project.extra = std::mem::take(&mut p.extra);
    match &name {
        Some(name) => s.project.remember(name),
        None => s.project.current = None,
    }
    s.project.save_recent();
    Ok(warnings.into_iter().map(|e| format!("impossible d'écrire {e}")).collect())
}

/// Saves the current state as project `name` (in `projects/`).
fn save_project(shared: &Mutex<Shared>, name: &str) -> Result<Vec<String>, Reply> {
    let (mut p, path, shows) = {
        let s = shared.lock().unwrap();
        let mut p = snapshot(&s);
        p.extra = s.project.extra.clone();
        (p, s.project.path_of(name), crate::timeline::ShowStore::new(s.shows.dir().to_path_buf()))
    };
    p.timelines = shows.load_all();
    p.name = name.to_string();
    stamp(&mut p);
    write_project(&path, &p).map_err(|e| Reply::Text(500, format!("échec de l'enregistrement : {e:#}")))?;
    let mut s = shared.lock().unwrap();
    s.project.saved = p.fingerprint();
    s.project.remember(name);
    s.project.save_recent();
    Ok(Vec::new())
}

/// `GET /api/project`: name, file, whether it changed since it was opened
/// or saved, the projects folder's files and the recent list.
fn info(shared: &Mutex<Shared>) -> Value {
    let (mut p, current, saved, dir, recent, shows) = {
        let s = shared.lock().unwrap();
        let pr = &s.project;
        (snapshot(&s), pr.current.clone(), pr.saved, pr.dir.clone(), pr.recent.clone(), crate::timeline::ShowStore::new(s.shows.dir().to_path_buf()))
    };
    p.timelines = shows.load_all();
    let projects = list(&dir);
    let recent: Vec<&String> = recent.iter().filter(|n| projects.contains(n)).collect();
    json!({
        "name": current.clone().unwrap_or_else(|| UNTITLED.into()),
        "file": current.as_ref().map(|n| dir.join(format!("{n}.{EXTENSION}"))),
        "modified": p.fingerprint() != saved,
        "dir": dir,
        "projects": projects,
        "recent": recent,
    })
}

/// Project names in `dir`, sorted (temporary and hidden files skipped).
pub fn list(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == EXTENSION) && p.is_file())
        .map(|p| name_of(&p))
        .filter(|n| valid_name(n))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Settings;
    use crate::test_support;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn scene(name: &str, scale: f32) -> Scene {
        Scene { name: name.into(), settings: Settings { scale, ..Default::default() }, duration_secs: 4.0 }
    }

    /// A studio state whose stores all live in `dir`.
    fn studio(dir: &Path) -> Mutex<Shared> {
        let mut s = test_support::shared();
        s.scenes = crate::scenes::SceneStore::load_or_create(dir.join("scenes.json"));
        s.deck = CueDeck::load(dir.join("grid.json"));
        s.shows = crate::timeline::ShowStore::new(dir.join("shows"));
        s.lfos = crate::lfo::LfoStore::load_or_create(dir.join("lfos.json"), &s.controls);
        s.routes = crate::audio::routes::RouteStore::load_or_create(dir.join("audio_routes.json"), &s.controls);
        s.palettes = crate::live::PaletteStore::load_or_create(dir.join("palettes.json"));
        s.midi.store = ProfileStore::load(dir.join("midi"));
        s.figures = crate::figures::FigureStore::load(dir.join("figures"));
        s.project = ProjectState::load(dir);
        startup(&mut s);
        Mutex::new(s)
    }

    fn post(shared: &Mutex<Shared>, _dir: &Path, action: &str, body: Value) -> Result<Value, (u16, String)> {
        match route(shared, true, action, &body.to_string()).expect("known route") {
            Reply::Json(v) => Ok(v),
            Reply::Text(code, t) => Err((code, t)),
        }
    }

    fn modified(shared: &Mutex<Shared>) -> bool {
        info(shared)["modified"].as_bool().unwrap()
    }

    #[test]
    fn save_change_open_restores_scenes_playlist_grid_and_more() {
        let dir = temp_dir("roundtrip");
        let shared = studio(&dir);
        {
            let mut s = shared.lock().unwrap();
            s.scenes.upsert(scene("Intro", 0.3)).unwrap();
            s.scenes.upsert(scene("Drop", 0.8)).unwrap();
            let id = s.presets[0].id.clone();
            s.deck.set_slot(&id, crate::cues::CueSlot { group: Some(2), layer: Some(3), ..Default::default() });
            s.deck.multi = true;
            s.live.size = 0.5;
            s.mixer.layers[1].dimmer = 0.25;
            let t = s.now_s();
            s.tempo.set_bpm_manual(128.0, t);
            s.shows.save(&Show { name: "Ouverture".into(), ..Default::default() }).unwrap();
        }
        assert!(modified(&shared));
        post(&shared, &dir, "save-as", json!({ "path": "Mon show" })).unwrap();
        assert!(!modified(&shared), "just saved");
        let saved_file = std::fs::read_to_string(dir.join("projects/Mon show.lsproj")).unwrap();
        let before = parse(&saved_file).unwrap();
        assert_eq!(before.playlist, ["Intro", "Drop"]);
        assert_eq!(before.timelines.len(), 1);

        // Change everything, then open the project again.
        {
            let mut s = shared.lock().unwrap();
            s.scenes.upsert(scene("Autre", 1.0)).unwrap();
            s.scenes.remove("Intro").unwrap();
            s.deck.multi = false;
            s.live.size = 1.5;
            let t = s.now_s();
            s.tempo.set_bpm_manual(90.0, t);
            s.shows.save(&Show { name: "Rappel".into(), ..Default::default() }).unwrap();
        }
        assert!(modified(&shared));
        let reply = post(&shared, &dir, "open", json!({ "path": "Mon show.lsproj" })).unwrap();
        assert_eq!(reply["name"], "Mon show");
        assert!(!modified(&shared), "just opened");
        let s = shared.lock().unwrap();
        let mut now = snapshot(&s);
        now.timelines = s.shows.load_all();
        assert_eq!(serde_json::to_value(&now.scenes).unwrap(), serde_json::to_value(&before.scenes).unwrap());
        assert_eq!(now.playlist, before.playlist);
        assert_eq!(serde_json::to_value(&now.grid).unwrap(), serde_json::to_value(&before.grid).unwrap());
        assert_eq!(now.fingerprint(), before.fingerprint(), "every section is back");
        assert_eq!(s.tempo.bpm, 128.0);
        assert_eq!(now.timelines.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Ouverture"], "the other timeline went");
        // The working copy on disk is the project's too.
        let on_disk: Vec<Scene> = serde_json::from_str(&std::fs::read_to_string(dir.join("scenes.json")).unwrap()).unwrap();
        assert_eq!(on_disk.len(), 2);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn audio_routes_are_saved_in_the_project_and_old_projects_still_open() {
        use crate::audio::routes::{AudioRoute, AudioRouting};
        let dir = temp_dir("routes");
        let shared = studio(&dir);
        let routing = AudioRouting { mix: 0.7, routes: vec![AudioRoute { source: "kick".into(), target: "master.brightness".into(), ..Default::default() }, AudioRoute::default()] };
        {
            let s = &mut *shared.lock().unwrap();
            s.routes.set(routing.clone(), &s.controls).unwrap();
        }
        assert!(modified(&shared), "a route is a change");
        post(&shared, &dir, "save-as", json!({ "path": "Avec routes" })).unwrap();
        {
            let s = &mut *shared.lock().unwrap();
            s.routes.set(AudioRouting::default(), &s.controls).unwrap();
        }
        post(&shared, &dir, "open", json!({ "path": "Avec routes" })).unwrap();
        assert_eq!(shared.lock().unwrap().routes.routing(), &routing, "reopened identical");
        let on_disk: AudioRouting = serde_json::from_str(&std::fs::read_to_string(dir.join("audio_routes.json")).unwrap()).unwrap();
        assert_eq!(on_disk, routing, "the working copy is the project's");
        // A project from before T-153 opens with no routes.
        std::fs::write(dir.join("projects/Ancien.lsproj"), json!({ "format_version": 1, "scenes": [] }).to_string()).unwrap();
        post(&shared, &dir, "open", json!({ "path": "Ancien" })).unwrap();
        assert_eq!(shared.lock().unwrap().routes.routing(), &AudioRouting::default());
        assert!(!modified(&shared));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn figures_are_saved_in_the_project_and_old_projects_still_open() {
        let dir = temp_dir("figures");
        let shared = studio(&dir);
        let logo = crate::figures::test_figure("Logo", 3);
        {
            let mut s = shared.lock().unwrap();
            s.figures.save(logo.clone()).unwrap();
            crate::figures::refresh(&mut s);
        }
        assert!(modified(&shared), "a new figure is a change");
        post(&shared, &dir, "save-as", json!({ "path": "Avec figures" })).unwrap();
        {
            let mut s = shared.lock().unwrap();
            s.figures.save(crate::figures::test_figure("Autre", 1)).unwrap();
            s.figures.remove("Logo").unwrap();
            crate::figures::refresh(&mut s);
        }
        post(&shared, &dir, "open", json!({ "path": "Avec figures" })).unwrap();
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.figures.list(), &[logo.clone()][..], "reopened identical");
            assert!(s.presets.iter().any(|p| p.id == "figure:Logo"), "its cue is back");
            assert!(!s.presets.iter().any(|p| p.id == "figure:Autre"));
        }
        assert!(dir.join("figures/Logo.json").is_file());
        assert!(!dir.join("figures/Autre.json").exists(), "the working copy is the project's");
        // A project from before figures (no « figures » section) opens, with none.
        std::fs::write(dir.join("projects/ancien.lsproj"), json!({ "format_version": 1, "name": "ancien" }).to_string()).unwrap();
        post(&shared, &dir, "open", json!({ "path": "ancien" })).unwrap();
        assert!(shared.lock().unwrap().figures.list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_fields_survive_a_save() {
        let dir = temp_dir("unknown");
        let shared = studio(&dir);
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        let text = json!({ "format_version": 1, "name": "x", "venue": { "room": [10, 4, 3] }, "pages": [1, 2] }).to_string();
        std::fs::write(dir.join("projects/x.lsproj"), text).unwrap();
        post(&shared, &dir, "open", json!({ "path": "x" })).unwrap();
        post(&shared, &dir, "save", json!({})).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("projects/x.lsproj")).unwrap()).unwrap();
        assert_eq!(v["venue"], json!({ "room": [10, 4, 3] }));
        assert_eq!(v["pages"], json!([1, 2]));
        assert_eq!(v["format_version"], 1);
        assert!(v["saved_at"].as_str().unwrap().ends_with('Z'));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_never_arms_nor_touches_calibration_safety_or_midi_safety() {
        let dir = temp_dir("safety");
        let shared = studio(&dir);
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        // A file that tries its luck with everything that is not its business.
        let text = json!({
            "format_version": 1, "armed": true, "arm": true, "estop": false,
            "calibration": { "offset_x": 5.0, "scale_x": 9.0 },
            "safety": { "max_flash_hz": 100.0 }, "presence": { "hold_to_run": false },
            "midi": { "profiles": {}, "safety": { "allow_arm": true }, "devices": { "safety": { "allow_arm": true } } },
        })
        .to_string();
        std::fs::write(dir.join("projects/piège.lsproj"), text).unwrap();
        let before = {
            let mut s = shared.lock().unwrap();
            s.emergency_stop(crate::interlock::ArmSource::Keyboard);
            (s.calibration, serde_json::to_value(s.safety.get()).unwrap(), s.midi.store.devices.safety, serde_json::to_value(&s.presence.settings).unwrap())
        };
        post(&shared, &dir, "open", json!({ "path": "piège" })).unwrap();
        post(&shared, &dir, "new", json!({})).unwrap();
        let s = shared.lock().unwrap();
        assert!(!s.gate.is_armed());
        assert!(s.estop.is_latched(), "the e-stop stays latched");
        assert_eq!(s.calibration, before.0);
        assert_eq!(serde_json::to_value(s.safety.get()).unwrap(), before.1);
        assert_eq!(s.midi.store.devices.safety, before.2);
        assert!(!s.midi.store.devices.safety.allow_arm);
        assert_eq!(serde_json::to_value(&s.presence.settings).unwrap(), before.3);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_invalid_project_changes_nothing() {
        let dir = temp_dir("atomic");
        let shared = studio(&dir);
        shared.lock().unwrap().scenes.upsert(scene("Garde", 0.4)).unwrap();
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        let scenes = json!([{ "name": "A", "settings": Settings::default(), "duration_secs": 1.0 }]);
        let bad = [
            ("json", "{ pas du json".to_string()),
            ("version", json!({ "format_version": 99, "scenes": scenes }).to_string()),
            ("noversion", json!({ "scenes": scenes }).to_string()),
            ("dup", json!({ "format_version": 1, "scenes": [scenes[0], scenes[0]] }).to_string()),
            ("show", json!({ "format_version": 1, "scenes": scenes, "timelines": [{ "name": "../../evil" }] }).to_string()),
            ("lfo", json!({ "format_version": 1, "scenes": scenes, "lfos": [{ "target": "safety.estop" }] }).to_string()),
            ("route", json!({ "format_version": 1, "scenes": scenes, "audio_routes": { "routes": [{ "source": "kick", "target": "transport.arm" }] } }).to_string()),
            ("routesrc", json!({ "format_version": 1, "scenes": scenes, "audio_routes": { "routes": [{ "source": "volume" }] } }).to_string()),
            ("midi", json!({ "format_version": 1, "scenes": scenes, "midi": { "profiles": { "../x": {} } } }).to_string()),
            ("palette", json!({ "format_version": 1, "scenes": scenes, "palettes": [{ "name": "", "colors": [] }] }).to_string()),
            ("figure", json!({ "format_version": 1, "scenes": scenes, "figures": [{ "name": "../../evil" }] }).to_string()),
            ("figdup", json!({ "format_version": 1, "scenes": scenes, "figures": [{ "name": "a" }, { "name": "a" }] }).to_string()),
        ];
        for (name, text) in bad {
            std::fs::write(dir.join(format!("projects/{name}.lsproj")), text).unwrap();
            let (code, msg) = post(&shared, &dir, "open", json!({ "path": name })).unwrap_err();
            assert_eq!(code, 400, "{name}: {msg}");
            let s = shared.lock().unwrap();
            assert_eq!(s.scenes.list().len(), 1, "{name}");
            assert_eq!(s.scenes.list()[0].name, "Garde");
        }
        assert!(!dir.join("evil.json").exists());
        let (code, msg) = post(&shared, &dir, "open", json!({ "path": "version" })).unwrap_err();
        assert!(code == 400 && msg.contains("plus récente"), "{msg}");
        let (code, _) = post(&shared, &dir, "open", json!({ "path": "absent" })).unwrap_err();
        assert_eq!(code, 404);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn paths_outside_the_projects_folder_are_refused() {
        let dir = temp_dir("paths");
        let projects = dir.join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        for bad in ["", "../x", "../../etc/passwd", "/etc/passwd", "a/b", "..", ".", ".cache", "x.json", "a\\b", "nom:bizarre", "x\0y"] {
            assert!(resolve(&projects, bad).is_err(), "{bad:?}");
        }
        let other = dir.join("autre");
        std::fs::create_dir_all(&other).unwrap();
        assert!(resolve(&projects, other.join("x.lsproj").to_str().unwrap()).is_err());
        assert!(resolve(&projects, projects.join("../x.lsproj").to_str().unwrap()).is_err());
        assert_eq!(resolve(&projects, "Mon show").unwrap(), projects.join("Mon show.lsproj"));
        assert_eq!(resolve(&projects, " Été 2026.lsproj ").unwrap(), projects.join("Été 2026.lsproj"));
        assert_eq!(resolve(&projects, projects.join("Fête.lsproj").to_str().unwrap()).unwrap(), projects.join("Fête.lsproj"));

        let shared = studio(&dir);
        for action in ["open", "save-as"] {
            let (code, msg) = post(&shared, &dir, action, json!({ "path": "../scenes" })).unwrap_err();
            assert_eq!(code, 400);
            assert!(msg.contains("chemin refusé"), "{msg}");
        }
        assert!(!dir.join("scenes.lsproj").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_symlink_in_the_projects_folder_is_not_followed() {
        let dir = temp_dir("symlink");
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        std::fs::write(dir.join("secret.json"), r#"{"format_version": 1}"#).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("secret.json"), dir.join("projects/lien.lsproj")).unwrap();
            assert!(read(&dir.join("projects/lien.lsproj")).is_err());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_partial_write_is_never_visible() {
        let dir = temp_dir("atomicwrite");
        let path = dir.join("p.lsproj");
        write_atomic(&path, b"ancien").unwrap();
        // Crash between the write and the rename: the old file is intact,
        // and the temporary file is hidden from the project list.
        let tmp = write_tmp(&path, b"nouv").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"ancien");
        assert_eq!(list(&dir), ["p"]);
        commit(&tmp, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"nouv");
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn first_start_imports_scenes_json_and_keeps_it() {
        let dir = temp_dir("import");
        let scenes = serde_json::to_string(&vec![scene("Ancienne", 0.5)]).unwrap();
        std::fs::write(dir.join("scenes.json"), &scenes).unwrap();
        let shared = studio(&dir);
        let p = read(&dir.join("projects/Sans titre.lsproj")).unwrap();
        assert_eq!(p.scenes[0].name, "Ancienne");
        assert_eq!(std::fs::read_to_string(dir.join("scenes.json")).unwrap(), scenes, "not deleted");
        let v = info(&shared);
        assert_eq!(v["name"], "Sans titre");
        assert_eq!(v["modified"], false);
        assert_eq!(v["recent"], json!(["Sans titre"]));
        // A second start doesn't import again.
        drop(shared);
        let shared = studio(&dir);
        assert_eq!(list(&dir.join("projects")), ["Sans titre"]);
        assert_eq!(info(&shared)["modified"], false);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_empty_data_dir_starts_untitled_without_a_file() {
        let dir = temp_dir("empty");
        let shared = studio(&dir);
        let v = info(&shared);
        assert_eq!((v["name"].as_str(), v["file"].is_null(), v["modified"].as_bool()), (Some("Sans titre"), true, Some(false)));
        let (code, _) = post(&shared, &dir, "save", json!({})).unwrap_err();
        assert_eq!(code, 409, "no file yet: Enregistrer sous…");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_projects_are_capped_and_most_recent_first() {
        let dir = temp_dir("recent");
        let shared = studio(&dir);
        for i in 0..12 {
            post(&shared, &dir, "save-as", json!({ "path": format!("p{i}") })).unwrap();
        }
        post(&shared, &dir, "open", json!({ "path": "p3" })).unwrap();
        let recent = info(&shared)["recent"].clone();
        assert_eq!(recent.as_array().unwrap().len(), 10);
        assert_eq!(recent[0], "p3");
        assert_eq!(recent[1], "p11");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_a_large_project_holds_the_lock_only_briefly() {
        let dir = temp_dir("large");
        let shared = studio(&dir);
        {
            let mut s = shared.lock().unwrap();
            let scenes: Vec<Scene> = (0..1000).map(|i| scene(&format!("Scène {i}"), 0.5)).collect();
            s.scenes.replace_in_memory(scenes);
        }
        // The engine takes the lock every frame: time how long the save
        // keeps it (copying 1000 scenes), which is all the engine can wait.
        let start = std::time::Instant::now();
        let held = {
            let s = shared.lock().unwrap();
            let t = std::time::Instant::now();
            let p = snapshot(&s);
            assert_eq!(p.scenes.len(), 1000);
            t.elapsed()
        };
        assert!(held < std::time::Duration::from_millis(16), "snapshot under the lock took {held:?}");
        post(&shared, &dir, "save-as", json!({ "path": "gros" })).unwrap();
        assert_eq!(read(&dir.join("projects/gros.lsproj")).unwrap().scenes.len(), 1000);
        log::debug!("whole save: {:?}", start.elapsed());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn utc_dates_are_iso() {
        let d = utc_now();
        assert_eq!(d.len(), 20);
        assert!(d.starts_with("20") && d.ends_with('Z') && &d[10..11] == "T");
    }
}
