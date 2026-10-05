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
use crate::cuelist::CueList;
use crate::cues::CueDeck;
use crate::figures::Figure;
use crate::grid::CueGrid;
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

    /// Unknown fields of the open project.
    pub fn extra(&self) -> &Map<String, Value> {
        &self.extra
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
const WORKING_FILES: [&str; 12] = ["scenes.json", "grid.json", "pages.json", "live.json", "layers.json", "lfos.json", "audio_routes.json", "palettes.json", "shows", "figures", "user_cues.json", "cuelists.json"];

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
    s.lfos.replace_in_memory(p.lfos.clone());
    s.routes.replace_in_memory(p.audio_routes.clone(), &s.controls);
    s.palettes.replace_in_memory(p.palettes.clone());
    s.scan_presets.replace_in_memory(p.scan_presets.clone());
    s.midi.store.replace_user_in_memory(p.midi.profiles.clone());
    s.figures.replace_in_memory(p.figures.clone());
    s.user_cues.replace_in_memory(p.user_cues.clone());
    s.cue_lists.replace_in_memory(p.cue_lists.clone());
    // The loaded list follows its new version, or goes (another project).
    match s.cue_list.list.as_ref().and_then(|l| s.cue_lists.get(&l.name)).cloned() {
        Some(l) => {
            s.cue_list.top();
            s.cue_list.refresh(&l);
        }
        None => s.cue_list.unload(),
    }
    crate::usercues::sync_presets(s);
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
    live: PathBuf,
    layers: PathBuf,
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
    if let Some(pages) = &w.pages {
        put(pages, serde_json::to_vec_pretty(&p.pages));
    }
    if let Some((path, bytes)) = &w.user_cues {
        put(path, Ok(bytes.clone()));
    }
    if let Some(path) = &w.cue_lists {
        put(path, serde_json::to_vec_pretty(&p.cue_lists));
    }
    put(&w.lfos, serde_json::to_vec_pretty(&p.lfos));
    put(&w.routes, serde_json::to_vec_pretty(&p.audio_routes));
    put(&w.palettes, serde_json::to_vec_pretty(&p.palettes));
    put(&w.scan_presets, serde_json::to_vec_pretty(&p.scan_presets));
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
        _ => return None,
    };
    Some(match reply {
        Ok(done) => {
            let mut v = info(shared);
            v["warnings"] = json!(done.warnings);
            v["notice"] = json!(done.notice);
            v["suggested_name"] = json!(done.suggested_name);
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
        working_paths(&s, &data_dir)
    };
    let warnings = persist(&w, p);
    for e in &warnings {
        log::warn!("project open: failed to write {e}");
    }
    Ok(warnings.into_iter().map(|e| format!("impossible d'écrire {e}")).collect())
}

/// Opens `path`, or a new empty project with `None`. Atomic: nothing
/// changes unless the whole file is valid. An older file is migrated (its
/// original kept, T-288); a newer one opens read only.
fn open_project(shared: &Mutex<Shared>, path: Option<&Path>) -> Result<Done, Reply> {
    let loaded = match path {
        Some(path) => Some(load_file(path).map_err(|e| {
            let code = if path.exists() { 400 } else { 404 };
            Reply::Text(code, format!("{e:#}"))
        })?),
        None => None,
    };
    let (mut p, migrated, newer) = match &loaded {
        Some((l, _)) => (l.project.clone(), l.migrated(), l.newer()),
        None => (Project::default(), None, None),
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
    Ok(Done { warnings, notice, suggested_name: None })
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
}
