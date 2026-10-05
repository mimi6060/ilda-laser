//! Named project versions (T-288): *Enregistrer une version…* keeps the
//! show as it is now under a free name, in
//! `<data-dir>/versions/<projet>/<AAAAMMJJ-HHMM>-<nom>.lsproj` (UTC, so
//! the order never jumps with daylight saving time; the page shows local
//! time). A version is an ordinary project file: *Ouvrir comme copie*
//! opens it as a new unsaved project, never over the one it came from.
//!
//! Only plain file names directly in the project's versions folder are
//! read or deleted, whatever the page sends.

use crate::project::{civil, EXTENSION};
use crate::timeline::valid_show_name;
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const DIR: &str = "versions";

/// One version, for the list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VersionInfo {
    /// File name in the versions folder (what the page sends back).
    pub file: String,
    /// The name the user gave it.
    pub name: String,
    /// When it was saved (ms since 1970, UTC; the page shows local time).
    pub at_ms: u64,
}

/// `<data-dir>/versions/<projet>`.
pub fn dir_of(data_dir: &Path, project: &str) -> PathBuf {
    data_dir.join(DIR).join(project)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `AAAAMMJJ-HHMM`, UTC.
fn stamp(secs: u64) -> String {
    let (y, m, d, hh, mm, _) = civil(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}")
}

/// Seconds since 1970 of an `AAAAMMJJ-HHMM` stamp (UTC).
fn parse_stamp(stamp: &str) -> Option<u64> {
    let b = stamp.as_bytes();
    if b.len() != 13 || b[8] != b'-' || !b.iter().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| stamp[r].parse::<i64>().ok();
    let (y, m, d, hh, mm) = (n(0..4)?, n(4..6)?, n(6..8)?, n(9..11)?, n(11..13)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 {
        return None;
    }
    // Days from civil (H. Hinnant), the inverse of `project::civil`.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60).ok()
}

/// `<stamp>-<name>` → (stamp seconds, name).
fn split(stem: &str) -> Option<(u64, &str)> {
    let at = parse_stamp(stem.get(..13)?)?;
    let name = stem.get(13..)?.strip_prefix('-')?;
    valid_show_name(name).then_some((at, name))
}

/// A free path for version `name` saved at `secs`: `<stamp>-<name>.lsproj`,
/// then `<name> 2`, `<name> 3`… within the same minute.
pub fn new_path(dir: &Path, secs: u64, name: &str) -> PathBuf {
    let stamp = stamp(secs);
    (1..)
        .map(|n| if n == 1 { name.to_string() } else { format!("{name} {n}") })
        .map(|n| dir.join(format!("{stamp}-{n}.{EXTENSION}")))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .expect("some name is free")
}

/// The versions in `dir`, newest first (other files skipped).
pub fn list(dir: &Path) -> Vec<VersionInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<VersionInfo> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == EXTENSION) && p.is_file())
        .filter_map(|p| {
            let file = p.file_name()?.to_str()?.to_string();
            let (at, name) = split(p.file_stem()?.to_str()?)?;
            Some(VersionInfo { name: name.to_string(), file, at_ms: at * 1000 })
        })
        .collect();
    out.sort_by(|a, b| (b.at_ms, &b.name).cmp(&(a.at_ms, &a.name)));
    out
}

/// The path of version `file` (a bare file name from `list`) and its name.
pub fn resolve(dir: &Path, file: &str) -> Result<(PathBuf, String)> {
    let refused = || anyhow::anyhow!("version inconnue : « {file} »");
    if file.contains(['/', '\\']) || file.starts_with('.') {
        return Err(refused());
    }
    let stem = file.strip_suffix(&format!(".{EXTENSION}")).ok_or_else(refused)?;
    let (_, name) = split(stem).ok_or_else(refused)?;
    Ok((dir.join(file), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_round_trip_in_utc() {
        // 2026-10-05 21:04 UTC.
        let secs = 1_791_234_240;
        let st = stamp(secs);
        assert_eq!(st.len(), 13);
        assert_eq!(parse_stamp(&st), Some(secs - secs % 60));
        assert_eq!(parse_stamp("20260229-1200").map(stamp).as_deref(), Some("20260301-1200"), "no 29 Feb 2026: next day");
        assert_eq!(parse_stamp("20240229-2359").map(stamp).as_deref(), Some("20240229-2359"));
        for bad in ["2026100-2104", "20261005_2104", "20261305-2104", "20261005-2504", "abcdefgh-ijkl"] {
            assert_eq!(parse_stamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn names_list_and_resolve_only_inside_the_folder() {
        let dir = std::env::temp_dir().join(format!("laser-studio-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = 1_791_234_240;
        let a = new_path(&dir, t, "avant festival");
        std::fs::write(&a, "{}").unwrap();
        let b = new_path(&dir, t, "avant festival");
        assert!(b.ends_with(format!("{}-avant festival 2.lsproj", stamp(t))), "{b:?}");
        std::fs::write(&b, "{}").unwrap();
        std::fs::write(new_path(&dir, t + 3600, "version courte"), "{}").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("pas-une-version.lsproj"), "{}").unwrap();
        let l = list(&dir);
        assert_eq!(l.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["version courte", "avant festival 2", "avant festival"]);
        assert_eq!(l[0].at_ms, (t + 3600 - t % 60) * 1000);
        let (path, name) = resolve(&dir, &l[2].file).unwrap();
        assert_eq!((path, name.as_str()), (a, "avant festival"));
        for bad in ["../x.lsproj", "/etc/passwd", "pas-une-version.lsproj", "notes.txt", ".20261005-2104-x.lsproj", "20261005-2104-a/b.lsproj"] {
            assert!(resolve(&dir, bad).is_err(), "{bad}");
        }
        assert!(list(&dir.join("absent")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
