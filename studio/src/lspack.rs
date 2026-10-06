//! The export package `.lspack` (T-289): a show taken elsewhere with its
//! media. A zip holding
//!
//! - `manifest.json`: the package format, the project's name, the files
//!   and the SHA-256 and size of every media file;
//! - `project.lsproj`: the project, as *Enregistrer* writes it;
//! - `site.lssite` (optional): the site profile in force (calibration,
//!   safety, projection zones; never the output addresses of this Mac);
//! - `media/ilda/<fichier>.ild` and `media/audio/<morceau>`: the user's own
//!   files the project refers to (cues `ilda:<fichier>`, timeline songs).
//!
//! Nothing else goes in: no log, no absolute path of this Mac, no machine
//! setting but the optional site. The media are the user's: whoever
//! exports or imports a package is responsible for the rights on them
//! (the page says so), and packages never belong in the repository.
//!
//! A package is untrusted input. *Importer un paquet…* reads it whole in
//! memory and checks everything before writing anything:
//! - at most `MAX_PACK_BYTES`, `MAX_ENTRIES` entries, `MAX_TOTAL_BYTES`
//!   once decompressed, each file within its own kind's limit (read
//!   through a bounded reader: the sizes the zip declares are not trusted);
//! - every entry name is one of the expected relative paths: an absolute
//!   path, `..`, a backslash, a link or any other entry refuses the whole
//!   package (no « zip slip »: nothing is ever written from an entry name);
//! - a newer package or project format is refused, saying so;
//! - every media file must match the manifest (size and SHA-256), every
//!   ILDA file must parse, every song decode (in isolation), the project
//!   and the site go through their normal checks and sanitizers.
//!
//! Then the media go into the libraries (`media/ilda/`, `media/audio/`)
//! through their own imports (identical files are shared, another file of
//! the same name is renamed « nom 2 » and the project's references are
//! rewritten to follow), and the project is written atomically as a new
//! file of `projects/` (never over an existing one). A failure part-way
//! removes the media this import added. An embedded site is kept with the
//! project, never applied: opening the project offers it (sites.rs).
//! Importing never arms, never changes a safety setting.

use crate::audio::media::{valid_song_name, MediaStore};
use crate::ilda_library::{self, IldaLibrary};
use crate::project::{self, Project};
use crate::sites::{self, SiteProfile};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// Current package format.
pub const PACK_FORMAT: u32 = 1;
pub const EXTENSION: &str = "lspack";
pub const MANIFEST: &str = "manifest.json";
pub const PROJECT_FILE: &str = "project.lsproj";
pub const SITE_FILE: &str = "site.lssite";
const ILDA_DIR: &str = "media/ilda/";
const AUDIO_DIR: &str = "media/audio/";
/// Largest package accepted (as sent).
pub const MAX_PACK_BYTES: u64 = 1 << 30;
/// Largest total once decompressed.
pub const MAX_TOTAL_BYTES: u64 = 2 << 30;
/// Most entries in a package.
pub const MAX_ENTRIES: usize = 2_000;
/// Largest manifest, project or site file.
const MAX_JSON_BYTES: u64 = 64 << 20;

/// `manifest.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PackManifest {
    pub format_version: u32,
    pub app_version: String,
    /// The project's name (a suggestion for the imported project's).
    pub name: String,
    /// The project file in the package.
    pub project: String,
    /// The site profile file, if one is included.
    pub site: Option<String>,
    pub media: Vec<MediaEntry>,
    /// Media the project names that were not in the library (not packed).
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaEntry {
    /// `media/ilda/<fichier>` or `media/audio/<fichier>`.
    pub path: String,
    /// Lower-case hex.
    pub sha256: String,
    pub bytes: u64,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Ilda,
    Audio,
}

impl Kind {
    fn of(path: &str) -> Option<(Kind, &str)> {
        if let Some(f) = path.strip_prefix(ILDA_DIR) {
            return ilda_library::valid_file_name(f).then_some((Kind::Ilda, f));
        }
        if let Some(f) = path.strip_prefix(AUDIO_DIR) {
            return valid_song_name(f).then_some((Kind::Audio, f));
        }
        None
    }

