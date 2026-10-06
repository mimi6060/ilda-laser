//! « À propos de Laser Studio » and the release notes.
//!
//! Everything here is compiled in or read from the data folder: the
//! version (Cargo), the build day and commit (`build.rs`), the release
//! notes (`CHANGELOG.md`) and the third-party files shipped with the
//! application (`assets/fonts/SOURCES.txt`). Nothing here talks to the
//! network: there is no update check (a test looks for client code).
//!
//! The last version whose notes the operator has seen is a machine
//! setting, `<data-dir>/prefs.json` → `about.last_seen_version`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const NAME: &str = "Laser Studio";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_DATE: &str = env!("LASER_STUDIO_BUILD_DATE");
/// The commit the application was built from; empty without git.
const GIT_COMMIT: &str = env!("LASER_STUDIO_GIT_COMMIT");

const CHANGELOG_MD: &str = include_str!("../CHANGELOG.md");
const SOURCES_TXT: &str = include_str!("../assets/fonts/SOURCES.txt");

pub const NO_THIRD_PARTY_FR: &str = "Aucun fichier tiers : tout le contenu livré est le nôtre.";
pub const NO_NETWORK_FR: &str = "Laser Studio ne contacte aucun serveur : aucune vérification de mise à jour, aucune donnée envoyée.";
pub const SAFETY_FR: &str = "Laser Studio ne remplace pas l'arrêt matériel (arrêt d'urgence, verrouillage, clé du projecteur) ni les vérifications de sécurité sur place. Avant chaque spectacle : zones contrôlées, distances respectées, arrêt d'urgence testé et à portée de main.";
pub const LIBRARIES_FR: &str = "Le logiciel est construit avec des bibliothèques Rust libres publiées sur crates.io, chacune sous sa propre licence (indiquée avec ses sources).";

/// A third-party file (or group of files) shipped with the application.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ThirdParty {
    pub files: String,
    pub url: String,
    pub licence: String,
    pub checked_on: String,
}

/// One version of the release notes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Release {
    pub version: String,
    pub date: String,
    /// Simple Markdown (`###` titles, `-` lists, paragraphs).
    pub notes_md: String,
}

#[derive(Serialize)]
pub struct AboutInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub commit: Option<&'static str>,
    pub build_date: &'static str,
    pub project_format: u32,
    pub data_dir: PathBuf,
    pub projects_dir: PathBuf,
    pub third_party: Vec<ThirdParty>,
    /// `NO_THIRD_PARTY_FR` when `third_party` is empty.
    pub third_party_note: Option<&'static str>,
    pub libraries: &'static str,
    pub network: &'static str,
    pub safety: &'static str,
    pub changelog: Vec<Release>,
    /// The notes to show once after an update.
    pub unseen: Option<Release>,
}

/// `prefs.json` → `about`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AboutSeen {
    /// "" = never recorded (a new data folder).
    pub last_seen_version: String,
}

/// What the page does with the release notes on load.
#[derive(Debug, PartialEq, Eq)]
pub enum SeenAction {
    /// First launch: record the version, show nothing.
    RecordSilently,
    /// The version changed since the last notes seen: show them.
    Show,
    /// Already seen.
    Nothing,
}

pub fn seen_action(last_seen: &str, current: &str) -> SeenAction {
    match last_seen.trim() {
        "" => SeenAction::RecordSilently,
        v if v == current => SeenAction::Nothing,
        _ => SeenAction::Show,
    }
}

/// The third-party entries of `SOURCES.txt`: a header ending in a blank
/// line (with « Checked on YYYY-MM-DD »), then one entry per line at
/// column 0 (the files), followed by indented `Source:` / `Licence:` /
/// `Note:` fields, continued on more indented lines. An entry without a
/// source URL or a licence (our own files, the licence texts) is skipped.
pub fn parse_sources(txt: &str) -> Vec<ThirdParty> {
    let (header, body) = txt.split_once("\n\n").unwrap_or(("", txt));
    let checked_on = header
        .split_whitespace()
        .skip_while(|w| *w != "on")
        .nth(1)
        .map(|d| d.trim_end_matches('.').to_string())
        .filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-')
        .unwrap_or_default();
    struct Entry {
        files: String,
        fields: Vec<(String, String)>,
    }
    let mut entries: Vec<Entry> = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            entries.push(Entry { files: line.trim().to_string(), fields: Vec::new() });
            continue;
        }
        let Some(e) = entries.last_mut() else { continue };
        let t = line.trim();
        match t.split_once(':').filter(|(k, _)| !k.is_empty() && k.chars().all(char::is_alphabetic)) {
            Some((k, v)) => e.fields.push((k.to_string(), v.trim().to_string())),
            None => {
                if let Some((_, v)) = e.fields.last_mut() {
                    v.push(' ');
                    v.push_str(t);
                }
            }
        }
    }
    entries
        .into_iter()
        .filter_map(|e| {
            let field = |k: &str| e.fields.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
            let source = field("Source")?;
            let url = source.split_whitespace().find(|w| w.starts_with("https://") || w.starts_with("http://"))?.to_string();
            let licence = field("Licence").filter(|l| !l.is_empty())?;
            Some(ThirdParty { files: e.files, url, licence, checked_on: checked_on.clone() })
        })
        .collect()
}

