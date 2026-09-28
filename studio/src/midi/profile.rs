//! Controller profiles: which driver a port uses (APC40, APC40 mkII or
//! generic), its Shift key and its mappings (T-202). Built-in profiles are
//! compiled in and never written; user profiles live in
//! `<data-dir>/midi/profiles/<slug>.json`, and `<data-dir>/midi/devices.json`
//! remembers, per port name, the chosen profile and whether it is enabled.
//!
//! Nothing here panics on bad files: a broken profile is skipped with a
//! readable error (shown by `/api/midi`) and the port falls back to `generic`.

use super::mapping::{Mapping, MidiInput, RelEncoding};
use super::safety::MidiSafety;
use super::detect::{Model, MODE_ABLETON, MODE_ALTERNATE, MODE_GENERIC, PID_APC40, PID_APC40_MK2};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const GENERIC: &str = "generic";

/// Built-in profiles: our own layout (T-204 fills the mappings).
const BUILTIN: [(&str, &str); 3] = [
    ("apc40-mk2", include_str!("../../profiles/apc40-mk2.json")),
    ("apc40", include_str!("../../profiles/apc40.json")),
    (GENERIC, include_str!("../../profiles/generic.json")),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    #[default]
    Generic,
    Apc40,
    Apc40Mk2,
}

impl Driver {
    /// Akai product id for the Introduction, for APC drivers.
    pub fn apc_pid(self) -> Option<u8> {
        match self {
            Driver::Generic => None,
            Driver::Apc40 => Some(PID_APC40),
            Driver::Apc40Mk2 => Some(PID_APC40_MK2),
        }
    }

    /// The model whose LED layout this driver speaks.
    pub fn model(self) -> Model {
        match self {
            Driver::Generic => Model::Unknown,
            Driver::Apc40 => Model::Apc40,
            Driver::Apc40Mk2 => Model::Apc40Mk2,
        }
    }
}

/// Which devices a profile picks automatically.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceMatch {
    /// Case-insensitive substrings of the port name.
    #[serde(default)]
    pub port_contains: Vec<String>,
    /// Akai product id from the Device Inquiry (0x73, 0x29…).
    #[serde(default)]
    pub product_id: Option<u8>,
}

fn version_1() -> u32 {
    1
}

fn mode_41() -> u8 {
    MODE_ABLETON
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default = "version_1")]
    pub version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub driver: Driver,
    #[serde(default, rename = "match")]
    pub matches: DeviceMatch,
    /// Introduction mode sent to an APC (0x40 / 0x41 / 0x42).
    #[serde(default = "mode_41")]
    pub host_mode: u8,
    /// The Shift key (APC: note 0x62): while held, `shift: true` mappings
    /// are looked up first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift_key: Option<MidiInput>,
    /// Endless encoders of the device (APC: Cue Level, Tempo): MIDI learn
    /// (T-203) maps them in `relative` mode with this encoding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub encoders: Vec<Encoder>,
    #[serde(default)]
    pub mappings: Vec<Mapping>,
    /// Fields added by later tasks (shift key, encoders…), kept on save.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A CC known to come from an endless encoder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Encoder {
    #[serde(flatten)]
    pub input: MidiInput,
    #[serde(default)]
    pub encoding: RelEncoding,
}

impl Profile {
    pub fn parse(json: &str) -> Result<Profile, String> {
        let p: Profile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if p.version != 1 {
            return Err(format!("version {} non prise en charge", p.version));
        }
        if !(MODE_GENERIC..=MODE_ALTERNATE).contains(&p.host_mode) {
            return Err(format!("host_mode {:#04x} invalide (0x40, 0x41 ou 0x42)", p.host_mode));
        }
        Ok(p)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortPrefs {
    /// Chosen profile slug; `None` = automatic.
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl Default for PortPrefs {
    fn default() -> Self {
        PortPrefs { profile: None, enabled: true }
    }
}

/// `devices.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DevicesFile {
    #[serde(default)]
    pub ports: BTreeMap<String, PortPrefs>,
    /// Global MIDI safety options (T-208).
    #[serde(default)]
    pub safety: MidiSafety,
    /// Fields added later, kept on save.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProfileInfo {
    pub slug: String,
    pub name: String,
    pub driver: Driver,
    pub builtin: bool,
}

/// Result of picking a profile for a port.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub slug: String,
    pub error: Option<String>,
}

pub struct ProfileStore {
    /// `<data-dir>/midi`; `None` = in memory only (tests, `--no-midi` without data).
    dir: Option<PathBuf>,
    builtin: Vec<(String, Profile)>,
    user: BTreeMap<String, Profile>,
    pub devices: DevicesFile,
    /// Load errors, in French, for `/api/midi`.
    pub errors: Vec<String>,
}

impl ProfileStore {
    pub fn in_memory() -> Self {
        let builtin = BUILTIN
            .iter()
            .map(|(slug, json)| (slug.to_string(), Profile::parse(json).unwrap_or_else(|e| panic!("built-in profile {slug}: {e}"))))
            .collect();
        ProfileStore { dir: None, builtin, user: BTreeMap::new(), devices: DevicesFile::default(), errors: Vec::new() }
    }

