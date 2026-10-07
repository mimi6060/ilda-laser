//! Project files (T-286): one `.lsproj` file holds a whole show - scenes
//! and playlist, cue-grid properties, timelines, default tempo, master
//! live modifiers, layers, LFOs, audio routes (T-153), user palettes, MIDI mappings, the
//! figure library (T-296), the user cues (T-405) and the cue lists (T-410) - as
//! pretty-printed, versioned JSON in `<data-dir>/projects/`.
//!
//! The data directory's own files (`scenes.json`, `grid.json`, `shows/`…)
//! stay the working copy the studio runs from; a project is a snapshot of
//! them. *Enregistrer* writes the snapshot, *Ouvrir* replaces them.
//!
//! What is **not** in a project, and never changed by opening one: the
//! calibration, the safety settings (strobe limiter, horizon, hold-to-run,
//! heartbeat), the output power caps and projector sheets (`outputs.json`),
//! the MIDI safety options (`midi/devices.json`), the arming
//! state and the e-stop. They belong to the rig and the venue (the site
//! profile, sites.rs, T-289), so a project made elsewhere can't loosen
//! them. A project may carry a site profile (key `site`, kept with its
//! unknown sections): opening never applies it, the operator chooses.
//! *Importer depuis un projet…* (T-289) merges chosen sections of another
//! project into the open one. Nothing here can arm the laser.
//!
//! Opening is atomic: the file is read and every section checked before
//! anything changes; on error the current state is untouched and the
//! reply says why, in French. Files are only ever read or written inside
//! `projects/`, whatever name or path is asked for or found in a file.
//! Writes are atomic (`.tmp`, fsync, rename) and happen on the HTTP worker
//! thread with the engine lock released: the lock is only held to copy
//! the state out, or to swap the checked state in.

use crate::audio::routes::AudioRouting;
use crate::cuelist::CueList;
use crate::cues::CueDeck;
use crate::executors::ExecutorBank;
use crate::figures::Figure;
use crate::grid::{CueGrid, CuePage, CueRef};
use crate::layers::Mixer;
use crate::lfo::Modulator;
use crate::live::{LiveModifiers, Palette};
use crate::midi::profile::{valid_slug, Profile, ProfileStore};
use crate::scan::ScanPreset;
use crate::scenes::Scene;
use crate::tempo::{MAX_BPM, MIN_BPM};
use crate::timeline::{valid_show_name, Show};
use crate::usercues::UserCue;
use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Current format (T-288): `MIGRATIONS` leads every older one here.
pub const PROJECT_FORMAT: u32 = 1;
/// Project migrations: `MIGRATIONS[i]` takes format `i + 1` to `i + 2`,
/// on the raw JSON, before decoding (`formats::run_chain`). Format 1 is
/// the first, so none yet; a new format adds its step here and bumps
/// `PROJECT_FORMAT` (a test checks they agree), with a fixture of the
/// previous format in `tests/fixtures/projects/`.
pub const MIGRATIONS: &[crate::formats::Step] = &[];
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
    /// Cue-grid size and pages (`pages.json`, T-272). Missing in projects
    /// saved before T-272: they open with the default 8×5 pages.
    pub pages: CueGrid,
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
    /// The operator's own cues (`user_cues.json`, T-405). Missing in
    /// projects saved before T-405: they open with none.
    pub user_cues: Vec<UserCue>,
    /// Cue lists with « Go » (`cuelists.json`, T-410). Missing in projects
    /// saved before T-410: they open with none.
    #[serde(rename = "cuelists")]
    pub cue_lists: Vec<CueList>,
    /// User scan presets (`scan_presets.json`, T-415). Missing in projects
    /// saved before T-415: they open with none (the built-in ones are
    /// always there).
    pub scan_presets: Vec<ScanPreset>,
    /// The zone groups' masters (T-420), in group order: playing values,
    /// like the layer dimmers (the groups themselves are machine settings,
    /// projection.json). Missing in projects saved before T-420: every
    /// group opens at 100 %.
    pub group_masters: Vec<f32>,
    /// The executor bank's assignments (`executors.json`, T-418). Missing
    /// in projects saved before T-418: they open with one empty page.
    pub executors: ExecutorBank,
    /// Speed groups A..D and the *Cadence* (T-419). Missing in projects
    /// saved before T-419: every group at ×1, *Cadence* 100 %.
    pub speed: crate::tempo::SpeedGroups,
    /// The custom desk's tabs (`desks.json`, T-427). Missing in projects
    /// saved before T-427: they open with an empty desk.
    pub desks: Vec<crate::desk::DeskTab>,
    /// Macros (`macros.json`, T-425). Missing in projects saved before
    /// T-425: they open with none. A macro whose line no longer parses
    /// opens disabled, with a warning.
    pub macros: Vec<crate::macros::Macro>,
    /// The show player's list (`showlist.json`, T-444). Missing in projects
    /// saved before T-444: they open with an empty list.
    pub show_list: crate::showlist::ShowList,
    /// Referenced presets (`pools.json`, T-422). Missing in projects saved
    /// before T-422: they open with none (and cues without references).
    pub pools: Vec<crate::pools::PoolPreset>,
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
            pages: CueGrid::default(),
            timelines: Vec::new(),
            tempo: TempoDefaults::default(),
            live: LiveModifiers::default(),
            layers: Mixer::default(),
            lfos: Vec::new(),
            audio_routes: AudioRouting::default(),
            palettes: Vec::new(),
            midi: MidiSection::default(),
            figures: Vec::new(),
            user_cues: Vec::new(),
            cue_lists: Vec::new(),
            scan_presets: Vec::new(),
            group_masters: Vec::new(),
            executors: ExecutorBank::default(),
            speed: Default::default(),
            desks: Vec::new(),
            macros: Vec::new(),
            show_list: Default::default(),
            pools: Vec::new(),
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

/// What *Nouveau projet* takes from the open project (T-451). All false
/// when absent: an empty `POST /api/project/new` keeps nothing, as before.
/// The machine's settings (calibration, safety, venue, outputs, the
/// safety lock) are never part of a project: a new one always keeps them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct KeepOnNew {
    /// The MIDI mappings (`midi`).
    pub midi: bool,
    /// The colour palettes (`palettes`).
    pub palettes: bool,
    /// The operator's grid pages (T-405) with the user cues they point at,
    /// and the grid properties (`grid`).
    pub pages: bool,
    /// The site profile the project carries (key `site`, T-289): only
    /// carried, never applied.
    pub venue: bool,
}

/// Body of `POST /api/project/new`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NewRequest {
    pub keep: KeepOnNew,
    /// A template of `templates/`: the new project is an untitled copy of
    /// it, and `keep` does not apply.
    pub template: Option<String>,
}

/// Templates folder in the data directory (`<data-dir>/templates/`).
pub const TEMPLATES_DIR: &str = "templates";

/// An empty project that takes the ticked sections from `from`.
pub fn new_from(from: &Project, keep: KeepOnNew) -> Project {
    let mut p = Project::default();
    if keep.midi {
        p.midi = from.midi.clone();
    }
    if keep.palettes {
        p.palettes = from.palettes.clone();
    }
    if keep.pages {
        p.grid = from.grid.clone();
        p.pages = from.pages.clone();
        p.user_cues = from.user_cues.clone();
    }
    if keep.venue {
        if let Some(site) = from.extra.get(crate::sites::PROJECT_KEY) {
            p.extra.insert(crate::sites::PROJECT_KEY.into(), site.clone());
        }
    }
    p
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
    /// Whether the project differs from `saved`, as the autosave thread
    /// last saw it (every 500 ms) or as opening and saving left it: what
    /// `/api/frame` says (`project_modified`, T-433) without a fingerprint.
    pub last_modified: bool,
    /// Autosave settings, status and recovery offer (autosave.rs, T-287).
    pub autosave: crate::autosave::AutosaveState,
    /// The open project's file is from a newer version (its format): read
    /// only, *Enregistrer* refused so it is never written over (T-288).
    pub read_only: Option<u32>,
    /// The copy to a second location (backup.rs, T-450).
    pub backup: std::sync::Arc<crate::backup::BackupHub>,
}

impl ProjectState {
    /// Reads `recent.json`; `startup` completes it once the stores are loaded.
    pub fn load(data_dir: &Path) -> Self {
        let recent_path = data_dir.join("recent.json");
        let file: RecentFile = crate::load_json(&recent_path);
        Self { data_dir: data_dir.to_path_buf(), dir: data_dir.join("projects"), recent_path, current: file.current, recent: file.recent, extra: Map::new(), saved: 0, last_modified: false, autosave: crate::autosave::AutosaveState::load(data_dir), read_only: None, backup: crate::backup::BackupHub::load(data_dir) }
    }

    /// Fingerprint of the project as last opened or saved.
    pub fn saved_fingerprint(&self) -> u64 {
        self.saved
    }

    /// Unknown fields of the open project (and its site profile, T-289).
    pub fn extra(&self) -> &Map<String, Value> {
        &self.extra
    }

