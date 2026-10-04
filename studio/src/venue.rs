//! The venue and its projectors (T-277).
//!
//! A *venue* is the room the show is designed for (size, stage, audience
//! area, gauze screens) and the projectors placed in it. Its geometry
//! (positions, angles, scan angle, brightness, *Miroir X*, the room) only
//! feeds the 3D view: it never changes what a laser draws.
//!
//! What it does change is **how many outputs the engine drives**. Every
//! projector is one output stage of its own:
//!
//! - the first projector is the main one, id `main` (`power::MAIN_OUTPUT`):
//!   it drives the output chosen in « Sorties et projecteurs » (the output
//!   manager's selection), exactly as a studio without a venue does. A
//!   venue with one projector is today's single-output studio;
//! - every other projector names its own output (`output`, an output
//!   manager id) or none (a preview-only projector, drawn in the 3D view).
//!
//! Each projector's frame is rendered from the projection zones it shows
//! (`ProjectionConfig::active_on`), then goes through **its own** full
//! safety chain (zone geometry and calibration, frame size, colour
//! rendering, safety zones, horizon, audience mode, strobe limiter,
//! static-beam guard, its own power caps from `outputs.json` under its id)
//! and its own output gate. The arm state stays global: one arm, one
//! e-stop. Escape and the e-stop darken every projector at once (each
//! output's kill switch is in the e-stop), and losing **any** projector's
//! output disarms all of them (« Sortie perdue ») until it is back and the
//! operator re-arms.
//!
//! Changing which outputs the projectors drive (adding or removing a
//! projector with an output, changing an output) is refused while armed.
//! Stored in `<data-dir>/venue.json` (a machine file, like the outputs,
//! never in a project for now). A damaged file falls back to the default
//! venue: one projector, i.e. fewer outputs, never more.

use crate::power::MAIN_OUTPUT;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Most projectors in a venue (one output stage each).
pub const MAX_PROJECTORS: usize = 8;
/// Most gauze screens.
pub const MAX_SCREENS: usize = 8;
const MAX_NAME: usize = 40;

/// The room presets: (id, label, width, depth, height, ceiling).
pub const PRESETS: [(&str, &str, f32, f32, f32, bool); 3] = [
    ("club", "Club", 10.0, 15.0, 5.0, true),
    ("salle", "Salle", 20.0, 30.0, 10.0, true),
    // No ceiling: the height is only how far up the 3D view draws.
    ("festival", "Festival plein air", 40.0, 60.0, 30.0, false),
];

/// The room: a box, x across (width), y up, z from the audience end (0)
/// to the stage end (depth), in metres. The 3D view's world.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Room {
    pub width_m: f32,
    pub depth_m: f32,
    pub height_m: f32,
    pub ceiling: bool,
}

impl Default for Room {
    fn default() -> Self {
        Self { width_m: 10.0, depth_m: 15.0, height_m: 5.0, ceiling: true }
    }
}

/// An axis-aligned block (the stage): its minimum corner and its size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rect3 {
    pub pos_m: [f32; 3],
    pub size_m: [f32; 3],
}

impl Default for Rect3 {
    fn default() -> Self {
        Self { pos_m: [-4.0, 0.0, 11.0], size_m: [8.0, 1.0, 4.0] }
    }
}

/// The audience area: a floor rectangle `[x0, z0, x1, z1]` and the
/// minimum beam height above it (3 m by default; follow local rules).
/// For the 3D overlays (T-279); the safety zones stay in `safety.json`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudienceArea {
    pub rect_m: [f32; 4],
    pub min_height_m: f32,
}

impl Default for AudienceArea {
    fn default() -> Self {
        Self { rect_m: [-4.0, 1.0, 4.0, 9.0], min_height_m: 3.0 }
    }
}

/// A gauze screen: a quad (centre, size, rotation about y).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Screen {
    pub name: String,
    pub pos_m: [f32; 3],
    pub size_m: [f32; 2],
    pub yaw_deg: f32,
}

impl Default for Screen {
    fn default() -> Self {
        Self { name: "Écran".into(), pos_m: [0.0, 3.0, 8.0], size_m: [6.0, 4.0], yaw_deg: 0.0 }
    }
}

