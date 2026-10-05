//! Format versions and migrations (T-288).
//!
//! Every file the studio writes has an explicit format version: a project
//! (`.lsproj`) carries its own `format_version`; the machine files of the
//! data directory (`scenes.json`, `safety.json`, `shows/*.json`…) have
//! theirs in the data directory's manifest, `<data-dir>/format.json`
//! (many of them are bare JSON arrays, which have no room for a header,
//! and an older studio must still be able to read the files it knows).
//!
//! A file in an older format goes through a chain of migrations, one step
//! per version, applied to the raw JSON before it is decoded, after a copy
//! of the original is kept beside it (`<file>.v<N>.bak`). A file in a
//! **newer** format is never migrated, decoded with fields dropped, nor
//! written over:
//! - a project opens read only (`project.rs`), *Enregistrer* refused;
//! - a data directory written by a newer studio is refused at startup with
//!   a French message: nothing in it is read or written.
//!
//! A damaged manifest is set aside like any damaged file and rebuilt.
//! A data file whose migration fails is moved to the quarantine folder
//! like a damaged file (T-402): its store starts from its defaults, and a
//! safety file closes the `data_ack` interlock until acknowledged, exactly
//! as an unreadable one does. Nothing here can arm the laser.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One migration step: the JSON of version N in, version N+1 out.
pub type Step = fn(Value) -> Result<Value>;

/// Runs `chain` on `v`, which is at format `from`: `chain[i]` takes
/// version `i + 1` to `i + 2`, so the result is at `chain.len() + 1`.
/// Returns the value unchanged when it is already current. A newer or
/// invalid (0) version is the caller's to refuse: an error here.
pub fn run_chain(mut v: Value, from: u32, chain: &[Step]) -> Result<Value> {
    let current = current_of(chain);
    if from == 0 || from > current {
        bail!("format {from} inconnu (cette version lit les formats 1 à {current})");
    }
    for (i, step) in chain.iter().enumerate().skip(from as usize - 1) {
        v = step(v).with_context(|| format!("migration du format {} au format {}", i + 1, i + 2))?;
    }
    Ok(v)
}

/// The format a chain leads to.
pub fn current_of(chain: &[Step]) -> u32 {
    chain.len() as u32 + 1
}

/// `<file>.v<N>.bak<.ext>` beside `path`: `Mon show.lsproj` →
/// `Mon show.v1.bak.lsproj`, `scenes.json` → `scenes.v1.bak.json`.
pub fn backup_path(path: &Path, from: u32) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("fichier");
    let name = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.v{from}.bak.{ext}"),
        None => format!("{stem}.v{from}.bak"),
    };
    path.with_file_name(name)
}

/// Keeps the original bytes of a file about to be migrated. An existing
/// copy is the older original: it is kept, never written over.
pub fn keep_original(path: &Path, from: u32, bytes: &[u8]) -> Result<PathBuf> {
    let bak = backup_path(path, from);
    match std::fs::symlink_metadata(&bak) {
        Ok(m) if m.is_file() => {}
        Ok(_) => bail!("copie de sauvegarde {} impossible : ce n'est pas un fichier", bak.display()),
        Err(_) => crate::datafile::write_atomic(&bak, bytes).with_context(|| format!("copie de sauvegarde {} impossible", bak.display()))?,
    }
    Ok(bak)
}

// ---------- the data directory ----------

/// The data directory's manifest.
pub const MANIFEST: &str = "format.json";

/// Format of the manifest itself.
pub const MANIFEST_FORMAT: u32 = 1;

/// A machine file of the data directory: its path relative to the data
/// directory (`dir/*.json` for every file of a folder), the migrations of
/// its format and whether it is a safety file (strict fallback: a failed
/// migration closes `data_ack`, like an unreadable file).
pub struct DataFile {
    pub rel: &'static str,
    pub chain: &'static [Step],
    pub safety: bool,
}

const fn file(rel: &'static str, safety: bool) -> DataFile {
    DataFile { rel, chain: &[], safety }
}

