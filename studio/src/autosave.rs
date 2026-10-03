//! Autosave and crash recovery (T-287).
//!
//! A thread of its own watches the open project: when it has unsaved
//! changes and nothing changed for `delay_s` seconds, a copy is written to
//! `<data-dir>/autosave/<name>-<YYYYMMDD-HHMMSS>.lsproj` (UTC, so the order
//! never jumps with daylight saving time), keeping the `keep` most recent
//! per project. Clean shutdown writes one last copy if needed.
//!
//! It never touches the engine: the `Shared` lock is held only to copy
//! the state out (as for *Enregistrer*), the file is written with the
//! lock released, atomically (`project::write_atomic`). A write that fails
//! (folder not writable, disk full) is reported in the status line and
//! retried after the delay; the studio keeps running.
//!
//! On startup, if the newest autosave of the open project is newer than
//! its file and holds something that is neither saved nor in the working
//! copy, the UI offers *Récupérer* (open it as a modified project) or
//! *Ignorer*. Recovering goes through the same path as opening a project:
//! it never arms the laser and never touches calibration or safety.
//!
//! The settings (`AutosaveSettings`) are app preferences in
//! `<data-dir>/prefs.json`, not part of a project.

use crate::project::{self, UNTITLED};
use crate::Shared;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_DELAY_S: u32 = 30;
pub const MIN_DELAY_S: u32 = 10;
/// With `--test-hooks` (e2e): the delay can go down to 1 s.
pub const TEST_MIN_DELAY_S: u32 = 1;
pub const MAX_DELAY_S: u32 = 300;
pub const DEFAULT_KEEP: u32 = 20;
const MAX_KEEP: u32 = 200;
/// How often the thread looks for changes.
const POLL: Duration = Duration::from_millis(500);
/// Name of the file remembering the autosave the user chose to ignore.
const IGNORED: &str = ".ignored";

/// Autosave preferences (`prefs.json` → `autosave`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutosaveSettings {
    pub enabled: bool,
    pub delay_s: u32,
    pub keep: u32,
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        Self { enabled: true, delay_s: DEFAULT_DELAY_S, keep: DEFAULT_KEEP }
    }
}

impl AutosaveSettings {
    fn sanitize(&mut self, min_delay_s: u32) {
        self.delay_s = self.delay_s.clamp(min_delay_s, MAX_DELAY_S);
        self.keep = self.keep.clamp(1, MAX_KEEP);
    }
}

/// `<data-dir>/prefs.json`: app preferences. Unknown keys are kept.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Prefs {
    autosave: AutosaveSettings,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// An autosave the user can recover, found at startup.
#[derive(Clone, Debug, Serialize)]
pub struct Offer {
    /// The project it belongs to (*Sans titre* for an unsaved one).
    pub project: String,
    /// Its file name in `autosave/`.
    pub file: String,
    /// When it was written (ms since 1970, for the UI's local time).
    pub saved_at_ms: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

/// Autosave settings and status, part of `ProjectState`.
pub struct AutosaveState {
    pub settings: AutosaveSettings,
    prefs_path: PathBuf,
    /// `<data-dir>/autosave`.
    pub dir: PathBuf,
    min_delay_s: u32,
    /// Last successful autosave (ms since 1970).
    pub last_ok_ms: Option<u64>,
    /// Why the last autosave failed; cleared by the next success.
    pub error: Option<String>,
    /// Pending recovery offer. No autosave is written while it is pending, so a
    /// newer copy can't bury the work to recover.
    pub offer: Option<Offer>,
}

impl AutosaveState {
    pub fn load(data_dir: &Path) -> Self {
        let prefs_path = data_dir.join("prefs.json");
        let prefs: Prefs = crate::load_json(&prefs_path);
        let mut settings = prefs.autosave;
        settings.sanitize(MIN_DELAY_S);
        Self { settings, prefs_path, dir: data_dir.join("autosave"), min_delay_s: MIN_DELAY_S, last_ok_ms: None, error: None, offer: None }
    }