    #[cfg(test)]
    pub fn set_extra_for_tests(&mut self, extra: Map<String, Value>) {
        self.extra = extra;
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

    pub fn path_of(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.{EXTENSION}"))
    }
}

/// Files of the working copy: if any exists on a first start (no
/// `recent.json` yet), they are imported into a *Sans titre* project.
const WORKING_FILES: [&str; 17] = ["scenes.json", "grid.json", "pages.json", "live.json", "layers.json", "lfos.json", "audio_routes.json", "palettes.json", "shows", "figures", "user_cues.json", "cuelists.json", "executors.json", "desks.json", "macros.json", "showlist.json", "pools.json"];

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
        Some(name) => match open_in_data_dir(&s.project.path_of(&name), &data_dir) {
            Ok(loaded) => {
                s.project.extra = loaded.project.extra.clone();
                s.project.read_only = loaded.newer();
                if let Some(v) = loaded.newer() {
                    log::warn!("projet « {name} » : format {v}, plus récent que cette version : lecture seule");
                }
                loaded.project.fingerprint()
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
    // Unsaved work left by a crash (T-287): offered, never applied here.
    crate::autosave::find_offer(s);
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
    parse(&read_text(path)?)
}

/// The text of a project file, checked to be a plain file of sensible size.
fn read_text(path: &Path) -> Result<String> {
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
    std::fs::read_to_string(path).context("projet illisible")
}

/// `read` for a file of the data directory (the open project at startup,
/// an autosave): if it is not even JSON (truncated, binary), it is moved
/// to the quarantine folder and reported (T-402). A valid JSON file that
/// fails the checks (e.g. a newer format) stays where it is.
pub fn read_in_data_dir(path: &Path, data_dir: &Path) -> Result<Project> {
    read(path).inspect_err(|_| set_aside_if_damaged(path, data_dir))
}

/// `load_file` then `upgrade_file`, for the open project at startup, with
/// the same quarantine as `read_in_data_dir`.
pub fn open_in_data_dir(path: &Path, data_dir: &Path) -> Result<Loaded> {
    let (loaded, text) = load_file(path).inspect_err(|_| set_aside_if_damaged(path, data_dir))?;
    if let Err(e) = upgrade_file(path, &loaded, &text) {
        log::warn!("projet {} : mise à jour du format non écrite : {e:#}", path.display());
    }
    Ok(loaded)
}

fn set_aside_if_damaged(path: &Path, data_dir: &Path) {
    let q = data_dir.join(crate::datafile::QUARANTINE);
    let damaged = std::fs::read(path).is_ok_and(|b| serde_json::from_slice::<serde::de::IgnoredAny>(&b).is_err());
    if damaged {
        crate::datafile::set_aside(path, &q, "projet illisible (JSON invalide)", false);
    }
}

/// Parses and checks a project from JSON text, migrated to the current
/// format. A project from a newer version is refused (see `parse_file`
/// for opening one read only).
pub fn parse(text: &str) -> Result<Project> {
    let loaded = parse_file(text)?;
    if let Some(v) = loaded.newer() {
        bail!("{} (format {v}, cette version lit le format {PROJECT_FORMAT})", NEWER_FR);
    }
    Ok(loaded.project)
}

/// Why *Enregistrer* is refused on a project from a newer version.
pub const NEWER_FR: &str = "Projet créé par une version plus récente : ouverture en lecture seule";
/// Said once a project was migrated on opening.
pub const MIGRATED_FR: &str = "Projet mis à jour vers le nouveau format (copie de sauvegarde créée)";

/// A project file as read: checked, migrated to the current format, or
/// (newer format) decoded as is, for a read-only opening.
pub struct Loaded {
    pub project: Project,
    /// The format written in the file.
    pub from: u32,
    /// The format this version writes.
    pub current: u32,
}

impl Loaded {
    /// Format of a file from an older version, migrated in memory.
    pub fn migrated(&self) -> Option<u32> {
        (self.from < self.current).then_some(self.from)
    }

    /// Format of a file from a newer version.
    pub fn newer(&self) -> Option<u32> {
        (self.from > self.current).then_some(self.from)
    }
}

fn version_of(v: &Value) -> Result<u32> {
    let Some(obj) = v.as_object() else { bail!("projet illisible : ce n'est pas un fichier projet") };
    match obj.get("format_version").and_then(Value::as_u64) {
        None => bail!("projet illisible : « format_version » manquant"),
        Some(0) => bail!("projet illisible : « format_version » invalide"),
        Some(v) => Ok(u32::try_from(v).unwrap_or(u32::MAX)),
    }
}

/// `parse` without refusing a newer format: its JSON is decoded as is, its
/// unknown sections kept in `extra`; the caller opens it read only. A
/// newer file whose known sections no longer decode is refused, saying so.
pub fn parse_file(text: &str) -> Result<Loaded> {
    parse_with(text, MIGRATIONS)
}

fn parse_with(text: &str, chain: &[crate::formats::Step]) -> Result<Loaded> {
    let value: Value = serde_json::from_str(text).map_err(|e| anyhow::anyhow!("projet illisible (JSON invalide : {e})"))?;
    let from = version_of(&value)?;
    let current = crate::formats::current_of(chain);
    let newer = from > current;
    let value = if newer { value } else { crate::formats::run_chain(value, from, chain).map_err(|e| anyhow::anyhow!("projet illisible : {e:#}"))? };
    let decoded: Result<Project> = serde_json::from_value(value).map_err(|e| anyhow::anyhow!("projet illisible : {e}")).and_then(|mut p| {
        check(&mut p)?;
        Ok(p)
    });
    let project = match decoded {
        Ok(p) => p,
        Err(e) if newer => bail!("projet créé par une version plus récente de Laser Studio (format {from}, cette version lit le format {current}) : {e:#}"),
        Err(e) => return Err(e),
    };
    Ok(Loaded { project, from, current })
}

/// Reads a project file for opening (*Ouvrir*, the open project at
/// startup): its text and what it holds, migrated in memory if older, as
/// is if newer (`parse_file`). Nothing is written: see `upgrade_file`.
pub fn load_file(path: &Path) -> Result<(Loaded, String)> {
    load_file_with(path, MIGRATIONS)
}

fn load_file_with(path: &Path, chain: &[crate::formats::Step]) -> Result<(Loaded, String)> {
    let text = read_text(path)?;
    Ok((parse_with(&text, chain)?, text))
}

/// Once a migrated project is open: its original is copied to
/// `<nom>.v<N>.bak.lsproj`, then the file is rewritten in the current
/// format (header kept). If the copy fails, nothing is written. Returns
/// the copy, or `None` when the file was current (or newer: never written).
pub fn upgrade_file(path: &Path, loaded: &Loaded, original: &str) -> Result<Option<PathBuf>> {
    if loaded.migrated().is_none() {
        return Ok(None);
    }
    let bak = crate::formats::keep_original(path, loaded.from, original.as_bytes())?;
    let mut p = loaded.project.clone();
    p.format_version = loaded.current;
    write_project(path, &p)?;
    log::info!("Projet {} : format {} mis à jour, original dans {}", name_of(path), loaded.from, bak.display());
    Ok(Some(bak))
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
    p.pages.sanitize();
    p.executors.sanitize();
    crate::desk::sanitize(&mut p.desks);
    crate::macros::sanitize(&mut p.macros);
    // A name that can't be a show's is dropped, not refused (T-444).
    p.show_list.items.retain(|i| valid_show_name(i.show.trim()));
    p.show_list.sanitize();

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
    crate::usercues::validate(&mut p.user_cues)?;
    let mut lists = HashSet::new();
    for l in &mut p.cue_lists {
        l.sanitize();
        if !crate::timeline::valid_show_name(&l.name) {
            bail!("nom de liste de cues invalide : « {} » ({NAME_RULE})", l.name);
        }
        if !lists.insert(l.name.clone()) {
            bail!("liste de cues en double : « {} »", l.name);
        }
    }

    let bpm = if p.tempo.bpm.is_finite() { p.tempo.bpm } else { 120.0 };
    p.tempo = TempoDefaults { bpm: bpm.clamp(MIN_BPM, MAX_BPM), beats_per_bar: p.tempo.beats_per_bar.clamp(1, 16) };
    p.layers.sanitize();
    crate::live::validate_palettes(&p.palettes).map_err(|e| anyhow::anyhow!("palettes : {e}"))?;
    crate::scan::validate(&mut p.scan_presets).map_err(|e| anyhow::anyhow!("préréglages de balayage : {e}"))?;
    crate::pools::validate(&mut p.pools).map_err(|e| anyhow::anyhow!("préréglages : {e}"))?;
    for slot in p.grid.slots.values_mut() {
        slot.refs.normalize();
    }

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
/// modulated; route sources must be known analysis values or events; the
/// controls cue list steps fire must be allowed (never `transport.arm`).
fn check_lfos(p: &Project, reg: &crate::controls::ControlRegistry) -> Result<()> {
    crate::lfo::validate(&p.lfos, reg).map_err(|e| anyhow::anyhow!("LFO : {e}"))?;
    for l in &p.cue_lists {
        l.validate(reg).map_err(|e| anyhow::anyhow!("liste de cues « {} » : {e:#}", l.name))?;
    }
    crate::audio::routes::validate(&p.audio_routes, reg).map_err(|e| anyhow::anyhow!("liens audio : {e}"))
}

/// The project sections as they are now, without the timelines (on disk,
/// see `ShowStore::load_all`): called under the lock, so copies only.
pub fn snapshot(s: &Shared) -> Project {
    Project {
        scenes: s.scenes.list().to_vec(),
        playlist: s.scenes.list().iter().map(|sc| sc.name.clone()).collect(),
        grid: s.deck.clone(),
        pages: s.grid.clone(),
        tempo: TempoDefaults { bpm: s.tempo.bpm, beats_per_bar: s.tempo.beats_per_bar },
        live: s.live.clone(),
        layers: s.mixer.clone(),
        lfos: s.lfos.list().to_vec(),
        audio_routes: s.routes.routing().clone(),
        palettes: s.palettes.list().to_vec(),
        midi: MidiSection { profiles: s.midi.store.user_profiles().clone() },
        figures: s.figures.list().to_vec(),
        user_cues: s.user_cues.list().to_vec(),
        cue_lists: s.cue_lists.list().to_vec(),
        scan_presets: s.scan_presets.user().to_vec(),
        group_masters: s.projection.masters(),
        executors: s.executors.bank.clone(),
        speed: s.speed.clone(),
        desks: s.desks.tabs.clone(),
        macros: s.macros.list().to_vec(),
        show_list: s.showlist.list().clone(),
        pools: s.pools.list().to_vec(),
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
    // Resolved by figures::refresh below, with the project's figures.
    s.grid.set_config(&p.pages);
    s.timeline.stop();
    let t = s.now_s();
    s.tempo.set_bpm_manual(p.tempo.bpm, t);
    s.tempo.beats_per_bar = p.tempo.beats_per_bar;
    s.live = p.live.clone();
    s.live.release_momentary();
    s.mixer = p.layers.clone();
    // Group masters by group order (missing: 100 %); projection.json is
    // written by the engine within the second.
    s.projection.set_masters(&p.group_masters);
    s.groups_dirty = true;
    // Speed groups (T-419): each group changes from where its beat is.
    let (beat, t_eff, bpb) = s.speed_clock();
    s.speed.replace(&p.speed, beat, t_eff, bpb);
    s.speed_dirty = true;
    s.lfos.replace_in_memory(p.lfos.clone());
    s.routes.replace_in_memory(p.audio_routes.clone(), &s.controls);
    s.palettes.replace_in_memory(p.palettes.clone());
    s.scan_presets.replace_in_memory(p.scan_presets.clone());
    s.midi.store.replace_user_in_memory(p.midi.profiles.clone());
    s.figures.replace_in_memory(p.figures.clone());
    s.user_cues.replace_in_memory(p.user_cues.clone());
    s.cue_lists.replace_in_memory(p.cue_lists.clone());
    s.executors.set_bank(p.executors.clone());
    s.desks.set_tabs(p.desks.clone());
    // The loaded list follows its new version, or goes (another project).
    match s.cue_list.list.as_ref().and_then(|l| s.cue_lists.get(&l.name)).cloned() {
        Some(l) => {
            s.cue_list.top();
            s.cue_list.refresh(&l);
        }
        None => s.cue_list.unload(),
    }
    s.macros.replace_in_memory(p.macros.clone());
    s.showlist.replace_in_memory(p.show_list.clone());
    s.pools.replace_in_memory(p.pools.clone());
    crate::usercues::sync_presets(s);
    crate::figures::refresh(s);
    // Lines checked on the registry just rebuilt (macros included): a bad
    // one disables its macro, with a warning the open report carries.
    crate::macros::revalidate(s);
    // Saved below, not by the engine thread.
    s.live_dirty = false;
    s.mixer_dirty = false;
    s.settings_rev += 1;
}

/// Where the working copy lives, read under the lock.
struct WorkingPaths {
    scenes: PathBuf,
    grid: Option<PathBuf>,
    pages: Option<PathBuf>,
    shows: PathBuf,
    lfos: PathBuf,
    routes: PathBuf,
    palettes: PathBuf,
    scan_presets: PathBuf,
    midi: Option<PathBuf>,
    figures: Option<PathBuf>,
    /// `user_cues.json` and its content (with the id counter) as opened.
    user_cues: Option<(PathBuf, Vec<u8>)>,
    cue_lists: Option<PathBuf>,
    executors: Option<PathBuf>,
    desks: Option<PathBuf>,
    macros: Option<PathBuf>,
    show_list: Option<PathBuf>,
    pools: Option<PathBuf>,
    live: PathBuf,
    layers: PathBuf,
    /// Macros disabled on opening (T-425) and references to unknown presets (T-422), for the open report.
    macro_warnings: Vec<String>,
}

fn working_paths(s: &Shared, data_dir: &Path) -> WorkingPaths {
    WorkingPaths {
        scenes: s.scenes.path().to_path_buf(),
        grid: s.deck.path().map(Path::to_path_buf),
        pages: s.grid.path().map(Path::to_path_buf),
        shows: s.shows.dir().to_path_buf(),
        lfos: s.lfos.path().to_path_buf(),
        routes: s.routes.path().to_path_buf(),
        palettes: s.palettes.path().to_path_buf(),
        scan_presets: s.scan_presets.path().to_path_buf(),
        midi: s.midi.store.dir().map(|d| d.join("profiles")),
        figures: s.figures.dir().map(Path::to_path_buf),
        user_cues: s.user_cues.path().map(|p| (p.to_path_buf(), s.user_cues.file_bytes().unwrap_or_default())),
        cue_lists: s.cue_lists.path().map(Path::to_path_buf),
        executors: s.executors.path().map(Path::to_path_buf),
        desks: s.desks.path().map(Path::to_path_buf),
        macros: s.macros.path().map(Path::to_path_buf),
        show_list: s.showlist.path().map(Path::to_path_buf),
        pools: s.pools.path().map(Path::to_path_buf),
        live: data_dir.join("live.json"),
        layers: data_dir.join("layers.json"),
        macro_warnings: Vec::new(),
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
    if let Some(pages) = &w.pages {
        put(pages, serde_json::to_vec_pretty(&p.pages));
    }
    if let Some((path, bytes)) = &w.user_cues {
        put(path, Ok(bytes.clone()));
    }
    if let Some(path) = &w.cue_lists {
        put(path, serde_json::to_vec_pretty(&p.cue_lists));
    }
    if let Some(path) = &w.executors {
        put(path, serde_json::to_vec_pretty(&p.executors));
    }
    if let Some(path) = &w.desks {
        put(path, serde_json::to_vec_pretty(&p.desks));
    }
    if let Some(path) = &w.macros {
        put(path, serde_json::to_vec_pretty(&p.macros));
    }
    if let Some(path) = &w.show_list {
        put(path, serde_json::to_vec_pretty(&p.show_list));
    }
    if let Some(path) = &w.pools {
        put(path, serde_json::to_vec_pretty(&p.pools));
    }
    put(&w.lfos, serde_json::to_vec_pretty(&p.lfos));
    put(&w.routes, serde_json::to_vec_pretty(&p.audio_routes));
    put(&w.palettes, serde_json::to_vec_pretty(&p.palettes));
    put(&w.scan_presets, serde_json::to_vec_pretty(&p.scan_presets));
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
            // A damaged file that could not be moved aside is kept (T-402).
            let is_json = path.extension().is_some_and(|x| x == "json") && path.is_file() && !crate::datafile::is_frozen(&path);
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
    // The engine's own files go through the writer thread, after any
    // older copy it queued before the swap (T-482).
    for (path, bytes) in [(&w.live, serde_json::to_vec_pretty(&p.live)), (&w.layers, serde_json::to_vec_pretty(&p.layers))] {
        if let Err(e) = bytes.map_err(anyhow::Error::from).and_then(|b| crate::datafile::replace(path, &b)) {
            errors.push(format!("{} : {e:#}", path.display()));
        }
    }
    errors
}

/// Header fields for a save.
fn stamp(p: &mut Project) {
    p.format_version = PROJECT_FORMAT;
    p.app_version = env!("CARGO_PKG_VERSION").to_string();
    p.saved_at = utc_now();
}

/// Stamps the header and writes `p` atomically (autosave). Returns the
/// bytes written (for the copy to a second location, T-450).
pub fn write_project_stamped(path: &Path, p: &mut Project) -> Result<Vec<u8>> {
    stamp(p);
    let json = project_bytes(p)?;
    write_atomic(path, &json)?;
    Ok(json)
}

/// A project as written to its file: pretty JSON, a final newline.
pub fn project_bytes(p: &Project) -> Result<Vec<u8>> {
    let mut json = serde_json::to_vec_pretty(p).context("échec de la mise en forme du projet")?;
    json.push(b'\n');
    Ok(json)
}

fn write_project(path: &Path, p: &Project) -> Result<()> {
    write_atomic(path, &project_bytes(p)?)
}

/// Atomic write (`datafile::write_atomic`, which also never writes over a
/// damaged file that could not be moved aside).
pub use crate::datafile::write_atomic;
#[cfg(test)]
use crate::datafile::{commit, write_tmp};

/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// UTC year, month, day, hour, minute, second of `secs` since 1970.
pub fn civil(secs: u64) -> (i64, i64, i64, u64, u64, u64) {
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
    (y, m, d, rem / 3600, rem / 60 % 60, rem % 60)
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

#[derive(Deserialize)]
struct VersionRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    file: String,
}

/// What a successful action adds to the project info it replies with.
#[derive(Default)]
struct Done {
    /// Files that could not be written.
    warnings: Vec<String>,
    /// One French line for the page (a migration, read only, a version).
    notice: Option<String>,
    /// A name to offer in « Enregistrer sous… » (a version opened as a copy).
    suggested_name: Option<String>,
    /// The opened project carries a site profile that differs from the one
    /// in force (T-289): its name, for « Garder le profil actuel » /
    /// « Utiliser celui du projet ». Never applied by opening.
    site_offer: Option<String>,
    /// What a partial import did, or would do (`dry_run`).
    import: Option<Value>,
}

impl From<Vec<String>> for Done {
    fn from(warnings: Vec<String>) -> Self {
        Done { warnings, ..Done::default() }
    }
}

/// `GET /api/project`, `POST /api/project/{new,open,save,save-as}`, for
/// autosave (T-287) `POST /api/project/{autosave,recover,recover-ignore}`,
/// and for named versions (T-288) `GET /api/project/versions`,
/// `POST /api/project/{version-save,version-open,version-delete}`.
/// Runs on the HTTP worker thread; the lock is held only to copy the
/// state out or swap it in, never for file I/O.
pub fn route(shared: &Mutex<Shared>, post: bool, action: &str, body: &str) -> Option<Reply> {
    let path_req = || serde_json::from_str::<PathRequest>(body).map_err(|e| Reply::Text(400, format!("JSON invalide : {e}")));
    let version_req = || serde_json::from_str::<VersionRequest>(body).map_err(|e| Reply::Text(400, format!("JSON invalide : {e}")));
    if post && matches!(action, "new" | "open" | "save" | "save-as" | "version-open") {
        // Another project, or a save: the recovery question is moot.
        shared.lock().unwrap().project.autosave.offer = None;
    }
    let reply = match (post, action) {
        (false, "") => return Some(Reply::Json(info(shared))),
        (false, "quit-check") => return Some(Reply::Json(json!(quit_check(shared)))),
        (false, "versions") => return Some(Reply::Json(versions_info(shared))),
        (false, "templates") => return Some(Reply::Json(templates_info(shared))),
        (true, "autosave") => match serde_json::from_str::<crate::autosave::SettingsRequest>(body) {
            Ok(req) => crate::autosave::set_settings(shared, req)
                .map(|()| Done::default())
                .map_err(|e| Reply::Text(500, format!("impossible d'enregistrer les préférences : {e:#}"))),
            Err(e) => Err(Reply::Text(400, format!("JSON invalide : {e}"))),
        },
        (true, "recover") => recover(shared).map(Done::from),
        (true, "recover-ignore") => {
            crate::autosave::ignore(shared);
            Ok(Done::default())
        }
        (true, "new") => new_project(shared, body),
        (true, "save-template") => template_req(body).and_then(|req| save_template(shared, &req.name, req.replace)),
        (true, "delete-template") => template_req(body).and_then(|req| delete_template(shared, &req.name)),
        (true, "open") => match path_req() {
            Ok(req) => {
                let dir = shared.lock().unwrap().project.dir.clone();
                match resolve(&dir, &req.path) {
                    Ok(path) => open_project(shared, Some(&path), Project::default()),
                    Err(e) => Err(Reply::Text(400, format!("{e:#}"))),
                }
            }
            Err(e) => Err(e),
        },
        (true, "save") => {
            let (current, read_only) = {
                let s = shared.lock().unwrap();
                (s.project.current.clone(), s.project.read_only)
            };
            match (current, read_only) {
                (Some(_), Some(v)) => Err(Reply::Text(409, read_only_fr(v))),
                (Some(name), None) => save_project(shared, &name).map(Done::from),
                (None, _) => Err(Reply::Text(409, "projet sans nom : utilisez « Enregistrer sous… »".into())),
            }
        }
        (true, "save-as") => match path_req() {
            Ok(req) => {
                let (dir, current, read_only) = {
                    let s = shared.lock().unwrap();
                    (s.project.dir.clone(), s.project.current.clone(), s.project.read_only)
                };
                match resolve(&dir, &req.path) {
                    // Never over the newer file itself.
                    Ok(path) if read_only.is_some() && current.as_deref() == Some(name_of(&path).as_str()) => {
                        Err(Reply::Text(409, format!("{NEWER_FR} : choisissez un autre nom pour la copie")))
                    }
                    Ok(path) => save_project(shared, &name_of(&path)).map(Done::from),
                    Err(e) => Err(Reply::Text(400, format!("{e:#}"))),
                }
            }
            Err(e) => Err(e),
        },
        (true, "version-save") => version_req().and_then(|req| save_version(shared, &req.name)),
        (true, "version-open") => version_req().and_then(|req| open_version(shared, &req.file)),
        (true, "version-delete") => version_req().and_then(|req| delete_version(shared, &req.file)),
        (true, "import-partial") => match serde_json::from_str::<PartialRequest>(body) {
            Ok(req) => import_partial(shared, &req),
            Err(e) => Err(Reply::Text(400, format!("JSON invalide : {e}"))),
        },
        _ => return None,
    };
    Some(match reply {
        Ok(done) => {
            let mut v = info(shared);
            v["warnings"] = json!(done.warnings);
            v["notice"] = json!(done.notice);
            v["suggested_name"] = json!(done.suggested_name);
            v["site_offer"] = json!(done.site_offer);
            v["import"] = json!(done.import);
            Reply::Json(v)
        }
        Err(r) => r,
    })
}

/// The French line of a read-only project (format `v`).
fn read_only_fr(v: u32) -> String {
    format!("{NEWER_FR} (format {v}) : « Enregistrer » est désactivé ; « Enregistrer sous… » en fait une copie au format actuel")
}

/// Checks `p` against the controls and swaps it in, then writes the
/// working copy (lock released). Returns the files not written.
fn swap_in(shared: &Mutex<Shared>, p: &Project) -> Result<Vec<String>, Reply> {
    let w = {
        let mut s = shared.lock().unwrap();
        check_lfos(p, &s.controls).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
        apply(&mut s, p);
        let data_dir = s.project.data_dir.clone();
        let mut w = working_paths(&s, &data_dir);
        w.macro_warnings = s.macros.warnings.clone();
        // References to presets the project doesn't hold (T-422): ignored
        // when rendering (the cues' own values), said once here.
        w.macro_warnings.extend(crate::pools::dangling(&p.grid, &p.timelines, &p.pools, &|cue| crate::pools::cue_label(&s, cue)));
        w
    };
    // The macros as revalidated (some may be disabled) go to the working copy.
    let mut p = p.clone();
    p.macros = shared.lock().unwrap().macros.list().to_vec();
    let warnings = persist(&w, &p);
    for e in &warnings {
        log::warn!("project open: failed to write {e}");
    }
    // The show list's times, from the shows just written (T-444).
    crate::showlist::refresh(&mut shared.lock().unwrap());
    Ok(w.macro_warnings.iter().cloned().chain(warnings.into_iter().map(|e| format!("impossible d'écrire {e}"))).collect())
}

/// Opens `path`, or with `None` the untitled project `fresh` (*Nouveau
/// projet*, T-451: empty, or with sections kept, or a template copy). Atomic: nothing
/// changes unless the whole file is valid. An older file is migrated (its
/// original kept, T-288); a newer one opens read only.
fn open_project(shared: &Mutex<Shared>, path: Option<&Path>, fresh: Project) -> Result<Done, Reply> {
    let loaded = match path {
        Some(path) => Some(load_file(path).map_err(|e| {
            let code = if path.exists() { 400 } else { 404 };
            Reply::Text(code, format!("{e:#}"))
        })?),
        None => None,
    };
    let (mut p, migrated, newer) = match &loaded {
        Some((l, _)) => (l.project.clone(), l.migrated(), l.newer()),
        None => (fresh, None, None),
    };
    let name = path.map(name_of);
    p.name = name.clone().unwrap_or_else(|| UNTITLED.into());
    let mut warnings = swap_in(shared, &p)?;
    let mut notice = newer.map(read_only_fr);
    if let (Some(path), Some((l, text)), Some(_)) = (path, &loaded, migrated) {
        // The file follows, its original copied first (lock released).
        match upgrade_file(path, l, text) {
            Ok(_) => notice = Some(MIGRATED_FR.into()),
            Err(e) => warnings.push(format!("{e:#} : le fichier garde son ancien format")),
        }
    }
    let mut s = shared.lock().unwrap();
    let mut now = snapshot(&s);
    now.timelines = p.timelines.clone();
    s.project.saved = now.fingerprint();
    s.project.last_modified = false;
    s.project.extra = std::mem::take(&mut p.extra);
    s.project.read_only = newer;
    match &name {
        Some(name) => s.project.remember(name),
        None => s.project.current = None,
    }
    s.project.save_recent();
    let shown = name.clone().unwrap_or_else(|| UNTITLED.into());
    log::info!("Projet ouvert : « {shown} »{}", if warnings.is_empty() { "" } else { " (avec des erreurs d'écriture)" });
    s.gate.log().record("project_open", Some("ui"), serde_json::json!({ "name": shown, "new": name.is_none(), "armed": s.gate.is_armed() }));
    // A site profile in the project (T-289): offered, never applied here.
    let site_offer = match crate::sites::of_project(&s) {
        Ok(Some(site)) if !crate::sites::differences(&s, &site).is_empty() || !crate::sites::loosenings(&s, &site).is_empty() => Some(site.name),
        Ok(_) => None,
        Err(e) => {
            warnings.push(format!("profil de site du projet ignoré : {e:#}"));
            None
        }
    };
    Ok(Done { warnings, notice, site_offer, ..Done::default() })
}

/// *Récupérer* (T-287): opens the offered autosave as the open project,
/// modified: the project's name and saved fingerprint stay, so the
/// recovered work shows as unsaved. Same checks and swap as opening.
fn recover(shared: &Mutex<Shared>) -> Result<Vec<String>, Reply> {
    let offer = shared.lock().unwrap().project.autosave.offer.clone().ok_or_else(|| Reply::Text(409, "aucune sauvegarde auto à récupérer".into()))?;
    let mut p = read(&offer.path).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
    let w = {
        let mut s = shared.lock().unwrap();
        check_lfos(&p, &s.controls).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
        apply(&mut s, &p);
        s.project.autosave.offer = None;
        s.project.last_modified = true;
        let file = offer.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        log::info!("Sauvegarde auto récupérée : {file}");
        s.gate.log().record("autosave_recovered", Some("ui"), serde_json::json!({ "file": file, "armed": s.gate.is_armed() }));
        s.project.extra = std::mem::take(&mut p.extra);
        let data_dir = s.project.data_dir.clone();
        working_paths(&s, &data_dir)
    };
    let warnings = persist(&w, &p);
    for e in &warnings {
        log::warn!("autosave recovery: failed to write {e}");
    }
    crate::showlist::refresh(&mut shared.lock().unwrap());
    Ok(warnings.into_iter().map(|e| format!("impossible d'écrire {e}")).collect())
}

/// Saves the current state as project `name` (in `projects/`), then hands
/// the same bytes to the copy to a second location (T-450), which writes
/// them on its own thread: its result never decides the save's.
fn save_project(shared: &Mutex<Shared>, name: &str) -> Result<Vec<String>, Reply> {
    let (mut p, path, shows, backup) = {
        let s = shared.lock().unwrap();
        let mut p = snapshot(&s);
        p.extra = s.project.extra.clone();
        (p, s.project.path_of(name), crate::timeline::ShowStore::new(s.shows.dir().to_path_buf()), std::sync::Arc::clone(&s.project.backup))
    };
    p.timelines = shows.load_all();
    p.name = name.to_string();
    stamp(&mut p);
    let bytes = project_bytes(&p).and_then(|json| write_atomic(&path, &json).map(|()| json)).map_err(|e| {
        log::error!("Projet « {name} » : échec de l'enregistrement : {e:#}");
        Reply::Text(500, format!("échec de l'enregistrement : {e:#}"))
    })?;
    log::info!("Projet enregistré : « {name} »");
    backup.queue(crate::backup::Job::project(name, bytes));
    let mut s = shared.lock().unwrap();
    s.project.saved = p.fingerprint();
    s.project.last_modified = false;
    s.project.read_only = None;
    s.project.remember(name);
    s.project.save_recent();
    Ok(Vec::new())
}

/// The current project with its timelines and unknown sections, stamped
/// and named: what *Enregistrer* would write (lock held only to copy).
pub fn current_project(shared: &Mutex<Shared>) -> Project {
    let (mut p, name, shows) = {
        let s = shared.lock().unwrap();
        let mut p = snapshot(&s);
        p.extra = s.project.extra.clone();
        (p, s.project.current.clone().unwrap_or_else(|| UNTITLED.into()), crate::timeline::ShowStore::new(s.shows.dir().to_path_buf()))
    };
    p.timelines = shows.load_all();
    p.name = name;
    stamp(&mut p);
    p
}

// ---------- new project and templates (T-451) ----------

#[derive(Deserialize)]
struct TemplateRequest {
    name: String,
    /// *Un modèle porte déjà ce nom : le remplacer ?* answered yes.
    #[serde(default)]
    replace: bool,
}

fn template_req(body: &str) -> Result<TemplateRequest, Reply> {
    serde_json::from_str(body).map_err(|e| Reply::Text(400, format!("JSON invalide : {e}")))
}

fn templates_dir(shared: &Mutex<Shared>) -> PathBuf {
    shared.lock().unwrap().project.data_dir.join(TEMPLATES_DIR)
}

/// A template's file in `dir`: the same name rules as a project.
fn template_path(dir: &Path, name: &str) -> Result<PathBuf, Reply> {
    resolve(dir, name).map_err(|_| Reply::Text(400, format!("nom de modèle refusé ({NAME_RULE})")))
}

/// *Nouveau projet*: an empty body (or `{}`) keeps nothing, as before
/// T-451; `keep` takes the ticked sections of the open project; a
/// `template` opens an untitled copy of it (never the template's file:
/// *Enregistrer* asks for a name), `keep` not applying. Machine settings
/// are not in a project, so nothing here reads or writes them, and the
/// arming state does not change.
fn new_project(shared: &Mutex<Shared>, body: &str) -> Result<Done, Reply> {
    let req: NewRequest = if body.trim().is_empty() { NewRequest::default() } else { serde_json::from_str(body).map_err(|e| Reply::Text(400, format!("JSON invalide : {e}")))? };
    let fresh = match req.template.as_deref() {
        Some(name) => {
            let path = template_path(&templates_dir(shared), name)?;
            let (loaded, _) = load_file(&path).map_err(|e| Reply::Text(if path.exists() { 400 } else { 404 }, format!("modèle « {} » : {e:#}", name_of(&path))))?;
            if let Some(v) = loaded.newer() {
                return Err(Reply::Text(400, format!("modèle créé par une version plus récente de Laser Studio (format {v}) : impossible de s'en servir")));
            }
            loaded.project
        }
        None if req.keep == KeepOnNew::default() => Project::default(),
        None => new_from(&current_project(shared), req.keep),
    };
    let mut done = open_project(shared, None, fresh)?;
    if let Some(name) = &req.template {
        done.notice = Some(format!("Nouveau projet depuis le modèle « {} »", name.trim()));
    }
    Ok(done)
}

/// `GET /api/project/templates`.
fn templates_info(shared: &Mutex<Shared>) -> Value {
    let dir = templates_dir(shared);
    json!({ "dir": dir, "templates": list(&dir) })
}

/// *Enregistrer comme modèle…*: the current project (what *Enregistrer*
/// would write) as `templates/<name>.lsproj`. An existing template is only
/// replaced when asked (409 otherwise). The open project is unchanged.
fn save_template(shared: &Mutex<Shared>, name: &str, replace: bool) -> Result<Done, Reply> {
    let dir = templates_dir(shared);
    let path = template_path(&dir, name)?;
    if path.exists() && !replace {
        return Err(Reply::Text(409, "Un modèle porte déjà ce nom : le remplacer ?".into()));
    }
    let mut p = current_project(shared);
    p.name = name_of(&path);
    std::fs::create_dir_all(&dir).map_err(anyhow::Error::from).and_then(|()| write_project(&path, &p)).map_err(|e| Reply::Text(500, format!("échec de l'enregistrement du modèle : {e:#}")))?;
    log::info!("Modèle enregistré : « {} »", name_of(&path));
    Ok(Done { notice: Some(format!("Modèle « {} » enregistré", name_of(&path))), ..Done::default() })
}

/// Deletes a template (the page asks first).
fn delete_template(shared: &Mutex<Shared>, name: &str) -> Result<Done, Reply> {
    let path = template_path(&templates_dir(shared), name)?;
    if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
        return Err(Reply::Text(404, format!("modèle introuvable : {}", name_of(&path))));
    }
    std::fs::remove_file(&path).map_err(|e| Reply::Text(500, format!("impossible de supprimer le modèle : {e}")))?;
    log::info!("Modèle supprimé : « {} »", name_of(&path));
    Ok(Done { notice: Some(format!("Modèle « {} » supprimé", name_of(&path))), ..Done::default() })
}

// ---------- named versions (T-288) ----------

/// The versions folder of the open project, and its name.
fn versions_dir(shared: &Mutex<Shared>) -> (PathBuf, String) {
    let s = shared.lock().unwrap();
    let project = s.project.current.clone().unwrap_or_else(|| UNTITLED.into());
    (crate::versions::dir_of(&s.project.data_dir, &project), project)
}

/// `GET /api/project/versions`: the open project's versions, newest first.
fn versions_info(shared: &Mutex<Shared>) -> Value {
    let (dir, project) = versions_dir(shared);
    json!({ "project": project, "dir": dir, "versions": crate::versions::list(&dir) })
}

/// *Enregistrer une version…*: the current state, as *Enregistrer* would
/// write it, under `versions/<projet>/`. The project itself is untouched
/// (still modified if it was).
fn save_version(shared: &Mutex<Shared>, label: &str) -> Result<Done, Reply> {
    let label = label.trim();
    if !valid_name(label) {
        return Err(Reply::Text(400, format!("nom de version invalide ({NAME_RULE})")));
    }
    let (dir, _) = versions_dir(shared);
    let p = current_project(shared);
    let path = crate::versions::new_path(&dir, crate::versions::now_secs(), label);
    write_project(&path, &p).map_err(|e| Reply::Text(500, format!("échec de l'enregistrement de la version : {e:#}")))?;
    log::info!("Version « {label} » du projet « {} » enregistrée : {}", p.name, path.display());
    Ok(Done { notice: Some(format!("Version « {label} » enregistrée")), ..Done::default() })
}

/// *Ouvrir comme copie*: the version becomes a new *Sans titre* project,
/// modified; the project it was saved from is never written. Same checks
/// and swap as opening; a version in a newer format is refused.
fn open_version(shared: &Mutex<Shared>, file: &str) -> Result<Done, Reply> {
    let (dir, project) = versions_dir(shared);
    let (path, label) = crate::versions::resolve(&dir, file).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
    let (loaded, _) = load_file(&path).map_err(|e| Reply::Text(if path.exists() { 400 } else { 404 }, format!("{e:#}")))?;
    if let Some(v) = loaded.newer() {
        return Err(Reply::Text(400, format!("version créée par une version plus récente de Laser Studio (format {v}) : ouvrez-la avec celle-ci")));
    }
    let mut p = loaded.project;
    p.name = UNTITLED.into();
    let warnings = swap_in(shared, &p)?;
    let mut s = shared.lock().unwrap();
    // Unsaved: nothing on disk is this copy yet.
    s.project.saved = 0;
    s.project.last_modified = true;
    s.project.extra = std::mem::take(&mut p.extra);
    s.project.read_only = None;
    s.project.current = None;
    s.project.save_recent();
    log::info!("Version « {label} » du projet « {project} » ouverte comme copie");
    s.gate.log().record("project_open", Some("ui"), serde_json::json!({ "name": UNTITLED, "version_of": project, "version": label, "armed": s.gate.is_armed() }));
    let suggested: String = format!("{project} {label}").chars().take(64).collect::<String>().trim().to_string();
    Ok(Done {
        warnings,
        notice: Some(format!("Version « {label} » ouverte comme copie : « Enregistrer sous… » pour la garder")),
        suggested_name: valid_name(&suggested).then_some(suggested),
        ..Done::default()
    })
}

/// *Supprimer* a version (the page asks first).
fn delete_version(shared: &Mutex<Shared>, file: &str) -> Result<Done, Reply> {
    let (dir, _) = versions_dir(shared);
    let (path, label) = crate::versions::resolve(&dir, file).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Done { notice: Some(format!("Version « {label} » supprimée")), ..Done::default() }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Reply::Text(404, format!("version introuvable : {label}"))),
        Err(e) => Err(Reply::Text(500, format!("impossible de supprimer la version : {e}"))),
    }
}

// ---------- partial import (T-289) ----------

/// What *Importer depuis un projet…* may take, by key.
pub const PARTIAL_SECTIONS: [&str; 6] = ["pages", "user_cues", "scenes", "midi", "site", "timelines"];

/// A name already used: *Renommer* (« nom 2 »), *Remplacer* or *Ignorer*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Conflict {
    #[default]
    Rename,
    Replace,
    Skip,
}

#[derive(Deserialize)]
struct PartialRequest {
    /// A project of the projects folder.
    from: String,
    #[serde(default)]
    sections: Vec<String>,
    #[serde(default)]
    conflict: Conflict,
    /// Only say what would happen.
    #[serde(default)]
    dry_run: bool,
}

/// What a partial import did to one section.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SectionReport {
    /// Items in the other project.
    pub found: usize,
    pub added: usize,
    pub replaced: usize,
    pub skipped: usize,
    /// (name in the other project, name here).
    pub renamed: Vec<(String, String)>,
    /// Names already used here.
    pub conflicts: Vec<String>,
}

