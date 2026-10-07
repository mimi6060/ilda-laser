//! Figures (T-296): the operator's own drawings, made in the CRÉATION ›
//! Figures editor. A figure is a list of frames; a frame is a list of
//! strokes drawn in order, each one a polyline with its own colour, lit or
//! blanked (an explicit move with the beam off). Several frames make an
//! animation, stepped per beat (locked to the tempo clock) or per second.
//!
//! Figures live in `studio-data/figures/<name>.json` (names are file names,
//! confined to that folder like shows) and in the project file. Each one
//! is also a cue on the « Figures » page of the grid: `Content::Figure`
//! plays through the normal path - deck, layers and point budget, live
//! modifiers, calibration, safety, then the arm gate. Nothing here can
//! arm the laser.

use crate::engine::{Content, Settings};
use crate::patterns::Point;
use crate::presets::Preset;
use crate::timeline::valid_show_name;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The cue page figures appear on (last of `presets::CATEGORIES`).
pub const CATEGORY: &str = "Figures";
/// Cue id prefix: `figure:<name>`.
pub const ID_PREFIX: &str = "figure:";
pub const MAX_FRAMES: usize = 256;
pub const MAX_STROKES: usize = 1_000;
/// Points in the whole figure, every frame together.
pub const MAX_POINTS: usize = 20_000;
/// Frames per beat or per second.
pub const MIN_RATE: f32 = 0.05;
pub const MAX_RATE: f32 = 60.0;
/// Parts a figure may name (T-441): strokes carry 0 (no part) or 1..=8.
pub const MAX_PARTS: u8 = 8;
/// A one-point stroke (the point tool) is held this many samples, so the
/// dot is visible.
const DOT_DWELL: usize = 8;
const NAME_RULE: &str = "lettres, chiffres, espaces, - et _ seulement, 64 caractères au plus";

/// One polyline. A single point is a dot. `lit: false` is a blanked move:
/// the beam travels along it switched off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
    pub color: [u8; 3],
    pub lit: bool,
    /// Part (T-441): 0 = no part, 1..=`MAX_PARTS` = one of the figure's
    /// `parts`, the target of per-part effects. 0 in older files.
    #[serde(skip_serializing_if = "is_zero")]
    pub part: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

impl Default for Stroke {
    fn default() -> Self {
        Self { points: Vec::new(), color: [0, 255, 0], lit: true, part: 0 }
    }
}

/// A named part of a figure (T-441): « Yeux », « Contour »… Its strokes,
/// in every frame, carry its `id`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartDef {
    /// 1..=`MAX_PARTS`.
    pub id: u8,
    pub name: String,
}

/// Longest part name kept.
const MAX_PART_NAME: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FigureFrame {
    /// Drawn in this order, with blanked travel between them.
    pub strokes: Vec<Stroke>,
}

/// What `Figure::rate` counts frames per.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateUnit {
    /// Frames per beat: locked to the tempo clock, counted from the bar
    /// the cue started in.
    #[default]
    Beat,
    Second,
}

/// What happens after the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    Loop,
    /// Back and forth: 1 2 3 2 1 2 3…
    PingPong,
    /// Stops on the last frame.
    Once,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Figure {
    pub name: String,
    pub frames: Vec<FigureFrame>,
    /// Frames per `per`.
    pub rate: f32,
    pub per: RateUnit,
    pub loop_mode: LoopMode,
    /// Scan preset (T-415, `scan.rs`); `None` = the output's default. A
    /// grid cell's or a timeline event's own preset comes first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
    /// Named parts (T-441), common to every frame. Empty in older files.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartDef>,
}

impl Default for Figure {
    fn default() -> Self {
        Self { name: String::new(), frames: vec![FigureFrame::default()], rate: 1.0, per: RateUnit::Beat, loop_mode: LoopMode::Loop, scan: None, parts: Vec::new() }
    }
}

impl Figure {
    pub fn point_count(&self) -> usize {
        self.frames.iter().flat_map(|f| &f.strokes).map(|s| s.points.len()).sum()
    }

