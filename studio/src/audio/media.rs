//! The user's songs, `studio-data/media/audio/` (T-161).
//!
//! Imported through the API (the page sends the file's bytes): checked by
//! decoding it **before** anything is written, then stored under a safe
//! name. Names are file names: letters, digits, spaces, `-`, `_`, then a
//! known extension, so a name can never leave the folder; symlinks are not
//! followed. The waveform overview is computed once and cached next to the
//! files (`.peaks/<file>.peaks`), keyed by the file's size and date, and
//! in memory. These are the user's own files: never in git, never in a
//! project file (a project only names them).

use super::decode::{self, Decoded, WaveformPeaks, PEAK_BLOCK};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

/// Extensions accepted on import (the content is what is checked).
pub const EXTENSIONS: [&str; 5] = ["wav", "mp3", "aif", "aiff", "flac"];
/// Largest file accepted on import.
pub const MAX_IMPORT_BYTES: u64 = 512 << 20;
const PEAKS_MAGIC: &[u8; 8] = b"LSPEAKS1";

/// A song of the library, as listed by `GET /api/media/audio`.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SongInfo {
    pub file: String,
    pub size: u64,
}

/// What an import returns.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Imported {
    pub file: String,
    pub duration_s: f64,
    pub sample_rate: u32,
    pub channels: u16,
    /// The same bytes were already in the library under this name.
    pub existing: bool,
}

/// A song file name: `<stem>.<ext>`, the stem made of letters, digits,
/// spaces, `-` and `_` (1 to 64 characters, trimmed), the extension one of
/// `EXTENSIONS` in lower case.
pub fn valid_song_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    crate::timeline::valid_show_name(stem) && EXTENSIONS.contains(&ext)
}

/// A safe library name for an uploaded file name: other characters become
/// `_`, the extension is lower-cased. Refused: no or unknown extension.
pub fn safe_song_name(original: &str) -> Result<String> {
    // Only the last path component, whatever the browser sent.
    let base = original.rsplit(['/', '\\']).next().unwrap_or("");
    let Some((stem, ext)) = base.rsplit_once('.') else { bail!("extension manquante (WAV, MP3, AIFF ou FLAC attendu)") };
    let ext = ext.to_ascii_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        bail!("extension .{ext} non gérée (WAV, MP3, AIFF ou FLAC attendu)");
    }
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).take(56).collect();
    let stem = stem.trim();
    let stem = if stem.is_empty() { "morceau" } else { stem };
    Ok(format!("{stem}.{ext}"))
}

type Stamp = (u64, u128);

pub struct MediaStore {
    dir: PathBuf,
    peaks: Mutex<HashMap<String, (Stamp, Arc<WaveformPeaks>)>>,
}

