//! Wireframe 3D objects (T-462): an operator's own object, imported from a
//! generic OBJ file (`figure_import::obj`), kept as its edges only and
//! turned in perspective like the `wire3d` solids of T-108.
//!
//! An object lives in `studio-data/objects/<name>.json` (our format, never
//! the OBJ file) and in the project file. Each one is also a cue of the
//! « Objets 3D » page of the grid: `Content::Wire` plays through the normal
//! path - deck, layers and point budget, live modifiers, calibration,
//! safety, then the arm gate. Nothing here can arm the laser.
//!
//! Drawing: vertices sit inside the unit sphere, the perspective only ever
//! shrinks them, and every point is clamped to -1..1 anyway. The edges are
//! stored in drawing order: an edge that starts where the previous one
//! ended continues the stroke, any other starts with a blanked point, so
//! the travel to it is dark. Hidden lines are not removed (a transparent
//! wireframe); the work is bounded by `MAX_EDGES` whatever the look holds.

use crate::engine::{Content, Settings};
use crate::figures::RateUnit;
use crate::patterns::Point;
use crate::presets::Preset;
use crate::timeline::valid_show_name;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

/// The cue page objects appear on (last of `presets::CATEGORIES`).
pub const CATEGORY: &str = "Objets 3D";
/// Cue id prefix: `object:<name>`.
pub const ID_PREFIX: &str = "object:";
/// Edges an object keeps, after merging and simplification.
pub const MAX_EDGES: usize = 2_000;
/// Vertices a stored object may hold: at most both ends of every edge.
pub const MAX_STORED_VERTICES: usize = 2 * MAX_EDGES;
/// One turn takes this many beats (or seconds), 1/4 to 1024.
pub const MIN_PERIOD: f32 = 0.25;
pub const MAX_PERIOD: f32 = 1024.0;
/// The second axis turns this many times faster than the first (signed).
pub const MAX_AXIS_RATIO: f32 = 4.0;
/// Camera distance in radii: close (strong perspective) to far (flat).
pub const MIN_PERSPECTIVE: f32 = 1.5;
pub const MAX_PERSPECTIVE: f32 = 20.0;
/// Size of an object cue (the look's `scale`).
pub const CUE_SCALE: f32 = 0.8;
/// The horizontal axis' fixed tilt (radians), so it is never seen edge-on.
const TILT: f32 = 0.45;
/// With *Couleur selon la profondeur*: the brightness of the farthest point.
const FAR_GAIN: f32 = 0.3;
const NAME_RULE: &str = "lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WireObject {
    pub name: String,
    /// Normalised: centred, radius ≤ 1.
    pub vertices: Vec<[f32; 3]>,
    /// Vertex index pairs, in drawing order.
    pub edges: Vec<[u32; 2]>,
    pub color: [u8; 3],
    /// One turn per this many beats (or seconds, see `per`): 8 = 2 bars.
    pub period_beats: f32,
    /// Free-running (seconds) instead of locked to the tempo.
    #[serde(skip_serializing_if = "is_beat")]
    pub per: RateUnit,
    /// Speed of the second axis relative to the first.
    pub axis_ratio: f32,
    /// Camera distance, in radii.
    pub perspective: f32,
    /// Far edges dimmer than near ones.
    pub depth_color: bool,
}

fn is_beat(u: &RateUnit) -> bool {
    *u == RateUnit::Beat
}

impl Default for WireObject {
    fn default() -> Self {
        Self {
            name: String::new(),
            vertices: Vec::new(),
            edges: Vec::new(),
            color: [0, 255, 0],
            period_beats: 8.0,
            per: RateUnit::Beat,
            axis_ratio: 0.5,
            perspective: 3.0,
            depth_color: false,
        }
    }
}