/// Every machine file of the data directory, at format 1 so far (the
/// first versioned one): a step added to a `chain` bumps that file only.
pub const DATA_FILES: &[DataFile] = &[
    file("scenes.json", false),
    file("grid.json", false),
    file("pages.json", false),
    file("live.json", false),
    file("layers.json", false),
    file("lfos.json", false),
    file("audio_routes.json", false),
    file("palettes.json", false),
    file("scan_presets.json", false),
    file("user_cues.json", false),
    file("cuelists.json", false),
    file("sdbank.json", false),
    file("venue.json", false),
    file("timecode.json", false),
    file("netin.json", false),
    file("external.json", false),
    file("ponk.json", false),
    file("checklist.json", false),
    file("onboarding.json", false),
    file("festival.json", false),
    file("latency.json", false),
    file("audio_triggers.json", false),
    file("shownet_artnet.json", false),
    file("prefs.json", false),
    file("recent.json", false),
    file("shows/*.json", false),
    file("figures/*.json", false),
    file("midi/profiles/*.json", false),
    // Safety files: strict fallback (T-402).
    file("safety.json", true),
    file("calibration.json", true),
    file("outputs.json", true),
    file("presence.json", true),
    file("projection.json", true),
    file("audio.json", true),
    file("midi/devices.json", true),
];

/// `<data-dir>/format.json`. Entries and keys it doesn't know (written by
/// a newer studio, for files this one never touches) are kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub format_version: u32,
    pub app_version: String,
    /// Format of each data file, by `DataFile::rel`.
    pub files: BTreeMap<String, u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What `prepare_data_dir` did.
#[derive(Debug, Default, PartialEq)]
pub struct Prepared {
    /// Files brought to the current format: (relative path, from).
    pub migrated: Vec<(String, u32)>,
    /// Files whose migration failed (moved aside, defaults used).
    pub failed: Vec<String>,
}

/// The French reason a data directory is refused.
fn newer_message(data_dir: &Path, what: &str, v: u32, ours: u32) -> String {
    format!(
        "Le dossier de données {} a été écrit par une version plus récente de Laser Studio ({what} au format {v}, cette version lit le format {ours}). \
         Rien n'y a été lu ni modifié. Mettez Laser Studio à jour, ou lancez-le avec un autre dossier de données (--data-dir).",
        data_dir.display()
    )
}

/// Called once at startup, before any store reads its file: refuses a data
/// directory from a newer studio (`Err`, French, nothing written), brings
/// older files to the current format (original kept as `.v<N>.bak`) and
/// writes the manifest. A directory from before T-288 (no manifest) is at
/// format 1 everywhere.
pub fn prepare_data_dir(data_dir: &Path) -> Result<Prepared> {
    prepare_with(data_dir, DATA_FILES)
}

fn prepare_with(data_dir: &Path, files: &[DataFile]) -> Result<Prepared> {
    let path = data_dir.join(MANIFEST);
    let existing = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Manifest>(&bytes) {
            Ok(m) => Some(m),
            Err(e) => {
                // Damaged like any data file (T-402): set aside, rebuilt.
                crate::datafile::set_aside(&path, &data_dir.join(crate::datafile::QUARANTINE), &format!("JSON invalide : {e}"), false);
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => bail!("{} illisible : {e}", path.display()),
    };
    let mut manifest = existing.clone().unwrap_or_default();
    if manifest.format_version > MANIFEST_FORMAT {
        bail!(newer_message(data_dir, MANIFEST, manifest.format_version, MANIFEST_FORMAT));
    }
    // Every check first: nothing is migrated in a directory that is refused.
    for f in files {
        let ours = current_of(f.chain);
        if let Some(&v) = manifest.files.get(f.rel) {
            if v > ours {
                bail!(newer_message(data_dir, f.rel, v, ours));
            }
        }
    }
    let mut done = Prepared::default();
    for f in files {
        let ours = current_of(f.chain);
        let from = manifest.files.get(f.rel).copied().unwrap_or(1).max(1);
        if from < ours {
            for target in expand(data_dir, f.rel) {
                let rel = target.strip_prefix(data_dir).unwrap_or(&target).to_string_lossy().replace('\\', "/");
                match migrate_file(&target, from, f.chain) {
                    Ok(true) => done.migrated.push((rel, from)),
                    Ok(false) => {}
                    Err(e) => {
                        let q = data_dir.join(crate::datafile::QUARANTINE);
                        crate::datafile::set_aside(&target, &q, &format!("mise à jour du format impossible : {e:#}"), f.safety);
                        done.failed.push(rel);
                    }
                }
            }
        }
        manifest.files.insert(f.rel.to_string(), ours);
    }
    manifest.format_version = MANIFEST_FORMAT;
    if existing.as_ref().map(|m| (m.format_version, &m.files)) != Some((manifest.format_version, &manifest.files)) {
        manifest.app_version = env!("CARGO_PKG_VERSION").to_string();
        let mut json = serde_json::to_vec_pretty(&manifest)?;
        json.push(b'\n');
        crate::datafile::write_atomic(&path, &json)?;
    }
    for (rel, from) in &done.migrated {
        log::info!("{rel} : format {from} mis à jour (original gardé à côté)");
    }
    Ok(done)
}

/// The files `rel` names: itself, or every `.json` of its folder for `dir/*.json`.
fn expand(data_dir: &Path, rel: &str) -> Vec<PathBuf> {
    match rel.strip_suffix("/*.json") {
        Some(dir) => {
            let Ok(entries) = std::fs::read_dir(data_dir.join(dir)) else { return Vec::new() };
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json") && p.is_file())
                .filter(|p| !p.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.starts_with('.') || f.contains(".bak.")))
                .collect();
            files.sort();
            files
        }
        None => {
            let p = data_dir.join(rel);
            if p.is_file() {
                vec![p]
            } else {
                Vec::new()
            }
        }
    }
}

