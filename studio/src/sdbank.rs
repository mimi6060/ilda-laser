//! The ShowNET SD card bank (T-302): which cue the ShowNET plays for which
//! pattern number, and the export of the whole bank as a folder ready to
//! copy to the root of its microSD card.
//!
//! The ShowNET plays `001.ild`-`229.ild` from its card on its own, chosen
//! by DMX / Art-Net (the number is the value of the pattern channel).
//! `000` (DMX blackout) and `230`-`255` (Laserworld's beams and test
//! patterns) are refused, and so is a number used twice.
//!
//! Each file is one of **our** cues rendered offline at `SD_FPS` (the
//! ShowNET's default playback rate) for the slot's duration, through
//! `ilda_library::export_look`: the same output safety chain as the engine
//! (calibration, zones, horizon, outdoor cut of the output's projector,
//! strobe limiter, static beam guard, power caps of the chosen output) is
//! baked into every file (T-355, T-477). The ShowNET plays them without the studio: its own hardware stop stays the
//! real safety, the page says so.
//!
//! The bank is a machine file, `<data-dir>/sdbank.json` (it describes the
//! card in this rig's ShowNET, like `outputs.json`), not part of a project.
//! The export writes into a folder the operator names, never inside the
//! data directory; `banque.json` beside the files lists number → cue name
//! (read back by T-301). Everything is rendered and written on the
//! request's own thread (`web.rs`, beside the single HTTP worker), the
//! engine lock held only to copy the cues and the chain out.
//!
//! Show mode (T-283): editing the bank and exporting it to a folder are
//! refused (they are not playback; an export also renders for seconds).
//! Only the single-file download (a GET) stays allowed.

use crate::engine::Settings;
use crate::ilda;
use crate::ilda_library::{self, ExportSafety, MAX_EXPORT_FRAMES};
use crate::Shared;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Images per second of an exported file: the ShowNET's default playback
/// rate, so the strobe limiter's timing matches what it plays.
pub const SD_FPS: f32 = 50.0;
pub const DEFAULT_SECONDS: f32 = 4.0;
pub const MIN_SECONDS: f32 = 0.1;
/// `MAX_EXPORT_FRAMES` at `SD_FPS`.
pub const MAX_SECONDS: f32 = MAX_EXPORT_FRAMES as f32 / SD_FPS;
/// Above this the Admin Tool refuses a file (a card reader still takes it).
pub const WARN_BYTES: usize = 6 * 1024 * 1024;
pub const FIRST: u8 = 1;
pub const LAST: u8 = 229;
/// The list of the exported folder.
pub const INDEX_FILE: &str = "banque.json";
const MAX_CUE_ID: usize = 200;

fn default_seconds() -> f32 {
    DEFAULT_SECONDS
}

/// One file of the card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SdSlot {
    /// 1..=229: the file `NNN.ild` and the DMX value that plays it.
    pub number: u8,
    pub cue_id: String,
    /// Length of the exported animation, then looped by the ShowNET.
    #[serde(default = "default_seconds")]
    pub seconds: f32,
}

impl Default for SdSlot {
    fn default() -> Self {
        Self { number: 0, cue_id: String::new(), seconds: DEFAULT_SECONDS }
    }
}

/// `sdbank.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SdBank {
    pub slots: Vec<SdSlot>,
}

impl SdBank {
    /// Refuses reserved or repeated numbers, empty cues and durations out
    /// of range (French messages for the UI).
    pub fn validate(&self) -> Result<()> {
        if self.slots.len() > LAST as usize {
            bail!("{LAST} fichiers au plus sur la carte");
        }
        let mut seen = [false; 256];
        for s in &self.slots {
            ilda::shownet_sd_name(s.number as u32)?;
            if std::mem::replace(&mut seen[s.number as usize], true) {
                bail!("Le numéro {:03} est utilisé deux fois", s.number);
            }
            if s.cue_id.trim().is_empty() || s.cue_id.len() > MAX_CUE_ID {
                bail!("{:03} : cue manquant", s.number);
            }
            if !s.seconds.is_finite() || !(MIN_SECONDS..=MAX_SECONDS).contains(&s.seconds) {
                bail!("{:03} : durée de {MIN_SECONDS} à {MAX_SECONDS} s", s.number);
            }
        }
        Ok(())
    }

    /// The lowest number not used yet.
    pub fn free_number(&self) -> Option<u8> {
        (FIRST..=LAST).find(|n| !self.slots.iter().any(|s| s.number == *n))
    }