/// `v` if finite, else `default`, within `lo..=hi`.
fn finite_or(v: f32, default: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

/// A vertex inside the unit sphere: not finite → the centre, farther than
/// 1 → brought back onto the sphere.
fn unit(v: [f32; 3]) -> [f32; 3] {
    if !v.iter().all(|c| c.is_finite()) {
        return [0.0; 3];
    }
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 1.0 {
        v.map(|c| c / n)
    } else {
        v
    }
}

impl WireObject {
    /// Checks the name and the size limits, and brings everything else
    /// into range: vertices inside the unit sphere, edges that name a
    /// missing vertex (or the same one twice) dropped, settings clamped.
    pub fn validate(&mut self) -> Result<()> {
        self.name = self.name.trim().to_string();
        if !valid_show_name(&self.name) {
            bail!("nom d'objet invalide : « {} » ({NAME_RULE})", self.name);
        }
        if self.edges.len() > MAX_EDGES {
            bail!("{}", too_detailed(self.edges.len()));
        }
        if self.vertices.len() > MAX_STORED_VERTICES {
            bail!("objet « {} » : {} sommets, {} au plus", self.name, self.vertices.len(), MAX_STORED_VERTICES);
        }
        for v in &mut self.vertices {
            *v = unit(*v);
        }
        let n = self.vertices.len() as u32;
        self.edges.retain(|&[a, b]| a < n && b < n && a != b);
        self.period_beats = finite_or(self.period_beats, 8.0, MIN_PERIOD, MAX_PERIOD);
        self.axis_ratio = finite_or(self.axis_ratio, 0.5, -MAX_AXIS_RATIO, MAX_AXIS_RATIO);
        self.perspective = finite_or(self.perspective, 3.0, MIN_PERSPECTIVE, MAX_PERSPECTIVE);
        Ok(())
    }

    /// The object `pos` beats (or seconds, see `per`) after its start, as
    /// laser points `scale` times its size. `paint` turns the object's
    /// colour into the output colour (hue shift, brightness). Works on any
    /// content, checked or not: at most `MAX_EDGES` edges are drawn, bad
    /// vertices are skipped, every point is within -1..1.
    pub fn points(&self, pos: f64, scale: f32, paint: impl Fn([u8; 3]) -> (f32, f32, f32)) -> Vec<Point> {
        let period = finite_or(self.period_beats, 8.0, MIN_PERIOD, MAX_PERIOD) as f64;
        let ratio = finite_or(self.axis_ratio, 0.5, -MAX_AXIS_RATIO, MAX_AXIS_RATIO) as f64;
        let camera = finite_or(self.perspective, 3.0, MIN_PERSPECTIVE, MAX_PERSPECTIVE);
        let scale = finite_or(scale, 0.0, 0.0, 1.0);
        let turns = if pos.is_finite() { pos / period } else { 0.0 };
        // Each axis' angle from its own fraction of a turn, so a long run
        // keeps its precision and the motion stays continuous.
        let yaw = TAU * turns.rem_euclid(1.0) as f32;
        let pitch = TILT + TAU * (turns * ratio).rem_euclid(1.0) as f32;
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let (r, g, b) = paint(self.color);
        let project = |i: u32| -> Option<Point> {
            let v = unit(*self.vertices.get(i as usize)?);
            let (x, z) = (v[0] * cy + v[2] * sy, -v[0] * sy + v[2] * cy);
            let (y, z) = (v[1] * cp - z * sp, v[1] * sp + z * cp);
            // z ≤ 1 inside the unit sphere: the factor is at most 1.
            let k = (camera - 1.0) / (camera - z.min(1.0));
            let (x, y) = ((x * k * scale).clamp(-1.0, 1.0), (y * k * scale).clamp(-1.0, 1.0));
            let d = if self.depth_color { FAR_GAIN + (1.0 - FAR_GAIN) * (z.clamp(-1.0, 1.0) + 1.0) / 2.0 } else { 1.0 };
            Some(Point::lit(x, y, r * d, g * d, b * d))
        };
        let mut out = Vec::new();
        let mut end: Option<u32> = None;
        for &[a, b] in self.edges.iter().take(MAX_EDGES) {
            let (Some(pa), Some(pb)) = (project(a), project(b)) else {
                end = None;
                continue;
            };
            if a == b {
                continue;
            }
            if end != Some(a) {
                // A new stroke: the travel to it is dark.
                out.push(Point::blanked(pa.x, pa.y));
                out.push(pa);
            }
            out.push(pb);
            end = Some(b);
        }
        if !out.iter().any(Point::is_lit) {
            // Black: travel only (as a black figure stroke).
            for p in &mut out {
                *p = Point::blanked(p.x, p.y);
            }
        }
        out
    }

    /// Strokes the object is drawn in (each one starts with a dark travel).
    pub fn strokes(&self) -> usize {
        let mut end = None;
        let mut n = 0;
        for &[a, b] in &self.edges {
            if end != Some(a) {
                n += 1;
            }
            end = Some(b);
        }
        n
    }

    /// Laser points per frame as the engine draws it at the cue's size
    /// (after `densify`, travel included): the most over one turn.
    pub fn estimated_points(&self) -> usize {
        let white = |_| (1.0, 1.0, 1.0);
        let period = finite_or(self.period_beats, 8.0, MIN_PERIOD, MAX_PERIOD) as f64;
        (0..8).map(|k| crate::engine::densify(&self.points(period * k as f64 / 8.0, CUE_SCALE, white)).len()).max().unwrap_or(0)
    }
}

/// « Objet trop détaillé : 5 400 arêtes, 2 000 au plus. … »
pub fn too_detailed(edges: usize) -> String {
    format!(
        "Objet trop détaillé : {} arêtes, {} au plus. Augmentez le seuil ou gardez moins d'arêtes.",
        thousands(edges),
        thousands(MAX_EDGES)
    )
}

/// 5400 → « 5 400 » (French grouping, with a plain space).
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The cue id of an object.
pub fn cue_id(name: &str) -> String {
    format!("{ID_PREFIX}{name}")
}

/// An object as a cue of the « Objets 3D » page.
pub fn preset_of(obj: &WireObject) -> Preset {
    Preset {
        id: cue_id(&obj.name),
        name: obj.name.clone(),
        category: CATEGORY,
        settings: Settings { content: Content::Wire(obj.clone()), color: obj.color, scale: CUE_SCALE, ..Settings::default() },
        tag: None,
    }
}

/// Puts the library's cues into `presets`, without rebuilding the grid
/// (`refresh` does; a project open lets `figures::refresh_with` do it).
pub fn set_presets(s: &mut crate::Shared, objects: Vec<Preset>) {
    s.presets.retain(|p| p.category != CATEGORY);
    s.presets.extend(objects);
}

/// Rebuilds the « Objets 3D » cue page (and the grid controls for MIDI)
/// from the library. Called whenever the library changes.
pub fn refresh(s: &mut crate::Shared) {
    let objects: Vec<Preset> = s.objects.list().iter().map(preset_of).collect();
    set_presets(s, objects);
    crate::grid::refresh(s);
}

/// The object library: `studio-data/objects/`, one JSON file per object,
/// kept in memory (sorted by name), like `figures::FigureStore`.
pub struct WireStore {
    dir: Option<PathBuf>,
    list: Vec<WireObject>,
}

impl WireStore {
    /// Every valid object in `dir`. Unreadable or invalid files are moved
    /// to `<data-dir>/quarantine` (T-402).
    pub fn load(dir: PathBuf) -> Self {
        let quarantine = dir.parent().unwrap_or(Path::new(".")).join(crate::datafile::QUARANTINE);
        let mut list = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for path in entries.flatten().map(|e| e.path()) {
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
                if path.extension().is_none_or(|x| x != "json") || !valid_show_name(stem) || !path.is_file() {
                    continue;
                }
                match read(&path, stem) {
                    Ok(obj) => list.push(obj),
                    Err(e) => {
                        crate::datafile::set_aside(&path, &quarantine, &format!("{e:#}"), false);
                    }
                }
            }
        }
        Self { dir: Some(dir), list: sorted(list) }
    }

    /// A library that never touches the disk (tests).
    pub fn in_memory() -> Self {
        Self { dir: None, list: Vec::new() }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn list(&self) -> &[WireObject] {
        &self.list
    }

    pub fn get(&self, name: &str) -> Option<&WireObject> {
        self.list.iter().find(|o| o.name == name.trim())
    }

    /// The file of an object: names are file names, so it can never leave
    /// the objects folder.
    fn path(&self, name: &str) -> Option<PathBuf> {
        valid_show_name(name).then(|| self.dir.as_ref().map(|d| d.join(format!("{name}.json"))))?
    }

    /// Checks, writes (atomically) and keeps an object, replacing the one
    /// with the same name. Returns what was kept.
    pub fn save(&mut self, mut obj: WireObject) -> Result<WireObject> {
        obj.validate()?;
        if let Some(path) = self.path(&obj.name) {
            let json = serde_json::to_vec_pretty(&obj)?;
            crate::project::write_atomic(&path, &json).context("échec de l'enregistrement de l'objet")?;
        }
        self.list.retain(|o| o.name != obj.name);
        self.list.push(obj.clone());
        self.list = sorted(std::mem::take(&mut self.list));
        Ok(obj)
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if !self.list.iter().any(|o| o.name == name) {
            bail!("objet introuvable : {name}");
        }
        if let Some(path) = self.path(name) {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).context("échec de la suppression de l'objet"),
                _ => {}
            }
        }
        self.list.retain(|o| o.name != name);
        Ok(())
    }

    /// A project's objects (already checked), memory only; `project.rs`
    /// writes the files.
    pub fn replace_in_memory(&mut self, list: Vec<WireObject>) {
        self.list = sorted(list);
    }
}