    fn limit(self) -> u64 {
        match self {
            Kind::Ilda => crate::ilda::MAX_READ_BYTES,
            Kind::Audio => crate::audio::media::MAX_IMPORT_BYTES,
        }
    }
}

/// The media a project's JSON names: the ILDA files (`ilda:<fichier>` cue
/// ids, `"file": "<fichier>.ild"` looks) and the songs (`"file"` of a
/// timeline's audio). Only valid library names.
pub fn referenced_media(v: &Value) -> (BTreeSet<String>, BTreeSet<String>) {
    fn walk(v: &Value, ilda: &mut BTreeSet<String>, audio: &mut BTreeSet<String>) {
        match v {
            Value::String(s) => {
                if let Some(f) = s.strip_prefix(ilda_library::ID_PREFIX).filter(|f| ilda_library::valid_file_name(f)) {
                    ilda.insert(f.to_string());
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, ilda, audio)),
            Value::Object(o) => {
                for (k, x) in o {
                    if let (true, Some(f)) = (k == "file", x.as_str()) {
                        if ilda_library::valid_file_name(f) {
                            ilda.insert(f.to_string());
                        } else if valid_song_name(f) {
                            audio.insert(f.to_string());
                        }
                    }
                    walk(x, ilda, audio);
                }
            }
            _ => {}
        }
    }
    let (mut ilda, mut audio) = (BTreeSet::new(), BTreeSet::new());
    walk(v, &mut ilda, &mut audio);
    (ilda, audio)
}

/// Reads a library file for a package: a plain file (never a link) of
/// sensible size, or `None` when it is not there.
fn read_media(dir: &Path, name: &str, limit: u64) -> Result<Option<Vec<u8>>> {
    let path = dir.join(name);
    let Ok(meta) = std::fs::symlink_metadata(&path) else { return Ok(None) };
    if !meta.is_file() {
        return Ok(None);
    }
    if meta.len() > limit {
        bail!("« {name} » est trop gros pour un paquet ({} Mo au plus)", limit >> 20);
    }
    Ok(Some(std::fs::read(&path).with_context(|| format!("« {name} » illisible"))?))
}

/// Builds a package of `project` (as *Enregistrer* would write it), with
/// `site` if given and the media it names found in `ilda_dir` and
/// `audio_dir`. Returns the zip's bytes and its manifest.
pub fn build(project: &Project, site: Option<&SiteProfile>, ilda_dir: &Path, audio_dir: &Path) -> Result<(Vec<u8>, PackManifest)> {
    let mut project = project.clone();
    // A site travels as its own file, only when asked for.
    project.extra.remove(sites::PROJECT_KEY);
    let project_bytes = project::project_bytes(&project)?;
    let (ilda, audio) = referenced_media(&serde_json::to_value(&project)?);
    let mut manifest = PackManifest {
        format_version: PACK_FORMAT,
        app_version: env!("CARGO_PKG_VERSION").into(),
        name: project.name.clone(),
        project: PROJECT_FILE.into(),
        site: site.map(|_| SITE_FILE.to_string()),
        ..Default::default()
    };
    let mut media: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = project_bytes.len() as u64;
    for (kind, dir, names) in [(Kind::Ilda, ilda_dir, &ilda), (Kind::Audio, audio_dir, &audio)] {
        for name in names {
            let prefix = if kind == Kind::Ilda { ILDA_DIR } else { AUDIO_DIR };
            match read_media(dir, name, kind.limit())? {
                Some(bytes) => {
                    total += bytes.len() as u64;
                    if total > MAX_PACK_BYTES {
                        bail!("paquet trop gros ({} Mo au plus)", MAX_PACK_BYTES >> 20);
                    }
                    manifest.media.push(MediaEntry { path: format!("{prefix}{name}"), sha256: sha256_hex(&bytes), bytes: bytes.len() as u64 });
                    media.push((format!("{prefix}{name}"), bytes));
                }
                None => manifest.missing.push(name.clone()),
            }
        }
    }
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o644);
    let stored = deflated.compression_method(zip::CompressionMethod::Stored);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut put = |name: &str, bytes: &[u8], compress: bool| -> Result<()> {
        zip.start_file(name, if compress { deflated } else { stored })?;
        zip.write_all(bytes)?;
        Ok(())
    };
    put(MANIFEST, &serde_json::to_vec_pretty(&manifest)?, true)?;
    put(PROJECT_FILE, &project_bytes, true)?;
    if let Some(site) = site {
        let mut site = site.clone();
        // The output addresses belong to this Mac and its network.
        site.outputs = None;
        put(SITE_FILE, &sites::bytes_of(&site)?, true)?;
    }
    for (path, bytes) in &media {
        // Songs are compressed already.
        put(path, bytes, path.starts_with(ILDA_DIR))?;
    }
    let bytes = zip.finish()?.into_inner();
    Ok((bytes, manifest))
}

