//! Copy to a second location (T-450): when a backup folder is chosen
//! (another disk, a USB key, an iCloud folder), every project save and
//! every autosave (T-287) is also written there, under
//! `<dossier>/Laser Studio/<nom>.lsproj` and
//! `<dossier>/Laser Studio/autosave/…` (the same ring of autosaves).
//!
//! The main write is the reference: it happens first and alone decides
//! whether the save worked. The copy gets the same bytes afterwards, on a
//! thread of its own (`spawn`): the HTTP and autosave threads only queue a
//! job (a short lock, never any I/O), the engine thread never sees any of
//! it. A copy that fails (key pulled out, folder not writable, disk full)
//! shows an orange warning until the next copy works; it is not retried
//! in a loop, the next save tries again.
//!
//! The chosen folder itself is never created: if it is gone (on macOS a
//! key pulled out takes `/Volumes/<nom>` with it), recreating it would
//! write on the internal disk while the operator believes it is on the
//! key. Only `Laser Studio/` is created inside a folder that exists.
//!
//! Free space is measured (data folder and backup folder) when the copy
//! is turned on, at startup, and at every copy: under `min_free_gb` in
//! either, a warning. Nothing is ever restored from the copy
//! automatically: the operator opens it like any project. Media are not
//! copied (the `.lspack` package of T-289 is for that).
//!
//! The settings are app preferences (`<data-dir>/prefs.json` → `backup`),
//! not part of a project. Nothing here can arm the laser.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The folder created inside the chosen one.
pub const FOLDER: &str = "Laser Studio";
pub const DEFAULT_MIN_FREE_GB: f32 = 1.0;
const MAX_MIN_FREE_GB: f32 = 100_000.0;
/// Jobs waiting; beyond, the oldest goes (a newer save of it follows).
const MAX_QUEUE: usize = 8;
const GB: f64 = 1e9;

/// `prefs.json` → `backup`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupSettings {
    pub enabled: bool,
    pub dir: Option<PathBuf>,
    pub min_free_gb: f32,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self { enabled: false, dir: None, min_free_gb: DEFAULT_MIN_FREE_GB }
    }
}

impl BackupSettings {
    /// The folder to copy into, when the copy is on.
    pub fn active_dir(&self) -> Option<&Path> {
        self.dir.as_deref().filter(|_| self.enabled)
    }

    fn sanitize(&mut self) {
        self.min_free_gb = if self.min_free_gb.is_finite() { self.min_free_gb.clamp(0.0, MAX_MIN_FREE_GB) } else { DEFAULT_MIN_FREE_GB };
        if self.dir.as_ref().is_some_and(|d| d.as_os_str().is_empty()) {
            self.dir = None;
        }
    }
}

/// Run-time state, in `/api/state` under `storage` and in `/api/project`
/// under `backup`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StorageStatus {
    pub data_free_bytes: Option<u64>,
    pub backup_free_bytes: Option<u64>,
    /// `None`: no copy configured (or none tried yet).
    pub backup_ok: Option<bool>,
    /// UTC, ISO 8601.
    pub last_backup_at: Option<String>,
    /// The same, in ms since 1970 (for the page's local time).
    pub last_backup_ms: Option<u64>,
    /// *Copie de sauvegarde indisponible : <dossier>*, until a copy works.
    pub error: Option<String>,
    /// *Espace disque faible : …*.
    pub low_space: Option<String>,
    /// The first of `error` and `low_space`.
    pub warning: Option<String>,
}

/// What to copy: a path under `Laser Studio/` and the bytes written.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub rel: String,
    pub bytes: Vec<u8>,
    /// An autosave: its project's ring is pruned to `keep` in the copy.
    pub ring: Option<(String, usize)>,
}

impl Job {
    /// A saved project (`<nom>.lsproj`).
    pub fn project(name: &str, bytes: Vec<u8>) -> Self {
        Job { rel: format!("{name}.{}", crate::project::EXTENSION), bytes, ring: None }
    }

    /// An autosave (`autosave/<fichier>`), with its project's ring size.
    pub fn autosave(file: &str, project: &str, keep: usize, bytes: Vec<u8>) -> Self {
        Job { rel: format!("autosave/{file}"), bytes, ring: Some((project.to_string(), keep)) }
    }
}