impl MediaStore {
    /// `<data-dir>/media/audio/` (created on the first import).
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("media").join("audio"), peaks: Mutex::new(HashMap::new()) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file of a song: a valid name, a regular file (not a symlink).
    pub fn path(&self, name: &str) -> Result<PathBuf> {
        if !valid_song_name(name) {
            bail!("nom de morceau invalide : {name}");
        }
        let path = self.dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => Ok(path),
            Ok(_) => bail!("morceau refusé (pas un fichier ordinaire) : {name}"),
            Err(_) => bail!("morceau introuvable : {name}"),
        }
    }

    fn stamp(path: &Path) -> Result<Stamp> {
        let m = std::fs::symlink_metadata(path)?;
        let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        Ok((m.len(), mtime))
    }

    /// The songs in the folder, by name.
    pub fn list(&self) -> Vec<SongInfo> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut list: Vec<SongInfo> = entries
            .filter_map(|e| {
                let e = e.ok()?;
                let file = e.file_name().to_str()?.to_string();
                let meta = e.path().symlink_metadata().ok()?;
                (valid_song_name(&file) && meta.file_type().is_file()).then_some(SongInfo { file, size: meta.len() })
            })
            .collect();
        list.sort_by(|a, b| a.file.cmp(&b.file));
        list
    }

    pub fn decode(&self, name: &str) -> Result<Decoded> {
        let path = self.path(name)?;
        let bytes = std::fs::read(&path).with_context(|| format!("lecture impossible : {name}"))?;
        decode::decode(&bytes)
    }

    /// Adds a song. The bytes are decoded first: a damaged or unknown file
    /// is refused and nothing is written. The same bytes imported again give
    /// the existing file; another file with the same name gets « nom 2 ».
    pub fn import(&self, original_name: &str, bytes: &[u8]) -> Result<Imported> {
        let name = safe_song_name(original_name)?;
        if bytes.len() as u64 > MAX_IMPORT_BYTES {
            bail!("fichier trop gros ({} Mo au plus)", MAX_IMPORT_BYTES >> 20);
        }
        let decoded = decode::decode(bytes)?;
        std::fs::create_dir_all(&self.dir).with_context(|| format!("failed to create {}", self.dir.display()))?;
        let (stem, ext) = name.rsplit_once('.').expect("safe names have an extension");
        let mut n = 1;
        let (file, existing) = loop {
            let candidate = if n == 1 { name.clone() } else { format!("{stem} {n}.{ext}") };
            let path = self.dir.join(&candidate);
            match std::fs::symlink_metadata(&path) {
                Err(_) => break (candidate, false),
                Ok(m) if m.file_type().is_file() && m.len() == bytes.len() as u64 && std::fs::read(&path).ok().as_deref() == Some(bytes) => {
                    break (candidate, true)
                }
                Ok(_) => n += 1,
            }
            if n > 999 {
                bail!("trop de morceaux nommés {name}");
            }
        };
        let path = self.dir.join(&file);
        if !existing {
            // Written aside, then renamed: a half-written song is never seen.
            let tmp = self.dir.join(format!(".{file}.part"));
            let written = std::fs::File::create(&tmp).and_then(|mut f| {
                f.write_all(bytes)?;
                f.sync_all()
            });
            if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, &path)) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e).with_context(|| format!("failed to write {}", path.display()));
            }
        }
        let peaks = Arc::new(WaveformPeaks::compute(&decoded, PEAK_BLOCK));
        self.remember(&file, &path, Arc::clone(&peaks));
        Ok(Imported { file, duration_s: decoded.duration_s(), sample_rate: decoded.sample_rate, channels: decoded.channels, existing })
    }

    fn peaks_path(&self, name: &str) -> PathBuf {
        self.dir.join(".peaks").join(format!("{name}.peaks"))
    }

    fn remember(&self, name: &str, path: &Path, peaks: Arc<WaveformPeaks>) {
        let Ok(stamp) = Self::stamp(path) else { return };
        if let Err(e) = self.save_peaks(name, stamp, &peaks) {
            log::warn!("waveform cache not written for {name}: {e:#}");
        }
        self.peaks.lock().unwrap().insert(name.to_string(), (stamp, peaks));
    }

    /// The waveform of a song: from memory, else the disk cache, else
    /// decoded now (and cached). A changed file is decoded again.
    pub fn peaks(&self, name: &str) -> Result<Arc<WaveformPeaks>> {
        let path = self.path(name)?;
        let stamp = Self::stamp(&path)?;
        if let Some((s, p)) = self.peaks.lock().unwrap().get(name) {
            if *s == stamp {
                return Ok(Arc::clone(p));
            }
        }
        if let Some(p) = self.load_peaks(name, stamp) {
            let p = Arc::new(p);
            self.peaks.lock().unwrap().insert(name.to_string(), (stamp, Arc::clone(&p)));
            return Ok(p);
        }
        let decoded = self.decode(name)?;
        let peaks = Arc::new(WaveformPeaks::compute(&decoded, PEAK_BLOCK));
        self.remember(name, &path, Arc::clone(&peaks));
        Ok(peaks)
    }

    /// Binary cache: magic, size, mtime (ns), rate, block, frames, count,
    /// then the mins and maxes (f32 little-endian).
    fn save_peaks(&self, name: &str, stamp: Stamp, p: &WaveformPeaks) -> Result<()> {
        let path = self.peaks_path(name);
        std::fs::create_dir_all(path.parent().expect("has a parent"))?;
        let mut out = Vec::with_capacity(64 + 8 * p.min.len());
        out.extend(PEAKS_MAGIC);
        out.extend(stamp.0.to_le_bytes());
        out.extend(stamp.1.to_le_bytes());
        out.extend(p.sample_rate.to_le_bytes());
        out.extend((p.block as u32).to_le_bytes());
        out.extend(p.frames.to_le_bytes());
        out.extend((p.min.len() as u32).to_le_bytes());
        for v in p.min.iter().chain(&p.max) {
            out.extend(v.to_le_bytes());
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, out)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn load_peaks(&self, name: &str, stamp: Stamp) -> Option<WaveformPeaks> {
        let b = std::fs::read(self.peaks_path(name)).ok()?;
        let u32_at = |i: usize| Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?));
        let u64_at = |i: usize| Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?));
        if b.get(..8)? != PEAKS_MAGIC || u64_at(8)? != stamp.0 || u128::from_le_bytes(b.get(16..32)?.try_into().ok()?) != stamp.1 {
            return None;
        }
        let (sample_rate, block, frames, n) = (u32_at(32)?, u32_at(36)? as usize, u64_at(40)?, u32_at(48)? as usize);
        let data = b.get(52..)?;
        if block == 0 || data.len() != n * 8 {
            return None;
        }
        let floats: Vec<f32> = data.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        Some(WaveformPeaks { block, min: floats[..n].to_vec(), max: floats[n..].to_vec(), sample_rate, frames })
    }
}