/// `base 2`, `base 3`… (`sep` between): the first that `ok` accepts and
/// `taken` doesn't hold, cut to `max` characters.
fn free_name(base: &str, max: usize, sep: &str, taken: &dyn Fn(&str) -> bool, ok: &dyn Fn(&str) -> bool) -> Option<String> {
    (2..1000).find_map(|n| {
        let suffix = format!("{sep}{n}");
        let stem: String = base.chars().take(max.saturating_sub(suffix.chars().count())).collect();
        let name = format!("{}{suffix}", stem.trim_end());
        (!taken(&name) && ok(&name)).then_some(name)
    })
}

/// How a section's items are named, for `merge_named`.
struct Naming<'a, T> {
    get: &'a dyn Fn(&T) -> String,
    set: &'a dyn Fn(&mut T, &str),
    max: usize,
    sep: &'a str,
    ok: &'a dyn Fn(&str) -> bool,
}

/// Merges `items` of the other project into `cur`, by name.
fn merge_named<T: Clone>(cur: &mut Vec<T>, items: &[T], conflict: Conflict, n: &Naming<T>) -> SectionReport {
    let mut r = SectionReport { found: items.len(), ..Default::default() };
    for item in items {
        let name = (n.get)(item);
        let Some(i) = cur.iter().position(|c| (n.get)(c) == name) else {
            cur.push(item.clone());
            r.added += 1;
            continue;
        };
        r.conflicts.push(name.clone());
        match conflict {
            Conflict::Skip => r.skipped += 1,
            Conflict::Replace => {
                cur[i] = item.clone();
                r.replaced += 1;
            }
            Conflict::Rename => {
                let names: Vec<String> = cur.iter().map(|c| (n.get)(c)).collect();
                match free_name(&name, n.max, n.sep, &|c| names.iter().any(|x| x == c), n.ok) {
                    Some(new) => {
                        let mut it = item.clone();
                        (n.set)(&mut it, &new);
                        cur.push(it);
                        r.renamed.push((name, new));
                    }
                    None => r.skipped += 1,
                }
            }
        }
    }
    r
}