    /// `--test-hooks`: delays down to 1 s (e2e tests). Re-reads the
    /// preferences so a 1 s delay seeded by a test is kept.
    pub fn allow_test_delays(&mut self) {
        self.min_delay_s = TEST_MIN_DELAY_S;
        let prefs: Prefs = crate::load_json(&self.prefs_path);
        self.settings.delay_s = prefs.autosave.delay_s;
        self.settings.sanitize(self.min_delay_s);
    }

    pub fn status(&self) -> Value {
        json!({
            "enabled": self.settings.enabled,
            "delay_s": self.settings.delay_s,
            "keep": self.settings.keep,
            "min_delay_s": self.min_delay_s,
            "max_delay_s": MAX_DELAY_S,
            "last_ok_ms": self.last_ok_ms,
            "error": self.error,
            "offer": self.offer,
            "dir": self.dir,
        })
    }
}

/// `POST /api/project/autosave` body: any of the settings.
#[derive(Deserialize)]
pub struct SettingsRequest {
    enabled: Option<bool>,
    delay_s: Option<u32>,
    keep: Option<u32>,
}

/// Changes the settings and writes `prefs.json` (lock released for the write).
pub fn set_settings(shared: &Mutex<Shared>, req: SettingsRequest) -> Result<()> {
    let (settings, path) = {
        let mut s = shared.lock().unwrap();
        let a = &mut s.project.autosave;
        if let Some(on) = req.enabled {
            a.settings.enabled = on;
        }
        if let Some(d) = req.delay_s {
            a.settings.delay_s = d;
        }
        if let Some(k) = req.keep {
            a.settings.keep = k;
        }
        let min = a.min_delay_s;
        a.settings.sanitize(min);
        (a.settings.clone(), a.prefs_path.clone())
    };
    let mut prefs: Prefs = crate::load_json(&path);
    prefs.autosave = settings;
    project::write_atomic(&path, &serde_json::to_vec_pretty(&prefs)?)
}

/// "Wait until nothing changed for `delay` seconds", on a clock the caller
/// gives (seconds), so tests can simulate time.
#[derive(Default)]
pub struct Debounce {
    /// Project name and fingerprint last seen, and since when.
    seen: Option<(String, u64)>,
    changed_at: f64,
    /// What is already on disk (an autosave, or the saved project).
    written: Option<(String, u64)>,
}

impl Debounce {
    /// True when the state `(name, fp)` should be autosaved now. `saved`
    /// is the fingerprint of the saved project. The first state seen is
    /// the baseline: it is on disk already (working copy or project).
    pub fn due(&mut self, name: &str, fp: u64, saved: u64, now: f64, delay: f64) -> bool {
        let key = (name.to_string(), fp);
        if self.seen.is_none() {
            self.written = Some(key.clone());
        }
        if self.seen.as_ref() != Some(&key) {
            self.seen = Some(key.clone());
            self.changed_at = now;
        }
        if fp == saved {
            self.written = Some(key);
            return false;
        }
        self.written.as_ref() != Some(&key) && now - self.changed_at >= delay
    }

    /// The autosave of `(name, fp)` is on disk.
    pub fn done(&mut self, name: &str, fp: u64) {
        self.written = Some((name.to_string(), fp));
    }