#[cfg(test)]
mod tests {
    use super::super::decode::testing::{sine, wav16};
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-media-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_stay_in_the_folder() {
        for ok in ["Mon morceau.mp3", "a-b_c 2.wav", "x.flac", "été.aiff"] {
            assert!(valid_song_name(ok), "{ok}");
        }
        for bad in ["", ".wav", "../x.wav", "a/b.wav", "x.WAV", "x.exe", "x", " x.wav", "x.wav/..", ".peaks", "a.b.wav"] {
            assert!(!valid_song_name(bad), "{bad}");
        }
        assert_eq!(safe_song_name("../../etc/Mon Titre (live).MP3").unwrap(), "Mon Titre _live_.mp3");
        assert_eq!(safe_song_name("C:\\Users\\moi\\a.b.wav").unwrap(), "a_b.wav");
        assert_eq!(safe_song_name("...wav").unwrap(), "__.wav");
        assert_eq!(safe_song_name("().flac").unwrap(), "__.flac");
        assert_eq!(safe_song_name("  .wav").unwrap(), "morceau.wav");
        assert!(safe_song_name("notes.txt").is_err());
        assert!(safe_song_name("sans extension").is_err());
        assert!(valid_song_name(&safe_song_name(&format!("{}.wav", "é".repeat(300))).unwrap()), "long names are cut");
    }

    #[test]
    fn import_checks_the_file_before_writing_and_caches_the_waveform() {
        let dir = temp_dir("import");
        let store = MediaStore::new(&dir);
        let wav = wav16(8_000, 1, &sine(8_000, 1.0, 100.0, 0.5));

        // A damaged file: refused, nothing written.
        let err = store.import("cassé.wav", b"RIFF\x10\0\0\0WAVEjunk").unwrap_err();
        assert!(format!("{err:#}").contains("WAV illisible"), "{err:#}");
        assert!(store.list().is_empty());
        assert!(!store.dir().exists() || std::fs::read_dir(store.dir()).unwrap().next().is_none());

        let got = store.import("../Démo.wav", &wav).unwrap();
        assert_eq!(got, Imported { file: "Démo.wav".into(), duration_s: 1.0, sample_rate: 8_000, channels: 1, existing: false });
        assert!(dir.join("media/audio/Démo.wav").is_file());
        assert_eq!(store.list(), vec![SongInfo { file: "Démo.wav".into(), size: wav.len() as u64 }]);
        // The same bytes again: the same file. Other bytes: « Démo 2 ».
        assert!(store.import("Démo.wav", &wav).unwrap().existing);
        let other = wav16(8_000, 1, &sine(8_000, 0.5, 100.0, 0.25));
        assert_eq!(store.import("Démo.wav", &other).unwrap().file, "Démo 2.wav");
        assert_eq!(store.list().len(), 2, "the cache folder is not listed");

        // Waveform: cached on disk, read back by a fresh store without decoding.
        let p = store.peaks("Démo.wav").unwrap();
        assert!((p.max.iter().copied().fold(0.0, f32::max) - 0.5).abs() < 0.01);
        let fresh = MediaStore::new(&dir);
        assert!(fresh.load_peaks("Démo.wav", MediaStore::stamp(&fresh.path("Démo.wav").unwrap()).unwrap()).is_some());
        assert_eq!(*fresh.peaks("Démo.wav").unwrap(), *p);
        // The file replaced behind our back: decoded again.
        std::fs::write(dir.join("media/audio/Démo.wav"), &other).unwrap();
        let p2 = fresh.peaks("Démo.wav").unwrap();
        assert!((p2.duration_s() - 0.5).abs() < 1e-9);
        assert!(fresh.peaks("nope.wav").is_err());
        assert!(fresh.peaks("../Démo.wav").is_err());
        assert!(store.import("x.wav", &[0u8; 16]).is_err());
    }

    #[test]
    fn a_symlink_is_not_followed() {
        let dir = temp_dir("symlink");
        let store = MediaStore::new(&dir);
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(dir.join("secret.wav"), wav16(8_000, 1, &[0.1; 100])).unwrap();
        std::os::unix::fs::symlink(dir.join("secret.wav"), store.dir().join("lien.wav")).unwrap();
        assert!(store.path("lien.wav").is_err());
        assert!(store.peaks("lien.wav").is_err());
        assert!(store.list().is_empty());
    }
}