/// The `## x.y.z — YYYY-MM-DD` sections of the release notes, in file
/// order (newest first). A `##` line that does not parse is skipped with
/// its text.
pub fn parse_changelog(md: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    let mut current: Option<Release> = None;
    for line in md.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            if let Some(r) = current.take() {
                out.push(r);
            }
            let mut parts = head.split(['—', '–']).map(str::trim);
            let (version, date) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let version_ok = !version.is_empty() && version.split('.').count() == 3 && version.split('.').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            let date_ok = date.len() == 10 && date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
            if version_ok && date_ok {
                current = Some(Release { version: version.into(), date: date.into(), notes_md: String::new() });
            }
            continue;
        }
        if let Some(r) = current.as_mut() {
            r.notes_md.push_str(line);
            r.notes_md.push('\n');
        }
    }
    out.extend(current);
    for r in &mut out {
        r.notes_md = r.notes_md.trim().to_string();
    }
    out
}

fn changelog() -> &'static [Release] {
    static C: OnceLock<Vec<Release>> = OnceLock::new();
    C.get_or_init(|| parse_changelog(CHANGELOG_MD))
}

fn third_party() -> &'static [ThirdParty] {
    static T: OnceLock<Vec<ThirdParty>> = OnceLock::new();
    T.get_or_init(|| parse_sources(SOURCES_TXT))
}

/// The notes of the running version (an empty note if it has no section,
/// which a test forbids).
fn current_release() -> Release {
    changelog().iter().find(|r| r.version == VERSION).cloned().unwrap_or(Release { version: VERSION.into(), date: BUILD_DATE.into(), notes_md: String::new() })
}

fn prefs_path(data_dir: &Path) -> PathBuf {
    data_dir.join("prefs.json")
}

pub fn load_seen(data_dir: &Path) -> AboutSeen {
    let prefs: Map<String, Value> = crate::load_json(&prefs_path(data_dir));
    prefs.get("about").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// Records the running version as seen; other preferences are kept.
pub fn mark_seen(data_dir: &Path) -> anyhow::Result<()> {
    let path = prefs_path(data_dir);
    let mut prefs: Map<String, Value> = crate::load_json(&path);
    prefs.insert("about".into(), json!(AboutSeen { last_seen_version: VERSION.into() }));
    crate::datafile::write_atomic(&path, &serde_json::to_vec_pretty(&prefs)?)
}

/// The commit, `None` when the build had no git.
pub fn commit() -> Option<&'static str> {
    Some(GIT_COMMIT).filter(|c| !c.is_empty())
}