/// Migrates one file in place: original kept, then the new JSON written
/// atomically. `Ok(false)`: nothing to do (missing file).
fn migrate_file(path: &Path, from: u32, chain: &[Step]) -> Result<bool> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => bail!("lecture impossible : {e}"),
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("JSON invalide : {e}"))?;
    let migrated = run_chain(value, from, chain)?;
    keep_original(path, from, &bytes)?;
    let mut json = serde_json::to_vec_pretty(&migrated)?;
    json.push(b'\n');
    crate::datafile::write_atomic(path, &json)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-formats-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rename_a_to_b(mut v: Value) -> Result<Value> {
        if let Some(o) = v.as_object_mut() {
            if let Some(a) = o.remove("a") {
                o.insert("b".into(), a);
            }
        }
        Ok(v)
    }

    fn double_b(mut v: Value) -> Result<Value> {
        let b = v["b"].as_i64().context("b manquant")?;
        v["b"] = json!(b * 2);
        Ok(v)
    }

    fn refuse(_: Value) -> Result<Value> {
        bail!("refusé")
    }

    const CHAIN: &[Step] = &[rename_a_to_b, double_b];

    #[test]
    fn the_chain_runs_each_step_from_the_file_version() {
        assert_eq!(current_of(CHAIN), 3);
        assert_eq!(run_chain(json!({ "a": 2 }), 1, CHAIN).unwrap(), json!({ "b": 4 }));
        assert_eq!(run_chain(json!({ "b": 2 }), 2, CHAIN).unwrap(), json!({ "b": 4 }));
        assert_eq!(run_chain(json!({ "b": 2 }), 3, CHAIN).unwrap(), json!({ "b": 2 }), "current: unchanged");
        assert!(run_chain(json!({}), 4, CHAIN).is_err(), "newer: refused");
        assert!(run_chain(json!({}), 0, CHAIN).is_err());
        let e = run_chain(json!({ "a": 1 }), 1, &[refuse]).unwrap_err();
        assert!(format!("{e:#}").contains("migration du format 1 au format 2"), "{e:#}");
        assert_eq!(run_chain(json!([1]), 1, &[]).unwrap(), json!([1]));
    }

    #[test]
    fn backup_names_keep_the_extension() {
        assert_eq!(backup_path(Path::new("/p/Mon show.lsproj"), 1), Path::new("/p/Mon show.v1.bak.lsproj"));
        assert_eq!(backup_path(Path::new("/d/scenes.json"), 2), Path::new("/d/scenes.v2.bak.json"));
    }

    #[test]
    fn a_fresh_or_old_data_dir_gets_a_manifest_and_nothing_else_changes() {
        let dir = temp_dir("fresh");
        std::fs::write(dir.join("scenes.json"), "[]").unwrap();
        let done = prepare_data_dir(&dir).unwrap();
        assert_eq!(done, Prepared::default());
        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(m.format_version, MANIFEST_FORMAT);
        assert_eq!(m.files.len(), DATA_FILES.len());
        assert!(m.files.values().all(|&v| v == 1));
        assert_eq!(std::fs::read_to_string(dir.join("scenes.json")).unwrap(), "[]");
        // Unchanged on the next start: not rewritten.
        let before = std::fs::metadata(dir.join(MANIFEST)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        prepare_data_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(dir.join(MANIFEST)).unwrap().modified().unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_newer_data_dir_is_refused_and_left_untouched() {
        for (manifest, what) in [
            (json!({ "format_version": 1, "files": { "scenes.json": 7, "nouveau.json": 3 } }), "scenes.json au format 7"),
            (json!({ "format_version": 2, "files": {} }), "format.json au format 2"),
        ] {
            let dir = temp_dir("newer");
            let text = manifest.to_string();
            std::fs::write(dir.join(MANIFEST), &text).unwrap();
            std::fs::write(dir.join("scenes.json"), r#"[{"name":"x","nouveau":1}]"#).unwrap();
            let e = format!("{:#}", prepare_data_dir(&dir).unwrap_err());
            assert!(e.contains("version plus récente") && e.contains(what) && e.contains("Rien n'y a été lu ni modifié"), "{e}");
            assert_eq!(std::fs::read_to_string(dir.join(MANIFEST)).unwrap(), text);
            assert_eq!(std::fs::read_to_string(dir.join("scenes.json")).unwrap(), r#"[{"name":"x","nouveau":1}]"#);
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "nothing written");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn unknown_entries_of_a_newer_studio_are_kept() {
        let dir = temp_dir("unknown");
        std::fs::write(dir.join(MANIFEST), json!({ "format_version": 1, "files": { "nouveau.json": 3 }, "autre": true }).to_string()).unwrap();
        prepare_data_dir(&dir).unwrap();
        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(m.files.get("nouveau.json"), Some(&3));
        assert_eq!(m.extra.get("autre"), Some(&json!(true)));
        let _ = std::fs::remove_dir_all(dir);
    }

    const CHAINED: &[DataFile] = &[
        DataFile { rel: "a.json", chain: CHAIN, safety: false },
        DataFile { rel: "shows/*.json", chain: CHAIN, safety: false },
        DataFile { rel: "safety.json", chain: &[refuse], safety: true },
    ];

    #[test]
    fn older_files_are_migrated_after_a_backup_and_a_failure_is_set_aside() {
        let dir = temp_dir("migrate");
        std::fs::create_dir_all(dir.join("shows")).unwrap();
        std::fs::write(dir.join("a.json"), r#"{"a":1}"#).unwrap();
        std::fs::write(dir.join("shows/Intro.json"), r#"{"a":5}"#).unwrap();
        std::fs::write(dir.join("safety.json"), r#"{"x":1}"#).unwrap();
        let done = prepare_with(&dir, CHAINED).unwrap();
        assert_eq!(done.migrated, [("a.json".to_string(), 1), ("shows/Intro.json".to_string(), 1)]);
        assert_eq!(done.failed, ["safety.json"]);
        let read = |rel: &str| serde_json::from_slice::<Value>(&std::fs::read(dir.join(rel)).unwrap()).unwrap();
        assert_eq!(read("a.json"), json!({ "b": 2 }));
        assert_eq!(read("shows/Intro.json"), json!({ "b": 10 }));
        assert_eq!(std::fs::read_to_string(dir.join("a.v1.bak.json")).unwrap(), r#"{"a":1}"#, "original kept");
        assert_eq!(std::fs::read_to_string(dir.join("shows/Intro.v1.bak.json")).unwrap(), r#"{"a":5}"#);
        // The safety file went to quarantine, strict: a safety warning.
        assert!(!dir.join("safety.json").exists());
        let w = crate::datafile::take_pending(&dir.join(crate::datafile::QUARANTINE));
        assert_eq!(w.len(), 1);
        assert!(w[0].safety && w[0].error.contains("mise à jour du format impossible"), "{w:?}");
        let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!((m.files["a.json"], m.files["shows/*.json"], m.files["safety.json"]), (3, 3, 2));
        // Done once: the next start finds everything current.
        assert_eq!(prepare_with(&dir, CHAINED).unwrap(), Prepared::default());
        assert_eq!(read("a.json"), json!({ "b": 2 }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_data_file_is_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for f in DATA_FILES {
            assert!(seen.insert(f.rel), "{} twice", f.rel);
            assert_eq!(current_of(f.chain), 1, "{}: first versioned format", f.rel);
        }
    }
}