/// One projector. Position and angles are the 3D view's (yaw 0, pitch 0:
/// facing the audience, -z). `brightness` and `mirror_x` are visual only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Projector {
    /// Stable id: `main` for the first projector, then `p2`, `p3`… It is
    /// also the key of its caps and colour rendering in `outputs.json`
    /// and what a projection zone can be bound to.
    pub id: String,
    pub name: String,
    pub pos_m: [f32; 3],
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    pub scan_deg: f32,
    pub brightness: f32,
    /// The output it drives (output manager id); None: preview only. For
    /// the main projector this is the selection of « Sorties et
    /// projecteurs » and the field is ignored on input.
    pub output: Option<String>,
    pub mirror_x: bool,
}

impl Default for Projector {
    fn default() -> Self {
        Self {
            id: MAIN_OUTPUT.into(),
            name: "Projecteur 1".into(),
            pos_m: [0.0, 3.0, 11.0],
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            scan_deg: 40.0,
            brightness: 1.0,
            output: None,
            mirror_x: false,
        }
    }
}

/// The 3D view's render settings (the page keeps its own too).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    pub haze: f32,
    pub room_light: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self { haze: 0.5, room_light: 0.1 }
    }
}

/// `venue.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Venue {
    pub preset: String,
    pub room: Room,
    pub stage: Rect3,
    pub audience: AudienceArea,
    pub screens: Vec<Screen>,
    pub projectors: Vec<Projector>,
    pub render: RenderSettings,
}

impl Default for Venue {
    fn default() -> Self {
        Self {
            preset: "club".into(),
            room: Room::default(),
            stage: Rect3::default(),
            audience: AudienceArea::default(),
            screens: Vec::new(),
            projectors: vec![Projector::default()],
            render: RenderSettings::default(),
        }
    }
}

/// A number from a hand-edited file or the API, clamped; not finite → `def`.
fn fix(v: f32, lo: f32, hi: f32, def: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        def
    }
}

fn name(s: &str, def: String) -> String {
    let s: String = s.trim().chars().take(MAX_NAME).collect();
    if s.is_empty() {
        def
    } else {
        s
    }
}

fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 16 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Venue {
    /// What `POST /api/venue` refuses (French).
    pub fn validate(&self) -> Result<()> {
        if self.projectors.is_empty() {
            bail!("il faut au moins un projecteur");
        }
        if self.projectors.len() > MAX_PROJECTORS {
            bail!("au plus {MAX_PROJECTORS} projecteurs");
        }
        if self.screens.len() > MAX_SCREENS {
            bail!("au plus {MAX_SCREENS} écrans");
        }
        for (i, p) in self.projectors.iter().enumerate().skip(1) {
            if !p.id.is_empty() && (!is_id(&p.id) || p.id == MAIN_OUTPUT) {
                bail!("identifiant de projecteur invalide : « {} »", p.id);
            }
            if !p.id.is_empty() && self.projectors[1..i].iter().any(|o| o.id == p.id) {
                bail!("deux projecteurs portent l'identifiant « {} »", p.id);
            }
            if let Some(out) = p.output.as_deref().filter(|o| !o.trim().is_empty()) {
                if self.projectors[1..i].iter().any(|o| o.output.as_deref() == Some(out)) {
                    bail!("deux projecteurs ne peuvent pas utiliser la même sortie (« {out} »)");
                }
            }
        }
        Ok(())
    }

    /// Repairs what a hand-edited file could hold: at least one projector,
    /// the first one `main`, the others with unique ids (missing ones
    /// given `p2`, `p3`…), at most `MAX_PROJECTORS`, numbers in range, an
    /// output used once. Never adds an output.
    pub fn sanitized(&self) -> Self {
        let room = Room {
            width_m: fix(self.room.width_m, 2.0, 200.0, 10.0),
            depth_m: fix(self.room.depth_m, 2.0, 200.0, 15.0),
            height_m: fix(self.room.height_m, 2.0, 100.0, 5.0),
            ceiling: self.room.ceiling,
        };
        let v3 = |p: [f32; 3], lo: f32, hi: f32| p.map(|v| fix(v, lo, hi, 0.0));
        let mut projectors: Vec<Projector> = Vec::new();
        for (i, p) in self.projectors.iter().take(MAX_PROJECTORS).enumerate() {
            let id = if i == 0 {
                MAIN_OUTPUT.to_string()
            } else if is_id(&p.id) && p.id != MAIN_OUTPUT && !projectors.iter().any(|o| o.id == p.id) {
                p.id.clone()
            } else {
                (2..).map(|n| format!("p{n}")).find(|c| !projectors.iter().any(|o| &o.id == c) && !self.projectors.iter().skip(i + 1).any(|o| &o.id == c)).expect("a free id")
            };
            let output = p.output.as_deref().map(str::trim).filter(|o| !o.is_empty()).map(str::to_string);
            // An output already driven by an earlier projector: dropped.
            let output = output.filter(|o| i == 0 || !projectors.iter().skip(1).any(|q| q.output.as_deref() == Some(o)));
            projectors.push(Projector {
                name: name(&p.name, format!("Projecteur {}", i + 1)),
                id,
                pos_m: v3(p.pos_m, -500.0, 500.0),
                yaw_deg: fix(p.yaw_deg, -180.0, 180.0, 0.0),
                pitch_deg: fix(p.pitch_deg, -90.0, 90.0, 0.0),
                roll_deg: fix(p.roll_deg, -180.0, 180.0, 0.0),
                scan_deg: fix(p.scan_deg, 1.0, 120.0, 40.0),
                brightness: fix(p.brightness, 0.0, 1.0, 1.0),
                output,
                mirror_x: p.mirror_x,
            });
        }
        if projectors.is_empty() {
            projectors.push(Projector::default());
        }
        let screens = self
            .screens
            .iter()
            .take(MAX_SCREENS)
            .enumerate()
            .map(|(i, s)| Screen {
                name: name(&s.name, format!("Écran {}", i + 1)),
                pos_m: v3(s.pos_m, -500.0, 500.0),
                size_m: s.size_m.map(|v| fix(v, 0.1, 100.0, 1.0)),
                yaw_deg: fix(s.yaw_deg, -180.0, 180.0, 0.0),
            })
            .collect();
        let preset = if PRESETS.iter().any(|p| p.0 == self.preset) { self.preset.clone() } else { String::new() };
        Self {
            preset,
            room,
            stage: Rect3 { pos_m: v3(self.stage.pos_m, -500.0, 500.0), size_m: self.stage.size_m.map(|v| fix(v, 0.0, 200.0, 1.0)) },
            audience: AudienceArea { rect_m: self.audience.rect_m.map(|v| fix(v, -500.0, 500.0, 0.0)), min_height_m: fix(self.audience.min_height_m, 0.0, 20.0, 3.0) },
            screens,
            projectors,
            render: RenderSettings { haze: fix(self.render.haze, 0.0, 1.0, 0.5), room_light: fix(self.render.room_light, 0.0, 1.0, 0.1) },
        }
    }

    /// Applies a room preset: the room changes size, the projectors and
    /// screens stay where they are. False for an unknown preset.
    pub fn apply_preset(&mut self, id: &str) -> bool {
        let Some(&(id, _, w, d, h, ceiling)) = PRESETS.iter().find(|p| p.0 == id) else { return false };
        self.preset = id.into();
        self.room = Room { width_m: w, depth_m: d, height_m: h, ceiling };
        true
    }

    /// The projectors after the main one: (id, output).
    pub fn extras(&self) -> impl Iterator<Item = &Projector> {
        self.projectors.iter().skip(1)
    }

    pub fn projector(&self, id: &str) -> Option<&Projector> {
        self.projectors.iter().find(|p| p.id == id)
    }

    /// Whether going from `self` to `next` changes which output any extra
    /// projector drives (a projector with an output added or removed, an
    /// output changed): refused while armed.
    pub fn routing_changes(&self, next: &Venue) -> bool {
        let routes = |v: &Venue| {
            let mut r: Vec<(String, String)> = v.extras().filter_map(|p| Some((p.id.clone(), p.output.clone()?))).collect();
            r.sort();
            r
        };
        routes(self) != routes(next)
    }

    /// Whether the zones of a multi-projector venue apply: more than one
    /// projector. One projector is exactly the single-output studio.
    pub fn is_multi(&self) -> bool {
        self.projectors.len() > 1
    }
}

/// The output the engine has installed for a venue projector (T-277).
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    /// The output manager id it was opened as.
    pub output: String,
    pub name: String,
    /// `Output::kind`.
    pub kind: &'static str,
}