/// `GET /api/about`. On a new data folder the version is recorded here
/// without showing anything (first launch); after an update, `unseen`
/// carries the new notes until `POST /api/about/seen`.
pub fn info(data_dir: &Path, projects_dir: &Path) -> AboutInfo {
    let seen = load_seen(data_dir);
    let unseen = match seen_action(&seen.last_seen_version, VERSION) {
        SeenAction::RecordSilently => {
            if let Err(e) = mark_seen(data_dir) {
                log::warn!("version vue non enregistrée : {e:#}");
            }
            None
        }
        SeenAction::Show => Some(current_release()),
        SeenAction::Nothing => None,
    };
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let third_party = third_party().to_vec();
    AboutInfo {
        name: NAME,
        version: VERSION,
        commit: commit(),
        build_date: BUILD_DATE,
        project_format: crate::project::PROJECT_FORMAT,
        data_dir: absolute(data_dir),
        projects_dir: absolute(projects_dir),
        third_party_note: third_party.is_empty().then_some(NO_THIRD_PARTY_FR),
        third_party,
        libraries: LIBRARIES_FR,
        network: NO_NETWORK_FR,
        safety: SAFETY_FR,
        changelog: changelog().to_vec(),
        unseen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-about-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sources_empty_list_gives_no_entry() {
        assert!(parse_sources("").is_empty());
        assert!(parse_sources("Header. Checked on 2026-01-01.\n\nSOURCES.txt\n  This list.\n").is_empty());
    }

    #[test]
    fn sources_one_entry_with_continuations() {
        let txt = "Files. Checked on 2026-10-05.\n\nfont/a.jhf, font/b.jhf\n  Source:  https://example.org/fonts (data)\n  Licence: Free use with the\n           acknowledgement.\n  Note:    unmodified.\n";
        let v = parse_sources(txt);
        assert_eq!(
            v,
            [ThirdParty { files: "font/a.jhf, font/b.jhf".into(), url: "https://example.org/fonts".into(), licence: "Free use with the acknowledgement.".into(), checked_on: "2026-10-05".into() }]
        );
    }

    #[test]
    fn sources_malformed_entries_are_skipped() {
        let txt = "Head.\n\n  orphan field: x\nno-licence.txt\n  Source: https://example.org\nno-url.txt\n  Source: ours\n  Licence: MIT\nok.txt\n  Source: https://ok.example\n  Licence: OFL-1.1\n";
        let v = parse_sources(txt);
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].files.as_str(), v[0].licence.as_str(), v[0].checked_on.as_str()), ("ok.txt", "OFL-1.1", ""));
    }

    #[test]
    fn shipped_sources_list_the_fonts() {
        let v = third_party();
        assert!(v.iter().any(|t| t.files.contains("rowmans.jhf") && t.licence.contains("Hershey")), "{v:?}");
        assert!(v.iter().any(|t| t.files.contains("Relief") && t.licence.contains("Open Font License")), "{v:?}");
        assert!(v.iter().all(|t| t.url.starts_with("https://") && !t.checked_on.is_empty()));
    }

    #[test]
    fn changelog_parses_sections() {
        let md = "# Titre\nintro\n\n## 0.2.0 — 2026-11-01\n\n### Nouveau\n- un\n- deux\n\n## pas une version\nignoré\n## 0.1.0 - 2026-10-06\n## 0.1.0 — 2026-10-06\nPremière.\n";
        let v = parse_changelog(md);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].version.as_str(), v[0].date.as_str()), ("0.2.0", "2026-11-01"));
        assert_eq!(v[0].notes_md, "### Nouveau\n- un\n- deux");
        assert_eq!((v[1].version.as_str(), v[1].notes_md.as_str()), ("0.1.0", "Première."));
    }

    /// Every version has its notes: bumping Cargo.toml without a section
    /// in CHANGELOG.md fails here.
    #[test]
    fn current_version_has_release_notes() {
        let r = changelog().iter().find(|r| r.version == VERSION);
        assert!(r.is_some_and(|r| !r.notes_md.is_empty()), "CHANGELOG.md has no « ## {VERSION} — AAAA-MM-JJ » section");
        assert_eq!(changelog().first().map(|r| r.version.as_str()), Some(VERSION), "the newest section comes first");
    }

    /// Shipped text is product text only: no internal references.
    #[test]
    fn changelog_is_user_facing() {
        for bad in ["T-", "TODO", "PR ", "agent", "Claude", "tasks/", "docs/"] {
            assert!(!CHANGELOG_MD.contains(bad), "CHANGELOG.md contains « {bad} »");
        }
    }

    #[test]
    fn seen_logic() {
        assert_eq!(seen_action("", "0.1.0"), SeenAction::RecordSilently);
        assert_eq!(seen_action("0.1.0", "0.1.0"), SeenAction::Nothing);
        assert_eq!(seen_action("0.0.9", "0.1.0"), SeenAction::Show);
    }

    #[test]
    fn first_launch_records_silently_then_update_shows_once() {
        let dir = temp_dir("seen");
        std::fs::write(dir.join("prefs.json"), r#"{"autosave":{"delay_s":60}}"#).unwrap();
        let first = info(&dir, &dir.join("projects"));
        assert!(first.unseen.is_none(), "nothing on a first launch");
        assert_eq!(load_seen(&dir).last_seen_version, VERSION);
        let prefs: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("prefs.json")).unwrap()).unwrap();
        assert_eq!(prefs["autosave"]["delay_s"], 60, "other preferences kept");

        std::fs::write(dir.join("prefs.json"), r#"{"about":{"last_seen_version":"0.0.1"}}"#).unwrap();
        let after_update = info(&dir, &dir.join("projects"));
        assert_eq!(after_update.unseen.as_ref().map(|r| r.version.as_str()), Some(VERSION));
        assert!(info(&dir, &dir).unseen.is_some(), "shown until closed");
        mark_seen(&dir).unwrap();
        assert!(info(&dir, &dir).unseen.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_info_is_well_formed() {
        let b = BUILD_DATE.as_bytes();
        assert!(b.len() == 10 && b[4] == b'-' && b[7] == b'-', "{BUILD_DATE}");
        assert!(commit().is_none_or(|c| c.chars().all(|c| c.is_ascii_hexdigit())));
        let i = info(&temp_dir("info"), Path::new("projects"));
        assert!(i.projects_dir.is_absolute());
        assert_eq!(i.third_party_note, None, "fonts are shipped");
        assert!(i.safety.contains("ne remplace pas l'arrêt matériel"));
    }

    /// No update check, no client code: this module never opens a
    /// connection (the patterns are split so this test does not match
    /// itself).
    #[test]
    fn no_network_client() {
        let src = include_str!("about.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        for p in [["Tcp", "Stream"], ["Udp", "Socket"], ["req", "west"], ["ure", "q::"], ["hyp", "er::"], ["Command", "::new"], ["std::", "net"], ["to_socket", "_addrs"]] {
            let p = p.concat();
            assert!(!code.contains(&p), "about.rs must not use {p}");
        }
    }
}