    /// Loads user profiles and `devices.json` from `<data-dir>/midi`.
    pub fn load(dir: PathBuf) -> Self {
        let mut store = ProfileStore::in_memory();
        let devices_path = dir.join("devices.json");
        match std::fs::read_to_string(&devices_path) {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(devices) => store.devices = devices,
                Err(e) => store.errors.push(format!("devices.json illisible ({e}) : réglages MIDI par défaut")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => store.errors.push(format!("devices.json illisible : {e}")),
        }
        if let Ok(entries) = std::fs::read_dir(dir.join("profiles")) {
            let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
            paths.sort();
            for path in paths {
                let Some(slug) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
                if !valid_slug(&slug) {
                    store.errors.push(format!("profil « {} » ignoré : nom de fichier invalide", path.display()));
                } else if store.is_builtin(&slug) {
                    store.errors.push(format!("profil « {slug} » ignoré : nom réservé à un profil intégré"));
                } else {
                    match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|j| Profile::parse(&j)) {
                        Ok(p) => {
                            store.user.insert(slug, p);
                        }
                        Err(e) => store.errors.push(format!("profil « {slug} » illisible : {e}")),
                    }
                }
            }
        }
        store.dir = Some(dir);
        store
    }

    /// `<data-dir>/midi`, or `None` in memory.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// User profiles by slug (built-ins excluded).
    pub fn user_profiles(&self) -> &BTreeMap<String, Profile> {
        &self.user
    }

    /// Replaces every user profile in memory only (slugs and profiles
    /// already checked); the caller writes the files (project open, T-286).
    /// `devices.json`, with the MIDI safety options, is never touched.
    pub fn replace_user_in_memory(&mut self, user: BTreeMap<String, Profile>) {
        self.user = user;
    }

    pub fn is_builtin(&self, slug: &str) -> bool {
        self.builtin.iter().any(|(s, _)| s == slug)
    }

    pub fn get(&self, slug: &str) -> Option<&Profile> {
        self.user.get(slug).or_else(|| self.builtin.iter().find(|(s, _)| s == slug).map(|(_, p)| p))
    }

    /// Built-ins first, then user profiles by slug.
    pub fn list(&self) -> Vec<ProfileInfo> {
        let info = |slug: &str, p: &Profile, builtin| ProfileInfo { slug: slug.to_string(), name: p.name.clone(), driver: p.driver, builtin };
        self.builtin.iter().map(|(s, p)| info(s, p, true)).chain(self.user.iter().map(|(s, p)| info(s, p, false))).collect()
    }

    fn all(&self) -> impl Iterator<Item = (&str, &Profile)> {
        // User profiles first: they win ties.
        self.user.iter().map(|(s, p)| (s.as_str(), p)).chain(self.builtin.iter().map(|(s, p)| (s.as_str(), p)))
    }

    /// Picks the profile for a port: saved preference, else a profile whose
    /// product id matches the detected model, else the longest matching
    /// port-name substring, else `generic`.
    pub fn choose(&self, port: &str, model: Model) -> Choice {
        if let Some(slug) = self.devices.ports.get(port).and_then(|p| p.profile.as_deref()) {
            return if self.get(slug).is_some() {
                Choice { slug: slug.to_string(), error: None }
            } else {
                Choice { slug: GENERIC.into(), error: Some(format!("profil « {slug} » introuvable ou illisible : profil générique utilisé")) }
            };
        }
        if let Some(pid) = model.product_id() {
            if let Some((slug, _)) = self.all().find(|(_, p)| p.matches.product_id == Some(pid)) {
                return Choice { slug: slug.to_string(), error: None };
            }
        }
        let port_lc = port.to_lowercase();
        let mut best: Option<(&str, usize)> = None;
        for (slug, p) in self.all() {
            for needle in &p.matches.port_contains {
                if !needle.is_empty() && port_lc.contains(&needle.to_lowercase()) && best.is_none_or(|(_, len)| needle.len() > len) {
                    best = Some((slug, needle.len()));
                }
            }
        }
        Choice { slug: best.map_or(GENERIC, |(s, _)| s).to_string(), error: None }
    }

    pub fn port_enabled(&self, port: &str) -> bool {
        self.devices.ports.get(port).is_none_or(|p| p.enabled)
    }

    pub fn set_port_enabled(&mut self, port: &str, enabled: bool) -> Result<(), String> {
        self.devices.ports.entry(port.to_string()).or_default().enabled = enabled;
        self.save_devices()
    }

