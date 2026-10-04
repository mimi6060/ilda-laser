//! One loading policy for every file of the data directory (T-402).
//!
//! A missing file gives the defaults, silently. A file that can't be read
//! or decoded is **moved** to `<data-dir>/quarantine/<name>-<YYYYMMDD-HHMMSS>.<ext>`
//! (renamed, else copied then deleted), the store starts from its defaults
//! and a `DataWarning` is recorded for the UI. Nothing is ever written over
//! a file that could not be read: once moved, its place is free; if it
//! could not be moved (read-only folder), the path is *frozen* and
//! `write_atomic` keeps that store in memory only for the rest of the run.
//!
//! Warnings of safety files (safety, calibration, power caps, presence,
//! MIDI safety) close the `data_ack` interlock until the operator has
//! acknowledged them (`Shared::sync_data_warnings`). The quarantine folder
//! is never emptied by the studio, and a damaged file's content is never
//! shown nor logged (it may hold personal paths).

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Folder of the data directory where damaged files are moved.
pub const QUARANTINE: &str = "quarantine";

/// Result of `load`.
pub enum Loaded<T> {
    /// No file: the defaults, no warning.
    Missing,
    Ok(T),
    /// Unreadable: moved aside (or frozen), the caller uses its defaults.
    Quarantined(DataWarning),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DataWarning {
    /// Relative to the data directory, e.g. `scenes.json`, `midi/profiles/x.json`.
    pub file: String,
    /// Where the file went, relative to the data directory
    /// (`quarantine/scenes-20261003-141502.json`); `None` if it could not
    /// be moved (the store then never writes it).
    pub moved_to: Option<String>,
    /// The serde or I/O message (never the file's content).
    pub error: String,
    /// UTC, `YYYY-MM-DDTHH:MM:SSZ` like `Project::saved_at`.
    pub at: String,
    /// A safety file: blocks arming until acknowledged.
    pub safety: bool,
    pub acknowledged: bool,
}

impl DataWarning {
    /// The French one-liner of the banner.
    pub fn message_fr(&self) -> String {
        let place = match &self.moved_to {
            Some(to) => format!("mis de côté dans {to}"),
            None => "impossible de le mettre de côté : il n'est plus enregistré jusqu'au prochain démarrage".into(),
        };
        format!("Fichier illisible : {} — {place}. Valeurs par défaut utilisées.", self.file)
    }
}

/// Warnings not yet collected by `Shared`, with the quarantine folder
/// they belong to (several studios run side by side in the tests).
static PENDING: Mutex<Vec<(PathBuf, DataWarning)>> = Mutex::new(Vec::new());
/// Paths whose damaged file could not be moved: never written this run.
static FROZEN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `<data-dir>/quarantine` for a file directly in the data directory.
pub fn beside(path: &Path) -> PathBuf {
    path.parent().unwrap_or(Path::new(".")).join(QUARANTINE)
}

/// Reads and decodes `path` (see the module docs).
pub fn load<T: DeserializeOwned>(path: &Path, quarantine: &Path, safety: bool) -> Loaded<T> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Quarantined(set_aside(path, quarantine, &format!("lecture impossible : {e}"), safety)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => Loaded::Ok(v),
        Err(e) => Loaded::Quarantined(set_aside(path, quarantine, &format!("JSON invalide : {e}"), safety)),
    }
}

/// `load`, the defaults when missing or damaged.
pub fn load_or_default<T: DeserializeOwned + Default>(path: &Path, quarantine: &Path, safety: bool) -> T {
    match load(path, quarantine, safety) {
        Loaded::Ok(v) => v,
        Loaded::Missing | Loaded::Quarantined(_) => T::default(),
    }
}

/// Moves a file the caller could not use (bad JSON, or valid JSON that
/// fails its checks) to the quarantine folder, records the warning and
/// returns it. If it can't be moved, the path is frozen instead.
pub fn set_aside(path: &Path, quarantine: &Path, error: &str, safety: bool) -> DataWarning {
    let root = quarantine.parent().unwrap_or(Path::new("."));
    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    let file = rel(path);
    let at = crate::project::utc_now();
    let moved = move_aside(path, quarantine, &file);
    let moved_to = match &moved {
        Ok(to) => Some(rel(to)),
        Err(e) => {
            log::warn!("damaged {} could not be moved aside ({e:#}): in memory only until restart", path.display());
            lock(&FROZEN).push(path.to_path_buf());
            None
        }
    };
    if let Some(to) = &moved_to {
        log::warn!("damaged {file} ({error}): moved to {to}, defaults used");
    }
    let warning = DataWarning { file, moved_to, error: error.to_string(), at, safety, acknowledged: false };
    lock(&PENDING).push((quarantine.to_path_buf(), warning.clone()));
    warning
}

