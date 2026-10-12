//! Cue names and thumbnails for a lighting console over CITP/MSEX (T-454).
//!
//! Laser Studio acts as a CITP « MediaServer »: a console that connects
//! browses the cue grid as media libraries (one library per page, one
//! element per cue) with names and pictures, numbered exactly as the DMX
//! personality (T-407) selects them, so the operator picks a laser cue by
//! its picture and the console sends the right *Page* and *Cue* values.
//! Design note: `docs/design/citp.md`; wire format: wire.rs (public
//! specification only).
//!
//! What it does, and what it never does:
//! - **Off by default** (`<data-dir>/citp.json`, a setting of this machine,
//!   never in a project). No socket exists while it is off.
//! - **Read-only for the console**: nothing received is applied. No
//!   message reaches `external::apply`, plays a cue, changes a setting or
//!   arms; the "citp" thread only reads the grid (names, looks) under the
//!   `Shared` lock, and counts what it ignores. Driving stays the DMX
//!   input, through the gate.
//! - Listens on the interface the operator picked (127.0.0.1 by default),
//!   on a port the system picks. The PINF/PLoc announce the specification
//!   asks of a media server goes to its multicast group only while on and
//!   only on a non-loopback interface (TTL 1); nothing joins a group.
//! - Bounded: 8 consoles, 64 KiB per received message, 256 KiB of input
//!   and 1 MiB of output per console (past it, its requests wait: TCP
//!   back-pressure), 512 cached pictures.
//! - Pictures are drawn on the "citp" thread, never the engine's.

pub mod server;
pub mod thumb;
pub mod wire;

use crate::engine::Settings;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const DEFAULT_NAME: &str = "Laser Studio";
pub const DEFAULT_THUMB: u16 = 128;
pub const MIN_THUMB: u16 = 32;
pub const MAX_THUMB: u16 = 256;
/// Pages announced at most: the *Page* channel goes from 1 to 255.
pub const MAX_PAGES: usize = 255;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CitpSettings {
    pub enabled: bool,
    /// The address of this Mac to listen on; None: 127.0.0.1.
    pub interface: Option<String>,
    /// The name the console shows (PINF, SInf).
    pub server_name: String,
    /// Largest picture side, in pixels.
    pub thumb_max_px: u16,
}

impl Default for CitpSettings {
    fn default() -> Self {
        CitpSettings { enabled: false, interface: None, server_name: DEFAULT_NAME.into(), thumb_max_px: DEFAULT_THUMB }
    }
}

impl CitpSettings {
    pub fn validate(&self) -> Result<(), String> {
        let n = self.server_name.trim().chars().count();
        if n == 0 || n > wire::MAX_NAME {
            return Err(format!("nom affiché : 1 à {} caractères", wire::MAX_NAME));
        }
        if !(MIN_THUMB..=MAX_THUMB).contains(&self.thumb_max_px) {
            return Err(format!("taille des vignettes : {MIN_THUMB} à {MAX_THUMB} pixels"));
        }
        if let Some(i) = &self.interface {
            parse_interface(i)?;
        }
        Ok(())
    }

    /// The address to listen on (port 0: the system picks), when on.
    pub fn listen_ip(&self) -> Option<Ipv4Addr> {
        if !self.enabled {
            return None;
        }
        match &self.interface {
            None => Some(Ipv4Addr::LOCALHOST),
            Some(i) => parse_interface(i).ok(),
        }
    }
}

fn parse_interface(s: &str) -> Result<Ipv4Addr, String> {
    let ip: Ipv4Addr = s.trim().parse().map_err(|_| format!("interface réseau invalide : « {s} » (une adresse IPv4 de ce Mac)"))?;
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return Err(format!("interface réseau invalide : {ip} (une adresse IPv4 de ce Mac)"));
    }
    Ok(ip)
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CitpStatus {
    pub listening: bool,
    /// `ip:port` of the listening socket.
    pub local: Option<String>,
    /// The PINF/PLoc announce is sent (non-loopback interface).
    pub announcing: bool,
    /// The consoles connected: their name (PINF) and address.
    pub peers: Vec<String>,
    pub served_thumbs: u64,
    pub ignored_messages: u64,
    pub error: Option<String>,
}