    /// Checks the name and the size limits, and brings everything else
    /// into range: coordinates in -1..1 (not finite → 0), empty strokes
    /// dropped, at least one frame, rate within `MIN_RATE..=MAX_RATE`.
    pub fn validate(&mut self) -> Result<()> {
        self.name = self.name.trim().to_string();
        if !valid_show_name(&self.name) {
            bail!("nom de figure invalide : « {} » ({NAME_RULE})", self.name);
        }
        if self.frames.len() > MAX_FRAMES {
            bail!("figure « {} » : {} images, {MAX_FRAMES} au plus", self.name, self.frames.len());
        }
        if let Some(n) = self.frames.iter().map(|f| f.strokes.len()).find(|&n| n > MAX_STROKES) {
            bail!("figure « {} » : {n} tracés dans une image, {MAX_STROKES} au plus", self.name);
        }
        if self.point_count() > MAX_POINTS {
            bail!("figure « {} » : {} points, {MAX_POINTS} au plus", self.name, self.point_count());
        }
        for frame in &mut self.frames {
            frame.strokes.retain(|s| !s.points.is_empty());
            for p in frame.strokes.iter_mut().flat_map(|s| s.points.iter_mut()).flatten() {
                *p = if p.is_finite() { p.clamp(-1.0, 1.0) } else { 0.0 };
            }
        }
        if self.frames.is_empty() {
            self.frames.push(FigureFrame::default());
        }
        // Parts (T-441): ids in 1..=MAX_PARTS, each once, named; a stroke
        // whose part is out of range is back in « no part ».
        let mut seen = [false; MAX_PARTS as usize + 1];
        self.parts.retain(|p| (1..=MAX_PARTS).contains(&p.id) && !std::mem::replace(&mut seen[p.id as usize], true));
        self.parts.sort_by_key(|p| p.id);
        for p in &mut self.parts {
            let name: String = p.name.trim().chars().filter(|c| !c.is_control()).take(MAX_PART_NAME).collect();
            p.name = if name.is_empty() { format!("Partie {}", p.id) } else { name };
        }
        for s in self.frames.iter_mut().flat_map(|f| f.strokes.iter_mut()) {
            if s.part > MAX_PARTS {
                s.part = 0;
            }
        }
        self.rate = if self.rate.is_finite() { self.rate.clamp(MIN_RATE, MAX_RATE) } else { 1.0 };
        Ok(())
    }

    /// The frame shown `pos` beats (or seconds, see `per`) after the start.
    pub fn frame_index(&self, pos: f64) -> usize {
        let n = self.frames.len();
        if n <= 1 {
            return 0;
        }
        let rate = if self.rate.is_finite() { self.rate.clamp(MIN_RATE, MAX_RATE) } else { 1.0 };
        // Capped so a very long run can't overflow the cast.
        let step = (pos.max(0.0) * rate as f64).floor().min(1e15) as u64;
        let n64 = n as u64;
        (match self.loop_mode {
            LoopMode::Loop => step % n64,
            LoopMode::Once => step.min(n64 - 1),
            LoopMode::PingPong => {
                let period = 2 * n64 - 2;
                let k = step % period;
                if k < n64 {
                    k
                } else {
                    period - k
                }
            }
        }) as usize
    }

    /// One frame as laser points, `scale` times its size. `paint` turns a
    /// stroke colour into the output colour (hue shift, brightness). Every
    /// stroke starts with a blanked point, so the travel to it is dark; a
    /// blanked stroke is travel all along.
    pub fn frame_points(&self, index: usize, scale: f32, paint: impl Fn([u8; 3]) -> (f32, f32, f32)) -> Vec<Point> {
        self.frame_points_tagged(index, scale, paint).0
    }

    /// `frame_points`, with the part of every point beside it (T-441):
    /// both vectors have the same length. The blanked arrival of a stroke
    /// belongs to that stroke's part, so the travel to it is never lit by
    /// an effect on another part (nor by one on its own: effects only dim
    /// and move points, a blanked point stays blanked).
    pub fn frame_points_tagged(&self, index: usize, scale: f32, paint: impl Fn([u8; 3]) -> (f32, f32, f32)) -> (Vec<Point>, Vec<u8>) {
        let Some(frame) = self.frames.get(index) else { return (Vec::new(), Vec::new()) };
        let (mut out, mut parts) = (Vec::new(), Vec::new());
        for stroke in &frame.strokes {
            let before = out.len();
            let pts = stroke.points.iter().map(|p| (p[0] * scale, p[1] * scale));
            let (r, g, b) = if stroke.lit { paint(stroke.color) } else { (0.0, 0.0, 0.0) };
            if r <= 0.0 && g <= 0.0 && b <= 0.0 {
                out.extend(pts.map(|(x, y)| Point::blanked(x, y)));
            } else if let Some(&[x0, y0]) = stroke.points.first() {
                out.push(Point::blanked(x0 * scale, y0 * scale));
                if stroke.points.len() == 1 {
                    out.extend(std::iter::repeat_n(Point::lit(x0 * scale, y0 * scale, r, g, b), DOT_DWELL));
                } else {
                    out.extend(pts.map(|(x, y)| Point::lit(x, y, r, g, b)));
                }
            }
            let part = if stroke.part <= MAX_PARTS { stroke.part } else { 0 };
            parts.resize(out.len().max(before), part);
        }
        (out, parts)
    }