/// Measures the free space of a folder (`free_bytes`; injected in tests).
pub type FreeFn = fn(&Path) -> Option<u64>;

struct Inner {
    settings: BackupSettings,
    status: StorageStatus,
    queue: VecDeque<Job>,
    /// Measure the free space at the next turn of the thread.
    check_space: bool,
    /// Stop once the queue is empty (shutdown).
    closing: bool,
}

/// Settings, status and queue, shared by the HTTP threads (settings,
/// *Copier maintenant*, saves), the autosave thread and the copy thread.
/// Its lock is only held to copy or swap these, never for I/O.
pub struct BackupHub {
    inner: Mutex<Inner>,
    wake: Condvar,
    data_dir: PathBuf,
    prefs_path: PathBuf,
    free: FreeFn,
}

impl BackupHub {
    /// Reads `prefs.json` (missing `backup`: off). The copy thread, once
    /// started, measures the free space if the copy is on (startup warning).
    pub fn load(data_dir: &Path) -> Arc<Self> {
        let prefs_path = data_dir.join("prefs.json");
        let prefs: Map<String, Value> = crate::load_json(&prefs_path);
        let mut settings: BackupSettings = prefs.get("backup").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        settings.sanitize();
        Self::new(data_dir, settings, free_bytes)
    }