/// What the HTTP side and the "citp" thread share.
#[derive(Default)]
pub struct CitpHub {
    cfg: Mutex<CitpSettings>,
    status: Mutex<CitpStatus>,
    /// Tests only: PLoc goes to this loopback address instead of the group.
    announce_to: Mutex<Option<SocketAddr>>,
    /// Where the settings live: the seed of this studio's CITP UUID.
    seed: String,
}

impl CitpHub {
    pub fn config(&self) -> CitpSettings {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn status(&self) -> CitpStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn seed(&self) -> String {
        self.seed.clone()
    }

    fn update_status(&self, f: impl FnOnce(&mut CitpStatus)) {
        f(&mut self.status.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn set_config(&self, cfg: CitpSettings) {
        *self.cfg.lock().unwrap_or_else(|e| e.into_inner()) = cfg;
    }

    #[cfg(test)]
    pub fn announce_to(&self, to: SocketAddr) {
        assert!(to.ip().is_loopback(), "tests announce on the loopback only");
        *self.announce_to.lock().unwrap() = Some(to);
    }

    fn announce_override(&self) -> Option<SocketAddr> {
        *self.announce_to.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// In `Shared` (`s.citp`).
pub struct CitpState {
    path: Option<PathBuf>,
    pub hub: Arc<CitpHub>,
}

impl CitpState {
    /// A missing, damaged or invalid file: off.
    pub fn load(path: PathBuf) -> Self {
        let cfg: CitpSettings = crate::load_json(&path);
        let cfg = if cfg.validate().is_ok() { cfg } else { CitpSettings::default() };
        let hub = CitpHub { seed: path.to_string_lossy().into_owned(), ..CitpHub::default() };
        hub.set_config(cfg);
        CitpState { path: Some(path), hub: Arc::new(hub) }
    }

    pub fn in_memory() -> Self {
        CitpState { path: None, hub: Arc::default() }
    }

    /// The file and its contents to write (outside the `Shared` lock).
    pub fn save_job(&self) -> Option<(PathBuf, CitpSettings)> {
        Some((self.path.clone()?, self.hub.config()))
    }
}

impl Default for CitpState {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// One cue as announced: element of its page's library.
#[derive(Clone, Debug)]
pub struct IndexCue {
    /// The *Cue* channel value that plays it (1..=40).
    pub dmx: u8,
    pub id: String,
    pub name: String,
    /// Changes when the cue's id, name or look changes.
    pub serial: u32,
    /// The look's fingerprint (pictures are cached by it).
    pub look: u64,
    pub settings: Arc<Settings>,
}

/// One page as announced: library `{1, n, 0, 0}`, *Page* value `n + 1`.
#[derive(Clone, Debug)]
pub struct IndexPage {
    pub name: String,
    pub serial: u32,
    pub cues: Vec<IndexCue>,
}

/// The table shared with the DMX personality (T-407): (page number, cue
/// number) → cue id, built through `dmx::fixture::cue_at`, the function
/// the DMX receiver plays with.
#[derive(Clone, Debug, Default)]
pub struct MediaIndex {
    pub pages: Vec<IndexPage>,
}

/// One cue copied under the lock: *Cue* value, id, name, look.
type RawCue = (u8, String, String, Settings);

/// What `MediaIndex` is built from, copied under the lock: each page's
/// name and cues.
pub struct RawIndex(Vec<(String, Vec<RawCue>)>);

impl MediaIndex {
    /// Copies the grid's pages, names and looks (cheap: under the lock).
    pub fn collect(s: &Shared) -> RawIndex {
        let names: std::collections::HashMap<&str, &crate::presets::Preset> = s.presets.iter().map(|p| (p.id.as_str(), p)).collect();
        let view = s.grid.view();
        let pages = (0..s.grid.page_count().min(MAX_PAGES))
            .map(|p| {
                let name = view.get(p).map_or_else(|| format!("Page {}", p + 1), |v| v.name.clone());
                let cues = (1..=crate::dmx::fixture::CUE_VALUES)
                    .filter_map(|v| {
                        let id = crate::dmx::fixture::cue_at(&s.grid, p, v)?;
                        let preset = names.get(id.as_str())?;
                        Some((v, id, preset.name.clone(), preset.settings.clone()))
                    })
                    .collect();
                (name, cues)
            })
            .collect();
        RawIndex(pages)
    }

    /// Fingerprints computed outside the lock.
    pub fn from_raw(raw: RawIndex) -> MediaIndex {
        let pages = raw
            .0
            .into_iter()
            .map(|(name, cues)| {
                let cues: Vec<IndexCue> = cues
                    .into_iter()
                    .map(|(dmx, id, name, settings)| {
                        let look = fnv(crate::web::look_key(&settings).as_bytes());
                        let serial = fold(fnv(format!("{id}\0{name}\0{look:x}").as_bytes()));
                        IndexCue { dmx, id, name: wire::short_name(&name), serial, look, settings: Arc::new(settings) }
                    })
                    .collect();
                let mut key = name.clone();
                for c in &cues {
                    key += &format!("\0{}:{}", c.dmx, c.serial);
                }
                IndexPage { name: wire::short_name(&name), serial: fold(fnv(key.as_bytes())), cues }
            })
            .collect();
        MediaIndex { pages }
    }

    #[cfg(test)]
    pub fn build(s: &Shared) -> MediaIndex {
        Self::from_raw(Self::collect(s))
    }

    #[cfg(test)]
    /// The cue the console selects with *Page* `page` (1..=255) and *Cue*
    /// `cue` (1..=40), as announced.
    pub fn cue_id(&self, page: u8, cue: u8) -> Option<&str> {
        let p = self.pages.get((page as usize).checked_sub(1)?)?;
        p.cues.iter().find(|c| c.dmx == cue).map(|c| c.id.as_str())
    }
}

/// FNV-1a, 64 bits: stable from one launch to the next.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

fn fold(h: u64) -> u32 {
    (h ^ (h >> 32)) as u32
}

/// Under `citp` of `/api/state`.
pub fn summary(s: &Shared) -> Value {
    let cfg = s.citp.hub.config();
    let st = s.citp.hub.status();
    json!({
        "enabled": cfg.enabled,
        "listening": st.listening,
        "local": st.local,
        "announcing": st.announcing,
        "peers": st.peers,
        "served_thumbs": st.served_thumbs,
        "ignored_messages": st.ignored_messages,
        "error": st.error,
    })
}

/// `GET /api/citp`.
pub fn view(s: &Shared) -> Value {
    json!({
        "config": s.citp.hub.config(),
        "status": s.citp.hub.status(),
        "interfaces": crate::netin::interfaces(),
        "limits": { "thumb_min": MIN_THUMB, "thumb_max": MAX_THUMB, "name_max": wire::MAX_NAME },
    })
}

/// `POST /api/citp` `{ "enabled": true, "interface": "192.168.1.10", … }`:
/// the fields given replace the current ones. The interface must be an
/// address of this Mac. Returns whether anything changed (the caller
/// saves `citp.json`).
pub fn update(s: &mut Shared, patch: &Value) -> Result<bool, String> {
    let obj = patch.as_object().ok_or("objet JSON attendu")?;
    let before = s.citp.hub.config();
    let mut v = json!(before);
    for (k, val) in obj {
        if v.get(k).is_none() {
            return Err(format!("réglage inconnu : {k}"));
        }
        v[k] = val.clone();
    }
    let mut cfg: CitpSettings = serde_json::from_value(v).map_err(|e| format!("réglage invalide : {e}"))?;
    cfg.interface = cfg.interface.map(|i| i.trim().to_string()).filter(|i| !i.is_empty());
    cfg.server_name = cfg.server_name.trim().to_string();
    cfg.validate()?;
    if let Some(i) = cfg.interface.as_ref().filter(|i| before.interface.as_ref() != Some(*i)) {
        let ip = IpAddr::V4(parse_interface(i)?);
        let local = crate::netin::interfaces().iter().any(|l| l["ip"].as_str().and_then(|a| a.parse::<IpAddr>().ok()) == Some(ip));
        if !local {
            return Err(format!("{i} n'est pas une adresse de ce Mac"));
        }
    }
    if cfg == before {
        return Ok(false);
    }
    if let Some(change) = crate::safety_log::settings_change(&before, &cfg) {
        s.gate.log().record("citp_settings", Some(crate::interlock::ArmSource::Ui.id()), change);
    }
    if cfg.enabled != before.enabled {
        log::info!("CITP {}", if cfg.enabled { "activé" } else { "désactivé" });
    }
    s.citp.hub.set_config(cfg);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::shared;

    #[test]
    fn off_by_default_and_a_bad_file_is_off() {
        let cfg = CitpSettings::default();
        assert!(!cfg.enabled && cfg.listen_ip().is_none());
        assert_eq!((cfg.server_name.as_str(), cfg.thumb_max_px, cfg.interface.as_deref()), ("Laser Studio", 128, None));
        let dir = std::env::temp_dir().join(format!("citp-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("citp.json");
        std::fs::write(&path, r#"{"enabled":true,"thumb_max_px":9999}"#).unwrap();
        assert!(!CitpState::load(path.clone()).hub.config().enabled);
        std::fs::write(&path, r#"{"enabled":true}"#).unwrap();
        assert_eq!(CitpState::load(path).hub.config().listen_ip(), Some(Ipv4Addr::LOCALHOST));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn update_validates() {
        let mut s = shared();
        assert!(update(&mut s, &json!({ "enabled": true })).unwrap());
        assert!(!update(&mut s, &json!({ "enabled": true })).unwrap(), "no change");
        assert!(update(&mut s, &json!({ "nope": 1 })).is_err());
        assert!(update(&mut s, &json!({ "thumb_max_px": 20 })).is_err());
        assert!(update(&mut s, &json!({ "server_name": "  " })).is_err());
        assert!(update(&mut s, &json!({ "interface": "0.0.0.0" })).is_err());
        assert!(update(&mut s, &json!({ "interface": "224.0.0.180" })).is_err());
        assert!(update(&mut s, &json!({ "interface": "203.0.113.7" })).is_err(), "not this Mac");
        assert!(update(&mut s, &json!({ "interface": "127.0.0.1", "server_name": " Scène A " })).unwrap());
        assert_eq!(s.citp.hub.config().server_name, "Scène A");
        assert!(update(&mut s, &json!({ "interface": "" })).unwrap());
        assert_eq!(s.citp.hub.config().interface, None);
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn the_index_follows_the_grid_with_dmx_numbers() {
        let s = shared();
        let idx = MediaIndex::build(&s);
        assert_eq!(idx.pages.len(), s.grid.page_count().min(MAX_PAGES));
        for (p, page) in idx.pages.iter().enumerate() {
            assert_eq!(page.name, wire::short_name(&s.grid.view()[p].name));
            for c in &page.cues {
                assert!((1..=40).contains(&c.dmx));
                assert_eq!(idx.cue_id(p as u8 + 1, c.dmx), Some(c.id.as_str()));
                let preset = s.presets.iter().find(|x| x.id == c.id).unwrap();
                assert_eq!(c.name, wire::short_name(&preset.name));
            }
            // Contiguous element numbers, DMX values in cell order.
            assert!(page.cues.windows(2).all(|w| w[0].dmx < w[1].dmx));
        }
        assert!(idx.pages.iter().map(|p| p.cues.len()).sum::<usize>() > 0);
        assert_eq!(idx.cue_id(0, 1), None);
        // Stable: the same grid gives the same serials.
        let again = MediaIndex::build(&s);
        assert_eq!(idx.pages.iter().map(|p| p.serial).collect::<Vec<_>>(), again.pages.iter().map(|p| p.serial).collect::<Vec<_>>());
    }
}