/// `scenes.json` → `scenes-20261003-141502.json`, `midi/profiles/a.json`
/// → `midi-profiles-a-20261003-141502.json`, then `-2`, `-3`... if taken.
fn quarantine_name(rel: &str, stamp: &str, n: u32) -> String {
    let flat = rel.replace('/', "-");
    let (stem, ext) = match flat.rfind('.') {
        Some(i) if i > 0 => (&flat[..i], &flat[i..]),
        _ => (flat.as_str(), ""),
    };
    if n <= 1 {
        format!("{stem}-{stamp}{ext}")
    } else {
        format!("{stem}-{stamp}-{n}{ext}")
    }
}

fn move_aside(path: &Path, quarantine: &Path, rel: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(quarantine).with_context(|| format!("impossible de créer {}", quarantine.display()))?;
    let stamp = file_stamp();
    let target = (1..)
        .map(|n| quarantine.join(quarantine_name(rel, &stamp, n)))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .expect("some name is free");
    if std::fs::rename(path, &target).is_ok() {
        return Ok(target);
    }
    std::fs::copy(path, &target).with_context(|| format!("impossible de copier vers {}", target.display()))?;
    if let Err(e) = std::fs::remove_file(path) {
        // The copy is safe; the original may be written over later.
        log::warn!("damaged {} copied to {} but not removed: {e}", path.display(), target.display());
    }
    Ok(target)
}

/// `YYYYMMDD-HHMMSS`, UTC.
fn file_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d, hh, mm, ss) = crate::project::civil(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// Warnings recorded for `quarantine` since the last call (oldest first).
pub fn take_pending(quarantine: &Path) -> Vec<DataWarning> {
    let mut pending = lock(&PENDING);
    let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *pending).into_iter().partition(|(q, _)| q == quarantine);
    *pending = others;
    mine.into_iter().map(|(_, w)| w).collect()
}

/// Whether `path` held a damaged file that could not be moved.
pub fn is_frozen(path: &Path) -> bool {
    lock(&FROZEN).iter().any(|p| p == path)
}

/// Atomic write: a hidden `.tmp` file in the same folder, flushed to disk,
/// then renamed over the target. A crash leaves either the old file or
/// the new one, never half of it. A frozen path is not written at all
/// (the store stays in memory only): the damaged file is kept.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if is_frozen(path) {
        log::warn!("{} not saved: its damaged file could not be moved aside", path.display());
        return Ok(());
    }
    let tmp = write_tmp(path, bytes)?;
    commit(&tmp, path)
}