    /// `None` = back to automatic choice ("Réinitialiser le profil").
    pub fn set_port_profile(&mut self, port: &str, slug: Option<&str>) -> Result<(), String> {
        if let Some(slug) = slug {
            if self.get(slug).is_none() {
                return Err(format!("profil inconnu : {slug}"));
            }
        }
        self.devices.ports.entry(port.to_string()).or_default().profile = slug.map(str::to_string);
        self.save_devices()
    }

    pub fn set_safety(&mut self, safety: MidiSafety) -> Result<(), String> {
        self.devices.safety = safety;
        self.save_devices()
    }

    /// Saves an edited profile. Editing a built-in one saves a
    /// `<slug>-perso` copy instead; with a port, that copy becomes the
    /// port's profile. Returns the slug actually written.
    pub fn save_profile(&mut self, port: Option<&str>, slug: &str, profile: Profile) -> Result<String, String> {
        let slug = if self.is_builtin(slug) { format!("{slug}-perso") } else { slug.to_string() };
        if !valid_slug(&slug) || self.is_builtin(&slug) {
            return Err(format!("nom de profil invalide : {slug}"));
        }
        if let Some(dir) = &self.dir {
            let dir = dir.join("profiles");
            let json = serde_json::to_string_pretty(&profile).map_err(|e| e.to_string())?;
            write_file(&dir.join(format!("{slug}.json")), &json)?;
        }
        self.user.insert(slug.clone(), profile);
        if let Some(port) = port {
            self.set_port_profile(port, Some(&slug))?;
        }
        Ok(slug)
    }

    /// Where edits of profile `slug` go (MIDI learn, deleting a mapping):
    /// the profile itself if it is a user one; for a built-in one, a new
    /// `<slug>-perso` copy (`-perso-2`… if taken: an older copy is never
    /// overwritten).
    pub fn edit_slug(&self, slug: &str) -> String {
        if !self.is_builtin(slug) {
            return slug.to_string();
        }
        let base = format!("{slug}-perso");
        (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|s| self.get(s).is_none()).unwrap_or(base)
    }

    fn save_devices(&self) -> Result<(), String> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let json = serde_json::to_string_pretty(&self.devices).map_err(|e| e.to_string())?;
        write_file(&dir.join("devices.json"), &json)
    }
}

fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("impossible de créer {} : {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("impossible d'écrire {} : {e}", path.display()))
}