/// Merges the chosen `sections` of `src` into `cur` (`PARTIAL_SECTIONS`),
/// then checks `cur` like an opened project. The pages of `cur` must be
/// materialised (not the built-in default ones). Only the chosen sections
/// of `cur` change.
pub fn merge(cur: &mut Project, src: &Project, sections: &[String], conflict: Conflict) -> Result<BTreeMap<String, SectionReport>> {
    for sec in sections {
        if !PARTIAL_SECTIONS.contains(&sec.as_str()) {
            bail!("partie inconnue : « {sec} »");
        }
    }
    if sections.is_empty() {
        bail!("rien à importer : cochez au moins une partie");
    }
    let want = |k: &str| sections.iter().any(|s| s == k);
    let named = |s: &str| !s.trim().is_empty();
    let mut out = BTreeMap::new();
    if want("scenes") {
        let n = Naming::<Scene> { get: &|s| s.name.clone(), set: &|s, n| s.name = n.to_string(), max: 64, sep: " ", ok: &named };
        out.insert("scenes".to_string(), merge_named(&mut cur.scenes, &src.scenes, conflict, &n));
    }
    if want("timelines") {
        let n = Naming::<Show> { get: &|s| s.name.clone(), set: &|s, n| s.name = n.to_string(), max: 64, sep: " ", ok: &valid_show_name };
        out.insert("timelines".to_string(), merge_named(&mut cur.timelines, &src.timelines, conflict, &n));
    }
    if want("midi") {
        let builtin = ProfileStore::in_memory();
        let ok = |slug: &str| valid_slug(slug) && !builtin.is_builtin(slug);
        let mut list: Vec<(String, Profile)> = cur.midi.profiles.clone().into_iter().collect();
        let theirs: Vec<(String, Profile)> = src.midi.profiles.clone().into_iter().collect();
        let n = Naming::<(String, Profile)> {
            get: &|(slug, _)| slug.clone(),
            set: &|(slug, p), n| {
                *slug = n.to_string();
                let k = n.rsplit('-').next().unwrap_or("2");
                p.name = format!("{} {k}", p.name).trim().to_string();
            },
            max: 64,
            sep: "-",
            ok: &ok,
        };
        out.insert("midi".to_string(), merge_named(&mut list, &theirs, conflict, &n));
        cur.midi.profiles = list.into_iter().collect();
    }
    // User cues: matched by name; every cue taken gets an id of this
    // project (ids are per project), and the imported pages follow.
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    if want("user_cues") {
        let mut r = SectionReport { found: src.user_cues.len(), ..Default::default() };
        let mut next = cur.user_cues.iter().filter_map(|c| c.id.strip_prefix("u-").and_then(|n| n.parse::<u64>().ok())).max().unwrap_or(0) + 1;
        let mut fresh = || {
            let id = format!("u-{next}");
            next += 1;
            id
        };
        for cue in &src.user_cues {
            let Some(i) = cur.user_cues.iter().position(|c| c.name == cue.name) else {
                let id = fresh();
                ids.insert(cue.id.clone(), id.clone());
                cur.user_cues.push(UserCue { id, ..cue.clone() });
                r.added += 1;
                continue;
            };
            r.conflicts.push(cue.name.clone());
            match conflict {
                Conflict::Skip => {
                    ids.insert(cue.id.clone(), cur.user_cues[i].id.clone());
                    r.skipped += 1;
                }
                Conflict::Replace => {
                    let id = cur.user_cues[i].id.clone();
                    ids.insert(cue.id.clone(), id.clone());
                    cur.user_cues[i] = UserCue { id, ..cue.clone() };
                    r.replaced += 1;
                }
                Conflict::Rename => {
                    let names: Vec<String> = cur.user_cues.iter().map(|c| c.name.clone()).collect();
                    match free_name(&cue.name, 40, " ", &|c| names.iter().any(|x| x == c), &named) {
                        Some(new) => {
                            let id = fresh();
                            ids.insert(cue.id.clone(), id.clone());
                            r.renamed.push((cue.name.clone(), new.clone()));
                            cur.user_cues.push(UserCue { id, name: new, ..cue.clone() });
                        }
                        None => r.skipped += 1,
                    }
                }
            }
        }
        out.insert("user_cues".to_string(), r);
    }
    if want("pages") {
        // The operator's pages only (library pages follow the libraries).
        let theirs: Vec<CuePage> = src
            .pages
            .pages
            .iter()
            .filter(|p| p.category.is_none())
            .map(|p| {
                let mut p = p.clone();
                for slot in &mut p.slots {
                    // A user cue of the other project: its cue here, or nothing.
                    if let Some(CueRef::User(id)) = slot.clone() {
                        if id.starts_with("u-") {
                            *slot = ids.get(&id).map(|n| CueRef::User(n.clone()));
                        }
                    }
                }
                p
            })
            .collect();
        let n = Naming::<CuePage> { get: &|p| p.name.clone(), set: &|p, n| p.name = n.to_string(), max: 40, sep: " ", ok: &named };
        let mut r = merge_named(&mut cur.pages.pages, &theirs, conflict, &n);
        if cur.pages.pages.len() > crate::grid::MAX_PAGES {
            let extra = cur.pages.pages.len() - crate::grid::MAX_PAGES;
            cur.pages.pages.truncate(crate::grid::MAX_PAGES);
            r.skipped += extra;
            r.added = r.added.saturating_sub(extra);
        }
        out.insert("pages".to_string(), r);
    }
    if want("site") {
        let mut r = SectionReport::default();
        if let Some(site) = src.extra.get(crate::sites::PROJECT_KEY) {
            r.found = 1;
            let name = site.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            match cur.extra.get(crate::sites::PROJECT_KEY) {
                Some(_) if conflict == Conflict::Skip => {
                    r.conflicts.push(name);
                    r.skipped = 1;
                }
                Some(_) => {
                    r.conflicts.push(name);
                    r.replaced = 1;
                    cur.extra.insert(crate::sites::PROJECT_KEY.into(), site.clone());
                }
                None => {
                    r.added = 1;
                    cur.extra.insert(crate::sites::PROJECT_KEY.into(), site.clone());
                }
            }
        }
        out.insert("site".to_string(), r);
    }
    check(cur)?;
    Ok(out)
}

/// *Importer depuis un projet…*: the chosen sections of another project
/// of the folder merged into the open one, which becomes modified (not
/// saved). Same checks and swap as opening. An embedded site is only
/// carried (offered later), never applied. Never arms.
fn import_partial(shared: &Mutex<Shared>, req: &PartialRequest) -> Result<Done, Reply> {
    let dir = shared.lock().unwrap().project.dir.clone();
    let path = resolve(&dir, &req.from).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
    let (loaded, _) = load_file(&path).map_err(|e| Reply::Text(if path.exists() { 400 } else { 404 }, format!("{e:#}")))?;
    if let Some(v) = loaded.newer() {
        return Err(Reply::Text(400, format!("projet créé par une version plus récente de Laser Studio (format {v}) : import impossible")));
    }
    let src = loaded.project;
    let mut cur = current_project(shared);
    if req.sections.iter().any(|s| s == "pages") {
        let presets = shared.lock().unwrap().presets.clone();
        cur.pages.materialize(&presets);
    }
    let name = cur.name.clone();
    let report = merge(&mut cur, &src, &req.sections, req.conflict).map_err(|e| Reply::Text(400, format!("{e:#}")))?;
    let report = json!(report);
    if req.dry_run {
        return Ok(Done { import: Some(report), ..Done::default() });
    }
    let warnings = swap_in(shared, &cur)?;
    let mut s = shared.lock().unwrap();
    s.project.extra = cur.extra.clone();
    s.project.last_modified = true;
    log::info!("Projet « {name} » : import partiel depuis « {} » ({})", name_of(&path), req.sections.join(", "));
    s.gate.log().record("project_import", Some("ui"), serde_json::json!({ "from": name_of(&path), "sections": req.sections, "armed": s.gate.is_armed() }));
    Ok(Done { warnings, import: Some(report), notice: Some(format!("Importé depuis « {} »", name_of(&path))), ..Done::default() })
}


/// The current project for a diagnostic report (T-403), without its
/// media: the figure library is left out (only how many there were).
/// Read under the lock, the timelines from disk after it is released.
pub fn diagnostic_snapshot(shared: &Mutex<Shared>) -> Value {
    let (mut p, shows) = {
        let s = shared.lock().unwrap();
        let mut p = snapshot(&s);
        p.name = s.project.current.clone().unwrap_or_else(|| UNTITLED.into());
        (p, crate::timeline::ShowStore::new(s.shows.dir().to_path_buf()))
    };
    p.timelines = shows.load_all();
    let figures = std::mem::take(&mut p.figures).len();
    let mut v = json!(p);
    if let Some(o) = v.as_object_mut() {
        o.remove("figures");
        o.insert("figures_omitted".into(), json!(figures));
    }
    v
}