/// A package read and checked, nothing written yet.
#[derive(Debug)]
pub struct Unpacked {
    pub manifest: PackManifest,
    /// The project's JSON, checked to parse (`project::parse`).
    pub project: Value,
    pub site: Option<SiteProfile>,
    /// (kind, file name, bytes), checked against the manifest.
    media: Vec<(Kind, String, Vec<u8>)>,
}

/// Whether `name` is a safe relative path made of plain components: no
/// absolute path, drive, backslash, `.` or `..`, no empty component.
fn safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.contains('\0')
        && name.trim_end_matches('/').split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// Reads and checks a whole package: see the module docs.
pub fn unpack(bytes: &[u8]) -> Result<Unpacked> {
    if bytes.len() as u64 > MAX_PACK_BYTES {
        bail!("paquet trop gros ({} Mo au plus)", MAX_PACK_BYTES >> 20);
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("paquet illisible (ce n'est pas un fichier .lspack valide : {e})"))?;
    if zip.len() > MAX_ENTRIES {
        bail!("paquet refusé : trop d'éléments ({MAX_ENTRIES} au plus)");
    }
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| anyhow::anyhow!("paquet illisible (élément {i} : {e})"))?;
        let name = entry.name().to_string();
        if !safe_entry_name(&name) || entry.enclosed_name().is_none() {
            bail!("paquet refusé : chemin interdit « {name} »");
        }
        if entry.encrypted() {
            bail!("paquet refusé : élément chiffré « {name} »");
        }
        // Links and special files: only plain files and folders.
        let unix_type = entry.unix_mode().map(|m| m & 0o170000).unwrap_or(0);
        if entry.is_symlink() || ![0, 0o100000, 0o040000].contains(&unix_type) {
            bail!("paquet refusé : « {name} » n'est pas un fichier ordinaire (lien ?)");
        }
        if entry.is_dir() {
            if !["media/", ILDA_DIR, AUDIO_DIR].contains(&name.as_str()) {
                bail!("paquet refusé : dossier inattendu « {name} »");
            }
            continue;
        }
        let limit = match name.as_str() {
            MANIFEST | PROJECT_FILE | SITE_FILE => MAX_JSON_BYTES,
            other => match Kind::of(other) {
                Some((kind, _)) => kind.limit(),
                None => bail!("paquet refusé : élément inattendu « {name} »"),
            },
        };
        if entry.size() > limit {
            bail!("paquet refusé : « {name} » trop gros ({} Mo au plus)", limit >> 20);
        }
        // Never trust the declared size: read at most one byte more.
        let mut data = Vec::new();
        (&mut entry).take(limit + 1).read_to_end(&mut data).map_err(|e| anyhow::anyhow!("paquet illisible (« {name} » : {e})"))?;
        if data.len() as u64 > limit {
            bail!("paquet refusé : « {name} » trop gros ({} Mo au plus)", limit >> 20);
        }
        total += data.len() as u64;
        if total > MAX_TOTAL_BYTES {
            bail!("paquet refusé : trop gros une fois décompressé ({} Mo au plus)", MAX_TOTAL_BYTES >> 20);
        }
        if files.insert(name.clone(), data).is_some() {
            bail!("paquet refusé : « {name} » en double");
        }
    }

    let manifest_bytes = files.remove(MANIFEST).ok_or_else(|| anyhow::anyhow!("paquet refusé : {MANIFEST} manquant"))?;
    let manifest_value: Value = serde_json::from_slice(&manifest_bytes).map_err(|e| anyhow::anyhow!("paquet illisible ({MANIFEST} : {e})"))?;
    match manifest_value.get("format_version").and_then(Value::as_u64) {
        None | Some(0) => bail!("paquet illisible : « format_version » manquant ou invalide"),
        Some(v) if v > PACK_FORMAT as u64 => {
            bail!("paquet créé par une version plus récente de Laser Studio (format {v}, cette version lit le format {PACK_FORMAT}) : mettez Laser Studio à jour pour l'importer")
        }
        _ => {}
    }
    let manifest: PackManifest = serde_json::from_value(manifest_value).map_err(|e| anyhow::anyhow!("paquet illisible ({MANIFEST} : {e})"))?;
    if manifest.project != PROJECT_FILE {
        bail!("paquet refusé : projet inattendu « {} »", manifest.project);
    }
    if manifest.site.as_deref().is_some_and(|s| s != SITE_FILE) {
        bail!("paquet refusé : profil de site inattendu");
    }
    let project_bytes = files.remove(PROJECT_FILE).ok_or_else(|| anyhow::anyhow!("paquet refusé : {PROJECT_FILE} manquant"))?;
    let project_text = String::from_utf8(project_bytes).map_err(|_| anyhow::anyhow!("paquet illisible : projet non UTF-8"))?;
    // Every check of *Ouvrir* (a newer format refused, with its message).
    project::parse(&project_text)?;
    let project: Value = serde_json::from_str(&project_text)?;
    let site = match (manifest.site.is_some(), files.remove(SITE_FILE)) {
        (true, Some(b)) => Some(sites::parse(&b)?),
        (false, None) => None,
        (true, None) => bail!("paquet refusé : {SITE_FILE} manquant"),
        (false, Some(_)) => bail!("paquet refusé : {SITE_FILE} absent du manifeste"),
    };

    // The media: exactly those of the manifest, with their fingerprints.
    let mut listed = BTreeSet::new();
    for m in &manifest.media {
        if !listed.insert(m.path.as_str()) {
            bail!("paquet refusé : « {} » en double dans le manifeste", m.path);
        }
    }
    let mut media = Vec::new();
    for (path, data) in files {
        let Some((kind, name)) = Kind::of(&path) else { bail!("paquet refusé : élément inattendu « {path} »") };
        let entry = manifest.media.iter().find(|m| m.path == path).ok_or_else(|| anyhow::anyhow!("paquet refusé : « {path} » absent du manifeste"))?;
        if entry.bytes != data.len() as u64 || !entry.sha256.eq_ignore_ascii_case(&sha256_hex(&data)) {
            bail!("paquet refusé : l'empreinte de « {path} » ne correspond pas (fichier abîmé ou modifié)");
        }
        media.push((kind, name.to_string(), data));
    }
    if media.len() != manifest.media.len() {
        let missing: Vec<&str> = manifest.media.iter().map(|m| m.path.as_str()).filter(|p| !media.iter().any(|(_, n, _)| p.ends_with(&format!("/{n}")))).collect();
        bail!("paquet refusé : média manquant ({})", missing.join(", "));
    }
    Ok(Unpacked { manifest, project, site, media })
}