/// What one more projector's stage did on the last frame, for
/// `GET /api/frames` (the main projector's is `Shared::frame`).
#[derive(Clone, Debug, Default)]
pub struct ProjectorFrame {
    pub id: String,
    /// After its whole safety chain: what its output was sent, before the
    /// gate (like the preview of the main output).
    pub points: Vec<crate::patterns::Point>,
    /// Lit points actually sent (0 whenever disarmed).
    pub lit: usize,
}

/// The `projectors` interlock's label: which projectors wait for their
/// output, or None when every one is there. `needed` are (projector name,
/// projector id, output id) of the projectors that drive an output.
pub fn missing_outputs(needed: &[(String, String, String)], installed: &std::collections::BTreeMap<String, Installed>) -> Option<String> {
    let missing: Vec<&str> = needed
        .iter()
        .filter(|(_, id, out)| installed.get(id).is_none_or(|i| &i.output != out))
        .map(|(name, _, _)| name.as_str())
        .collect();
    (!missing.is_empty()).then(|| format!("Sortie de projecteur non connectée : {}", missing.join(", ")))
}

/// The venue and its file.
pub struct VenueStore {
    venue: Venue,
    path: Option<PathBuf>,
}

impl VenueStore {
    /// `venue.json`, or the default venue (one projector). A damaged file
    /// is set aside (datafile.rs) and the default used: fewer outputs,
    /// never more, so it is not a safety file.
    pub fn load_or_create(path: PathBuf) -> Self {
        let venue: Venue = crate::datafile::load_or_default(&path, &crate::datafile::beside(&path), false);
        Self { venue: venue.sanitized(), path: Some(path) }
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self { venue: Venue::default(), path: None }
    }

    pub fn get(&self) -> &Venue {
        &self.venue
    }