/// `GET /api/project`: name, file, whether it changed since it was opened
/// or saved, the projects folder's files and the recent list.
fn info(shared: &Mutex<Shared>) -> Value {
    let (mut p, current, saved, dir, recent, shows, autosave, read_only, backup) = {
        let s = shared.lock().unwrap();
        let pr = &s.project;
        let shows = crate::timeline::ShowStore::new(s.shows.dir().to_path_buf());
        (snapshot(&s), pr.current.clone(), pr.saved, pr.dir.clone(), pr.recent.clone(), shows, pr.autosave.status(), pr.read_only, pr.backup.view())
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
        "autosave": autosave,
        // T-288: the file is from a newer version (its format): read only.
        "read_only": read_only.is_some(),
        "file_format": read_only,
        "read_only_message": read_only.map(read_only_fr),
        // T-450: the copy to a second location.
        "backup": backup,
    })
}

/// What quitting now would lose (T-433), for `GET /api/project/quit-check`:
/// the project changed since it was opened or saved (an autosave is not a
/// save) and the timeline editor's show has edits not saved yet. The
/// figure being drawn in CRÉATION lives in the page only: the page adds it.
#[derive(Clone, Debug, Serialize)]
pub struct QuitCheck {
    /// Something would be lost.
    pub modified: bool,
    pub name: String,
    /// *Sans titre*: « Enregistrer » needs a name first.
    pub untitled: bool,
    /// What: "projet", "timeline".
    pub unsaved: Vec<&'static str>,
    /// The timeline show with unsaved edits.
    pub timeline: Option<String>,
}