/// Where an import writes.
pub struct Targets<'a> {
    pub projects_dir: &'a Path,
    pub ilda: &'a IldaLibrary,
    pub audio: &'a MediaStore,
}

/// What an import did (`POST /api/pack/import`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct Installed {
    /// The new project's name, in `projects/`.
    pub project: String,
    /// Library names of the media (after renaming).
    pub media: Vec<String>,
    /// Media already in the library with the same content.
    pub existing: Vec<String>,
    /// (name in the package, name in the library).
    pub renamed: Vec<(String, String)>,
    /// The site profile carried by the project, if any (not applied).
    pub site: Option<String>,
    /// Media the exporting project named but did not hold.
    pub missing: Vec<String>,
}

/// A valid project name from `wanted`: other characters become `_`.
fn clean_name(wanted: &str) -> String {
    let s: String = wanted.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).take(56).collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "Paquet importé".into()
    } else {
        s
    }
}

/// The first `name`, `name 2`… not used in `dir`.
fn free_project_name(dir: &Path, wanted: &str) -> Result<String> {
    let base = clean_name(wanted);
    for n in 1..1000 {
        let name = if n == 1 { base.clone() } else { format!("{base} {n}") };
        if std::fs::symlink_metadata(dir.join(format!("{name}.{}", project::EXTENSION))).is_err() {
            return Ok(name);
        }
    }
    bail!("trop de projets nommés « {base} »")
}