    /// Adds `cue_id` under the lowest free number, which it returns.
    pub fn add(&mut self, cue_id: &str, seconds: f32) -> Result<u8> {
        let Some(number) = self.free_number() else { bail!("La carte est pleine (001 à {LAST} pris)") };
        let mut next = self.clone();
        next.slots.push(SdSlot { number, cue_id: cue_id.to_string(), seconds });
        next.validate()?;
        *self = next;
        Ok(number)
    }

    /// Numbers the slots 001, 002… in their order on screen.
    pub fn renumber(&mut self) {
        for (i, s) in self.slots.iter_mut().enumerate().take(LAST as usize) {
            s.number = i as u8 + FIRST;
        }
    }

    /// Slots in number order (the order of the card and of `banque.json`).
    pub fn sort(&mut self) {
        self.slots.sort_by_key(|s| s.number);
    }
}

pub struct SdBankStore {
    /// None: in memory only (tests).
    path: Option<PathBuf>,
    bank: SdBank,
}

impl SdBankStore {
    /// A missing file is an empty bank; a damaged one is set aside and
    /// reported (datafile.rs, T-402).
    pub fn load(path: PathBuf) -> Self {
        Self { bank: crate::load_json(&path), path: Some(path) }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { path: None, bank: SdBank::default() }
    }

    pub fn get(&self) -> &SdBank {
        &self.bank
    }

    /// The data directory the bank lives in (exports never go inside it).
    pub fn data_dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    /// Replaces the bank (checked first, sorted by number) and saves it.
    pub fn set(&mut self, mut bank: SdBank) -> Result<()> {
        bank.validate()?;
        bank.sort();
        if let Some(path) = &self.path {
            let json = serde_json::to_vec_pretty(&bank).context("banque SD illisible")?;
            crate::datafile::replace(path, &json).with_context(|| format!("impossible d'écrire {}", path.display()))?;
        }
        self.bank = bank;
        Ok(())
    }
}

/// One slot ready to render: copied out of `Shared` under the lock.
#[derive(Clone, Debug)]
pub struct JobSlot {
    pub number: u8,
    pub cue_id: String,
    pub name: String,
    pub seconds: f32,
    pub settings: Settings,
}

/// What an export needs, without the lock.
#[derive(Clone, Debug)]
pub struct ExportJob {
    pub slots: Vec<JobSlot>,
    pub bpm: f64,
    pub chain: ExportSafety,
    /// Never written into.
    pub data_dir: Option<PathBuf>,
}

/// Copies the bank, its cues and the safety chain of `output`. Refused:
/// an empty or invalid bank, a cue that no longer exists.
pub fn prepare(s: &Shared, output: &str) -> Result<ExportJob> {
    let bank = s.sdbank.get();
    if bank.slots.is_empty() {
        bail!("La banque est vide : ajoutez des cues d'abord");
    }
    bank.validate()?;
    let mut missing = Vec::new();
    let mut slots = Vec::new();
    for slot in &bank.slots {
        match s.presets.iter().find(|p| p.id == slot.cue_id) {
            Some(p) => slots.push(JobSlot { number: slot.number, cue_id: p.id.clone(), name: p.name.clone(), seconds: slot.seconds, settings: p.settings.clone() }),
            None => missing.push(format!("{:03} ({})", slot.number, slot.cue_id)),
        }
    }
    if !missing.is_empty() {
        bail!("Cues introuvables : {}", missing.join(", "));
    }
    slots.sort_by_key(|s| s.number);
    Ok(ExportJob { slots, bpm: s.tempo.bpm, chain: ExportSafety::of(s, output)?, data_dir: s.sdbank.data_dir().map(Path::to_path_buf) })
}

/// The folder the operator typed: `~/…` is the home folder; otherwise an
/// absolute path, without `..`.
pub fn resolve_dir(raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("Choisissez un dossier");
    }
    let path = match raw.strip_prefix("~/").or(if raw == "~" { Some("") } else { None }) {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").context("dossier personnel inconnu")?).join(rest),
        None => PathBuf::from(raw),
    };
    if !path.is_absolute() {
        bail!("Chemin complet attendu (ex. ~/Bureau/carte-shownet)");
    }
    if path.components().any(|c| c == Component::ParentDir) {
        bail!("Chemin sans « .. » attendu");
    }
    Ok(path)
}

/// One written file.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FileReport {
    pub number: u8,
    pub file: String,
    pub name: String,
    pub cue_id: String,
    pub seconds: f32,
    pub frames: usize,
    pub bytes: usize,
}