    /// Whether any stroke of any frame belongs to a part.
    pub fn has_parts(&self) -> bool {
        self.frames.iter().flat_map(|f| &f.strokes).any(|s| s.part != 0)
    }

    /// The name of part `id`, if the figure has it.
    pub fn part_name(&self, id: u8) -> Option<&str> {
        self.parts.iter().find(|p| p.id == id).map(|p| p.name.as_str())
    }
}

/// The cue id of a figure.
pub fn cue_id(name: &str) -> String {
    format!("{ID_PREFIX}{name}")
}

/// A figure as a cue of the « Figures » page: drawn at its own size.
pub fn preset_of(fig: &Figure) -> Preset {
    Preset {
        id: cue_id(&fig.name),
        name: fig.name.clone(),
        category: CATEGORY,
        settings: Settings { content: Content::Figure(fig.clone()), scale: 1.0, ..Settings::default() },
        tag: None,
    }
}

/// Rebuilds the « Figures » cue page (and the grid controls for MIDI)
/// from the library. Called whenever the library changes.
pub fn refresh(s: &mut crate::Shared) {
    s.presets.retain(|p| p.category != CATEGORY);
    let figures: Vec<Preset> = s.figures.list().iter().map(preset_of).collect();
    s.presets.extend(figures);
    crate::grid::refresh(s);
}

/// The figure library: `studio-data/figures/`, one JSON file per figure,
/// kept in memory (sorted by name) so the project and the cue grid read
/// it without touching the disk.
pub struct FigureStore {
    dir: Option<PathBuf>,
    list: Vec<Figure>,
}

impl FigureStore {
    /// Every valid figure in `dir`. Unreadable or invalid files are moved
    /// to `<data-dir>/quarantine` and reported (T-402): left in place, the
    /// next project open would delete them.
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
                    Ok(fig) => list.push(fig),
                    Err(e) => {
                        crate::datafile::set_aside(&path, &quarantine, &format!("{e:#}"), false);
                    }
                }
            }
        }
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Self { dir: Some(dir), list }
    }

    /// A library that never touches the disk (tests).
    pub fn in_memory() -> Self {
        Self { dir: None, list: Vec::new() }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn list(&self) -> &[Figure] {
        &self.list
    }

    pub fn get(&self, name: &str) -> Option<&Figure> {
        self.list.iter().find(|f| f.name == name.trim())
    }

    /// The file of a figure: names are file names, so it can never leave
    /// the figures folder.
    fn path(&self, name: &str) -> Option<PathBuf> {
        valid_show_name(name).then(|| self.dir.as_ref().map(|d| d.join(format!("{name}.json"))))?
    }

    /// Checks, writes (atomically) and keeps a figure, replacing the one
    /// with the same name. Returns what was kept.
    pub fn save(&mut self, mut fig: Figure) -> Result<Figure> {
        fig.validate()?;
        if let Some(path) = self.path(&fig.name) {
            let json = serde_json::to_vec_pretty(&fig)?;
            crate::project::write_atomic(&path, &json).context("échec de l'enregistrement de la figure")?;
        }
        self.list.retain(|f| f.name != fig.name);
        self.list.push(fig.clone());
        self.list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(fig)
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if !self.list.iter().any(|f| f.name == name) {
            bail!("figure introuvable : {name}");
        }
        if let Some(path) = self.path(name) {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).context("échec de la suppression de la figure"),
                _ => {}
            }
        }
        self.list.retain(|f| f.name != name);
        Ok(())
    }

    /// A project's figures (already checked), memory only; `project.rs`
    /// writes the files.
    pub fn replace_in_memory(&mut self, mut list: Vec<Figure>) {
        list.sort_by(|a, b| a.name.cmp(&b.name));
        self.list = list;
    }
}