/// Replaces every reference to a renamed media file in a project's JSON.
fn rewrite(v: &mut Value, ilda: &BTreeMap<String, String>, audio: &BTreeMap<String, String>) {
    match v {
        Value::String(s) => {
            if let Some(new) = s.strip_prefix(ilda_library::ID_PREFIX).and_then(|f| ilda.get(f)) {
                *s = ilda_library::cue_id(new);
            } else if let Some(new) = ilda.get(s.as_str()).or_else(|| audio.get(s.as_str())) {
                *s = new.clone();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rewrite(x, ilda, audio)),
        Value::Object(o) => o.values_mut().for_each(|x| rewrite(x, ilda, audio)),
        _ => {}
    }
}

/// Writes a checked package: media into the libraries, then the project
/// as a new file of `projects/` named after `wanted` (or the package's
/// project). All or nothing as far as this import goes: on an error the
/// media it added are removed and no project is written.
pub fn install(pack: Unpacked, wanted: Option<&str>, t: &Targets) -> Result<Installed> {
    // ILDA files parse before anything is written (songs decode in their import).
    for (kind, name, data) in &pack.media {
        if *kind == Kind::Ilda {
            crate::ilda::read(data).with_context(|| format!("« {name} » : fichier ILDA abîmé"))?;
        }
    }
    let mut added: Vec<(Kind, String)> = Vec::new();
    let undo = |added: &[(Kind, String)]| {
        for (kind, file) in added {
            let _ = match kind {
                Kind::Ilda => t.ilda.remove(file),
                Kind::Audio => std::fs::remove_file(t.audio.dir().join(file)).map_err(anyhow::Error::from),
            };
        }
    };
    let mut out = Installed { missing: pack.manifest.missing.clone(), ..Default::default() };
    let (mut ilda_map, mut audio_map) = (BTreeMap::new(), BTreeMap::new());
    for (kind, name, data) in &pack.media {
        let imported = match kind {
            Kind::Ilda => t.ilda.import(name, data).map(|i| (i.info.file, i.existing)),
            Kind::Audio => t.audio.import(name, data).map(|i| (i.file, i.existing)),
        };
        let (file, existing) = match imported {
            Ok(x) => x,
            Err(e) => {
                undo(&added);
                return Err(e.context(format!("« {name} » refusé")));
            }
        };
        if existing {
            out.existing.push(file.clone());
        } else {
            added.push((*kind, file.clone()));
        }
        if file != *name {
            out.renamed.push((name.clone(), file.clone()));
            match kind {
                Kind::Ilda => ilda_map.insert(name.clone(), file.clone()),
                Kind::Audio => audio_map.insert(name.clone(), file.clone()),
            };
        }
        out.media.push(file);
    }
    let result = (|| -> Result<String> {
        let mut v = pack.project.clone();
        rewrite(&mut v, &ilda_map, &audio_map);
        if let (Some(site), Some(o)) = (&pack.site, v.as_object_mut()) {
            o.insert(sites::PROJECT_KEY.into(), serde_json::to_value(site)?);
        }
        let mut p = project::parse(&serde_json::to_string(&v)?)?;
        std::fs::create_dir_all(t.projects_dir).with_context(|| format!("impossible de créer {}", t.projects_dir.display()))?;
        let name = free_project_name(t.projects_dir, wanted.filter(|w| !w.trim().is_empty()).unwrap_or(&pack.manifest.name))?;
        p.name = name.clone();
        let path: PathBuf = t.projects_dir.join(format!("{name}.{}", project::EXTENSION));
        crate::datafile::write_atomic(&path, &project::project_bytes(&p)?)?;
        Ok(name)
    })();
    match result {
        Ok(name) => {
            out.project = name;
            out.site = pack.site.map(|s| s.name);
            Ok(out)
        }
        Err(e) => {
            undo(&added);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Content, Settings};
    use crate::ilda::IldaFrame;
    use crate::patterns::Point;
    use crate::scenes::Scene;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-lspack-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ilda_bytes(size: f32) -> Vec<u8> {
        let pts = vec![Point::blanked(-size, -size), Point::lit(-size, -size, 1.0, 0.0, 0.0), Point::lit(size, size, 0.0, 1.0, 0.0)];
        crate::ilda::write_format5(&[IldaFrame::new("f0", pts)]).unwrap()
    }

    fn wav() -> Vec<u8> {
        crate::audio::decode::testing::wav16(8_000, 1, &crate::audio::decode::testing::sine(8_000, 0.2, 440.0, 0.5))
    }

    /// A data dir with an ILDA file and a song, and a project naming them.
    fn source(tag: &str) -> (PathBuf, Project, String) {
        let dir = temp_dir(tag);
        let ilda = format!("logo {tag}.ild");
        let lib = IldaLibrary::new(&dir);
        lib.import(&ilda, &ilda_bytes(0.5)).unwrap();
        MediaStore::new(&dir).import("intro.wav", &wav()).unwrap();
        let look = Settings { content: Content::Ilda { file: ilda.clone(), fps: 25.0 }, ..Default::default() };
        let show = crate::timeline::Show { name: "Show".into(), audio: Some(crate::timeline::AudioRef { file: "intro.wav".into(), ..Default::default() }), ..Default::default() };
        let p = Project { name: "Mon show".into(), scenes: vec![Scene { name: "Logo".into(), settings: look, duration_secs: 8.0 }], timelines: vec![show], ..Project::default() };
        (dir, p, ilda)
    }

    fn targets(dir: &Path) -> (PathBuf, IldaLibrary, MediaStore) {
        (dir.join("projects"), IldaLibrary::new(dir), MediaStore::new(dir))
    }

    fn rezip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut f = zip.by_index(i).unwrap();
                let mut d = Vec::new();
                f.read_to_end(&mut d).unwrap();
                (f.name().to_string(), d)
            })
            .collect()
    }

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn referenced_media_finds_cues_looks_and_songs() {
        let v = serde_json::json!({ "a": "ilda:x.ild", "b": { "file": "y.ild" }, "c": [{ "file": "s.mp3" }], "d": "ilda:../bad.ild", "e": { "file": "../z.wav" } });
        let (ilda, audio) = referenced_media(&v);
        assert_eq!(ilda.into_iter().collect::<Vec<_>>(), ["x.ild", "y.ild"]);
        assert_eq!(audio.into_iter().collect::<Vec<_>>(), ["s.mp3"]);
    }

    #[test]
    fn export_then_import_on_an_empty_data_dir_gives_the_same_project_and_media() {
        let (src, p, ilda) = source("roundtrip");
        let (bytes, manifest) = build(&p, None, &src.join("media/ilda"), &src.join("media/audio")).unwrap();
        assert_eq!(manifest.media.len(), 2, "{manifest:?}");
        assert!(manifest.missing.is_empty());
        // No path of this Mac in the package.
        let text = String::from_utf8_lossy(&bytes).to_string();
        assert!(!text.contains(src.to_str().unwrap()));
        let dst = temp_dir("roundtrip-dst");
        let (pdir, lib, audio) = targets(&dst);
        let done = install(unpack(&bytes).unwrap(), None, &Targets { projects_dir: &pdir, ilda: &lib, audio: &audio }).unwrap();
        assert_eq!(done.project, "Mon show");
        assert!(done.renamed.is_empty());
        for m in &manifest.media {
            let (kind_dir, name) = m.path.rsplit_once('/').unwrap();
            let got = std::fs::read(dst.join(kind_dir).join(name)).unwrap();
            assert_eq!(sha256_hex(&got), m.sha256, "{}", m.path);
            assert_eq!(sha256_hex(&std::fs::read(src.join(&m.path)).unwrap()), m.sha256);
        }
        let back = project::read(&pdir.join("Mon show.lsproj")).unwrap();
        assert_eq!(back.scenes.len(), 1);
        assert_eq!(back.scenes[0].name, "Logo");
        assert_eq!(back.timelines[0].audio.as_ref().unwrap().file, "intro.wav");
        assert!(matches!(&back.scenes[0].settings.content, Content::Ilda { file, .. } if *file == ilda));
        // A second import never writes over the first project.
        let again = install(unpack(&bytes).unwrap(), None, &Targets { projects_dir: &pdir, ilda: &lib, audio: &audio }).unwrap();
        assert_eq!(again.project, "Mon show 2");
        assert_eq!(again.existing.len(), 2, "identical media are shared");
    }

    #[test]
    fn a_clashing_media_name_is_renamed_and_the_project_follows() {
        let (src, p, ilda) = source("clash");
        let (bytes, _) = build(&p, None, &src.join("media/ilda"), &src.join("media/audio")).unwrap();
        let dst = temp_dir("clash-dst");
        let (pdir, lib, audio) = targets(&dst);
        // Another file of the same name is already there.
        lib.import(&ilda, &ilda_bytes(0.9)).unwrap();
        let done = install(unpack(&bytes).unwrap(), Some("Copie"), &Targets { projects_dir: &pdir, ilda: &lib, audio: &audio }).unwrap();
        let renamed = done.renamed.iter().find(|(old, _)| *old == ilda).expect("renamed").1.clone();
        assert_ne!(renamed, ilda);
        let back = project::read(&pdir.join("Copie.lsproj")).unwrap();
        assert!(matches!(&back.scenes[0].settings.content, Content::Ilda { file, .. } if *file == renamed));
    }

    #[test]
    fn a_site_travels_beside_the_project_without_the_output_addresses() {
        let (src, p, _) = source("site");
        let s = crate::test_support::shared();
        let mut site = sites::capture(&s, "Club");
        site.outputs = Some(crate::output_manager::OutputPrefs { selected: Some("etherdream:10.0.0.9".into()), ..Default::default() });
        let (bytes, manifest) = build(&p, Some(&site), &src.join("media/ilda"), &src.join("media/audio")).unwrap();
        assert_eq!(manifest.site.as_deref(), Some(SITE_FILE));
        assert!(!String::from_utf8_lossy(&bytes).contains("10.0.0.9"));
        let dst = temp_dir("site-dst");
        let (pdir, lib, audio) = targets(&dst);
        let done = install(unpack(&bytes).unwrap(), None, &Targets { projects_dir: &pdir, ilda: &lib, audio: &audio }).unwrap();
        assert_eq!(done.site.as_deref(), Some("Club"));
        let back = project::read(&pdir.join("Mon show.lsproj")).unwrap();
        let embedded = sites::parse_value(back.extra[sites::PROJECT_KEY].clone()).unwrap();
        assert_eq!(embedded.calibration, site.calibration);
        assert!(embedded.outputs.is_none());
    }

    #[test]
    fn zip_slip_and_odd_entries_are_refused_and_nothing_is_written() {
        let (src, p, _) = source("slip");
        let (good, _) = build(&p, None, &src.join("media/ilda"), &src.join("media/audio")).unwrap();
        let mut files = entries(&good);
        for bad in ["../evil.ild", "media/ilda/../../evil.ild", "/etc/passwd", "media\\ilda\\x.ild", "C:/x.ild", "media/ilda/./x.ild", "notes.txt", "media/other/x.ild", "media/ilda/x.exe"] {
            files.push((bad.to_string(), b"x".to_vec()));
            let zipped = rezip(&files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
            let e = unpack(&zipped).unwrap_err().to_string();
            assert!(e.contains("refusé"), "{bad}: {e}");
            files.pop();
        }
        assert!(!src.parent().unwrap().join("evil.ild").exists());
        for good in ["media/ilda/a b.ild", "project.lsproj", "media/audio/x.wav"] {
            assert!(safe_entry_name(good), "{good}");
        }
    }

    #[test]
    fn a_symlink_entry_is_refused() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.add_symlink("media/ilda/x.ild", "/etc/passwd", zip::write::SimpleFileOptions::default()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let e = unpack(&bytes).unwrap_err().to_string();
        assert!(e.contains("lien"), "{e}");
    }

    #[test]
    fn oversize_packages_and_entries_are_refused() {
        // More entries than allowed.
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("media/ilda/f{i}.ild")).collect();
        let many = rezip(&names.iter().map(|n| (n.as_str(), &b""[..])).collect::<Vec<_>>());
        assert!(unpack(&many).unwrap_err().to_string().contains("trop d'éléments"));
        // A manifest bigger than its limit (a zip bomb: deflates to little).
        let big = vec![b' '; MAX_JSON_BYTES as usize + 10];
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(MANIFEST, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
        zip.write_all(&big).unwrap();
        let bomb = zip.finish().unwrap().into_inner();
        assert!(bomb.len() < 1 << 20, "{}", bomb.len());
        assert!(unpack(&bomb).unwrap_err().to_string().contains("trop gros"));
        // Not a zip at all.
        assert!(unpack(b"PK nope").unwrap_err().to_string().contains("illisible"));
    }

    #[test]
    fn a_changed_media_file_fails_its_fingerprint() {
        let (src, p, ilda) = source("sha");
        let (good, _) = build(&p, None, &src.join("media/ilda"), &src.join("media/audio")).unwrap();
        let mut files = entries(&good);
        let i = files.iter().position(|(n, _)| n.ends_with(&ilda)).unwrap();
        files[i].1 = ilda_bytes(0.1);
        let zipped = rezip(&files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
        assert!(unpack(&zipped).unwrap_err().to_string().contains("empreinte"));
        // A media file the manifest doesn't list.
        let mut files = entries(&good);
        files.push(("media/ilda/extra.ild".into(), ilda_bytes(0.2)));
        let zipped = rezip(&files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
        assert!(unpack(&zipped).unwrap_err().to_string().contains("absent du manifeste"));
    }

    #[test]
    fn newer_package_or_project_formats_are_refused_clearly() {
        let (src, p, _) = source("newer");
        let (good, _) = build(&p, None, &src.join("media/ilda"), &src.join("media/audio")).unwrap();
        let mut files = entries(&good);
        let mut m: Value = serde_json::from_slice(&files[0].1).unwrap();
        m["format_version"] = (PACK_FORMAT + 1).into();
        files[0].1 = serde_json::to_vec(&m).unwrap();
        let zipped = rezip(&files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
        assert!(unpack(&zipped).unwrap_err().to_string().contains("plus récente"));
        let mut files = entries(&good);
        let i = files.iter().position(|(n, _)| n == PROJECT_FILE).unwrap();
        let mut pj: Value = serde_json::from_slice(&files[i].1).unwrap();
        pj["format_version"] = (project::PROJECT_FORMAT + 1).into();
        files[i].1 = serde_json::to_vec(&pj).unwrap();
        let zipped = rezip(&files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
        assert!(unpack(&zipped).unwrap_err().to_string().contains("plus récente"));
    }

    #[test]
    fn a_failing_song_removes_what_the_import_added() {
        let (src, p, ilda) = source("undo");
        let (good, _) = build(&p, None, &src.join("media/ilda"), &src.join("media/audio")).unwrap();
        // A song that does not decode, with a manifest that agrees.
        let mut files = entries(&good);
        let junk = b"RIFF not a wav".to_vec();
        let i = files.iter().position(|(n, _)| n.ends_with("intro.wav")).unwrap();
        files[i].1 = junk.clone();
        let mut m: PackManifest = serde_json::from_slice(&files[0].1).unwrap();
        for e in &mut m.media {
            if e.path.ends_with("intro.wav") {
                e.sha256 = sha256_hex(&junk);
                e.bytes = junk.len() as u64;
            }
        }
        files[0].1 = serde_json::to_vec(&m).unwrap();
        let zipped = rezip(&files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect::<Vec<_>>());
        let dst = temp_dir("undo-dst");
        let (pdir, lib, audio) = targets(&dst);
        assert!(install(unpack(&zipped).unwrap(), None, &Targets { projects_dir: &pdir, ilda: &lib, audio: &audio }).is_err());
        assert!(!dst.join("media/ilda").join(&ilda).exists(), "the ILDA file it added is removed");
        assert!(project::list(&pdir).is_empty());
    }
}