/// See `QuitCheck`. The fingerprint is computed fresh (the timelines read
/// from disk with the lock released), never the cached `last_modified`.
pub fn quit_check(shared: &Mutex<Shared>) -> QuitCheck {
    let (mut p, current, saved, shows, timeline) = {
        let s = shared.lock().unwrap();
        (snapshot(&s), s.project.current.clone(), s.project.saved, crate::timeline::ShowStore::new(s.shows.dir().to_path_buf()), s.timeline.unsaved().map(str::to_string))
    };
    p.timelines = shows.load_all();
    let project = p.fingerprint() != saved;
    let mut unsaved = Vec::new();
    if project {
        unsaved.push("projet");
    }
    if timeline.is_some() {
        unsaved.push("timeline");
    }
    QuitCheck { modified: !unsaved.is_empty(), name: current.clone().unwrap_or_else(|| UNTITLED.into()), untitled: current.is_none(), unsaved, timeline }
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
        s.grid = crate::grid::CueGrid::load(dir.join("pages.json"));
        crate::grid::refresh(&mut s);
        s.shows = crate::timeline::ShowStore::new(dir.join("shows"));
        s.lfos = crate::lfo::LfoStore::load_or_create(dir.join("lfos.json"), &s.controls);
        s.routes = crate::audio::routes::RouteStore::load_or_create(dir.join("audio_routes.json"), &s.controls);
        s.palettes = crate::live::PaletteStore::load_or_create(dir.join("palettes.json"));
        s.scan_presets = crate::scan::ScanPresetStore::load_or_create(dir.join("scan_presets.json"));
        s.midi.store = ProfileStore::load(dir.join("midi"));
        s.figures = crate::figures::FigureStore::load(dir.join("figures"));
        s.user_cues = crate::usercues::UserCueStore::load_or_create(dir.join("user_cues.json"));
        s.cue_lists = crate::cuelist::CueListStore::load_or_create(dir.join("cuelists.json"));
        s.macros = crate::macros::Macros::load(dir.join("macros.json"));
        s.showlist = crate::showlist::ShowListRunner::load_inline(dir.join("showlist.json"));
        s.pools = crate::pools::PoolStore::load_or_create(dir.join("pools.json"));
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
    fn cue_notes_tags_and_colour_mark_the_project_modified_and_come_back() {
        let dir = temp_dir("cue-notes");
        let shared = studio(&dir);
        post(&shared, &dir, "save-as", json!({ "path": "Notes" })).unwrap();
        assert!(!modified(&shared));
        let id = shared.lock().unwrap().presets[0].id.clone();
        let slot = crate::cues::CueSlot { notes: "seulement avec brouillard".into(), tags: vec!["Intro".into()], color: Some([255, 0, 64]), ..Default::default() };
        shared.lock().unwrap().deck.set_slot(&id, slot.clone());
        assert!(modified(&shared), "a note changes the fingerprint");
        post(&shared, &dir, "save", json!({})).unwrap();
        shared.lock().unwrap().deck.set_slot(&id, crate::cues::CueSlot::default());
        post(&shared, &dir, "open", json!({ "path": "Notes.lsproj" })).unwrap();
        assert_eq!(shared.lock().unwrap().deck.slot(&id), slot);
        let _ = std::fs::remove_dir_all(dir);
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
            s.grid.set_size(6, 4).unwrap();
            let presets = s.presets.clone();
            s.grid.new_page("Rappel", &presets).unwrap();
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
            s.grid = crate::grid::CueGrid::load(dir.join("pages.json"));
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
        assert_eq!((s.grid.cols, s.grid.rows, s.grid.view()[0].cells.len()), (6, 4, 24), "the grid size is back, resolved");
        assert!(s.grid.view().iter().any(|p| p.name == "Rappel"));
        let pages_on_disk: crate::grid::CueGrid = serde_json::from_str(&std::fs::read_to_string(dir.join("pages.json")).unwrap()).unwrap();
        assert_eq!(pages_on_disk, s.grid);
        assert_eq!(now.fingerprint(), before.fingerprint(), "every section is back");
        assert_eq!(s.tempo.bpm, 128.0);
        assert_eq!(now.timelines.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Ouverture"], "the other timeline went");
        // The working copy on disk is the project's too.
        let on_disk: Vec<Scene> = serde_json::from_str(&std::fs::read_to_string(dir.join("scenes.json")).unwrap()).unwrap();
        assert_eq!(on_disk.len(), 2);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-419: the speed groups and the *Cadence* travel with the project;
    /// an older project opens with every group at ×1 and 100 %, and
    /// every cell, layer and modulator in the default group.
    #[test]
    fn speed_groups_are_saved_in_the_project_and_old_projects_open_at_x1() {
        use crate::speed::SpeedMode;
        let dir = temp_dir("speed-groups");
        let shared = studio(&dir);
        {
            let mut s = shared.lock().unwrap();
            s.speed.change_on_bar = false;
            s.speed.set_mode(1, SpeedMode::Multiple { mult: 0.5 }, 0.0, 0.0, 4).unwrap();
            s.speed.set_mode(4, SpeedMode::FreeBpm { bpm: 90.0 }, 0.0, 0.0, 4).unwrap();
            s.speed.set_rate(1.5).unwrap();
        }
        post(&shared, &dir, "save-as", json!({ "path": "Vitesses" })).unwrap();
        let saved = parse(&std::fs::read_to_string(dir.join("projects/Vitesses.lsproj")).unwrap()).unwrap();
        assert_eq!(saved.speed.groups[0], SpeedMode::Multiple { mult: 0.5 });
        shared.lock().unwrap().speed = Default::default();
        post(&shared, &dir, "open", json!({ "path": "Vitesses.lsproj" })).unwrap();
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.speed.groups[3], SpeedMode::FreeBpm { bpm: 90.0 });
            assert_eq!(s.speed.rate(), 1.5);
        }
        let old: Project = serde_json::from_str(r#"{"grid":{"slots":{"a":{"layer":2}}},"layers":{"layers":[{"dimmer":0.5},{},{},{}]},"lfos":[{"target":"master.size"}]}"#).unwrap();
        assert_eq!(old.speed, crate::tempo::SpeedGroups::default());
        assert_eq!(old.grid.slot("a").speed_group, None);
        assert!(old.layers.layers.iter().all(|l| l.speed_group == 0));
        assert!(old.lfos.iter().all(|m| m.speed_group == 0));
        let mut s = shared.lock().unwrap();
        apply(&mut s, &old);
        assert!(s.speed.groups.iter().all(|g| *g == SpeedMode::Multiple { mult: 1.0 }));
        assert_eq!(s.speed.rate(), 1.0);
        assert!(!s.gate.is_armed());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-420: group masters travel with the project; an older project
    /// (no `group_masters`) opens every group at 100 %.
    #[test]
    fn group_masters_are_saved_in_the_project_and_old_projects_open_at_full() {
        use crate::projection::{ProjectionConfig, ProjectionZone, ZoneGroup};
        let dir = temp_dir("group-masters");
        let shared = studio(&dir);
        {
            let mut s = shared.lock().unwrap();
            let cfg = ProjectionConfig {
                zones: vec![ProjectionZone { id: 1, ..Default::default() }, ProjectionZone { id: 2, ..Default::default() }],
                default_zones: vec![1],
                groups: vec![ZoneGroup { name: "Centre".into(), zones: vec![1], master: 1.0 }, ZoneGroup { name: "Côtés".into(), zones: vec![2], master: 1.0 }],
            };
            s.projection.set(cfg).unwrap();
            s.projection.set_master(2, 0.4);
        }
        post(&shared, &dir, "save-as", json!({ "path": "Groupes" })).unwrap();
        let saved = parse(&std::fs::read_to_string(dir.join("projects/Groupes.lsproj")).unwrap()).unwrap();
        assert_eq!(saved.group_masters, [1.0, 0.4]);
        shared.lock().unwrap().projection.set_master(2, 1.0);
        post(&shared, &dir, "open", json!({ "path": "Groupes.lsproj" })).unwrap();
        assert_eq!(shared.lock().unwrap().projection.masters(), [1.0, 0.4]);
        let old: Project = serde_json::from_str("{}").unwrap();
        assert!(old.group_masters.is_empty());
        let mut s = shared.lock().unwrap();
        apply(&mut s, &old);
        assert_eq!(s.projection.masters(), [1.0, 1.0]);
        assert!(!s.gate.is_armed());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-418: the executor bank travels with the project; an older project
    /// (no `executors`) opens with one empty page.
    #[test]
    fn executors_are_saved_in_the_project_and_old_projects_open_with_one_empty_page() {
        use crate::executors::{ButtonFn, ExecTarget, Executor, ExecutorBank, FaderFn};
        let dir = temp_dir("executors");
        let shared = studio(&dir);
        let cue = shared.lock().unwrap().presets[0].id.clone();
        let mut bank = ExecutorBank::default();
        bank.pages[0].name = "Beams".into();
        bank.pages[0].handles[2] = Executor { label: "Montée".into(), target: ExecTarget::Cue { id: cue.clone() }, fader: FaderFn::FadeIn, buttons: [ButtonFn::Flash, ButtonFn::BlackHeld] };
        shared.lock().unwrap().executors.set_bank(bank.clone());
        assert!(modified(&shared), "an assignment is a change");
        post(&shared, &dir, "save-as", json!({ "path": "Exécuteurs" })).unwrap();
        let saved = parse(&std::fs::read_to_string(dir.join("projects/Exécuteurs.lsproj")).unwrap()).unwrap();
        assert_eq!(saved.executors, bank);
        shared.lock().unwrap().executors.set_bank(ExecutorBank::default());
        post(&shared, &dir, "open", json!({ "path": "Exécuteurs.lsproj" })).unwrap();
        assert_eq!(shared.lock().unwrap().executors.bank, bank);
        assert!(!modified(&shared));
        let old: Project = serde_json::from_str("{}").unwrap();
        assert_eq!(old.executors, ExecutorBank::default());
        let mut s = shared.lock().unwrap();
        apply(&mut s, &old);
        assert_eq!(s.executors.bank.pages.len(), 1);
        assert_eq!(s.executors.bank.pages[0].handles[2], Executor::default());
        assert!(!s.gate.is_armed());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-427: the desk travels with the project; an older project (no
    /// `desks`) opens with an empty desk; a forbidden binding in a file
    /// is dropped on open.
    #[test]
    fn desks_are_saved_in_the_project_and_old_projects_open_with_an_empty_desk() {
        use crate::desk::{Binding, DeskItem, DeskKind, DeskTab};
        let dir = temp_dir("desks");
        let shared = studio(&dir);
        let xy = DeskItem { id: 1, kind: DeskKind::XyPad, w: 4, h: 4, binding: Binding::Control { id: "master.pos_x".into() }, binding_y: Binding::Control { id: "master.pos_y".into() }, spring: true, ..Default::default() };
        let tabs = vec![DeskTab { name: "Intro".into(), items: vec![xy], ..Default::default() }];
        shared.lock().unwrap().desks.set_tabs(tabs.clone());
        assert!(modified(&shared), "a desk is a change");
        post(&shared, &dir, "save-as", json!({ "path": "Pupitre" })).unwrap();
        let text = std::fs::read_to_string(dir.join("projects/Pupitre.lsproj")).unwrap();
        assert_eq!(parse(&text).unwrap().desks, tabs);
        shared.lock().unwrap().desks.set_tabs(Vec::new());
        post(&shared, &dir, "open", json!({ "path": "Pupitre.lsproj" })).unwrap();
        assert_eq!(shared.lock().unwrap().desks.tabs, tabs);
        assert!(!modified(&shared));
        // A hand-edited file binding the arming: dropped.
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["desks"][0]["items"][0] = json!({ "id": 1, "kind": "button", "binding": { "type": "control", "id": "transport.arm" } });
        let p = parse(&v.to_string()).unwrap();
        assert_eq!(p.desks[0].items[0].binding, Binding::None);
        let old: Project = serde_json::from_str("{}").unwrap();
        assert!(old.desks.is_empty());
        let mut s = shared.lock().unwrap();
        apply(&mut s, &old);
        assert!(s.desks.tabs.is_empty());
        assert!(!s.gate.is_armed());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-425: macros are saved in the project and come back; a project
    /// without `macros` opens with none; one whose macro holds a line that
    /// no longer parses opens, that macro disabled, with a warning; a
    /// refused line is emptied, never kept.
    #[test]
    fn macros_are_saved_in_the_project_and_a_bad_line_opens_disabled() {
        use crate::macros::{Macro, MacroStep, Wait};
        let dir = temp_dir("macros");
        let shared = studio(&dir);
        let m = Macro { id: "m1".into(), name: "Montée".into(), key: Some("M".into()), steps: vec![MacroStep { command: "master.size 50".into(), wait: Wait::Beats(1.0), enabled: true }], ..Default::default() };
        crate::macros::set(&mut shared.lock().unwrap(), vec![m.clone()]).unwrap();
        assert!(modified(&shared), "a macro is a change");
        post(&shared, &dir, "save-as", json!({ "path": "Macros" })).unwrap();
        let path = dir.join("projects/Macros.lsproj");
        let saved = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.macros, vec![m.clone()]);
        crate::macros::set(&mut shared.lock().unwrap(), Vec::new()).unwrap();
        post(&shared, &dir, "open", json!({ "path": "Macros.lsproj" })).unwrap();
        assert_eq!(shared.lock().unwrap().macros.list(), std::slice::from_ref(&m));
        assert!(shared.lock().unwrap().controls.get("macro.m1.go").is_some());
        assert!(!modified(&shared));
        // Older project: no macros.
        let old: Project = serde_json::from_str("{}").unwrap();
        assert!(old.macros.is_empty());
        // A line that no longer parses, and a refused one written by hand.
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v["macros"][0]["steps"][0]["command"] = json!("cue \"Plus là\" go");
        v["macros"][0]["steps"].as_array_mut().unwrap().push(json!({ "command": "armer" }));
        std::fs::write(&path, v.to_string()).unwrap();
        let r = post(&shared, &dir, "open", json!({ "path": "Macros.lsproj" })).unwrap();
        let warnings = r["warnings"].as_array().cloned().unwrap_or_default();
        assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("macro désactivée")), "{r}");
        let s = shared.lock().unwrap();
        let m = &s.macros.list()[0];
        assert!(!m.enabled);
        assert_eq!(m.steps[0].command, "cue \"Plus là\" go");
        assert!(m.steps[1].command.is_empty() && !m.steps[1].enabled);
        assert!(!s.gate.is_armed());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-444: the show list is saved in the project and comes back; a
    /// project saved before it opens with an empty list.
    #[test]
    fn the_show_list_is_saved_in_the_project_and_old_projects_open_empty() {
        use crate::showlist::{ShowList, ShowListItem};
        let dir = temp_dir("showlist");
        let shared = studio(&dir);
        let list = ShowList { items: vec![ShowListItem { show: "Intro".into(), follow_system_tempo: false, ..Default::default() }, ShowListItem { show: "Final".into(), wait_timecode: true, ..Default::default() }], loop_list: true };
        crate::showlist::set(&mut shared.lock().unwrap(), list).unwrap();
        let saved_list = shared.lock().unwrap().showlist.list().clone();
        assert_eq!(saved_list.items.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 2]);
        post(&shared, &dir, "save-as", json!({ "path": "Liste" })).unwrap();
        let path = dir.join("projects/Liste.lsproj");
        let saved = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.show_list, saved_list);
        crate::showlist::set(&mut shared.lock().unwrap(), ShowList::default()).unwrap();
        post(&shared, &dir, "open", json!({ "path": "Liste.lsproj" })).unwrap();
        assert_eq!(shared.lock().unwrap().showlist.list(), &saved_list);
        assert_eq!(crate::load_json::<ShowList>(&dir.join("showlist.json")), saved_list, "the working copy too");
        // Older project: no list.
        let old: Project = serde_json::from_str("{}").unwrap();
        assert!(old.show_list.items.is_empty() && !old.show_list.loop_list);
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("show_list");
        std::fs::write(&path, v.to_string()).unwrap();
        post(&shared, &dir, "open", json!({ "path": "Liste.lsproj" })).unwrap();
        assert!(shared.lock().unwrap().showlist.list().items.is_empty());
        assert!(!shared.lock().unwrap().gate.is_armed());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-422: presets and the references to them come back identical; a
    /// preset change changes the fingerprint; an older project (no pools,
    /// no refs) opens with none; a reference to an unknown preset opens
    /// with a warning, never an error.
    #[test]
    fn pools_and_references_round_trip_and_unknown_ones_warn() {
        use crate::pools::{PoolPreset, PoolValues, PresetRefs};
        let dir = temp_dir("pools");
        let shared = studio(&dir);
        let preset = PoolPreset { id: "size:petit".into(), name: "Petit".into(), values: PoolValues::Size { size: 0.4, size_x: 1.0, size_y: 1.0 } };
        let cue = shared.lock().unwrap().presets[0].id.clone();
        let refs = PresetRefs { size: Some("size:petit".into()), ..Default::default() };
        {
            let mut s = shared.lock().unwrap();
            let before = snapshot(&s).fingerprint();
            s.pools.set(vec![preset.clone()]).unwrap();
            assert_ne!(snapshot(&s).fingerprint(), before, "a preset is a change");
            let mut slot = s.deck.slot(&cue);
            slot.refs = refs.clone();
            s.deck.set_slot(&cue, slot);
        }
        post(&shared, &dir, "save-as", json!({ "path": "Prereglages" })).unwrap();
        let path = dir.join("projects/Prereglages.lsproj");
        let saved = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.pools, vec![preset.clone()]);
        assert_eq!(saved.grid.slots[&cue].refs, refs);
        {
            let mut s = shared.lock().unwrap();
            let f = snapshot(&s).fingerprint();
            s.pools.set(vec![PoolPreset { name: "Renommé".into(), ..preset.clone() }]).unwrap();
            assert_ne!(snapshot(&s).fingerprint(), f, "a rename is a change too");
            s.deck.set_slot(&cue, Default::default());
            s.pools.set(Vec::new()).unwrap();
        }
        let r = post(&shared, &dir, "open", json!({ "path": "Prereglages.lsproj" })).unwrap();
        assert!(r["warnings"].as_array().is_none_or(|w| w.is_empty()), "{r}");
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.pools.list(), std::slice::from_ref(&preset));
            assert_eq!(s.deck.slot(&cue).refs, refs);
        }
        assert!(!modified(&shared), "opened as saved");
        assert_eq!(crate::load_json::<Vec<PoolPreset>>(&dir.join("pools.json")), vec![preset.clone()], "the working copy too");
        // The preset gone from the file: the reference stays, ignored, said.
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("pools");
        std::fs::write(&path, v.to_string()).unwrap();
        let r = post(&shared, &dir, "open", json!({ "path": "Prereglages.lsproj" })).unwrap();
        let warnings = r["warnings"].as_array().cloned().unwrap_or_default();
        assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("préréglage inconnu « size:petit »")), "{r}");
        {
            let s = shared.lock().unwrap();
            assert!(s.pools.list().is_empty());
            let base = crate::live::LiveModifiers::default();
            assert_eq!(crate::pools::resolve(&s.deck.slot(&cue).refs, &s.pools, &base), base, "rendered with the cue's own values");
            assert!(!s.gate.is_armed());
        }
        // An older project: no pools, no refs.
        let old: Project = serde_json::from_str("{}").unwrap();
        assert!(old.pools.is_empty());
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
        // ... and, from before T-272, the default 8×5 pages.
        assert_eq!(shared.lock().unwrap().grid, crate::grid::CueGrid::default());
        assert_eq!(shared.lock().unwrap().grid.view()[0].cells.len(), 40);
        assert!(!modified(&shared));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// T-405: user cues and the cells holding them come back with the
    /// project; a project from before them opens with none.
    /// T-410: cue lists come back with the project, identical; a project
    /// from before them opens with none; a list firing `transport.arm` is
    /// refused and nothing changes.
    #[test]
    fn cue_lists_are_saved_in_the_project_and_old_projects_still_open() {
        use crate::cuelist::{CueList, SplitTiming, Step, StepAction, StepSource, StepTrigger};
        let dir = temp_dir("cuelists");
        let shared = studio(&dir);
        let list = {
            let mut s = shared.lock().unwrap();
            let steps = vec![
                Step { label: "Ouverture".into(), source: StepSource::Cue { id: s.presets[0].id.clone() }, fade_in_s: 2.0, ..Step::default() },
                Step {
                    source: StepSource::Black,
                    trigger: StepTrigger::Beats { n: 4 },
                    split: Some(SplitTiming { geometry_s: 1.0, color_s: 3.0, brightness_s: 0.5 }),
                    fire: Some(StepAction { control: "tempo.resync".into(), value: 1.0 }),
                    ..Step::default()
                },
            ];
            crate::cuelist::save(&mut s, CueList { name: "Acte 1".into(), layer: 2, wrap: true, steps }).unwrap();
            crate::cuelist::load(&mut s, "Acte 1").unwrap();
            s.cue_lists.get("Acte 1").unwrap().clone()
        };
        assert!(modified(&shared), "a cue list is a change");
        post(&shared, &dir, "save-as", json!({ "path": "Avec listes" })).unwrap();
        assert_eq!(read(&dir.join("projects/Avec listes.lsproj")).unwrap().cue_lists, std::slice::from_ref(&list));
        // Another project without lists: the loaded list goes.
        std::fs::write(dir.join("projects/ancien.lsproj"), json!({ "format_version": 1, "name": "ancien" }).to_string()).unwrap();
        post(&shared, &dir, "open", json!({ "path": "ancien" })).unwrap();
        {
            let s = shared.lock().unwrap();
            assert!(s.cue_lists.list().is_empty());
            assert!(s.cue_list.list.is_none());
        }
        post(&shared, &dir, "open", json!({ "path": "Avec listes" })).unwrap();
        assert_eq!(shared.lock().unwrap().cue_lists.list(), std::slice::from_ref(&list), "reopened identical");
        let on_disk = crate::cuelist::CueListStore::load_or_create(dir.join("cuelists.json"));
        assert_eq!(on_disk.list(), [list], "the working copy is the project's");
        // A step firing the arm control is refused, nothing changes.
        let bad = json!({ "format_version": 1, "cuelists": [{ "name": "X", "steps": [{ "fire": { "control": "transport.arm" } }] }] });
        std::fs::write(dir.join("projects/Mauvais.lsproj"), bad.to_string()).unwrap();
        assert_eq!(post(&shared, &dir, "open", json!({ "path": "Mauvais" })).unwrap_err().0, 400);
        assert_eq!(shared.lock().unwrap().cue_lists.list()[0].name, "Acte 1");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn user_cues_are_saved_in_the_project_and_old_projects_still_open() {
        let dir = temp_dir("usercues");
        let shared = studio(&dir);
        let look = Settings { color: [255, 0, 255], scale: 0.4, ..Settings::default() };
        let (page, id) = {
            let mut s = shared.lock().unwrap();
            let mut g = s.grid.clone();
            let page = g.new_page("Mes looks", &s.presets).unwrap();
            s.grid = g;
            crate::usercues::refresh(&mut s);
            s.settings = look.clone();
            let id = crate::usercues::store(&mut s, page, 0, Some("Intro"), crate::usercues::StoreFilter::All, false).unwrap();
            (page, id)
        };
        assert!(modified(&shared), "a stored cue is a change");
        post(&shared, &dir, "save-as", json!({ "path": "Avec cues" })).unwrap();
        // Through the file: parse → same cues.
        let p = read(&dir.join("projects/Avec cues.lsproj")).unwrap();
        assert_eq!(p.user_cues.len(), 1);
        assert_eq!((p.user_cues[0].id.as_str(), &p.user_cues[0].settings), (id.as_str(), &look));
        {
            let mut s = shared.lock().unwrap();
            crate::usercues::edit(&mut s, crate::usercues::GridEdit::Delete { from: (page, 0) }).unwrap();
            assert!(s.user_cues.list().is_empty());
        }
        post(&shared, &dir, "open", json!({ "path": "Avec cues" })).unwrap();
        {
            let mut s = shared.lock().unwrap();
            assert_eq!(s.user_cues.get(&id).unwrap().settings, look, "reopened identical");
            assert_eq!(s.grid.slot(page, 0), Some(id.as_str()), "its cell plays it again");
            assert!(crate::controls::press_cue(&mut s, &id, None, true));
            // Ids are never given twice, even after the project brought u-1 back.
            let next = s.user_cues.clone().create(None, None, Settings::default(), None).unwrap();
            assert_ne!(next, id);
        }
        assert!(dir.join("user_cues.json").is_file(), "the working copy is the project's");
        let back = crate::usercues::UserCueStore::load_or_create(dir.join("user_cues.json"));
        assert_eq!(back.list().len(), 1);
        // A damaged section is refused, nothing changes.
        std::fs::write(dir.join("projects/Mauvais.lsproj"), json!({ "format_version": 1, "user_cues": [{ "id": "tunnels-001" }] }).to_string()).unwrap();
        assert_eq!(post(&shared, &dir, "open", json!({ "path": "Mauvais" })).unwrap_err().0, 400);
        assert_eq!(shared.lock().unwrap().user_cues.list().len(), 1);
        // A project from before T-405 opens with no user cues.
        std::fs::write(dir.join("projects/ancien.lsproj"), json!({ "format_version": 1, "name": "ancien" }).to_string()).unwrap();
        post(&shared, &dir, "open", json!({ "path": "ancien" })).unwrap();
        let s = shared.lock().unwrap();
        assert!(s.user_cues.list().is_empty());
        assert!(!s.presets.iter().any(|p| p.category == crate::usercues::CATEGORY));
        drop(s);
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
        let text = json!({ "format_version": 1, "name": "x", "venue": { "room": [10, 4, 3] }, "cue_lists": [1, 2] }).to_string();
        std::fs::write(dir.join("projects/x.lsproj"), text).unwrap();
        post(&shared, &dir, "open", json!({ "path": "x" })).unwrap();
        post(&shared, &dir, "save", json!({})).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("projects/x.lsproj")).unwrap()).unwrap();
        assert_eq!(v["venue"], json!({ "room": [10, 4, 3] }));
        assert_eq!(v["cue_lists"], json!([1, 2]));
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
            "safety": { "max_flash_hz": 100.0 }, "outputs": { "outputs": [{ "id": "main", "limits": { "max_power": 1.0 } }] }, "presence": { "hold_to_run": false },
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
        assert_eq!(s.outputs.active_limits(), crate::power::OutputLimits::default(), "power caps untouched");
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
            // A newer project opens read only (T-288), unless its known sections no longer decode.
            ("version", json!({ "format_version": 99, "scenes": 5 }).to_string()),
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
    fn quit_check_follows_saves_edits_and_the_timeline_editor() {
        let dir = temp_dir("quitcheck");
        let shared = studio(&dir);
        let get = |shared: &Mutex<Shared>| match route(shared, false, "quit-check", "").expect("known route") {
            Reply::Json(v) => v,
            Reply::Text(code, t) => panic!("{code} {t}"),
        };
        // Untitled and untouched: nothing to lose.
        let q = quit_check(&shared);
        assert!(!q.modified && q.untitled && q.unsaved.is_empty());
        assert_eq!(q.name, UNTITLED);
        // An edit: the project would be lost, and « Enregistrer » needs a name.
        shared.lock().unwrap().live.size = 0.4;
        let v = get(&shared);
        assert_eq!((v["modified"].as_bool(), v["untitled"].as_bool()), (Some(true), Some(true)));
        assert_eq!(v["unsaved"], json!(["projet"]));
        // Saved: nothing again, and the cached flag for /api/frame agrees.
        shared.lock().unwrap().project.last_modified = true;
        post(&shared, &dir, "save-as", json!({ "path": "Mon show" })).unwrap();
        let q = quit_check(&shared);
        assert!(!q.modified && !q.untitled);
        assert_eq!(q.name, "Mon show");
        assert!(!shared.lock().unwrap().project.last_modified);
        // A show edited in the timeline and not saved: the timeline only.
        {
            let mut s = shared.lock().unwrap();
            s.timeline.load(Show { name: "Intro".into(), ..Default::default() });
            assert!(s.timeline.unsaved().is_none(), "just loaded");
            s.timeline.edit(Show::default());
        }
        let q = quit_check(&shared);
        assert!(q.modified);
        assert_eq!(q.unsaved, vec!["timeline"]);
        assert_eq!(q.timeline.as_deref(), Some("Intro"));
        shared.lock().unwrap().timeline.saved();
        assert!(!quit_check(&shared).modified);
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

    #[test]
    fn a_damaged_open_project_is_quarantined_but_a_newer_one_is_kept() {
        let (dir, path) = crate::datafile::tests::with_damaged("project", "projects/Mon show.lsproj");
        assert!(read_in_data_dir(&path, &dir).is_err());
        assert!(!path.exists());
        crate::datafile::tests::check_kept(&dir, "projects/Mon show.lsproj", false);
        // Valid JSON from a newer version: an error, but not damaged.
        let dir = temp_dir("project-newer");
        let path = dir.join("projects/Futur.lsproj");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, json!({ "format_version": 999 }).to_string()).unwrap();
        assert!(read_in_data_dir(&path, &dir).is_err());
        assert!(path.exists() && !dir.join(crate::datafile::QUARANTINE).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_open_project_at_startup_is_quarantined_and_reported() {
        let dir = temp_dir("project-startup");
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        std::fs::write(dir.join("recent.json"), r#"{"current":"Cassé","recent":["Cassé"]}"#).unwrap();
        std::fs::write(dir.join("projects/Cassé.lsproj"), crate::datafile::tests::DAMAGED).unwrap();
        let shared = studio(&dir);
        let mut s = shared.lock().unwrap();
        s.sync_data_warnings();
        assert_eq!(s.data_warnings.len(), 1);
        assert_eq!(s.data_warnings[0].file, "projects/Cassé.lsproj");
        assert!(!s.data_warnings[0].safety);
        assert!(s.gate.interlock_ok(crate::interlock::DATA_ACK), "not a safety file");
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------- T-288: formats, migrations, named versions ----------

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/projects")
    }

    fn get(shared: &Mutex<Shared>, action: &str) -> Value {
        match route(shared, false, action, "").expect("known route") {
            Reply::Json(v) => v,
            Reply::Text(code, t) => panic!("{code} {t}"),
        }
    }

    #[test]
    fn the_migration_chain_leads_to_the_current_format() {
        assert_eq!(crate::formats::current_of(MIGRATIONS), PROJECT_FORMAT);
    }

    /// Every fixture (made by our own code, one or more per format) opens;
    /// those of our formats open strictly, a newer one only read only.
    #[test]
    fn every_fixture_project_opens() {
        let mut seen = 0;
        for entry in std::fs::read_dir(fixtures()).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != EXTENSION) {
                continue;
            }
            seen += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            let loaded = parse_file(&text).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            if loaded.from <= PROJECT_FORMAT {
                assert!(parse(&text).is_ok(), "{}", path.display());
            } else {
                assert!(loaded.newer().is_some() && parse(&text).is_err(), "{}", path.display());
            }
            // And through the whole opening path, in a studio of its own.
            let dir = temp_dir(&format!("fixture-{seen}"));
            std::fs::create_dir_all(dir.join("projects")).unwrap();
            std::fs::copy(&path, dir.join("projects").join(path.file_name().unwrap())).unwrap();
            let shared = studio(&dir);
            let reply = post(&shared, &dir, "open", json!({ "path": name_of(&path) })).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
            assert_eq!(reply["read_only"], loaded.newer().is_some(), "{}", path.display());
            assert!(!shared.lock().unwrap().gate.is_armed());
            let _ = std::fs::remove_dir_all(dir);
        }
        assert!(seen >= 3, "fixtures found: {seen}");
    }

    #[test]
    fn the_full_fixture_brings_its_sections_back() {
        let p = read(&fixtures().join("v1-complet.lsproj")).unwrap();
        assert_eq!(p.playlist, ["Ouverture", "Final"]);
        assert_eq!(p.timelines.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Intro"]);
        assert_eq!(p.tempo.bpm, 128.0);
        assert_eq!((p.pages.cols, p.pages.rows), (6, 4));
    }

    /// Writes the fixtures of the current format from a studio state (run
    /// by hand when a format is added: `cargo test -p laser-studio --
    /// --ignored write_fixtures`). The files are ours: no third-party content.
    #[test]
    #[ignore]
    fn write_fixtures() {
        let dir = temp_dir("write-fixtures");
        let shared = studio(&dir);
        {
            let mut s = shared.lock().unwrap();
            s.scenes.upsert(scene("Ouverture", 0.4)).unwrap();
            s.scenes.upsert(scene("Final", 0.9)).unwrap();
            s.grid.set_size(6, 4).unwrap();
            s.live.size = 0.8;
            let t = s.now_s();
            s.tempo.set_bpm_manual(128.0, t);
            s.shows.save(&Show { name: "Intro".into(), ..Default::default() }).unwrap();
        }
        post(&shared, &dir, "save-as", json!({ "path": "complet" })).unwrap();
        let mut p = read(&dir.join("projects/complet.lsproj")).unwrap();
        p.app_version = "0.1.0".into();
        p.saved_at = "2026-10-05T12:00:00Z".into();
        let out = fixtures().join(format!("v{PROJECT_FORMAT}-complet.lsproj"));
        std::fs::create_dir_all(fixtures()).unwrap();
        std::fs::write(&out, project_bytes(&p).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_newer_project_opens_read_only_and_is_never_written_over() {
        let dir = temp_dir("newer-open");
        let shared = studio(&dir);
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        let future = fixtures().join("v99-futur.lsproj");
        let original = std::fs::read(&future).unwrap();
        std::fs::write(dir.join("projects/Futur.lsproj"), &original).unwrap();
        let v = post(&shared, &dir, "open", json!({ "path": "Futur" })).unwrap();
        assert_eq!((v["read_only"].as_bool(), v["file_format"].as_u64()), (Some(true), Some(99)));
        assert!(v["notice"].as_str().unwrap().starts_with(NEWER_FR), "{v}");
        assert_eq!(shared.lock().unwrap().scenes.list().len(), 1, "its known sections are in");
        // Enregistrer: refused, also over the same name with « Enregistrer sous… ».
        shared.lock().unwrap().live.size = 0.3;
        let (code, t) = post(&shared, &dir, "save", json!({})).unwrap_err();
        assert_eq!(code, 409);
        assert!(t.starts_with(NEWER_FR), "{t}");
        assert_eq!(post(&shared, &dir, "save-as", json!({ "path": "Futur" })).unwrap_err().0, 409);
        assert_eq!(std::fs::read(dir.join("projects/Futur.lsproj")).unwrap(), original, "untouched");
        assert!(!dir.join("projects").read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".bak")), "no migration of a newer file");
        // A copy under another name: written in our format, no longer read only.
        let v = post(&shared, &dir, "save-as", json!({ "path": "Copie" })).unwrap();
        assert_eq!(v["read_only"], false);
        let copy = read(&dir.join("projects/Copie.lsproj")).unwrap();
        assert_eq!(copy.format_version, PROJECT_FORMAT);
        assert!(copy.extra.contains_key("section_du_futur"), "unknown sections kept in the copy");
        // At startup too: the open project from a newer version is read only.
        let shared = studio(&dir);
        post(&shared, &dir, "open", json!({ "path": "Futur" })).unwrap();
        drop(shared);
        let shared = studio(&dir);
        assert_eq!(shared.lock().unwrap().project.read_only, Some(99));
        assert_eq!(post(&shared, &dir, "save", json!({})).unwrap_err().0, 409);
        assert_eq!(std::fs::read(dir.join("projects/Futur.lsproj")).unwrap(), original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_newer_project_that_no_longer_decodes_is_refused_saying_why() {
        let text = json!({ "format_version": 7, "scenes": "plus une liste" }).to_string();
        let e = format!("{:#}", parse_file(&text).err().unwrap());
        assert!(e.contains("version plus récente") && e.contains("format 7"), "{e}");
        let e = format!("{:#}", parse(&json!({ "format_version": 7 }).to_string()).unwrap_err());
        assert!(e.contains(NEWER_FR) && e.contains("format 7"), "{e}");
    }

    fn add_note(mut v: Value) -> Result<Value> {
        v["note_v2"] = json!("ajoutée par la migration");
        Ok(v)
    }

    #[test]
    fn a_migration_keeps_the_original_before_writing() {
        const CHAIN: &[crate::formats::Step] = &[add_note];
        let dir = temp_dir("migrate");
        let path = dir.join("Ancien.lsproj");
        let original = std::fs::read_to_string(fixtures().join("v1-minimal.lsproj")).unwrap();
        std::fs::write(&path, &original).unwrap();
        let (loaded, text) = load_file_with(&path, CHAIN).unwrap();
        assert_eq!((loaded.from, loaded.current, loaded.migrated(), loaded.newer()), (1, 2, Some(1), None));
        assert_eq!(loaded.project.extra.get("note_v2"), Some(&json!("ajoutée par la migration")), "migrated before decoding");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "loading writes nothing");
        // The copy fails (a folder in its place): nothing is written.
        std::fs::create_dir_all(dir.join("Ancien.v1.bak.lsproj/x")).unwrap();
        assert!(upgrade_file(&path, &loaded, &text).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(dir.join("Ancien.v1.bak.lsproj")).unwrap();
        let bak = upgrade_file(&path, &loaded, &text).unwrap().unwrap();
        assert_eq!(bak, dir.join("Ancien.v1.bak.lsproj"));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), original, "original kept byte for byte");
        let now: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!((now["format_version"].as_u64(), now["note_v2"].as_str()), (Some(2), Some("ajoutée par la migration")));
        // Reopened: current, nothing more to do; the first original stays.
        let (again, text) = load_file_with(&path, CHAIN).unwrap();
        assert_eq!(again.migrated(), None);
        assert_eq!(upgrade_file(&path, &again, &text).unwrap(), None);
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);
        // `.bak` files are not projects.
        assert_eq!(list(&dir), ["Ancien"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_named_version_opens_as_a_copy_without_touching_the_project() {
        let dir = temp_dir("versions");
        let shared = studio(&dir);
        shared.lock().unwrap().scenes.upsert(scene("Avant", 0.5)).unwrap();
        post(&shared, &dir, "save-as", json!({ "path": "Festival" })).unwrap();
        // Unsaved change, then a version: the project stays modified.
        shared.lock().unwrap().scenes.upsert(scene("Version", 0.6)).unwrap();
        let v = post(&shared, &dir, "version-save", json!({ "name": "avant festival" })).unwrap();
        assert_eq!(v["notice"], "Version « avant festival » enregistrée");
        assert_eq!(v["modified"], true);
        assert_eq!(post(&shared, &dir, "version-save", json!({ "name": "../x" })).unwrap_err().0, 400);
        let list = get(&shared, "versions");
        assert_eq!(list["project"], "Festival");
        let versions = list["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0]["name"], "avant festival");
        let file = versions[0]["file"].as_str().unwrap().to_string();
        assert!(dir.join("versions/Festival").join(&file).is_file());
        // More changes, saved; then the version opens as a copy.
        shared.lock().unwrap().scenes.upsert(scene("Après", 0.7)).unwrap();
        post(&shared, &dir, "save", json!({})).unwrap();
        let saved = std::fs::read(dir.join("projects/Festival.lsproj")).unwrap();
        let v = post(&shared, &dir, "version-open", json!({ "file": file })).unwrap();
        assert_eq!((v["name"].as_str(), v["modified"].as_bool(), v["file"].is_null()), (Some(UNTITLED), Some(true), true));
        assert_eq!(v["suggested_name"], "Festival avant festival");
        let names: Vec<String> = shared.lock().unwrap().scenes.list().iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"Version".to_string()) && !names.contains(&"Après".to_string()), "{names:?}");
        assert_eq!(std::fs::read(dir.join("projects/Festival.lsproj")).unwrap(), saved, "the project is untouched");
        assert!(!shared.lock().unwrap().gate.is_armed());
        // « Enregistrer sous… » keeps it; the original project is still there.
        post(&shared, &dir, "save-as", json!({ "path": "Festival avant festival" })).unwrap();
        assert_eq!(list_names(&dir), ["Festival", "Festival avant festival"]);
        // Versions of the project, deleted.
        post(&shared, &dir, "open", json!({ "path": "Festival" })).unwrap();
        assert_eq!(post(&shared, &dir, "version-delete", json!({ "file": "../../projects/Festival.lsproj" })).unwrap_err().0, 400);
        assert_eq!(post(&shared, &dir, "version-open", json!({ "file": "20261005-2104-absente.lsproj" })).unwrap_err().0, 404);
        post(&shared, &dir, "version-delete", json!({ "file": file })).unwrap();
        assert!(get(&shared, "versions")["versions"].as_array().unwrap().is_empty());
        assert!(dir.join("projects/Festival.lsproj").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn list_names(dir: &Path) -> Vec<String> {
        list(&dir.join("projects"))
    }

    #[test]
    fn saving_hands_the_same_bytes_to_the_backup_copy() {
        let dir = temp_dir("backup-save");
        let copy = temp_dir("backup-save-copy");
        let shared = studio(&dir);
        let hub = std::sync::Arc::clone(&shared.lock().unwrap().project.backup);
        crate::backup::set_settings(&hub, serde_json::from_value(json!({ "enabled": true, "dir": copy })).unwrap()).unwrap();
        post(&shared, &dir, "save-as", json!({ "path": "Mon show" })).unwrap();
        hub.run_pending();
        let main = std::fs::read(dir.join("projects/Mon show.lsproj")).unwrap();
        let copy_file = copy.canonicalize().unwrap().join("Laser Studio/Mon show.lsproj");
        assert_eq!(std::fs::read(&copy_file).unwrap(), main, "identical byte for byte");
        // The copy folder is gone: the save still works, a warning shows.
        std::fs::remove_dir_all(&copy).unwrap();
        shared.lock().unwrap().live.size = 0.2;
        post(&shared, &dir, "save", json!({})).unwrap();
        hub.run_pending();
        assert_ne!(std::fs::read(dir.join("projects/Mon show.lsproj")).unwrap(), main, "main file written");
        assert!(!copy.exists(), "never recreated");
        let st = info(&shared)["backup"]["status"].clone();
        assert_eq!(st["backup_ok"], false);
        assert!(st["warning"].as_str().unwrap().starts_with("Copie de sauvegarde indisponible"), "{st}");
        let mut s = shared.lock().unwrap();
        s.sync_notices();
        assert!(s.notices.since(0).iter().any(|n| n.key.as_deref() == Some("backup") && !n.resolved));
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------- partial import and site offer (T-289) ----------

    fn midi(name: &str) -> Profile {
        Profile::parse(&format!(r#"{{ "name": "{name}" }}"#)).unwrap()
    }

    fn page(name: &str, cells: &[Option<CueRef>]) -> CuePage {
        CuePage { name: name.into(), slots: cells.to_vec(), category: None, user: true }
    }

    fn user_cue(id: &str, name: &str) -> UserCue {
        UserCue { id: id.into(), name: name.into(), ..Default::default() }
    }

    /// The open project and another one with clashing names.
    fn pair() -> (Project, Project) {
        let mut cur = Project { scenes: vec![scene("A", 0.5)], ..Project::default() };
        cur.midi.profiles.insert("pad".into(), midi("Pad"));
        cur.user_cues = vec![user_cue("u-1", "Logo")];
        cur.pages.pages = vec![page("Intro", &[Some(CueRef::User("u-1".into()))])];
        let mut src = Project { scenes: vec![scene("A", 0.9), scene("B", 0.3)], timelines: vec![Show { name: "Final".into(), ..Default::default() }], ..Project::default() };
        src.midi.profiles.insert("pad".into(), midi("Leur pad"));
        src.midi.profiles.insert("knobs".into(), midi("Knobs"));
        src.user_cues = vec![user_cue("u-1", "Logo"), user_cue("u-2", "Vague")];
        src.pages.pages = vec![page("Intro", &[Some(CueRef::User("u-2".into())), Some(CueRef::User("u-9".into())), Some(CueRef::Builtin("circle".into()))])];
        (cur, src)
    }

    #[test]
    fn importing_only_midi_touches_neither_pages_nor_scenes() {
        let (mut cur, src) = pair();
        let before = (cur.scenes.iter().map(|s| (s.name.clone(), s.settings.scale)).collect::<Vec<_>>(), cur.pages.clone(), cur.user_cues.clone(), cur.timelines.len());
        let r = merge(&mut cur, &src, &["midi".into()], Conflict::Rename).unwrap();
        assert_eq!(before, (cur.scenes.iter().map(|s| (s.name.clone(), s.settings.scale)).collect::<Vec<_>>(), cur.pages.clone(), cur.user_cues.clone(), cur.timelines.len()));
        let midi = &r["midi"];
        assert_eq!((midi.found, midi.added), (2, 1));
        assert_eq!(midi.renamed, [("pad".to_string(), "pad-2".to_string())]);
        assert_eq!(cur.midi.profiles["pad"].name, "Pad", "ours kept");
        assert_eq!(cur.midi.profiles["pad-2"].name, "Leur pad 2");
        assert!(cur.midi.profiles.contains_key("knobs"));
    }

    #[test]
    fn conflicts_are_renamed_replaced_or_skipped() {
        let (cur0, src) = pair();
        let scenes = |p: &Project| p.scenes.iter().map(|s| (s.name.clone(), s.settings.scale)).collect::<Vec<_>>();
        let mut cur = cur0.clone();
        let r = merge(&mut cur, &src, &["scenes".into()], Conflict::Rename).unwrap();
        assert_eq!(scenes(&cur), [("A".to_string(), 0.5), ("A 2".to_string(), 0.9), ("B".to_string(), 0.3)]);
        assert_eq!(r["scenes"].conflicts, ["A"]);
        let mut cur = cur0.clone();
        merge(&mut cur, &src, &["scenes".into()], Conflict::Replace).unwrap();
        assert_eq!(scenes(&cur), [("A".to_string(), 0.9), ("B".to_string(), 0.3)]);
        let mut cur = cur0.clone();
        let r = merge(&mut cur, &src, &["scenes".into()], Conflict::Skip).unwrap();
        assert_eq!(scenes(&cur), [("A".to_string(), 0.5), ("B".to_string(), 0.3)]);
        assert_eq!(r["scenes"].skipped, 1);
        // Nothing chosen, or an unknown part: refused.
        assert!(merge(&mut cur0.clone(), &src, &[], Conflict::Skip).is_err());
        assert!(merge(&mut cur0.clone(), &src, &["calibration".into()], Conflict::Skip).is_err());
    }

    #[test]
    fn imported_pages_point_at_the_imported_cues_or_at_nothing() {
        let (mut cur, src) = pair();
        merge(&mut cur, &src, &["user_cues".into(), "pages".into()], Conflict::Rename).unwrap();
        // « Logo » renamed « Logo 2 » with a new id, « Vague » added.
        let names: Vec<(&str, &str)> = cur.user_cues.iter().map(|c| (c.id.as_str(), c.name.as_str())).collect();
        assert_eq!(names, [("u-1", "Logo"), ("u-2", "Logo 2"), ("u-3", "Vague")]);
        let intro2 = cur.pages.pages.iter().find(|p| p.name == "Intro 2").unwrap();
        assert_eq!(intro2.slots, [Some(CueRef::User("u-3".into())), None, Some(CueRef::Builtin("circle".into()))]);
        assert_eq!(cur.pages.pages[0].slots, [Some(CueRef::User("u-1".into()))], "our page untouched");
        // Pages alone: their user cues don't exist here, the cells empty.
        let (mut cur, src) = pair();
        merge(&mut cur, &src, &["pages".into()], Conflict::Replace).unwrap();
        assert_eq!(cur.pages.pages.len(), 1);
        assert_eq!(cur.pages.pages[0].slots, [None, None, Some(CueRef::Builtin("circle".into()))]);
    }

    #[test]
    fn the_partial_import_route_previews_merges_and_leaves_the_project_modified() {
        let dir = temp_dir("partial");
        let shared = studio(&dir);
        shared.lock().unwrap().scenes.replace_in_memory(vec![scene("Ici", 1.0)]);
        post(&shared, &dir, "save-as", json!({ "path": "Autre" })).unwrap();
        shared.lock().unwrap().scenes.replace_in_memory(vec![scene("Là", 1.0)]);
        post(&shared, &dir, "save-as", json!({ "path": "Courant" })).unwrap();
        let preview = post(&shared, &dir, "import-partial", json!({ "from": "Autre", "sections": ["scenes"], "dry_run": true })).unwrap();
        assert_eq!(preview["import"]["scenes"]["added"], 1);
        assert_eq!(shared.lock().unwrap().scenes.list().len(), 1, "a preview changes nothing");
        assert!(!modified(&shared));
        let done = post(&shared, &dir, "import-partial", json!({ "from": "Autre", "sections": ["scenes"] })).unwrap();
        assert_eq!(done["name"], "Courant");
        let names: Vec<String> = shared.lock().unwrap().scenes.list().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, ["Là", "Ici"]);
        assert!(modified(&shared));
        assert!(!shared.lock().unwrap().gate.is_armed());
        assert_eq!(post(&shared, &dir, "import-partial", json!({ "from": "../x", "sections": ["scenes"] })).unwrap_err().0, 400);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_a_project_with_a_site_offers_it_and_changes_no_calibration() {
        let dir = temp_dir("site-offer");
        let shared = studio(&dir);
        let mut site = crate::sites::capture(&shared.lock().unwrap(), "Club");
        site.calibration.offset_x = 0.4;
        let p = Project { name: "Avec lieu".into(), extra: Map::from_iter([(crate::sites::PROJECT_KEY.to_string(), serde_json::to_value(&site).unwrap())]), ..Project::default() };
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        write_project(&dir.join("projects/Avec lieu.lsproj"), &p).unwrap();
        let before = shared.lock().unwrap().calibration;
        let v = post(&shared, &dir, "open", json!({ "path": "Avec lieu" })).unwrap();
        assert_eq!(v["site_offer"], "Club");
        assert_eq!(shared.lock().unwrap().calibration, before, "« Garder le profil actuel » is the default");
        // Saved again, the site stays with the project.
        post(&shared, &dir, "save", json!({})).unwrap();
        assert!(read(&dir.join("projects/Avec lieu.lsproj")).unwrap().extra.contains_key(crate::sites::PROJECT_KEY));
        // « Utiliser celui du projet ».
        crate::sites::use_project(&mut shared.lock().unwrap(), false, crate::interlock::ArmSource::Ui).unwrap();
        assert_eq!(shared.lock().unwrap().calibration.offset_x, 0.4);
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------- T-451: new project, templates ----------

    /// A project with something in every section `new_from` may keep, and
    /// in some it never keeps.
    fn rich() -> Project {
        let (mut p, _) = pair();
        p.timelines = vec![Show { name: "Final".into(), ..Default::default() }];
        p.palettes = vec![Palette { name: "Maison".into(), colors: vec![[255, 0, 0], [0, 0, 255]] }];
        p.grid.multi = !CueDeck::default().multi;
        p.tempo.bpm = 140.0;
        p.extra.insert(crate::sites::PROJECT_KEY.into(), json!({ "name": "Salle" }));
        p.extra.insert("calibration".into(), json!({ "offset_x": 5.0 }));
        p
    }

    #[test]
    fn new_from_keeps_exactly_the_ticked_sections() {
        let from = rich();
        let empty = Project::default();
        for bits in 0..16u8 {
            let keep = KeepOnNew { midi: bits & 1 != 0, palettes: bits & 2 != 0, pages: bits & 4 != 0, venue: bits & 8 != 0 };
            let p = new_from(&from, keep);
            assert_eq!(p.midi, if keep.midi { from.midi.clone() } else { empty.midi.clone() }, "{keep:?}");
            assert_eq!(p.palettes, if keep.palettes { from.palettes.clone() } else { Vec::new() }, "{keep:?}");
            assert_eq!(p.pages, if keep.pages { from.pages.clone() } else { empty.pages.clone() }, "{keep:?}");
            assert_eq!(p.grid.multi, if keep.pages { from.grid.multi } else { empty.grid.multi }, "{keep:?}");
            assert_eq!(p.user_cues.len(), if keep.pages { from.user_cues.len() } else { 0 }, "{keep:?}");
            assert_eq!(p.extra.get(crate::sites::PROJECT_KEY), if keep.venue { from.extra.get(crate::sites::PROJECT_KEY) } else { None }, "{keep:?}");
            // Never kept: the show itself, and what isn't the project's business.
            assert!(p.scenes.is_empty() && p.timelines.is_empty() && p.figures.is_empty() && p.lfos.is_empty(), "{keep:?}");
            assert_eq!(p.tempo, TempoDefaults::default());
            assert!(!p.extra.contains_key("calibration"));
        }
    }

    fn shared_with_content(dir: &Path) -> Mutex<Shared> {
        let shared = studio(dir);
        {
            let mut s = shared.lock().unwrap();
            s.scenes.replace_in_memory(vec![scene("Ouverture", 0.5)]);
            s.palettes.replace_in_memory(vec![Palette { name: "Maison".into(), colors: vec![[255, 0, 0], [0, 255, 0]] }]);
            let mut profiles = BTreeMap::new();
            profiles.insert("pad".to_string(), midi("Pad"));
            s.midi.store.replace_user_in_memory(profiles);
            s.shows.save(&Show { name: "Intro".into(), ..Default::default() }).unwrap();
        }
        shared
    }

    #[test]
    fn a_new_project_with_midi_ticked_keeps_the_mappings_and_nothing_else() {
        let dir = temp_dir("new-keep-midi");
        let shared = shared_with_content(&dir);
        let armed_before = shared.lock().unwrap().gate.is_armed();
        let v = post(&shared, &dir, "new", json!({ "keep": { "midi": true } })).unwrap();
        assert_eq!(v["name"], UNTITLED);
        let p = current_project(&shared);
        assert_eq!(p.midi.profiles.keys().collect::<Vec<_>>(), ["pad"]);
        assert!(p.scenes.is_empty() && p.timelines.is_empty() && p.figures.is_empty() && p.palettes.is_empty());
        assert_eq!(shared.lock().unwrap().gate.is_armed(), armed_before, "arming unchanged");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_project_without_a_body_keeps_nothing() {
        let dir = temp_dir("new-empty");
        for body in ["", "{}"] {
            let shared = shared_with_content(&dir);
            let Some(Reply::Json(v)) = route(&shared, true, "new", body) else { panic!("new failed for {body:?}") };
            assert_eq!(v["name"], UNTITLED);
            let p = current_project(&shared);
            assert!(p.midi.profiles.is_empty() && p.palettes.is_empty() && p.scenes.is_empty() && p.timelines.is_empty(), "{body:?}");
        }
        let shared = studio(&dir);
        assert!(matches!(route(&shared, true, "new", "{nope"), Some(Reply::Text(400, _))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_project_never_touches_the_machine_settings_nor_arms() {
        let dir = temp_dir("new-machine");
        let shared = shared_with_content(&dir);
        let before = {
            let mut s = shared.lock().unwrap();
            s.calibration.offset_x = 0.25;
            (s.calibration, serde_json::to_value(s.safety.get()).unwrap(), s.gate.is_armed(), serde_json::to_value(s.venue.get()).unwrap())
        };
        post(&shared, &dir, "new", json!({ "keep": { "midi": true, "palettes": true, "pages": true, "venue": true } })).unwrap();
        post(&shared, &dir, "save-template", json!({ "name": "Base" })).unwrap();
        post(&shared, &dir, "new", json!({ "template": "Base" })).unwrap();
        let s = shared.lock().unwrap();
        assert_eq!((s.calibration, serde_json::to_value(s.safety.get()).unwrap(), s.gate.is_armed(), serde_json::to_value(s.venue.get()).unwrap()), before);
        assert!(!s.gate.is_armed());
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_template_round_trips_into_an_untitled_copy() {
        let dir = temp_dir("template");
        let shared = shared_with_content(&dir);
        post(&shared, &dir, "save-as", json!({ "path": "Concert" })).unwrap();
        let saved = current_project(&shared);
        post(&shared, &dir, "save-template", json!({ "name": "Soirée type" })).unwrap();
        assert!(dir.join("templates/Soirée type.lsproj").is_file());
        assert_eq!(templates_info(&shared)["templates"], json!(["Soirée type"]));
        // Same name again: asked first, then replaced.
        assert_eq!(post(&shared, &dir, "save-template", json!({ "name": "Soirée type" })).unwrap_err().0, 409);
        post(&shared, &dir, "save-template", json!({ "name": "Soirée type", "replace": true })).unwrap();
        // Something else, then a new project from the template.
        post(&shared, &dir, "new", json!({})).unwrap();
        let v = post(&shared, &dir, "new", json!({ "template": "Soirée type", "keep": { "midi": false } })).unwrap();
        assert_eq!(v["name"], UNTITLED, "a copy, never the template itself");
        assert!(v["file"].is_null());
        let p = current_project(&shared);
        assert_eq!(p.fingerprint(), saved.fingerprint(), "same content");
        assert_eq!(post(&shared, &dir, "save", json!({})).unwrap_err().0, 409, "« Enregistrer » asks for a name");
        // Deleting.
        post(&shared, &dir, "delete-template", json!({ "name": "Soirée type" })).unwrap();
        assert_eq!(templates_info(&shared)["templates"], json!([]));
        assert_eq!(post(&shared, &dir, "delete-template", json!({ "name": "Soirée type" })).unwrap_err().0, 404);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_template_names_unknown_and_damaged_templates_change_nothing() {
        let dir = temp_dir("template-bad");
        let shared = shared_with_content(&dir);
        for name in ["../x", "../projects/Concert", "a/b", "", "x\u{0}"] {
            assert_eq!(post(&shared, &dir, "save-template", json!({ "name": name })).unwrap_err().0, 400, "{name:?}");
            assert_eq!(post(&shared, &dir, "new", json!({ "template": name })).unwrap_err().0, 400, "{name:?}");
            assert_eq!(post(&shared, &dir, "delete-template", json!({ "name": name })).unwrap_err().0, 400, "{name:?}");
        }
        assert!(!dir.join("x.lsproj").exists());
        let (code, _) = post(&shared, &dir, "new", json!({ "template": "Inconnu" })).unwrap_err();
        assert_eq!(code, 404);
        std::fs::create_dir_all(dir.join(TEMPLATES_DIR)).unwrap();
        std::fs::write(dir.join("templates/Abîmé.lsproj"), "{ pas du json").unwrap();
        let (code, text) = post(&shared, &dir, "new", json!({ "template": "Abîmé" })).unwrap_err();
        assert_eq!(code, 400);
        assert!(text.contains("modèle « Abîmé »"), "{text}");
        let p = current_project(&shared);
        assert_eq!(p.scenes.len(), 1, "the open project is intact");
        assert_eq!(p.palettes.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
