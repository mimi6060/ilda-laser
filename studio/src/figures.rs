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
}

impl Default for Stroke {
    fn default() -> Self {
        Self { points: Vec::new(), color: [0, 255, 0], lit: true }
    }
}

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
}

impl Default for Figure {
    fn default() -> Self {
        Self { name: String::new(), frames: vec![FigureFrame::default()], rate: 1.0, per: RateUnit::Beat, loop_mode: LoopMode::Loop }
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
        let Some(frame) = self.frames.get(index) else { return Vec::new() };
        let mut out = Vec::new();
        for stroke in &frame.strokes {
            let pts = stroke.points.iter().map(|p| (p[0] * scale, p[1] * scale));
            let (r, g, b) = if stroke.lit { paint(stroke.color) } else { (0.0, 0.0, 0.0) };
            if r <= 0.0 && g <= 0.0 && b <= 0.0 {
                out.extend(pts.map(|(x, y)| Point::blanked(x, y)));
                continue;
            }
            let Some(&[x0, y0]) = stroke.points.first() else { continue };
            out.push(Point::blanked(x0 * scale, y0 * scale));
            if stroke.points.len() == 1 {
                out.extend(std::iter::repeat_n(Point::lit(x0 * scale, y0 * scale, r, g, b), DOT_DWELL));
            } else {
                out.extend(pts.map(|(x, y)| Point::lit(x, y, r, g, b)));
            }
        }
        out
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
    }
}

/// Rebuilds the « Figures » cue page (and the grid controls for MIDI)
/// from the library. Called whenever the library changes.
pub fn refresh(s: &mut crate::Shared) {
    s.presets.retain(|p| p.category != CATEGORY);
    let figures: Vec<Preset> = s.figures.list().iter().map(preset_of).collect();
    s.presets.extend(figures);
    s.controls = crate::controls::ControlRegistry::build(&s.presets);
}

/// The figure library: `studio-data/figures/`, one JSON file per figure,
/// kept in memory (sorted by name) so the project and the cue grid read
/// it without touching the disk.
pub struct FigureStore {
    dir: Option<PathBuf>,
    list: Vec<Figure>,
}

impl FigureStore {
    /// Every valid figure in `dir` (unreadable or invalid files are
    /// skipped with a warning).
    pub fn load(dir: PathBuf) -> Self {
        let mut list = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for path in entries.flatten().map(|e| e.path()) {
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
                if path.extension().is_none_or(|x| x != "json") || !valid_show_name(stem) || !path.is_file() {
                    continue;
                }
                match read(&path, stem) {
                    Ok(fig) => list.push(fig),
                    Err(e) => log::warn!("figure {}: {e:#}", path.display()),
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
                    Stroke { points: vec![[-0.5, y], [0.5, y]], color: [255, 0, 0], lit: true },
                    Stroke { points: vec![[0.5, -0.5], [0.0, -0.8]], lit: false, ..Default::default() },
                    Stroke { points: vec![[0.0, -0.8]], color: [0, 255, 0], lit: true },
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
}