    fn new(data_dir: &Path, settings: BackupSettings, free: FreeFn) -> Arc<Self> {
        let check_space = settings.active_dir().is_some();
        let inner = Inner { settings, status: StorageStatus::default(), queue: VecDeque::new(), check_space, closing: false };
        Arc::new(Self { inner: Mutex::new(inner), wake: Condvar::new(), data_dir: data_dir.to_path_buf(), prefs_path: data_dir.join("prefs.json"), free })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn settings(&self) -> BackupSettings {
        self.lock().settings.clone()
    }

    pub fn status(&self) -> StorageStatus {
        self.lock().status.clone()
    }

    /// `{ settings, status, folder }` for the page.
    pub fn view(&self) -> Value {
        let inner = self.lock();
        json!({ "settings": inner.settings, "status": inner.status, "folder": FOLDER })
    }

    /// Queues a copy (any thread, constant time, never any I/O). Nothing
    /// when the copy is off. A queued copy of the same file is replaced.
    pub fn queue(&self, job: Job) {
        let mut inner = self.lock();
        if inner.settings.active_dir().is_none() {
            return;
        }
        inner.queue.retain(|j| j.rel != job.rel);
        if inner.queue.len() >= MAX_QUEUE {
            inner.queue.pop_front();
        }
        inner.queue.push_back(job);
        drop(inner);
        self.wake.notify_one();
    }

    /// Changes the settings (already checked); the status starts over and
    /// the free space is measured again.
    fn replace_settings(&self, settings: BackupSettings) {
        let mut inner = self.lock();
        if inner.settings.active_dir() != settings.active_dir() {
            inner.status = StorageStatus::default();
            inner.queue.clear();
        }
        inner.check_space = settings.active_dir().is_some();
        if !inner.check_space {
            inner.status = StorageStatus::default();
        }
        inner.settings = settings;
        drop(inner);
        self.wake.notify_one();
    }

    /// Asks the copy thread to stop once its queue is written.
    pub fn shutdown(&self) {
        self.lock().closing = true;
        self.wake.notify_one();
    }

    /// The next thing to do, waiting at most `wait`: `Some(Some(job))`, a
    /// space check (`Some(None)`), or `None` (nothing; or closed and empty).
    fn next(&self, wait: Duration) -> Option<Option<Job>> {
        let mut inner = self.lock();
        if inner.queue.is_empty() && !inner.check_space && !inner.closing {
            inner = self.wake.wait_timeout(inner, wait).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
        if let Some(job) = inner.queue.pop_front() {
            return Some(Some(job));
        }
        if std::mem::take(&mut inner.check_space) {
            return Some(None);
        }
        None
    }

    fn closed(&self) -> bool {
        let inner = self.lock();
        inner.closing && inner.queue.is_empty()
    }

    /// Writes one job (copy thread): the bytes, the autosave ring, then
    /// the free space; the status follows. Lock released for the I/O.
    pub fn process(&self, job: &Job) {
        let settings = self.settings();
        let Some(dir) = settings.active_dir().map(Path::to_path_buf) else { return };
        let result = mirror(&job.rel, &job.bytes, &settings).and_then(|_| match &job.ring {
            Some((project, keep)) => prune(&dir.join(FOLDER).join("autosave"), project, *keep),
            None => Ok(()),
        });
        let (data_free, backup_free) = ((self.free)(&self.data_dir), (self.free)(&dir));
        let mut inner = self.lock();
        if inner.settings.active_dir() != Some(dir.as_path()) {
            return; // changed meanwhile: this result is about another folder
        }
        let st = &mut inner.status;
        match result {
            Ok(()) => {
                st.backup_ok = Some(true);
                st.error = None;
                st.last_backup_at = Some(crate::project::utc_now());
                st.last_backup_ms = Some(now_ms());
            }
            Err(e) => {
                log::warn!("copie de sauvegarde de {} : {e:#}", job.rel);
                st.backup_ok = Some(false);
                st.error = Some(unavailable_fr(&dir));
            }
        }
        set_space(st, data_free, backup_free, settings.min_free_gb);
    }

    /// Measures the free space of both folders (copy thread).
    pub fn check_space(&self) {
        let settings = self.settings();
        let Some(dir) = settings.active_dir().map(Path::to_path_buf) else { return };
        let (data_free, backup_free) = ((self.free)(&self.data_dir), (self.free)(&dir));
        let mut inner = self.lock();
        if inner.settings.active_dir() == Some(dir.as_path()) {
            set_space(&mut inner.status, data_free, backup_free, settings.min_free_gb);
        }
    }

    /// Everything queued, written now (tests; the thread does this).
    #[cfg(test)]
    pub fn run_pending(&self) {
        while let Some(next) = self.next(Duration::ZERO) {
            match next {
                Some(job) => self.process(&job),
                None => self.check_space(),
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn unavailable_fr(dir: &Path) -> String {
    format!("Copie de sauvegarde indisponible : {}", dir.display())
}

fn set_space(st: &mut StorageStatus, data_free: Option<u64>, backup_free: Option<u64>, min_gb: f32) {
    st.data_free_bytes = data_free;
    st.backup_free_bytes = backup_free;
    st.low_space = low_space(data_free, backup_free, min_gb);
    st.warning = st.error.clone().or_else(|| st.low_space.clone());
}

/// `0,6 Go`.
pub fn gb_fr(bytes: u64) -> String {
    format!("{:.1} Go", bytes as f64 / GB).replace('.', ",")
}

/// The warning when either folder has less than `min_gb` free.
pub fn low_space(data_free: Option<u64>, backup_free: Option<u64>, min_gb: f32) -> Option<String> {
    let min = f64::from(min_gb) * GB;
    let low: Vec<String> = [(data_free, "le disque de données"), (backup_free, "le disque de sauvegarde")]
        .into_iter()
        .filter_map(|(free, what)| free.filter(|&f| (f as f64) < min).map(|f| format!("{} sur {what}", gb_fr(f))))
        .collect();
    (!low.is_empty()).then(|| format!("Espace disque faible : {}", low.join(", ")))
}

/// Free bytes for an unprivileged user on the disk holding `dir`.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a writable statvfs.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    (rc == 0).then(|| (st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

/// Writes `bytes` to `<dossier>/Laser Studio/<rel>`, atomically. The
/// chosen folder must exist: it is never created.
pub fn mirror(rel: &str, bytes: &[u8], backup: &BackupSettings) -> Result<PathBuf> {
    let Some(dir) = backup.active_dir() else { bail!("aucun dossier de copie") };
    if !dir.is_dir() {
        bail!("dossier introuvable : {}", dir.display());
    }
    let rel = Path::new(rel);
    if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        bail!("chemin refusé : {}", rel.display());
    }
    let target = dir.join(FOLDER).join(rel);
    crate::datafile::write_atomic(&target, bytes)?;
    Ok(target)
}

/// Keeps the `keep` newest autosaves of `project` in the copy's ring.
fn prune(dir: &Path, project: &str, keep: usize) -> Result<()> {
    let files = crate::autosave::ring(dir, project);
    for old in &files[..files.len().saturating_sub(keep)] {
        std::fs::remove_file(old).with_context(|| format!("impossible de supprimer {}", old.display()))?;
    }
    Ok(())
}

/// Checks a folder typed by the operator: it exists, is writable, and is
/// not inside the data folder (that would not be a second location).
pub fn validate_dir(input: &Path, data_dir: &Path) -> Result<PathBuf> {
    if !input.is_absolute() {
        bail!("Indiquez un chemin complet (par exemple /Volumes/MaCle)");
    }
    let dir = input.canonicalize().ok().filter(|d| d.is_dir()).context("Dossier introuvable")?;
    let data = data_dir.canonicalize().unwrap_or_else(|_| data_dir.to_path_buf());
    if dir.starts_with(&data) {
        bail!("Doit être hors du dossier de données");
    }
    let probe = dir.join(format!(".laser-studio-essai-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|_| anyhow::anyhow!("Dossier non inscriptible"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(dir)
}

/// `POST /api/backup` body: any of the settings (`dir: ""` or `null` clears it).
#[derive(Deserialize)]
pub struct SettingsRequest {
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "some_or_null")]
    dir: Option<Option<String>>,
    min_free_gb: Option<f32>,
}

fn some_or_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// Changes the settings (the folder checked first, French errors) and
/// writes `prefs.json`, other keys kept. Lock released for the I/O.
pub fn set_settings(hub: &BackupHub, req: SettingsRequest) -> Result<BackupSettings, (u16, String)> {
    let mut settings = hub.settings();
    if let Some(dir) = req.dir {
        settings.dir = match dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => Some(validate_dir(Path::new(d), &hub.data_dir).map_err(|e| (400, format!("{e:#}")))?),
            None => None,
        };
    }
    if let Some(on) = req.enabled {
        settings.enabled = on;
    }
    if let Some(g) = req.min_free_gb {
        settings.min_free_gb = g;
    }
    settings.sanitize();
    let mut prefs: Map<String, Value> = crate::load_json(&hub.prefs_path);
    prefs.insert("backup".into(), json!(settings));
    let bytes = serde_json::to_vec_pretty(&prefs).map_err(|e| (500, e.to_string()))?;
    crate::datafile::write_atomic(&hub.prefs_path, &bytes).map_err(|e| (500, format!("impossible d'enregistrer les préférences : {e:#}")))?;
    hub.replace_settings(settings.clone());
    Ok(settings)
}

/// Starts the copy thread. It stops after `BackupHub::shutdown`, once its
/// queue is written (the autosave thread's last copy included).
pub fn spawn(hub: Arc<BackupHub>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("backup".into())
        .spawn(move || loop {
            match hub.next(Duration::from_millis(500)) {
                Some(Some(job)) => hub.process(&job),
                Some(None) => hub.check_space(),
                None if hub.closed() => break,
                None => {}
            }
        })
        .expect("failed to start the backup thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-backup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn plenty(_: &Path) -> Option<u64> {
        Some(500_000_000_000)
    }

    fn on(dir: &Path) -> BackupSettings {
        BackupSettings { enabled: true, dir: Some(dir.to_path_buf()), min_free_gb: 1.0 }
    }

    #[test]
    fn mirror_writes_the_same_bytes_and_never_creates_the_chosen_folder() {
        let root = temp_dir("mirror");
        let copy = root.join("cle");
        std::fs::create_dir_all(&copy).unwrap();
        let path = mirror("Mon show.lsproj", b"{\"a\":1}\n", &on(&copy)).unwrap();
        assert_eq!(path, copy.join("Laser Studio/Mon show.lsproj"));
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":1}\n");
        mirror("autosave/Mon show-20261005-210400.lsproj", b"x", &on(&copy)).unwrap();
        assert!(copy.join("Laser Studio/autosave/Mon show-20261005-210400.lsproj").is_file());
        // The key is gone: an error, and its folder is not recreated.
        let gone = root.join("Volumes/MaCle");
        let e = mirror("Mon show.lsproj", b"x", &on(&gone)).unwrap_err();
        assert!(format!("{e:#}").contains("dossier introuvable"), "{e:#}");
        assert!(!gone.exists() && !root.join("Volumes").exists());
        assert!(mirror("../x.lsproj", b"x", &on(&copy)).is_err());
        assert!(mirror("a.lsproj", b"x", &BackupSettings::default()).is_err(), "off");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_read_only_folder_fails_the_copy() {
        use std::os::unix::fs::PermissionsExt;
        let copy = temp_dir("ro");
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = mirror("a.lsproj", b"x", &on(&copy));
        let refused = validate_dir(&copy, Path::new("/nulle-part"));
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
        assert_eq!(format!("{:#}", refused.unwrap_err()), "Dossier non inscriptible");
        let _ = std::fs::remove_dir_all(copy);
    }

    #[test]
    fn the_folder_must_exist_be_absolute_and_be_outside_the_data_folder() {
        let root = temp_dir("validate");
        let data = root.join("data");
        std::fs::create_dir_all(data.join("sous")).unwrap();
        std::fs::create_dir_all(root.join("ailleurs")).unwrap();
        let err = |p: &Path| format!("{:#}", validate_dir(p, &data).unwrap_err());
        assert_eq!(err(&root.join("absent")), "Dossier introuvable");
        assert_eq!(err(&data), "Doit être hors du dossier de données");
        assert_eq!(err(&data.join("sous")), "Doit être hors du dossier de données");
        assert!(err(Path::new("relatif")).starts_with("Indiquez un chemin complet"));
        assert_eq!(validate_dir(&root.join("ailleurs"), &data).unwrap(), root.join("ailleurs"));
        // The parent of the data folder is another place: its copy goes in `Laser Studio/`.
        assert_eq!(validate_dir(&root, &data).unwrap(), root);
        assert_eq!(std::fs::read_dir(root.join("ailleurs")).unwrap().count(), 0, "probe removed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_copy_keeps_the_autosave_ring() {
        let root = temp_dir("ring");
        let hub = BackupHub::new(&root.join("data"), on(&root), plenty);
        for i in 0..25 {
            hub.queue(Job::autosave(&format!("P-20261005-2100{i:02}.lsproj"), "P", 20, vec![i]));
            hub.run_pending();
        }
        hub.queue(Job::autosave("Q-20261005-210000.lsproj", "Q", 20, vec![1]));
        hub.run_pending();
        let ring = crate::autosave::ring(&root.join("Laser Studio/autosave"), "P");
        assert_eq!(ring.len(), 20);
        assert!(ring[0].ends_with("P-20261005-210005.lsproj"), "{ring:?}");
        assert_eq!(crate::autosave::ring(&root.join("Laser Studio/autosave"), "Q").len(), 1, "another project's ring");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_folder_warns_until_the_next_copy_works() {
        let root = temp_dir("missing");
        let key = root.join("cle");
        let hub = BackupHub::new(&root.join("data"), on(&key), plenty);
        hub.queue(Job::project("Show", b"1".to_vec()));
        hub.run_pending();
        let st = hub.status();
        assert_eq!(st.backup_ok, Some(false));
        let warning = format!("Copie de sauvegarde indisponible : {}", key.display());
        assert_eq!((st.error.as_deref(), st.warning.as_deref()), (Some(warning.as_str()), Some(warning.as_str())));
        assert!(st.last_backup_at.is_none());
        // Not retried by itself: nothing queued any more.
        hub.run_pending();
        assert_eq!(hub.status(), st);
        std::fs::create_dir_all(&key).unwrap();
        hub.queue(Job::project("Show", b"2".to_vec()));
        hub.run_pending();
        let st = hub.status();
        assert_eq!((st.backup_ok, st.error, st.warning), (Some(true), None, None));
        assert!(st.last_backup_at.as_deref().is_some_and(|t| t.ends_with('Z')));
        assert_eq!(std::fs::read(key.join("Laser Studio/Show.lsproj")).unwrap(), b"2");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn low_space_is_measured_with_an_injected_meter() {
        assert_eq!(low_space(Some(5_000_000_000), Some(5_000_000_000), 1.0), None);
        assert_eq!(low_space(Some(5_000_000_000), Some(600_000_000), 1.0).as_deref(), Some("Espace disque faible : 0,6 Go sur le disque de sauvegarde"));
        assert_eq!(low_space(Some(200_000_000), None, 1.0).as_deref(), Some("Espace disque faible : 0,2 Go sur le disque de données"));
        assert_eq!(low_space(Some(5_000_000_000), Some(5_000_000_000), 10.0).as_deref(), Some("Espace disque faible : 5,0 Go sur le disque de données, 5,0 Go sur le disque de sauvegarde"));
        fn tiny(_: &Path) -> Option<u64> {
            Some(300_000_000)
        }
        let root = temp_dir("space");
        let hub = BackupHub::new(&root, on(&root), tiny);
        // At startup (copy on): measured before any save.
        hub.run_pending();
        let st = hub.status();
        assert_eq!(st.low_space.as_deref(), Some("Espace disque faible : 0,3 Go sur le disque de données, 0,3 Go sur le disque de sauvegarde"));
        assert_eq!(st.warning, st.low_space);
        assert_eq!((st.data_free_bytes, st.backup_free_bytes, st.backup_ok), (Some(300_000_000), Some(300_000_000), None));
        // Off (the default): nothing measured, nothing said.
        let hub = BackupHub::new(&root, BackupSettings::default(), tiny);
        hub.run_pending();
        assert_eq!(hub.status(), StorageStatus::default());
        assert!(free_bytes(&root).is_some_and(|b| b > 0), "the real meter works");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn off_by_default_and_prefs_without_backup_load_the_defaults() {
        let root = temp_dir("prefs");
        std::fs::write(root.join("prefs.json"), r#"{"autosave":{"delay_s":45}}"#).unwrap();
        let hub = BackupHub::load(&root);
        assert_eq!(hub.settings(), BackupSettings::default());
        hub.queue(Job::project("Show", b"1".to_vec()));
        assert!(hub.lock().queue.is_empty(), "off: nothing queued");
        // Turned on: prefs keep the autosave key.
        let copy = temp_dir("prefs-copy");
        let req: SettingsRequest = serde_json::from_value(json!({ "enabled": true, "dir": copy, "min_free_gb": 2.5 })).unwrap();
        let s = set_settings(&hub, req).unwrap();
        assert_eq!(s, BackupSettings { enabled: true, dir: Some(copy.clone()), min_free_gb: 2.5 });
        let prefs: Value = serde_json::from_slice(&std::fs::read(root.join("prefs.json")).unwrap()).unwrap();
        assert_eq!(prefs["autosave"]["delay_s"], 45);
        assert_eq!(BackupHub::load(&root).settings(), s, "read back");
        // Refused folders change nothing.
        let req: SettingsRequest = serde_json::from_value(json!({ "dir": root.join("sous-dossier") })).unwrap();
        assert_eq!(set_settings(&hub, req).unwrap_err(), (400, "Dossier introuvable".to_string()));
        std::fs::create_dir_all(root.join("sous-dossier")).unwrap();
        let req: SettingsRequest = serde_json::from_value(json!({ "dir": root.join("sous-dossier") })).unwrap();
        assert_eq!(set_settings(&hub, req).unwrap_err().1, "Doit être hors du dossier de données");
        assert_eq!(hub.settings(), s);
        // `dir: null` clears it.
        let req: SettingsRequest = serde_json::from_value(json!({ "dir": null })).unwrap();
        assert_eq!(set_settings(&hub, req).unwrap().dir, None);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(copy);
    }

    #[test]
    fn queueing_never_waits_for_a_slow_copy() {
        // A slow disk, the first time it is measured.
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn slow(_: &Path) -> Option<u64> {
            if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(400));
            }
            Some(500_000_000_000)
        }
        let root = temp_dir("slow");
        let hub = BackupHub::new(&root.join("data"), on(&root), slow);
        hub.lock().check_space = false;
        hub.queue(Job::project("A", vec![0; 1 << 20]));
        let t = spawn(Arc::clone(&hub));
        std::thread::sleep(Duration::from_millis(50)); // the thread is now in the slow disk
        // What the HTTP, autosave and state paths do: a few µs, never the copy's time.
        let start = std::time::Instant::now();
        for i in 0..100 {
            hub.queue(Job::project(&format!("B{i}"), vec![1; 1024]));
            let _ = hub.status();
            let _ = hub.view();
        }
        assert!(start.elapsed() < Duration::from_millis(50), "{:?}", start.elapsed());
        assert!(hub.lock().queue.len() <= MAX_QUEUE, "bounded");
        hub.shutdown();
        t.join().unwrap();
        assert!(hub.lock().queue.is_empty(), "written before stopping");
        assert!(root.join("Laser Studio/A.lsproj").is_file());
        let _ = std::fs::remove_dir_all(root);
    }
}