    /// Validates, repairs and saves. The caller checks the arm state
    /// (`Venue::routing_changes`) first.
    pub fn set(&mut self, venue: Venue) -> Result<&Venue> {
        venue.validate()?;
        self.venue = venue.sanitized();
        if let Some(path) = &self.path {
            crate::datafile::save_json(Path::new(path), &self.venue);
        }
        Ok(&self.venue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Venue {
        let mut v = Venue::default();
        v.projectors.push(Projector { id: "p2".into(), name: "Jardin".into(), pos_m: [3.0, 3.0, 11.0], output: Some("b".into()), mirror_x: true, ..Default::default() });
        v
    }

    #[test]
    fn json_round_trip_is_exact() {
        let v = two().sanitized();
        let back: Venue = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back, v);
        assert_eq!(back.sanitized(), v, "sanitizing twice changes nothing");
    }

    #[test]
    fn a_file_without_screens_or_projectors_loads_with_defaults() {
        let v: Venue = serde_json::from_str(r#"{"preset":"salle","room":{"width_m":20}}"#).unwrap();
        assert!(v.screens.is_empty());
        assert_eq!(v.room.width_m, 20.0);
        assert_eq!(v.room.height_m, 5.0, "missing room fields take their default");
        assert_eq!(v.projectors.len(), 1);
        let v: Venue = serde_json::from_str(r#"{"projectors":[]}"#).unwrap();
        let v = v.sanitized();
        assert_eq!(v.projectors.len(), 1, "never without a projector");
        assert_eq!(v.projectors[0].id, MAIN_OUTPUT);
        let p: Projector = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!((p.scan_deg, p.brightness), (40.0, 1.0));
    }

    #[test]
    fn the_default_matches_the_3d_views_defaults() {
        // beam3d.js DEFAULT_ROOM {w: 10, d: 15, h: 5}, DEFAULT_PROJECTOR pos [0, 3, 11].
        let v = Venue::default();
        assert_eq!((v.room.width_m, v.room.depth_m, v.room.height_m), (10.0, 15.0, 5.0));
        assert_eq!(v.projectors[0].pos_m, [0.0, 3.0, 11.0]);
        assert!(!v.is_multi());
    }

    #[test]
    fn presets_resize_the_room_and_keep_the_projectors() {
        let mut v = two();
        assert!(v.apply_preset("festival"));
        assert_eq!((v.room.width_m, v.room.depth_m, v.room.ceiling), (40.0, 60.0, false));
        assert_eq!(v.projectors.len(), 2);
        assert!(v.apply_preset("salle"));
        assert_eq!((v.room.width_m, v.room.depth_m, v.room.height_m, v.room.ceiling), (20.0, 30.0, 10.0, true));
        assert!(!v.apply_preset("stade"));
        assert_eq!(v.preset, "salle");
    }

    #[test]
    fn sanitizing_fixes_ids_numbers_and_shared_outputs() {
        let mut v = two();
        v.projectors[0].id = "x".into();
        v.projectors.push(Projector { id: "p2".into(), output: Some("b".into()), scan_deg: f32::NAN, brightness: 3.0, ..Default::default() });
        v.projectors.push(Projector { id: "main".into(), output: Some("  ".into()), pitch_deg: 400.0, ..Default::default() });
        let s = v.sanitized();
        let ids: Vec<&str> = s.projectors.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["main", "p2", "p3", "p4"]);
        assert_eq!(s.projectors[2].output, None, "an output is driven by one projector only");
        assert_eq!((s.projectors[2].scan_deg, s.projectors[2].brightness), (40.0, 1.0));
        assert_eq!((s.projectors[3].output.clone(), s.projectors[3].pitch_deg), (None, 90.0));
        let many = Venue { projectors: vec![Projector::default(); 20], ..Default::default() }.sanitized();
        assert_eq!(many.projectors.len(), MAX_PROJECTORS);
    }

    #[test]
    fn validation_refuses_what_the_api_must_not_take() {
        assert!(two().validate().is_ok());
        assert!(Venue { projectors: vec![], ..Default::default() }.validate().is_err());
        let mut v = two();
        v.projectors.push(Projector { id: "p3".into(), output: Some("b".into()), ..Default::default() });
        assert!(v.validate().unwrap_err().to_string().contains("même sortie"));
        let mut v = two();
        v.projectors[1].id = "main".into();
        assert!(v.validate().is_err());
        let v = Venue { projectors: vec![Projector::default(); MAX_PROJECTORS + 1], ..Default::default() };
        assert!(v.validate().is_err());
    }

    #[test]
    fn routing_changes_only_when_an_extra_output_changes() {
        let a = two();
        let mut b = a.clone();
        b.projectors[1].pos_m = [9.0, 9.0, 9.0];
        b.projectors[1].mirror_x = false;
        b.projectors[0].output = Some("anything".into());
        assert!(!a.routing_changes(&b), "geometry and the main projector's field are not routing");
        b.projectors[1].output = Some("c".into());
        assert!(a.routing_changes(&b));
        assert!(a.routing_changes(&Venue::default()), "removing a projector with an output");
        let mut c = Venue::default();
        c.projectors.push(Projector { id: "p2".into(), ..Default::default() });
        assert!(!Venue::default().routing_changes(&c), "a preview-only projector drives nothing");
    }

    #[test]
    fn the_interlock_waits_for_every_projectors_output() {
        let needed = vec![("Jardin".to_string(), "p2".to_string(), "b".to_string()), ("Cour".to_string(), "p3".to_string(), "c".to_string())];
        let mut installed = std::collections::BTreeMap::new();
        assert_eq!(missing_outputs(&needed, &installed).unwrap(), "Sortie de projecteur non connectée : Jardin, Cour");
        installed.insert("p2".to_string(), Installed { output: "b".into(), name: "B".into(), kind: "test" });
        installed.insert("p3".to_string(), Installed { output: "autre".into(), name: "X".into(), kind: "test" });
        assert_eq!(missing_outputs(&needed, &installed).unwrap(), "Sortie de projecteur non connectée : Cour", "an older output does not count");
        installed.insert("p3".to_string(), Installed { output: "c".into(), name: "C".into(), kind: "test" });
        assert_eq!(missing_outputs(&needed, &installed), None);
        assert_eq!(missing_outputs(&[], &Default::default()), None);
    }

    #[test]
    fn the_store_saves_and_reloads() {
        let dir = std::env::temp_dir().join(format!("laser-studio-venue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("venue.json");
        let mut store = VenueStore::load_or_create(path.clone());
        assert_eq!(store.get(), &Venue::default());
        store.set(two()).unwrap();
        assert_eq!(VenueStore::load_or_create(path).get(), &two().sanitized());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