/// What an export did. `banque.json` holds the same, minus the warnings.
#[derive(Clone, Debug, Serialize)]
pub struct ExportReport {
    pub dir: String,
    pub output: String,
    pub fps: f32,
    pub files: Vec<FileReport>,
    pub warnings: Vec<String>,
    /// `NNN.ild` files already in the folder that are not in the bank:
    /// left alone, but they would play on the card too.
    pub stale: Vec<String>,
}

/// `banque.json`: number → cue name, and how each file was made.
#[derive(Serialize)]
struct IndexFile<'a> {
    generator: &'static str,
    fps: f32,
    output: &'a str,
    files: &'a [FileReport],
}

fn is_sd_name(name: &str) -> bool {
    name.len() == 7 && name.ends_with(".ild") && name[..3].bytes().all(|b| b.is_ascii_digit())
}

/// Renders every slot and writes `NNN.ild` + `banque.json` into `dir`
/// (created if needed). Every file is rendered and written aside first
/// (`.NNN.ild.part`); only when all of them succeeded are they renamed,
/// so a refused cue leaves the folder as it was.
pub fn export(job: &ExportJob, dir: &Path) -> Result<ExportReport> {
    if let Some(data) = &job.data_dir {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        // The folder may not exist yet: compare its closest existing parent.
        let mut probe = dir.to_path_buf();
        while !probe.exists() && probe.pop() {}
        if canon(&probe).starts_with(canon(data)) {
            bail!("Choisissez un dossier hors des données du studio");
        }
    }
    if dir.exists() && !dir.is_dir() {
        bail!("{} n'est pas un dossier", dir.display());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("impossible de créer {}", dir.display()))?;

    let mut parts: Vec<(PathBuf, PathBuf)> = Vec::new();
    let cleanup = |parts: &[(PathBuf, PathBuf)]| {
        for (tmp, _) in parts {
            let _ = std::fs::remove_file(tmp);
        }
    };
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for slot in &job.slots {
        let file = ilda::shownet_sd_name(slot.number as u32)?;
        let frames = ((slot.seconds * SD_FPS).round() as usize).clamp(1, MAX_EXPORT_FRAMES);
        let exported = match ilda_library::export_look(&slot.settings, frames, SD_FPS, job.bpm, &job.chain) {
            Ok(e) => e,
            Err(e) => {
                cleanup(&parts);
                bail!("{file} (« {} ») : {e:#}", slot.name);
            }
        };
        let (tmp, path) = (dir.join(format!(".{file}.part")), dir.join(&file));
        let written = std::fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(&exported.bytes)?;
            f.sync_all()
        });
        parts.push((tmp, path));
        if let Err(e) = written {
            cleanup(&parts);
            return Err(e).with_context(|| format!("impossible d'écrire {file} dans {}", dir.display()));
        }
        let bytes = exported.bytes.len();
        if bytes > WARN_BYTES {
            warnings.push(format!("{file} : {:.1} Mo, au-delà de 6 Mo l'Admin Tool le refuse ; copiez-le avec un lecteur de carte", bytes as f64 / 1048576.0));
        }
        if exported.lit_after < exported.lit_before {
            let cut = 100 - exported.lit_after * 100 / exported.lit_before.max(1);
            if cut > 0 {
                warnings.push(format!("{file} : la chaîne de sécurité a éteint {cut} % des points allumés (zones, horizon, limiteur, garde)"));
            }
        }
        files.push(FileReport { number: slot.number, file, name: slot.name.clone(), cue_id: slot.cue_id.clone(), seconds: slot.seconds, frames: exported.frames, bytes });
    }
    for (tmp, path) in &parts {
        if let Err(e) = std::fs::rename(tmp, path) {
            cleanup(&parts);
            return Err(e).with_context(|| format!("impossible d'écrire {}", path.display()));
        }
    }
    let index = IndexFile { generator: "Laser Studio", fps: SD_FPS, output: &job.chain.output, files: &files };
    let json = serde_json::to_vec_pretty(&index).context("banque.json")?;
    crate::datafile::write_atomic(&dir.join(INDEX_FILE), &json)?;

    let mut stale: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| is_sd_name(n) && !files.iter().any(|f| &f.file == n))
                .collect()
        })
        .unwrap_or_default();
    stale.sort();
    if !stale.is_empty() {
        warnings.push(format!("Anciens fichiers hors banque dans ce dossier : {} (laissés en place, retirez-les de la carte si besoin)", stale.join(", ")));
    }
    Ok(ExportReport { dir: dir.display().to_string(), output: job.chain.output.clone(), fps: SD_FPS, files, warnings, stale })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Content;
    use crate::zones::{Zone, ZoneKind};

    fn slot(number: u8, cue: &str) -> SdSlot {
        SdSlot { number, cue_id: cue.into(), seconds: 1.0 }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-sdbank-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn reserved_and_repeated_numbers_are_refused() {
        for bad in [0u8, 230, 240, 255] {
            let e = SdBank { slots: vec![slot(bad, "a")] }.validate().unwrap_err().to_string();
            assert!(e.contains("réservé"), "{bad}: {e}");
        }
        assert!(SdBank { slots: vec![slot(1, "a"), slot(229, "b")] }.validate().is_ok());
        let e = SdBank { slots: vec![slot(7, "a"), slot(7, "b")] }.validate().unwrap_err().to_string();
        assert!(e.contains("deux fois"), "{e}");
        assert!(SdBank { slots: vec![slot(3, " ")] }.validate().is_err());
        for secs in [0.0, -1.0, f32::NAN, MAX_SECONDS + 1.0] {
            assert!(SdBank { slots: vec![SdSlot { seconds: secs, ..slot(3, "a") }] }.validate().is_err(), "{secs}");
        }
    }

    #[test]
    fn numbers_are_given_automatically() {
        let mut bank = SdBank { slots: vec![slot(1, "a"), slot(3, "b")] };
        assert_eq!(bank.add("c", 2.0).unwrap(), 2);
        assert_eq!(bank.add("d", 2.0).unwrap(), 4);
        assert!(bank.add("e", f32::NAN).is_err(), "a bad duration adds nothing");
        assert_eq!(bank.slots.len(), 4);
        bank.slots.swap(0, 3);
        bank.renumber();
        let order: Vec<(u8, &str)> = bank.slots.iter().map(|s| (s.number, s.cue_id.as_str())).collect();
        assert_eq!(order, vec![(1, "d"), (2, "b"), (3, "c"), (4, "a")]);
        let mut full = SdBank { slots: (1..=LAST).map(|n| slot(n, "x")).collect() };
        assert!(full.add("y", 1.0).unwrap_err().to_string().contains("pleine"));
    }

    #[test]
    fn a_bank_without_durations_loads_with_the_default() {
        let bank: SdBank = serde_json::from_str(r#"{"slots":[{"number":5,"cue_id":"tunnels-001"}]}"#).unwrap();
        assert_eq!(bank.slots[0].seconds, DEFAULT_SECONDS);
        assert_eq!(bank.slots[0].number, 5);
        let empty: SdBank = serde_json::from_str("{}").unwrap();
        assert!(empty.slots.is_empty());
        // Saved and loaded back from a data directory.
        let dir = temp_dir("store");
        let mut store = SdBankStore::load(dir.join("sdbank.json"));
        assert!(store.get().slots.is_empty());
        store.set(SdBank { slots: vec![slot(9, "b"), slot(2, "a")] }).unwrap();
        assert!(store.set(SdBank { slots: vec![slot(0, "a")] }).is_err());
        let back = SdBankStore::load(dir.join("sdbank.json"));
        assert_eq!(back.get().slots.iter().map(|s| s.number).collect::<Vec<_>>(), vec![2, 9], "sorted by number");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_are_absolute_or_home() {
        assert!(resolve_dir("").is_err());
        assert!(resolve_dir("relatif/dossier").is_err());
        assert!(resolve_dir("/tmp/../etc").is_err());
        assert_eq!(resolve_dir(" /tmp/carte ").unwrap(), PathBuf::from("/tmp/carte"));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(resolve_dir("~/carte").unwrap(), PathBuf::from(home).join("carte"));
        }
    }

    fn bank_shared(slots: Vec<SdSlot>) -> Shared {
        let mut s = crate::test_support::shared();
        s.sdbank.set(SdBank { slots }).unwrap();
        s
    }

    fn cue(s: &Shared, n: usize) -> String {
        s.presets.iter().filter(|p| !matches!(p.settings.content, Content::Ilda { .. })).nth(n).unwrap().id.clone()
    }

    #[test]
    fn export_writes_readable_format5_files_and_the_list() {
        let s = crate::test_support::shared();
        let (a, b) = (cue(&s, 0), cue(&s, 5));
        let mut s = bank_shared(vec![SdSlot { number: 12, cue_id: b.clone(), seconds: 0.5 }, SdSlot { number: 1, cue_id: a.clone(), seconds: 1.0 }]);
        s.outputs = crate::power::OutputStore::in_memory();
        let job = prepare(&s, crate::power::MAIN_OUTPUT).unwrap();
        let dir = temp_dir("export").join("carte");
        // A stale card file from an earlier export is reported, not deleted.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("077.ild"), b"old").unwrap();
        let report = export(&job, &dir).unwrap();
        assert_eq!(report.files.iter().map(|f| f.file.as_str()).collect::<Vec<_>>(), vec!["001.ild", "012.ild"]);
        assert_eq!(report.stale, vec!["077.ild"]);
        assert!(dir.join("077.ild").exists());
        let one = ilda::read(&std::fs::read(dir.join("001.ild")).unwrap()).unwrap();
        assert_eq!(one.len(), 50, "1 s at {SD_FPS} images/s");
        assert_eq!(ilda::read(&std::fs::read(dir.join("012.ild")).unwrap()).unwrap().len(), 25);
        assert_eq!(std::fs::read(dir.join("001.ild")).unwrap()[7], 5, "format 5");
        // Never above the output's cap (50 % by default).
        let cap = s.outputs.active_limits().max_power;
        assert!(one.iter().flat_map(|f| &f.points).all(|p| p.r.max(p.g).max(p.b) <= cap + 1e-6));
        let index: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(INDEX_FILE)).unwrap()).unwrap();
        assert_eq!(index["files"][0]["number"], 1);
        assert_eq!(index["files"][0]["cue_id"], a.as_str());
        assert_eq!(index["files"][0]["name"], s.presets.iter().find(|p| p.id == a).unwrap().name.as_str());
        assert_eq!(index["files"][1]["file"], "012.ild");
        // No temporary file left.
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(names.iter().all(|n| !n.starts_with('.')), "{names:?}");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn exported_files_respect_the_blank_zones_of_the_rig() {
        let s0 = crate::test_support::shared();
        let mut s = bank_shared(vec![SdSlot { number: 3, cue_id: cue(&s0, 0), seconds: 0.4 }]);
        let zone = Zone { id: 1, name: "public".into(), kind: ZoneKind::Blank, level: 0.0, points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, -0.1], [-1.0, -0.1]], ..Default::default() };
        let mut cfg = s.safety.get();
        cfg.zones = vec![zone];
        s.safety.set(cfg.clone(), true).unwrap();
        let dir = temp_dir("zones");
        let report = export(&prepare(&s, crate::power::MAIN_OUTPUT).unwrap(), &dir).unwrap();
        let frames = ilda::read(&std::fs::read(dir.join("003.ild")).unwrap()).unwrap();
        assert!(frames.iter().flat_map(|f| &f.points).any(|p| p.is_lit()), "the part above the zone still shows");
        for f in &frames {
            assert_eq!(crate::zones::blank_violations(&f.points, &cfg.zones), 0, "{}", f.name);
        }
        assert!(report.warnings.iter().any(|w| w.contains("éteint")), "{:?}", report.warnings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_cue_leaves_the_folder_as_it_was() {
        let s0 = crate::test_support::shared();
        let mut s = bank_shared(vec![SdSlot { number: 1, cue_id: cue(&s0, 0), seconds: 0.2 }, SdSlot { number: 2, cue_id: cue(&s0, 1), seconds: 0.2 }]);
        // Missing cue: refused before anything is rendered.
        s.sdbank.set(SdBank { slots: vec![slot(1, "plus-la")] }).unwrap();
        assert!(prepare(&s, crate::power::MAIN_OUTPUT).unwrap_err().to_string().contains("introuvables"));
        s.sdbank.set(SdBank::default()).unwrap();
        assert!(prepare(&s, crate::power::MAIN_OUTPUT).unwrap_err().to_string().contains("vide"));
        // A chain that cuts everything: no file at all, not even the first.
        s.sdbank.set(SdBank { slots: vec![SdSlot { number: 1, cue_id: cue(&s0, 0), seconds: 0.2 }] }).unwrap();
        let mut job = prepare(&s, crate::power::MAIN_OUTPUT).unwrap();
        job.slots.push(JobSlot { number: 2, cue_id: "noir".into(), name: "Noir".into(), seconds: 0.2, settings: Settings { brightness: 0.0, ..Settings::default() } });
        let dir = temp_dir("refused");
        assert!(export(&job, &dir).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(prepare(&s, "ailleurs").is_err(), "unknown output");
        // Never inside the data directory.
        job.data_dir = Some(dir.clone());
        assert!(export(&job, &dir.join("sd")).unwrap_err().to_string().contains("hors des données"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