/// A library in the store's order (by name).
pub fn sorted(mut list: Vec<WireObject>) -> Vec<WireObject> {
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

fn read(path: &Path, name: &str) -> Result<WireObject> {
    let json = std::fs::read_to_string(path).context("objet illisible")?;
    let mut obj: WireObject = serde_json::from_str(&json).context("objet illisible")?;
    obj.name = name.to_string();
    obj.validate()?;
    Ok(obj)
}

/// A unit cube's 12 edges (tests).
#[cfg(test)]
pub fn test_cube(name: &str) -> WireObject {
    let k = 1.0 / 3f32.sqrt();
    let vertices = (0..8).map(|i| [if i & 1 == 0 { -k } else { k }, if i & 2 == 0 { -k } else { k }, if i & 4 == 0 { -k } else { k }]).collect();
    let edges = (0..8u32).flat_map(|i| [1u32, 2, 4].into_iter().filter(move |bit| i & bit == 0).map(move |bit| [i, i | bit])).collect();
    WireObject { name: name.into(), vertices, edges, ..WireObject::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, AudioFeatures, BeatClock};

    fn white(c: [u8; 3]) -> (f32, f32, f32) {
        (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-wire-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A tiny deterministic generator (no extra crate).
    fn lcg(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 40) as f32 / (1u64 << 24) as f32
    }

    #[test]
    fn every_point_stays_in_the_field_at_any_rotation() {
        let mut seed = 7;
        let mut cube = test_cube("c");
        for _ in 0..500 {
            cube.axis_ratio = lcg(&mut seed) * 8.0 - 4.0;
            cube.perspective = MIN_PERSPECTIVE + lcg(&mut seed) * 10.0;
            cube.depth_color = lcg(&mut seed) > 0.5;
            let pos = (lcg(&mut seed) as f64 - 0.5) * 1e7;
            let pts = cube.points(pos, 1.0, white);
            assert!(!pts.is_empty());
            assert!(pts.iter().all(|p| (-1.0..=1.0).contains(&p.x) && (-1.0..=1.0).contains(&p.y)), "{pos}");
            assert!(pts.iter().all(|p| (0.0..=1.0).contains(&p.g)));
        }
        // Wild content (a look sent by a client, never validated): still in the field.
        let wild = WireObject {
            vertices: vec![[f32::NAN, 0.0, 0.0], [1e30, -1e30, 5.0], [0.5, 0.5, 0.5], [f32::INFINITY; 3]],
            edges: vec![[0, 1], [1, 2], [2, 9], [3, 3], [2, 3], [u32::MAX, 0]],
            period_beats: f32::NAN,
            axis_ratio: f32::INFINITY,
            perspective: -4.0,
            ..WireObject::default()
        };
        for pos in [0.0, 1.5, f64::NAN, f64::INFINITY, -1e300] {
            let pts = wild.points(pos, 5.0, white);
            assert!(pts.iter().all(|p| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 1.0 && p.y.abs() <= 1.0));
        }
        // Many edges: only `MAX_EDGES` are drawn.
        let big = WireObject { vertices: vec![[0.0, 0.0, 0.0], [0.5, 0.0, 0.0]], edges: vec![[0, 1]; 50_000], ..WireObject::default() };
        assert_eq!(big.points(0.0, 1.0, white).len(), 3 * MAX_EDGES);
    }

    #[test]
    fn travel_between_edges_that_dont_follow_on_is_dark() {
        // Two separate segments: lit, dark jump, lit.
        let obj = WireObject { vertices: vec![[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.5, 0.5]], edges: vec![[0, 1], [2, 3]], ..WireObject::default() };
        let pts = obj.points(0.0, 1.0, white);
        assert_eq!(pts.len(), 6);
        assert!(!pts[0].is_lit() && pts[1].is_lit() && pts[2].is_lit());
        assert!(!pts[3].is_lit() && pts[4].is_lit() && pts[5].is_lit());
        assert_eq!(obj.strokes(), 2);
        // A chained path: one stroke.
        let path = WireObject { edges: vec![[0, 1], [1, 2], [2, 3]], ..obj.clone() };
        assert_eq!(path.points(0.0, 1.0, white).len(), 5);
        assert_eq!(path.strokes(), 1);
        // Black: travel only.
        assert!(obj.points(0.0, 1.0, |_| (0.0, 0.0, 0.0)).iter().all(|p| !p.is_lit()));
    }

    #[test]
    fn turns_one_turn_per_period_in_tempo_and_depth_dims_the_back() {
        let cube = test_cube("c");
        let at = |pos: f64| cube.points(pos, 1.0, white);
        let close = |a: &[Point], b: &[Point]| a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (p.x - q.x).abs() < 1e-3 && (p.y - q.y).abs() < 1e-3);
        assert!(close(&at(0.0), &at(16.0)), "a turn every 8 beats, the second axis at half speed: back where it started after 16");
        assert!(!close(&at(0.0), &at(8.0)), "the second axis is half-way");
        assert!(!close(&at(0.0), &at(1.0)), "it turns");
        let deep = WireObject { depth_color: true, ..cube.clone() };
        let pts = deep.points(1.0, 1.0, white);
        let greens: Vec<f32> = pts.iter().filter(|p| p.is_lit()).map(|p| p.g).collect();
        assert!(greens.iter().any(|g| *g < 0.7) && greens.iter().any(|g| *g > 0.8), "{greens:?}");
    }

    #[test]
    fn validate_confines_names_and_clamps_values() {
        let mut obj = test_cube("Logo 1");
        obj.vertices[0] = [3.0, 0.0, f32::NAN];
        obj.vertices[1] = [3.0, 4.0, 0.0];
        obj.edges.push([0, 99]);
        obj.edges.push([2, 2]);
        obj.period_beats = 1e9;
        obj.perspective = 0.0;
        obj.validate().unwrap();
        assert_eq!(obj.vertices[0], [0.0; 3]);
        assert!((obj.vertices[1][0] - 0.6).abs() < 1e-6 && (obj.vertices[1][1] - 0.8).abs() < 1e-6);
        assert_eq!(obj.edges.len(), 12);
        assert_eq!((obj.period_beats, obj.perspective), (MAX_PERIOD, MIN_PERSPECTIVE));
        for bad in ["", "../x", "a/b", "x.json", " ", &"n".repeat(65)] {
            assert!(WireObject { name: bad.into(), ..Default::default() }.validate().is_err(), "{bad:?}");
        }
        let mut big = WireObject { name: "big".into(), edges: vec![[0, 1]; MAX_EDGES + 1], ..Default::default() };
        let e = big.validate().unwrap_err().to_string();
        assert!(e.contains("Objet trop détaillé : 2 001 arêtes, 2 000 au plus"), "{e}");
        assert_eq!(thousands(5400), "5 400");
        assert_eq!(thousands(12), "12");
        assert_eq!(thousands(1_234_567), "1 234 567");
    }

    #[test]
    fn the_store_keeps_objects_across_a_restart_and_quarantines_bad_files() {
        let dir = temp_dir("store").join("objects");
        let mut store = WireStore::load(dir.clone());
        store.save(test_cube("Cube")).unwrap();
        assert!(dir.join("Cube.json").is_file());
        std::fs::write(dir.join("Abîmé.json"), "{ pas du json").unwrap();
        let again = WireStore::load(dir.clone());
        assert_eq!(again.list(), &[test_cube("Cube")][..]);
        assert!(!dir.join("Abîmé.json").exists(), "set aside");
        let mut again = again;
        again.remove("Cube").unwrap();
        assert!(again.remove("Cube").is_err());
        assert!(WireStore::load(dir.clone()).list().is_empty());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn an_object_cue_renders_through_the_engine_within_the_field() {
        let s = Settings { content: Content::Wire(test_cube("c")), scale: 1.0, brightness: 1.0, ..Settings::default() };
        let mut a = Animator::default();
        let mut seen = Vec::new();
        for k in 0..20 {
            let clock = BeatClock { beat: k as f64 * 0.25, ..BeatClock::default() };
            let pts = a.render(&s, AudioFeatures::default(), 1.0 / 60.0, &clock);
            assert!(pts.iter().any(Point::is_lit));
            assert!(pts.iter().all(|p| p.x.abs() <= 1.0 && p.y.abs() <= 1.0));
            seen.push(pts[1].x);
        }
        assert!(seen.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-3), "it turns with the tempo clock");
        // The mixer keeps the frame within the budget, the estimate is what the engine draws.
        let cube = test_cube("c");
        let mut a = Animator::default();
        let frame = a.render(&preset_of(&cube).settings, AudioFeatures::default(), 1.0 / 60.0, &BeatClock::default());
        assert!(frame.len() <= cube.estimated_points() && cube.estimated_points() < crate::layers::DEFAULT_POINT_BUDGET);
        let mixer = crate::layers::Mixer::default();
        let (mixed, report) = crate::layers::mix(vec![(1, frame.clone()), (2, frame)], &mixer);
        assert!(mixed.len() <= mixer.point_budget, "{report:?}");
        // The JSON of an object content round-trips; a bare one loads with the defaults.
        let json = serde_json::to_string(&s.content).unwrap();
        assert_eq!(serde_json::from_str::<Content>(&json).unwrap(), s.content);
        let bare: WireObject = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!((bare.period_beats, bare.axis_ratio, bare.perspective, bare.color), (8.0, 0.5, 3.0, [0, 255, 0]));
        assert!(Content::Wire(test_cube("c")).same_drawing(&Content::Wire(WireObject { axis_ratio: 2.0, ..test_cube("c") })));
        assert!(!Content::Wire(test_cube("c")).same_drawing(&Content::Wire(test_cube("d"))));
    }
}