fn read(path: &Path, name: &str) -> Result<Figure> {
    let json = std::fs::read_to_string(path).context("figure illisible")?;
    let mut fig: Figure = serde_json::from_str(&json).context("figure illisible")?;
    fig.name = name.to_string();
    fig.validate()?;
    Ok(fig)
}

#[cfg(test)]
pub fn test_figure(name: &str, frames: usize) -> Figure {
    // Frame i: a horizontal red line at height i/10, then a blanked move,
    // then a green dot.
    let frames = (0..frames)
        .map(|i| {
            let y = i as f32 / 10.0;
            FigureFrame {
                strokes: vec![
                    Stroke { points: vec![[-0.5, y], [0.5, y]], color: [255, 0, 0], lit: true, part: 0 },
                    Stroke { points: vec![[0.5, -0.5], [0.0, -0.8]], lit: false, ..Default::default() },
                    Stroke { points: vec![[0.0, -0.8]], color: [0, 255, 0], lit: true, part: 0 },
                ],
            }
        })
        .collect();
    Figure { name: name.into(), frames, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Animator, AudioFeatures, BeatClock};

    fn white(c: [u8; 3]) -> (f32, f32, f32) {
        (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laser-studio-figures-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn strokes_become_points_in_order_with_blanked_travel_and_moves() {
        let fig = test_figure("f", 1);
        let pts = fig.frame_points(0, 1.0, white);
        // Blanked arrival, the red line, the blanked move (2 points), then
        // the green dot: blanked arrival + held dot.
        assert_eq!(pts.len(), 1 + 2 + 2 + 1 + DOT_DWELL);
        assert!(!pts[0].is_lit() && (pts[0].x, pts[0].y) == (-0.5, 0.0));
        assert_eq!((pts[1].r, pts[1].g), (1.0, 0.0));
        assert!(pts[3..6].iter().all(|p| !p.is_lit()));
        assert!(pts[6..].iter().all(|p| (p.x, p.y) == (0.0, -0.8)));
        assert!(pts[6..].iter().filter(|p| p.is_lit()).all(|p| p.g == 1.0 && p.r == 0.0));
        // Scale, and a black lit stroke is travel.
        let half = fig.frame_points(0, 0.5, white);
        assert_eq!((half[1].x, half[1].y), (-0.25, 0.0));
        assert!(fig.frame_points(0, 1.0, |_| (0.0, 0.0, 0.0)).iter().all(|p| !p.is_lit()));
        assert!(fig.frame_points(3, 1.0, white).is_empty());
    }

    #[test]
    fn frames_loop_ping_pong_or_stop() {
        let mut fig = test_figure("f", 4);
        fig.rate = 2.0; // two frames per beat
        let at = |fig: &Figure, beats: &[f64]| beats.iter().map(|&b| fig.frame_index(b)).collect::<Vec<_>>();
        assert_eq!(at(&fig, &[0.0, 0.49, 0.5, 1.0, 1.5, 2.0, 2.6]), [0, 0, 1, 2, 3, 0, 1]);
        fig.loop_mode = LoopMode::PingPong;
        assert_eq!(at(&fig, &[0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5]), [0, 1, 2, 3, 2, 1, 0, 1]);
        fig.loop_mode = LoopMode::Once;
        assert_eq!(at(&fig, &[0.0, 1.5, 9.0, 1e30]), [0, 3, 3, 3]);
        assert_eq!(test_figure("one", 1).frame_index(123.0), 0);
        assert_eq!(fig.frame_index(-4.0), 0);
    }

    #[test]
    fn validate_confines_names_and_clamps_values() {
        let mut fig = test_figure("Logo 1", 2);
        fig.frames[0].strokes[0].points[0] = [3.0, f32::NAN];
        fig.frames[1].strokes.push(Stroke::default());
        fig.rate = 1000.0;
        fig.validate().unwrap();
        assert_eq!(fig.frames[0].strokes[0].points[0], [1.0, 0.0]);
        assert_eq!(fig.frames[1].strokes.len(), 3, "empty stroke dropped");
        assert_eq!(fig.rate, MAX_RATE);
        for bad in ["", "../x", "a/b", "x.json", " ", &"n".repeat(65)] {
            assert!(Figure { name: bad.into(), ..Default::default() }.validate().is_err(), "{bad:?}");
        }
        let mut empty = Figure { name: "e".into(), frames: Vec::new(), ..Default::default() };
        empty.validate().unwrap();
        assert_eq!(empty.frames.len(), 1);
        let many = FigureFrame { strokes: vec![Stroke { points: vec![[0.0, 0.0]; MAX_POINTS + 1], ..Default::default() }] };
        assert!(Figure { name: "big".into(), frames: vec![many], ..Default::default() }.validate().is_err());
        let frames = vec![FigureFrame::default(); MAX_FRAMES + 1];
        assert!(Figure { name: "long".into(), frames, ..Default::default() }.validate().is_err());
    }

    #[test]
    fn the_library_round_trips_and_stays_in_its_folder() {
        let dir = temp_dir("store");
        let mut store = FigureStore::load(dir.clone());
        assert!(store.list().is_empty());
        let fig = test_figure("Mon logo", 3);
        store.save(fig.clone()).unwrap();
        assert!(dir.join("Mon logo.json").is_file());
        let again = FigureStore::load(dir.clone());
        assert_eq!(again.list(), &[fig.clone()][..], "reopened identical");
        assert!(store.save(Figure { name: "../evil".into(), ..Default::default() }).is_err());
        assert!(!dir.parent().unwrap().join("evil.json").exists());
        // A file whose name is not a figure name, or that is not JSON, is ignored.
        std::fs::write(dir.join("bad.json"), "{").unwrap();
        std::fs::write(dir.join("x.txt"), "{}").unwrap();
        assert_eq!(FigureStore::load(dir.clone()).list().len(), 1);
        store.remove("Mon logo").unwrap();
        assert!(!dir.join("Mon logo.json").exists());
        assert!(store.remove("Mon logo").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn json_loads_with_defaults() {
        let fig: Figure = serde_json::from_str(r#"{"name":"a","frames":[{"strokes":[{"points":[[0,0],[0.5,0.5]]}]}]}"#).unwrap();
        assert_eq!((fig.rate, fig.per, fig.loop_mode), (1.0, RateUnit::Beat, LoopMode::Loop));
        assert!(fig.frames[0].strokes[0].lit && fig.frames[0].strokes[0].color == [0, 255, 0]);
        let s: Settings = serde_json::from_str(r#"{"content":{"kind":"figure","name":"a","per":"second","loop_mode":"ping_pong"}}"#).unwrap();
        let Content::Figure(f) = &s.content else { panic!("{:?}", s.content) };
        assert_eq!((f.per, f.loop_mode, f.frames.len()), (RateUnit::Second, LoopMode::PingPong, 1));
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    fn render(fig: &Figure, start: f64, beat: f64) -> Vec<Point> {
        let s = Settings { content: Content::Figure(fig.clone()), scale: 1.0, brightness: 1.0, ..Default::default() };
        Animator::starting_at(start).render(&s, AudioFeatures::default(), 1.0 / 60.0, &BeatClock { beat, bpm: 120.0, beats_per_bar: 4 })
    }

    /// Height of the (red) line drawn: which frame is on show.
    fn line_y(pts: &[Point]) -> f32 {
        pts.iter().find(|p| p.r > 0.5).map(|p| p.y).unwrap()
    }

    #[test]
    fn an_animation_steps_on_the_beat_from_the_bar_it_started_in() {
        let fig = test_figure("anim", 4); // one frame per beat
        assert_eq!(line_y(&render(&fig, 0.0, 0.2)), 0.0);
        assert!((line_y(&render(&fig, 0.0, 1.2)) - 0.1).abs() < 1e-6);
        assert!((line_y(&render(&fig, 0.0, 3.9)) - 0.3).abs() < 1e-6);
        assert_eq!(line_y(&render(&fig, 0.0, 4.1)), 0.0, "loops after 4 beats");
        // Launched mid-bar: counts from that bar's one, like other beat-synced looks.
        assert!((line_y(&render(&fig, 5.5, 6.2)) - 0.2).abs() < 1e-6);
        // Beats, not seconds: the same beat at any tempo.
        let s = Settings { content: Content::Figure(fig.clone()), scale: 1.0, brightness: 1.0, ..Default::default() };
        let at = |bpm| Animator::starting_at(0.0).render(&s, AudioFeatures::default(), 0.016, &BeatClock { beat: 2.5, bpm, beats_per_bar: 4 });
        assert_eq!(at(90.0), at(174.0));
    }

    #[test]
    fn per_second_animations_follow_time() {
        let fig = Figure { per: RateUnit::Second, rate: 10.0, ..test_figure("s", 3) };
        let s = Settings { content: Content::Figure(fig), scale: 1.0, brightness: 1.0, ..Default::default() };
        let mut a = Animator::default();
        let clock = BeatClock::default();
        let first = line_y(&a.render(&s, AudioFeatures::default(), 0.0, &clock));
        let later = line_y(&a.render(&s, AudioFeatures::default(), 0.15, &clock));
        assert_eq!((first, later), (0.0, 0.1));
    }

    #[test]
    fn the_rendered_figure_is_densified_and_blanked_between_strokes() {
        let pts = render(&test_figure("d", 1), 0.0, 0.0);
        assert!(pts.windows(2).all(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt() <= 0.03 + 1e-4));
        // Nothing lit between the line's end (0.5, 0) and the dot, except the dot.
        let after_line = pts.iter().rposition(|p| p.r > 0.5).unwrap();
        assert!(pts[after_line + 1..].iter().all(|p| !p.is_lit() || (p.x, p.y) == (0.0, -0.8)));
        // Brightness scales the colours.
        let s = Settings { content: Content::Figure(test_figure("d", 1)), scale: 1.0, brightness: 0.5, ..Default::default() };
        let dim = Animator::default().render(&s, AudioFeatures::default(), 0.016, &BeatClock::default());
        assert!(dim.iter().all(|p| p.r <= 0.5 + 1e-6 && p.g <= 0.5 + 1e-6));
    }

    #[test]
    fn saved_figures_are_cues_of_the_figures_page() {
        let mut s = crate::test_support::shared();
        let builtin = s.presets.len();
        s.figures.save(test_figure("Logo", 2)).unwrap();
        refresh(&mut s);
        assert_eq!(s.presets.len(), builtin + 1);
        let page = crate::presets::CATEGORIES.iter().position(|c| *c == CATEGORY).unwrap() + 1;
        assert!(s.controls.get(&format!("grid.{page}.1.1")).is_some(), "MIDI grid cell");
        assert!(crate::controls::press_cue(&mut s, "figure:Logo", None, true));
        assert!(matches!(s.settings.content, Content::Figure(ref f) if f.name == "Logo"));
        s.figures.remove("Logo").unwrap();
        refresh(&mut s);
        assert_eq!(s.presets.len(), builtin);
        assert!(s.controls.get(&format!("grid.{page}.1.1")).is_none());
    }

    // ---------- parts (T-441) ----------

    use crate::live::{self, LiveModifiers, LiveState};
    use std::collections::BTreeMap;

    /// `test_figure` with its dot (stroke 3) in part 2 « Yeux ».
    fn with_eyes() -> Figure {
        let mut fig = test_figure("Visage", 1);
        fig.frames[0].strokes[2].part = 2;
        fig.parts = vec![PartDef { id: 2, name: "Yeux".into() }];
        fig
    }

    #[test]
    fn older_strokes_load_without_a_part_and_none_is_written() {
        let s: Stroke = serde_json::from_str(r#"{"points":[[0,0]],"color":[255,0,0],"lit":true}"#).unwrap();
        assert_eq!(s.part, 0);
        assert!(!serde_json::to_string(&s).unwrap().contains("part"), "no part, no field");
        let fig: Figure = serde_json::from_str(r#"{"name":"a"}"#).unwrap();
        assert!(fig.parts.is_empty());
        assert!(!serde_json::to_string(&fig).unwrap().contains("parts"));
        let eyes = with_eyes();
        let back: Figure = serde_json::from_str(&serde_json::to_string(&eyes).unwrap()).unwrap();
        assert_eq!(back, eyes, "parts survive a save");
    }

    #[test]
    fn tagged_points_match_the_plain_ones_and_travel_belongs_to_the_arrival() {
        let fig = with_eyes();
        let (pts, parts) = fig.frame_points_tagged(0, 1.0, white);
        assert_eq!(pts, fig.frame_points(0, 1.0, white), "same points");
        assert_eq!(pts.len(), parts.len());
        // Line (blanked arrival + 2), move (2), dot (blanked arrival + dwell).
        assert_eq!(parts, [[0; 5].as_slice(), &[2; 1 + DOT_DWELL]].concat(), "the dot's blanked arrival is part 2");
        assert!(!pts[5].is_lit() && parts[5] == 2);
        assert_eq!(fig.frame_points_tagged(9, 1.0, white), (Vec::new(), Vec::new()));
        // A black stroke (all travel) keeps its part on every point.
        let (pts, parts) = fig.frame_points_tagged(0, 1.0, |_| (0.0, 0.0, 0.0));
        assert!(pts.iter().all(|p| !p.is_lit()) && pts.len() == parts.len());
        assert!(fig.has_parts() && !test_figure("x", 2).has_parts());
        assert_eq!((fig.part_name(2), fig.part_name(1)), (Some("Yeux"), None));
    }

    #[test]
    fn validate_brings_parts_into_range() {
        let mut fig = with_eyes();
        fig.frames[0].strokes[0].part = 9;
        fig.parts = vec![
            PartDef { id: 3, name: "  ".into() },
            PartDef { id: 2, name: "Yeux".into() },
            PartDef { id: 2, name: "Encore".into() },
            PartDef { id: 0, name: "Zéro".into() },
            PartDef { id: 12, name: "Trop".into() },
            PartDef { id: 1, name: "n".repeat(80) },
        ];
        fig.validate().unwrap();
        assert_eq!(fig.frames[0].strokes[0].part, 0, "out of range: no part");
        assert_eq!(fig.frames[0].strokes[2].part, 2);
        let ids: Vec<u8> = fig.parts.iter().map(|p| p.id).collect();
        assert_eq!(ids, [1, 2, 3], "sorted, once each, in range");
        assert_eq!((fig.parts[1].name.as_str(), fig.parts[2].name.as_str()), ("Yeux", "Partie 3"));
        assert_eq!(fig.parts[0].name.chars().count(), 32);
    }

    fn mods(part: u8, m: LiveModifiers) -> BTreeMap<u8, LiveModifiers> {
        BTreeMap::from([(part, m)])
    }

    fn render_parts(fig: &Figure, parts: Option<&BTreeMap<u8, LiveModifiers>>) -> Vec<Point> {
        let s = Settings { content: Content::Figure(fig.clone()), scale: 1.0, brightness: 1.0, ..Default::default() };
        let mut a = Animator::starting_at(0.0);
        a.set_parts(parts, &[]);
        a.render(&s, AudioFeatures::default(), 1.0 / 60.0, &BeatClock { beat: 0.5, bpm: 120.0, beats_per_bar: 4 })
    }

    #[test]
    fn a_figure_without_parts_or_effects_renders_exactly_as_before() {
        let plain = test_figure("p", 1);
        let dim = mods(1, LiveModifiers { brightness: 0.2, ..Default::default() });
        assert_eq!(render_parts(&plain, Some(&dim)), render_parts(&plain, None), "no parts: the same path");
        let eyes = with_eyes();
        assert_eq!(render_parts(&eyes, None), render_parts(&plain, None), "parts alone change nothing");
        assert_eq!(render_parts(&eyes, Some(&BTreeMap::new())), render_parts(&plain, None));
        // Part 2 dimmed: the red line is untouched, the dot is dimmer.
        let out = render_parts(&eyes, Some(&mods(2, LiveModifiers { brightness: 0.25, ..Default::default() })));
        assert!(out.iter().filter(|p| p.r > 0.0).all(|p| p.r == 1.0));
        assert!(out.iter().filter(|p| p.g > 0.0).all(|p| (p.g - 0.25).abs() < 1e-6));
    }

    /// A small deterministic generator (no extra crate).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
        fn wild(&mut self) -> f32 {
            match (self.next() * 10.0) as u32 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => -1e30,
                _ => self.range(-4.0, 4.0),
            }
        }
    }

    fn random_figure(rng: &mut Lcg) -> Figure {
        let strokes = (0..1 + (rng.next() * 12.0) as usize)
            .map(|_| Stroke {
                points: (0..1 + (rng.next() * 6.0) as usize).map(|_| [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)]).collect(),
                color: [(rng.next() * 255.0) as u8, (rng.next() * 255.0) as u8, (rng.next() * 255.0) as u8],
                lit: rng.next() > 0.2,
                part: (rng.next() * 10.0) as u8,
            })
            .collect();
        let mut fig = Figure { name: "r".into(), frames: vec![FigureFrame { strokes }], ..Default::default() };
        fig.validate().unwrap();
        fig
    }

    fn random_mods(rng: &mut Lcg) -> BTreeMap<u8, LiveModifiers> {
        (0..(rng.next() * 4.0) as usize)
            .map(|_| {
                let m = LiveModifiers {
                    brightness: rng.wild(),
                    size: rng.wild(),
                    size_x: rng.wild(),
                    pos_x: rng.wild(),
                    pos_y: rng.wild(),
                    rot_angle: [rng.wild(), rng.wild(), rng.wild()],
                    rot_speed: [0.0, 0.0, rng.range(-720.0, 720.0)],
                    perspective: rng.wild(),
                    trace: rng.range(0.0, 1.0),
                    dots: rng.range(0.0, 1.0),
                    color: if rng.next() > 0.5 { live::ColorOverride::Fixed { rgb: [255, 255, 255] } } else { live::ColorOverride::Normal },
                    ..Default::default()
                };
                ((rng.next() * 10.0) as u8, m)
            })
            .collect()
    }

    /// Property (T-441): random figures, parts and effects give finite
    /// points in -1..1, never light a point that was blanked (the travel
    /// to and from every stroke, so no travel segment is ever lit) nor
    /// join two strokes by a lit segment, never make a point more intense
    /// (a colour may change, its intensity only drops), and leave
    /// the points of every part without effects bit for bit.
    #[test]
    fn random_parts_and_effects_stay_finite_in_bounds_and_never_light_travel() {
        let mut rng = Lcg(0x5eed_0441);
        for case in 0..400 {
            let mut states = BTreeMap::new();
            for id in 1..=MAX_PARTS {
                let mut st = LiveState::default();
                st.set_clock(case as f64 * 0.1, case as f64 * 0.2);
                states.insert(id, st);
            }
            let fig = random_figure(&mut rng);
            let per_part = random_mods(&mut rng);
            let (pts, parts) = fig.frame_points_tagged(0, 1.0, white);
            let out = live::apply_parts(&pts, &parts, &per_part, &states, &[]);
            assert_eq!(out.len(), pts.len(), "case {case}: same samples");
            for (i, (a, b)) in pts.iter().zip(&out).enumerate() {
                assert!([b.x, b.y, b.r, b.g, b.b].iter().all(|v| v.is_finite()), "case {case}: {b:?}");
                assert!((-1.0..=1.0).contains(&b.x) && (-1.0..=1.0).contains(&b.y), "case {case}: {b:?}");
                assert!(a.is_lit() || !b.is_lit(), "case {case}: point {i} lit from blanked");
                assert!(b.r.max(b.g).max(b.b) <= a.r.max(a.g).max(a.b) + 1e-6, "case {case}: brighter");
                if parts[i] == 0 || !per_part.contains_key(&parts[i]) {
                    assert_eq!([a.x, a.y, a.r, a.g, a.b].map(f32::to_bits), [b.x, b.y, b.r, b.g, b.b].map(f32::to_bits), "case {case}: point {i}");
                }
            }
            for i in 1..out.len() {
                if out[i - 1].is_lit() && out[i].is_lit() {
                    assert!(pts[i - 1].is_lit() && pts[i].is_lit() && parts[i - 1] == parts[i], "case {case}: lit segment {i} across strokes");
                }
            }
            // The whole look: finite, in bounds, through rotation and densify.
            let s = Settings { content: Content::Figure(fig.clone()), scale: 1.0, brightness: 1.0, ..Default::default() };
            let mut a = Animator::starting_at(0.0);
            a.set_parts(Some(&per_part), &[]);
            for k in 0..3 {
                let frame = a.render(&s, AudioFeatures::default(), 0.1, &BeatClock { beat: k as f64, bpm: 128.0, beats_per_bar: 4 });
                assert!(frame.iter().all(|p| [p.x, p.y, p.r, p.g, p.b].iter().all(|v| v.is_finite() && *v >= -1.0 - 1e-5 && *v <= 1.0 + 1e-5)), "case {case}");
            }
        }
    }

    #[test]
    fn a_damaged_figure_is_quarantined_and_never_overwritten() {
        let (dir, path) = crate::datafile::tests::with_damaged("figures", "figures/logo.json");
        let mut store = FigureStore::load(path.parent().unwrap().to_path_buf());
        assert!(store.list().is_empty());
        store.save(test_figure("logo", 1)).unwrap();
        crate::datafile::tests::check_kept(&dir, "figures/logo.json", false);
    }
}