/// Slugs become file names: lowercase ASCII, digits, `-` and `_` only.
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty() && slug.len() <= 64 && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-midi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn builtins_parse() {
        let store = ProfileStore::in_memory();
        assert_eq!(store.get("apc40").unwrap().driver, Driver::Apc40);
        assert_eq!(store.get("apc40-mk2").unwrap().driver, Driver::Apc40Mk2);
        assert_eq!(store.get("apc40-mk2").unwrap().host_mode, 0x41);
        assert_eq!(store.get(GENERIC).unwrap().driver, Driver::Generic);
        assert_eq!(store.list().len(), 3);
    }

    #[test]
    fn minimal_profile_gets_defaults() {
        let p = Profile::parse(r#"{ "name": "Mon pad" }"#).unwrap();
        assert_eq!((p.version, p.driver, p.host_mode), (1, Driver::Generic, 0x41));
        assert!(p.mappings.is_empty() && p.matches.port_contains.is_empty() && p.matches.product_id.is_none());
    }

    #[test]
    fn profile_json_round_trip_keeps_unknown_fields() {
        let json = r#"{ "version": 1, "name": "APC", "driver": "apc40mk2",
            "match": { "port_contains": ["APC40 mkII"], "product_id": 41 }, "host_mode": 66,
            "shift_key": { "kind": "note", "number": 98 },
            "mappings": [{ "input": { "kind": "cc", "channel": null, "number": 14 }, "target": "master.brightness", "mode": "absolute" }] }"#;
        let p = Profile::parse(json).unwrap();
        assert_eq!(p.matches.product_id, Some(0x29));
        assert_eq!(p.shift_key.as_ref().map(|k| (k.channel, k.number)), Some((None, 0x62)));
        assert_eq!(p.mappings[0].target, "master.brightness");
        let back = Profile::parse(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn invalid_profiles_are_errors_not_panics() {
        assert!(Profile::parse("{ not json").is_err());
        assert!(Profile::parse(r#"{ "version": 2 }"#).is_err());
        assert!(Profile::parse(r#"{ "host_mode": 12 }"#).is_err());
        assert!(Profile::parse(r#"{ "driver": "launchpad" }"#).is_err());
    }

    #[test]
    fn choice_by_model_port_name_and_preference() {
        let mut store = ProfileStore::in_memory();
        assert_eq!(store.choose("APC40 mkII", Model::Apc40Mk2).slug, "apc40-mk2");
        assert_eq!(store.choose("Some Port", Model::Apc40).slug, "apc40", "the inquiry wins over the name");
        assert_eq!(store.choose("APC40 mkII", Model::Unknown).slug, "apc40-mk2", "longest name match");
        assert_eq!(store.choose("Akai APC40", Model::Unknown).slug, "apc40");
        assert_eq!(store.choose("APC MINI", Model::ApcMini).slug, GENERIC, "no APC mini profile yet");
        assert_eq!(store.choose("nanoKONTROL2", Model::Unknown).slug, GENERIC);

        store.set_port_profile("APC40 mkII", Some("generic")).unwrap();
        assert_eq!(store.choose("APC40 mkII", Model::Apc40Mk2).slug, GENERIC);
        store.set_port_profile("APC40 mkII", None).unwrap();
        assert_eq!(store.choose("APC40 mkII", Model::Apc40Mk2).slug, "apc40-mk2");
        assert!(store.set_port_profile("APC40 mkII", Some("nope")).is_err());
    }

    #[test]
    fn missing_preferred_profile_falls_back_to_generic_with_an_error() {
        let mut store = ProfileStore::in_memory();
        store.devices.ports.insert("APC40 mkII".into(), PortPrefs { profile: Some("broken".into()), enabled: true });
        let choice = store.choose("APC40 mkII", Model::Apc40Mk2);
        assert_eq!(choice.slug, GENERIC);
        assert!(choice.error.unwrap().contains("broken"));
    }

    #[test]
    fn user_profiles_match_by_port_name() {
        let mut store = ProfileStore::in_memory();
        let mut p = Profile::parse(r#"{ "name": "nano", "match": { "port_contains": ["nanokontrol"] } }"#).unwrap();
        p.name = "nanoKONTROL2".into();
        store.save_profile(None, "nano", p).unwrap();
        assert_eq!(store.choose("nanoKONTROL2 SLIDER/KNOB", Model::Unknown).slug, "nano");
    }

    #[test]
    fn editing_a_builtin_saves_a_perso_copy_that_survives_a_restart() {
        let dir = temp_dir("perso");
        let mut store = ProfileStore::load(dir.clone());
        let mut edited = store.get("apc40-mk2").unwrap().clone();
        edited.name = "Mon APC".into();
        let slug = store.save_profile(Some("APC40 mkII"), "apc40-mk2", edited).unwrap();
        assert_eq!(slug, "apc40-mk2-perso");
        assert_eq!(store.get("apc40-mk2").unwrap().name, "APC40 mkII — Laser Studio", "built-in untouched");
        store.set_port_enabled("Other", false).unwrap();
        store.set_safety(MidiSafety { allow_arm: true, blackout_on_disconnect: false }).unwrap();

        let reloaded = ProfileStore::load(dir.clone());
        assert!(reloaded.errors.is_empty(), "{:?}", reloaded.errors);
        assert_eq!(reloaded.get("apc40-mk2-perso").unwrap().name, "Mon APC");
        assert_eq!(reloaded.choose("APC40 mkII", Model::Apc40Mk2).slug, "apc40-mk2-perso");
        assert!(!reloaded.port_enabled("Other"));
        assert!(reloaded.port_enabled("APC40 mkII"));
        assert!(reloaded.devices.safety.allow_arm);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_files_give_readable_errors() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(dir.join("profiles")).unwrap();
        std::fs::write(dir.join("profiles/cassé.json"), "{}").unwrap();
        std::fs::write(dir.join("profiles/bad.json"), "{ oops").unwrap();
        std::fs::write(dir.join("profiles/generic.json"), "{}").unwrap();
        std::fs::write(dir.join("devices.json"), r#"{ "ports": { "APC40 mkII": { "profile": "bad" } } }"#).unwrap();
        let store = ProfileStore::load(dir.clone());
        assert_eq!(store.errors.len(), 3, "{:?}", store.errors);
        assert!(store.errors.iter().any(|e| e.contains("« bad » illisible")));
        let choice = store.choose("APC40 mkII", Model::Apc40Mk2);
        assert_eq!(choice.slug, GENERIC);
        assert!(choice.error.is_some());
        assert_eq!(store.get(GENERIC).unwrap().driver, Driver::Generic, "built-in not shadowed");

        std::fs::write(dir.join("devices.json"), "[1,2").unwrap();
        let store = ProfileStore::load(dir.clone());
        assert!(store.errors.iter().any(|e| e.starts_with("devices.json")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert!(valid_slug("apc40-mk2-perso"));
        assert!(!valid_slug("../evil"));
        assert!(!valid_slug("Mon Profil"));
        assert!(!valid_slug(""));
    }
}