    /// The write failed: try again after the delay.
    pub fn failed(&mut self, now: f64) {
        self.changed_at = now;
    }
}

/// Starts the autosave thread. It stops (after a last autosave) when
/// `running` goes false.
pub fn spawn(shared: Arc<Mutex<Shared>>, running: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("autosave".into())
        .spawn(move || {
            let epoch = Instant::now();
            let mut deb = Debounce::default();
            let mut next = Instant::now();
            while running.load(Ordering::SeqCst) {
                if Instant::now() >= next {
                    tick(&shared, &mut deb, epoch.elapsed().as_secs_f64(), false);
                    next = Instant::now() + POLL;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // Clean shutdown: whatever is unsaved goes to disk now.
            tick(&shared, &mut deb, epoch.elapsed().as_secs_f64(), true);
        })
        .expect("failed to start the autosave thread")
}

/// One look at the project: writes an autosave if one is due (`flush`:
/// without waiting for the delay). Returns the file written, if any.
pub fn tick(shared: &Mutex<Shared>, deb: &mut Debounce, now: f64, flush: bool) -> Option<PathBuf> {
    let (mut p, name, saved, shows, dir, settings, write) = {
        let s = shared.lock().unwrap_or_else(|e| e.into_inner());
        let a = &s.project.autosave;
        // Disabled, or a recovery offer pending: changes are still
        // watched (so the delay and the baseline stay right), not written.
        let write = a.settings.enabled && a.offer.is_none();
        let mut p = project::snapshot(&s);
        p.extra = s.project.extra().clone();
        let name = s.project.current.clone().unwrap_or_else(|| UNTITLED.into());
        let shows = crate::timeline::ShowStore::new(s.shows.dir().to_path_buf());
        (p, name, s.project.saved_fingerprint(), shows, a.dir.clone(), a.settings.clone(), write)
    };
    // Lock released: disk and serialisation from here on.
    p.timelines = shows.load_all();
    let fp = p.fingerprint();
    let delay = if flush { 0.0 } else { f64::from(settings.delay_s) };
    if !deb.due(&name, fp, saved, now, delay) || !write {
        return None;
    }
    p.name = name.clone();
    let now_ms = now_ms();
    let path = dir.join(format!("{name}-{}.{}", stamp(now_ms / 1000), project::EXTENSION));
    let result = project::write_project_stamped(&path, &mut p).and_then(|()| prune(&dir, &name, settings.keep as usize));
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    let a = &mut s.project.autosave;
    match result {
        Ok(()) => {
            deb.done(&name, fp);
            a.last_ok_ms = Some(now_ms);
            a.error = None;
            Some(path)
        }
        Err(e) => {
            log::warn!("autosave failed: {e:#}");
            deb.failed(now);
            a.error = Some(format!("{e:#}"));
            None
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `YYYYMMDD-HHMMSS`, UTC.
pub fn stamp(secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = project::civil(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// Splits `<name>-<YYYYMMDD-HHMMSS>` into name and stamp.
fn split_stamp(stem: &str) -> Option<(&str, &str)> {
    let cut = stem.len().checked_sub(16)?;
    let (name, rest) = (stem.get(..cut)?, stem.get(cut..)?);
    let stamp = rest.strip_prefix('-')?;
    let b = stamp.as_bytes();
    let ok = b.len() == 15 && b[8] == b'-' && b.iter().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit());
    (ok && !name.is_empty()).then_some((name, stamp))
}

/// The autosaves of project `name` in `dir`, oldest first.
pub fn ring(dir: &Path, name: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == project::EXTENSION) && p.is_file())
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?;
            if stem.starts_with('.') {
                return None;
            }
            let (n, stamp) = split_stamp(stem)?;
            (n == name).then(|| (stamp.to_string(), p.clone()))
        })
        .collect();
    files.sort();
    files.into_iter().map(|(_, p)| p).collect()
}

/// Deletes the oldest autosaves of `name` beyond `keep`.
fn prune(dir: &Path, name: &str, keep: usize) -> Result<()> {
    let files = ring(dir, name);
    let extra = files.len().saturating_sub(keep);
    for old in &files[..extra] {
        std::fs::remove_file(old).with_context(|| format!("impossible de supprimer {}", old.display()))?;
    }
    Ok(())
}

fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Startup (called by `project::startup`, before the engine runs): is
/// there unsaved work to offer? Only the newest autosave of the open
/// project counts, and only if it is newer than the project file, was
/// not ignored before, and differs from both the saved project and the
/// working copy (otherwise there is nothing to recover).
pub fn find_offer(s: &mut Shared) {
    let dir = s.project.autosave.dir.clone();
    let name = s.project.current.clone().unwrap_or_else(|| UNTITLED.into());
    let Some(newest) = ring(&dir, &name).pop() else { return };
    let file = newest.file_name().and_then(|f| f.to_str()).unwrap_or_default().to_string();
    if std::fs::read_to_string(dir.join(IGNORED)).is_ok_and(|ignored| ignored.trim() == file) {
        return;
    }
    let Some(written) = mtime(&newest) else { return };
    if let Some(current) = &s.project.current {
        if mtime(&s.project.path_of(current)).is_some_and(|saved| saved >= written) {
            return;
        }
    }
    let p = match project::read(&newest) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("autosave {}: {e:#}", newest.display());
            return;
        }
    };
    let fp = p.fingerprint();
    let mut working = project::snapshot(s);
    working.timelines = s.shows.load_all();
    if fp == s.project.saved_fingerprint() || fp == working.fingerprint() {
        return;
    }
    let saved_at_ms = written.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    println!("Sauvegarde auto plus récente que le projet « {name} » : {}", newest.display());
    s.project.autosave.offer = Some(Offer { project: name, file, saved_at_ms, path: newest });
}

/// *Ignorer*: forget the offer, and don't offer that autosave again.
pub fn ignore(shared: &Mutex<Shared>) {
    let (offer, dir) = {
        let mut s = shared.lock().unwrap();
        (s.project.autosave.offer.take(), s.project.autosave.dir.clone())
    };
    if let Some(offer) = offer {
        if let Err(e) = project::write_atomic(&dir.join(IGNORED), offer.file.as_bytes()) {
            log::warn!("autosave: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenes::Scene;
    use crate::test_support;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-autosave-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A studio whose stores all live in `dir`, like `main` builds it.
    fn studio(dir: &Path) -> Mutex<Shared> {
        let mut s = test_support::shared();
        s.scenes = crate::scenes::SceneStore::load_or_create(dir.join("scenes.json"));
        s.deck = crate::cues::CueDeck::load(dir.join("grid.json"));
        s.shows = crate::timeline::ShowStore::new(dir.join("shows"));
        s.lfos = crate::lfo::LfoStore::load_or_create(dir.join("lfos.json"), &s.controls);
        s.routes = crate::audio::routes::RouteStore::load_or_create(dir.join("audio_routes.json"), &s.controls);
        s.palettes = crate::live::PaletteStore::load_or_create(dir.join("palettes.json"));
        s.midi.store = crate::midi::profile::ProfileStore::load(dir.join("midi"));
        s.figures = crate::figures::FigureStore::load(dir.join("figures"));
        s.project = project::ProjectState::load(dir);
        project::startup(&mut s);
        Mutex::new(s)
    }

    fn post(shared: &Mutex<Shared>, action: &str, body: Value) -> Result<Value, (u16, String)> {
        match project::route(shared, true, action, &body.to_string()).expect("known route") {
            project::Reply::Json(v) => Ok(v),
            project::Reply::Text(code, t) => Err((code, t)),
        }
    }

    fn set_bpm(shared: &Mutex<Shared>, bpm: f64) {
        let mut s = shared.lock().unwrap();
        let t = s.now_s();
        s.tempo.set_bpm_manual(bpm, t);
    }

    fn scene(name: &str) -> Scene {
        Scene { name: name.into(), settings: Default::default(), duration_secs: 4.0 }
    }

    #[test]
    fn debounce_waits_for_quiet_on_a_simulated_clock() {
        let mut d = Debounce::default();
        // Baseline: what is there at start is on disk already.
        assert!(!d.due("p", 1, 1, 0.0, 30.0));
        assert!(!d.due("p", 2, 1, 5.0, 30.0), "just changed");
        assert!(!d.due("p", 2, 1, 34.9, 30.0));
        // Changed again: the 30 s start over.
        assert!(!d.due("p", 3, 1, 20.0, 30.0));
        assert!(!d.due("p", 3, 1, 49.0, 30.0));
        assert!(d.due("p", 3, 1, 50.0, 30.0));
        d.done("p", 3);
        assert!(!d.due("p", 3, 1, 200.0, 30.0), "already written");
        // A failed write is retried after the delay.
        assert!(d.due("p", 4, 1, 300.0, 0.0));
        d.failed(300.0);
        assert!(!d.due("p", 4, 1, 310.0, 30.0));
        assert!(d.due("p", 4, 1, 330.0, 30.0));
        // Back to the saved state: nothing to autosave.
        assert!(!d.due("p", 1, 1, 1000.0, 30.0));
        // Same content under another project name is new.
        assert!(!d.due("q", 4, 1, 1000.0, 30.0));
        assert!(d.due("q", 4, 1, 1030.0, 30.0));
    }

    #[test]
    fn stamps_sort_and_split() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_727_000_000), "20240922-101320");
        assert_eq!(split_stamp("Mon show-20240922-101320"), Some(("Mon show", "20240922-101320")));
        assert_eq!(split_stamp("a-b-20240922-101320"), Some(("a-b", "20240922-101320")));
        assert_eq!(split_stamp("Mon show"), None);
        assert_eq!(split_stamp("-20240922-101320"), None);
        assert_eq!(split_stamp("x-2024092a-101320"), None);
        assert_eq!(split_stamp("é-20240922-101320"), Some(("é", "20240922-101320")));
    }

    #[test]
    fn the_ring_keeps_the_most_recent_per_project() {
        let dir = temp_dir("ring");
        for i in 0..25u64 {
            std::fs::write(dir.join(format!("A-{}.lsproj", stamp(1_000_000 + i))), "{}").unwrap();
        }
        std::fs::write(dir.join(format!("A b-{}.lsproj", stamp(5))), "{}").unwrap();
        std::fs::write(dir.join(".A-20240101-000000.lsproj.tmp"), "").unwrap();
        prune(&dir, "A", 20).unwrap();
        let left = ring(&dir, "A");
        assert_eq!(left.len(), 20);
        assert!(left[0].ends_with(format!("A-{}.lsproj", stamp(1_000_005))));
        assert!(left[19].ends_with(format!("A-{}.lsproj", stamp(1_000_024))));
        assert_eq!(ring(&dir, "A b").len(), 1, "other projects are untouched");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_change_is_autosaved_after_the_delay_and_never_more_than_keep() {
        let dir = temp_dir("tick");
        let shared = studio(&dir);
        shared.lock().unwrap().project.autosave.settings.keep = 3;
        let mut d = Debounce::default();
        assert!(tick(&shared, &mut d, 0.0, false).is_none(), "baseline");
        for i in 0..6 {
            let t0 = 100.0 * f64::from(i);
            set_bpm(&shared, 100.0 + f64::from(i));
            assert!(tick(&shared, &mut d, t0 + 1.0, false).is_none());
            assert!(tick(&shared, &mut d, t0 + 30.0, false).is_none(), "the delay counts from the change");
            let path = tick(&shared, &mut d, t0 + 31.0, false).expect("autosaved after 30 s of quiet");
            let p = project::read(&path).unwrap();
            assert_eq!(p.tempo.bpm, 100.0 + f64::from(i));
            assert_eq!(p.name, UNTITLED);
            // Same second as the previous one: same file, so wait a bit.
            std::thread::sleep(Duration::from_millis(if i < 5 { 1001 } else { 0 }));
        }
        assert!(ring(&dir.join("autosave"), UNTITLED).len() <= 3);
        assert!(shared.lock().unwrap().project.autosave.last_ok_ms.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disabled_or_saved_means_no_autosave() {
        let dir = temp_dir("off");
        let shared = studio(&dir);
        let mut d = Debounce::default();
        tick(&shared, &mut d, 0.0, false);
        set_bpm(&shared, 140.0);
        shared.lock().unwrap().project.autosave.settings.enabled = false;
        assert!(tick(&shared, &mut d, 100.0, true).is_none());
        shared.lock().unwrap().project.autosave.settings.enabled = true;
        assert!(tick(&shared, &mut d, 100.0, true).is_some(), "changes made while off are written once back on");
        post(&shared, "save-as", json!({ "path": "Gardé" })).unwrap();
        assert!(tick(&shared, &mut d, 200.0, true).is_none(), "saved: nothing to autosave");
        assert!(ring(&dir.join("autosave"), "Gardé").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unwritable_autosave_folder_is_reported_not_fatal() {
        let dir = temp_dir("unwritable");
        // A file where the folder should be: every write fails.
        std::fs::write(dir.join("autosave"), "pas un dossier").unwrap();
        let shared = studio(&dir);
        let mut d = Debounce::default();
        tick(&shared, &mut d, 0.0, false);
        set_bpm(&shared, 150.0);
        assert!(tick(&shared, &mut d, 100.0, true).is_none());
        let s = shared.lock().unwrap();
        assert!(s.project.autosave.error.is_some());
        assert!(!s.gate.is_armed());
        drop(s);
        // Still working: the project API answers, the status says why.
        let v = post(&shared, "autosave", json!({ "delay_s": 60 })).unwrap();
        assert!(v["autosave"]["error"].is_string());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_are_clamped_and_kept_in_prefs() {
        let dir = temp_dir("prefs");
        std::fs::write(dir.join("prefs.json"), r#"{"autosave":{"delay_s":1},"other":7}"#).unwrap();
        let shared = studio(&dir);
        assert_eq!(shared.lock().unwrap().project.autosave.settings.delay_s, MIN_DELAY_S);
        post(&shared, "autosave", json!({ "enabled": false, "delay_s": 9999 })).unwrap();
        let prefs: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("prefs.json")).unwrap()).unwrap();
        assert_eq!(prefs["autosave"]["delay_s"], MAX_DELAY_S);
        assert_eq!(prefs["autosave"]["enabled"], false);
        assert_eq!(prefs["other"], 7, "unknown preferences are kept");
        // --test-hooks: 1 s is allowed.
        std::fs::write(dir.join("prefs.json"), r#"{"autosave":{"delay_s":1}}"#).unwrap();
        let mut a = AutosaveState::load(&dir);
        a.allow_test_delays();
        assert_eq!(a.settings.delay_s, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_crash_offers_the_newest_autosave_and_recovering_restores_it_disarmed() {
        let dir = temp_dir("recover");
        let shared = studio(&dir);
        post(&shared, "save-as", json!({ "path": "Show" })).unwrap();
        let mut d = Debounce::default();
        tick(&shared, &mut d, 0.0, false);
        set_bpm(&shared, 133.0);
        let first = tick(&shared, &mut d, 100.0, true).unwrap();
        std::thread::sleep(Duration::from_millis(1001));
        set_bpm(&shared, 137.0);
        let newest = tick(&shared, &mut d, 200.0, true).unwrap();
        assert_ne!(first, newest);
        // « Crash »: drop the state without saving, start again.
        drop(shared);
        let shared = studio(&dir);
        let offer = shared.lock().unwrap().project.autosave.offer.clone().expect("recovery offered");
        assert_eq!(offer.project, "Show");
        assert_eq!(offer.path, newest);
        // Autosave pauses while the offer is pending.
        let mut d = Debounce::default();
        set_bpm(&shared, 90.0);
        assert!(tick(&shared, &mut d, 0.0, true).is_none());
        let v = post(&shared, "recover", json!({})).unwrap();
        assert_eq!(v["name"], "Show");
        assert_eq!(v["modified"], true, "recovered work is unsaved");
        assert!(v["autosave"]["offer"].is_null());
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.tempo.bpm, 137.0);
            assert!(!s.gate.is_armed(), "recovery never arms");
        }
        // Changed right after recovering: autosaved as usual.
        set_bpm(&shared, 141.0);
        let path = tick(&shared, &mut d, 100.0, true).expect("autosave resumes after recovery");
        assert_eq!(project::read(&path).unwrap().tempo.bpm, 141.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_offer_when_saved_ignored_or_already_in_the_working_copy() {
        let dir = temp_dir("nooffer");
        let shared = studio(&dir);
        post(&shared, "save-as", json!({ "path": "Show" })).unwrap();
        let mut d = Debounce::default();
        tick(&shared, &mut d, 0.0, false);
        // A scene is in the working copy (scenes.json) as soon as it is made.
        shared.lock().unwrap().scenes.replace_in_memory(vec![scene("Une")]);
        crate::save_json(&dir.join("scenes.json"), &vec![scene("Une")]);
        tick(&shared, &mut d, 100.0, true).unwrap();
        drop(shared);
        let shared = studio(&dir);
        assert!(shared.lock().unwrap().project.autosave.offer.is_none(), "nothing lost: no question");

        // Unsaved tempo (not in the working copy): offered, then ignored for good.
        let mut d = Debounce::default();
        tick(&shared, &mut d, 0.0, false);
        set_bpm(&shared, 160.0);
        std::thread::sleep(Duration::from_millis(1001));
        tick(&shared, &mut d, 100.0, true).unwrap();
        drop(shared);
        let shared = studio(&dir);
        assert!(shared.lock().unwrap().project.autosave.offer.is_some());
        let v = post(&shared, "recover-ignore", json!({})).unwrap();
        assert!(v["autosave"]["offer"].is_null());
        assert_eq!(shared.lock().unwrap().tempo.bpm, 120.0, "ignored: the working copy stays");
        drop(shared);
        let shared = studio(&dir);
        assert!(shared.lock().unwrap().project.autosave.offer.is_none(), "not offered twice");

        // Saved after the autosave: nothing to offer.
        let mut d = Debounce::default();
        tick(&shared, &mut d, 0.0, false);
        set_bpm(&shared, 170.0);
        std::thread::sleep(Duration::from_millis(1001));
        tick(&shared, &mut d, 100.0, true).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        post(&shared, "save", json!({})).unwrap();
        drop(shared);
        let shared = studio(&dir);
        assert!(shared.lock().unwrap().project.autosave.offer.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_newest_autosave_is_found_by_its_stamp() {
        let dir = temp_dir("newest");
        for secs in [300u64, 100, 200] {
            std::fs::write(dir.join(format!("P-{}.lsproj", stamp(secs))), "{}").unwrap();
        }
        std::fs::write(dir.join(format!("P2-{}.lsproj", stamp(999))), "{}").unwrap();
        assert!(ring(&dir, "P").last().unwrap().ends_with(format!("P-{}.lsproj", stamp(300))));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_autosave_never_makes_the_engine_wait_a_frame() {
        let dir = temp_dir("engine");
        let shared = Arc::new(studio(&dir));
        let mut d = Debounce::default();
        tick(&shared, &mut d, 0.0, false);
        shared.lock().unwrap().scenes.replace_in_memory((0..500).map(|i| scene(&format!("Scène {i}"))).collect());
        // A stand-in for the engine: takes the lock in a tight loop and
        // records the longest wait.
        let stop = Arc::new(AtomicBool::new(false));
        let engine = std::thread::spawn({
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            move || {
                let mut worst = Duration::ZERO;
                while !stop.load(Ordering::SeqCst) {
                    let t = Instant::now();
                    drop(shared.lock().unwrap());
                    worst = worst.max(t.elapsed());
                    std::thread::sleep(Duration::from_millis(1));
                }
                worst
            }
        });
        for i in 0..3 {
            set_bpm(&shared, 100.0 + f64::from(i));
            assert!(tick(&shared, &mut d, 100.0 * f64::from(i + 1), true).is_some());
        }
        stop.store(true, Ordering::SeqCst);
        let worst = engine.join().unwrap();
        assert!(worst < Duration::from_micros(16_667), "the engine waited {worst:?} for the lock");
        let _ = std::fs::remove_dir_all(dir);
    }
}