/// Like `write_atomic` (temporary file then rename, so a cut write never
/// leaves half a file; frozen paths skipped) but without waiting for the
/// disk: for the small settings files saved while the engine lock is held
/// or from the engine thread, where an fsync could stall a frame.
pub fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    if is_frozen(path) {
        log::warn!("{} not saved: its damaged file could not be moved aside", path.display());
        return Ok(());
    }
    let dir = path.parent().context("chemin sans dossier")?;
    std::fs::create_dir_all(dir).with_context(|| format!("impossible de créer {}", dir.display()))?;
    let tmp = tmp_path(path);
    std::fs::write(&tmp, bytes).with_context(|| format!("impossible d'écrire {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("impossible d'écrire {}", path.display()))
}

fn tmp_path(path: &Path) -> PathBuf {
    let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("fichier");
    path.with_file_name(format!(".{file}.tmp"))
}

pub(crate) fn write_tmp(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let dir = path.parent().context("chemin sans dossier")?;
    std::fs::create_dir_all(dir).with_context(|| format!("impossible de créer {}", dir.display()))?;
    let tmp = tmp_path(path);
    let mut f = std::fs::File::create(&tmp).with_context(|| format!("impossible d'écrire {}", tmp.display()))?;
    f.write_all(bytes).and_then(|()| f.sync_all()).with_context(|| format!("impossible d'écrire {}", tmp.display()))?;
    Ok(tmp)
}

pub(crate) fn commit(tmp: &Path, path: &Path) -> Result<()> {
    std::fs::rename(tmp, path).with_context(|| format!("impossible d'écrire {}", path.display()))?;
    // The rename itself reaches the disk with the folder.
    if let Some(dir) = path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Pretty JSON through `replace`, errors logged (stores whose saves
/// are fire-and-forget: live, layers, grid, presence...).
pub fn save_json<T: Serialize>(path: &Path, value: &T) {
    match serde_json::to_vec_pretty(value) {
        Ok(json) => {
            let result = replace(path, &json);
            if let Err(e) = &result {
                log::warn!("failed to save {}: {e:#}", path.display());
            }
            // The notification centre (T-432): never waits.
            crate::notices::report_write(path, result.map_err(|e| format!("{e:#}")));
        }
        Err(e) => log::warn!("failed to serialize {}: {e}", path.display()),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-datafile-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The single file in `dir/quarantine` (panics if not exactly one).
    pub fn quarantined(dir: &Path) -> PathBuf {
        let files: Vec<PathBuf> = std::fs::read_dir(dir.join(QUARANTINE)).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(files.len(), 1, "{files:?}");
        files[0].clone()
    }

    /// Bytes of a truncated file, as left by a write cut short.
    pub const DAMAGED: &[u8] = b"[{\"name\": \"coup";

    /// A fresh data directory with a truncated `rel`; returns (dir, path).
    pub fn with_damaged(tag: &str, rel: &str) -> (PathBuf, PathBuf) {
        let dir = temp_dir(tag);
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, DAMAGED).unwrap();
        (dir, path)
    }

    /// After a store loaded the damaged `rel` and saved: the original bytes
    /// are intact in the quarantine folder, and one warning names the file.
    pub fn check_kept(dir: &Path, rel: &str, safety: bool) {
        assert_eq!(std::fs::read(quarantined(dir)).unwrap(), DAMAGED, "{rel}: quarantined copy intact");
        let w = take_pending(&dir.join(QUARANTINE));
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!((w[0].file.as_str(), w[0].safety, w[0].acknowledged), (rel, safety, false));
        assert!(w[0].moved_to.as_deref().is_some_and(|m| m.starts_with("quarantine/")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_gives_defaults_without_warning_nor_quarantine() {
        let dir = temp_dir("missing");
        let q = dir.join(QUARANTINE);
        assert!(matches!(load::<Vec<u32>>(&dir.join("a.json"), &q, false), Loaded::Missing));
        assert!(!q.exists());
        assert!(take_pending(&q).is_empty());
    }

    #[test]
    fn valid_loads_as_before() {
        let dir = temp_dir("valid");
        std::fs::write(dir.join("a.json"), "[1,2,3]").unwrap();
        let q = dir.join(QUARANTINE);
        assert!(matches!(load::<Vec<u32>>(&dir.join("a.json"), &q, false), Loaded::Ok(v) if v == vec![1, 2, 3]));
        assert!(!q.exists() && take_pending(&q).is_empty());
    }

    #[test]
    fn truncated_and_binary_files_are_moved_aside() {
        let dir = temp_dir("bad");
        let q = dir.join(QUARANTINE);
        for (name, content) in [("t.json", &b"[1,2"[..]), ("b.json", &[0xff, 0x00, 0xfe, 0x89][..])] {
            std::fs::write(dir.join(name), content).unwrap();
            let Loaded::Quarantined(w) = load::<Vec<u32>>(&dir.join(name), &q, true) else { panic!("{name} not quarantined") };
            assert_eq!(w.file, name);
            assert!(w.safety && !w.acknowledged && w.at.ends_with('Z'));
            assert!(w.error.starts_with("JSON invalide"), "{}", w.error);
            let to = w.moved_to.clone().unwrap();
            assert!(to.starts_with("quarantine/") && to.ends_with(".json"), "{to}");
            assert!(!dir.join(name).exists(), "moved, not copied");
            assert_eq!(std::fs::read(dir.join(&to)).unwrap(), content, "kept as is");
            assert!(w.message_fr().contains(name) && w.message_fr().contains(&to));
        }
        let pending = take_pending(&q);
        assert_eq!(pending.len(), 2);
        assert!(take_pending(&q).is_empty(), "taken once");
        // Missing now: the next load gives the defaults, no new warning.
        assert_eq!(load_or_default::<Vec<u32>>(&dir.join("t.json"), &q, false), Vec::<u32>::new());
        assert!(take_pending(&q).is_empty());
    }

    #[test]
    fn same_second_names_are_unique() {
        let dir = temp_dir("unique");
        let q = dir.join(QUARANTINE);
        let mut seen = Vec::new();
        for _ in 0..3 {
            std::fs::write(dir.join("scenes.json"), "{").unwrap();
            let Loaded::Quarantined(w) = load::<Vec<u32>>(&dir.join("scenes.json"), &q, false) else { panic!() };
            seen.push(w.moved_to.unwrap());
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert_eq!(std::fs::read_dir(&q).unwrap().count(), 3);
        assert_eq!(quarantine_name("midi/profiles/a.json", "20261003-141502", 1), "midi-profiles-a-20261003-141502.json");
        assert_eq!(quarantine_name("scenes.json", "20261003-141502", 2), "scenes-20261003-141502-2.json");
        take_pending(&q);
    }

    #[test]
    fn read_only_folder_freezes_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("ro");
        let q = dir.join(QUARANTINE);
        let path = dir.join("grid.json");
        std::fs::write(&path, "{ cassé").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let loaded = load::<Vec<u32>>(&path, &q, false);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let Loaded::Quarantined(w) = loaded else { panic!() };
        assert_eq!(w.moved_to, None);
        assert!(w.message_fr().contains("impossible de le mettre de côté"));
        assert!(is_frozen(&path));
        // Never written over, even now that the folder is writable again.
        write_atomic(&path, b"[]").unwrap();
        replace(&path, b"[]").unwrap();
        save_json(&path, &vec![1]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ cassé");
        assert_eq!(take_pending(&q).len(), 1);
    }
}
