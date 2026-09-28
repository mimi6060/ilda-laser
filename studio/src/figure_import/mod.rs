//! Import (T-297): an SVG drawing or a PNG / JPEG picture of the
//! operator's own becomes an editable `Figure` (T-296).
//!
//! - `svg`: paths, lines, polylines, polygons, rectangles, circles and
//!   ellipses, with their transforms and stroke (or fill) colours; curves
//!   are flattened.
//! - `raster`: the picture is vectorised: outlines of dark (or light)
//!   shapes, centre lines of strokes, edges, or outlines per colour.
//!
//! Both then go through the same finish: fit into the laser's frame,
//! simplify (Ramer-Douglas-Peucker), drawing order (nearest neighbour,
//! open strokes may be reversed, closed ones start at their nearest
//! point) and the point budget (the laser points after `densify`, as the
//! engine will draw them).
//!
//! Files come from the user and may be hostile: sizes are capped, nothing
//! is fetched (no external entities, no URLs), numbers must be finite, and
//! every error is a French message; the web handler also catches panics.
//! The source file is never stored: only the resulting figure is saved,
//! by the user, in `studio-data/figures`.

mod raster;
mod svg;

use crate::figures::{Figure, FigureFrame, Stroke, MAX_POINTS, MAX_STROKES};
use anyhow::{bail, Result};
use serde::Serialize;

/// Largest file accepted, any kind (the HTTP body is capped to this).
pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;
/// Largest SVG accepted: it is text, a real logo is far smaller.
pub const MAX_SVG_BYTES: usize = 5 * 1024 * 1024;

type Pt = [f64; 2];

/// One imported polyline, in source units, before fitting.
#[derive(Clone, Debug)]
struct Path {
    pts: Vec<Pt>,
    /// The first point is not repeated at the end: `finish` does it.
    closed: bool,
    color: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Svg,
    Png,
    Jpeg,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Svg => "svg",
            Kind::Png => "png",
            Kind::Jpeg => "jpeg",
        }
    }
}

/// How a picture is turned into lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Outlines of the shapes darker (or lighter) than the threshold.
    Contours,
    /// Centre lines of the strokes (a line drawing, handwriting).
    Lines,
    /// Edges found by the brightness gradient (a photo).
    Edges,
    /// Outlines of each of the N main colours, in their colour.
    Colors,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "contours" => Some(Mode::Contours),
            "lines" => Some(Mode::Lines),
            "edges" => Some(Mode::Edges),
            "colors" => Some(Mode::Colors),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// 0..=100: 0 keeps every detail, 100 simplifies a lot.
    pub simplify: f32,
    /// Half-size of the result in the laser frame, 0.1..=1.
    pub size: f32,
    /// Laser points per frame the figure must fit in (after densify).
    pub budget: usize,
    /// For black (invisible on a laser) SVG shapes and single-colour pictures.
    pub color: [u8; 3],
    /// SVG: outline filled shapes that have no stroke.
    pub fills: bool,
    pub mode: Mode,
    /// Picture: brightness threshold, `None` = automatic (Otsu).
    pub threshold: Option<u8>,
    /// Picture: trace the light parts instead of the dark ones.
    pub invert: bool,
    /// Picture: smoothing passes, 0..=5.
    pub smooth: u8,
    /// Picture, `Mode::Colors`: colours drawn, 1..=6 (plus the background).
    pub colors: u8,
    /// Picture: longest side of the working copy, in pixels.
    pub resolution: u32,
    /// Picture: blobs and lines smaller than this (pixels of the working copy) are ignored.
    pub min_size: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            simplify: 20.0,
            size: 0.9,
            budget: crate::layers::DEFAULT_POINT_BUDGET,
            color: [255, 255, 255],
            fills: true,
            mode: Mode::Contours,
            threshold: None,
            invert: false,
            smooth: 2,
            colors: 3,
            resolution: 320,
            min_size: 4,
        }
    }
}

impl Options {
    /// Brings every setting into its range.
    pub fn clamped(mut self) -> Self {
        self.simplify = if self.simplify.is_finite() { self.simplify.clamp(0.0, 100.0) } else { 20.0 };
        self.size = if self.size.is_finite() { self.size.clamp(0.1, 1.0) } else { 0.9 };
        self.budget = self.budget.clamp(crate::layers::MIN_POINT_BUDGET, crate::layers::MAX_POINT_BUDGET);
        self.smooth = self.smooth.min(5);
        self.colors = self.colors.clamp(1, 6);
        self.resolution = self.resolution.clamp(64, 800);
        self.min_size = self.min_size.min(200);
        self
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub kind: &'static str,
    pub strokes: usize,
    /// Points of the figure (what the editor shows).
    pub points: usize,
    /// Points the laser draws per frame, after densify (travel included).
    pub laser_points: usize,
    pub budget: usize,
    pub over_budget: bool,
    /// Blanked travel between strokes, before and after ordering (laser units).
    pub travel_before: f32,
    pub travel_after: f32,
    /// Strokes left out to fit the budget or the limits.
    pub dropped: usize,
    /// Picture: the threshold used (the automatic one if not set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u8>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Imported {
    pub figure: Figure,
    pub warnings: Vec<String>,
    pub stats: Stats,
}

/// What the bytes are, from their content (never from the name alone).
pub fn sniff(bytes: &[u8]) -> Result<Kind> {
    if bytes.is_empty() {
        bail!("fichier vide");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(Kind::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok(Kind::Jpeg);
    }
    if bytes.starts_with(&[0x1F, 0x8B]) {
        bail!("SVG compressé (.svgz) non pris en charge : enregistrez-le en .svg simple");
    }
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    if text.contains("<svg") {
        return Ok(Kind::Svg);
    }
    bail!("format non reconnu : un fichier SVG, PNG ou JPEG est attendu")
}

/// The whole import: bytes → figure, with warnings and statistics.
/// `file_name` only gives the figure its default name.
pub fn import(bytes: &[u8], file_name: &str, opts: &Options) -> Result<Imported> {
    let opts = opts.clone().clamped();
    if bytes.len() > MAX_FILE_BYTES {
        bail!("fichier trop gros : {} Mo au plus", MAX_FILE_BYTES / (1024 * 1024));
    }
    let kind = sniff(bytes)?;
    let mut warnings = Vec::new();
    let mut threshold = None;
    let paths = match kind {
        Kind::Svg => {
            if bytes.len() > MAX_SVG_BYTES {
                bail!("SVG trop gros : {} Mo au plus", MAX_SVG_BYTES / (1024 * 1024));
            }
            let Ok(text) = std::str::from_utf8(bytes) else { bail!("SVG illisible : le texte n'est pas en UTF-8") };
            svg::parse(text, &opts, &mut warnings)?
        }
        Kind::Png | Kind::Jpeg => {
            let (paths, t) = raster::vectorize(bytes, &opts, &mut warnings)?;
            threshold = t;
            paths
        }
    };
    if paths.is_empty() {
        bail!(match kind {
            Kind::Svg => "aucune forme dessinable dans ce SVG (chemins, lignes, polygones, cercles, rectangles)",
            _ => "aucun contour trouvé dans l'image : essayez un autre mode, un autre seuil ou « Inverser »",
        });
    }
    let (figure, mut stats) = finish(paths, &opts, &default_name(file_name), &mut warnings)?;
    stats.kind = kind.label();
    stats.threshold = threshold;
    Ok(Imported { figure, warnings, stats })
}

/// A valid figure name from a file name: its stem, unsupported characters
/// replaced by spaces.
fn default_name(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    let cleaned: String = stem.chars().map(|c| if c.is_alphanumeric() || "-_".contains(c) { c } else { ' ' }).collect();
    let name: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(64).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        "Import".into()
    } else {
        name
    }
}

/// Fit, simplify, order and budget: the common end of both importers.
fn finish(mut paths: Vec<Path>, opts: &Options, name: &str, warnings: &mut Vec<String>) -> Result<(Figure, Stats)> {
    paths.retain(|p| p.pts.iter().all(|q| q[0].is_finite() && q[1].is_finite()) && !p.pts.is_empty());
    fit(&mut paths, opts.size as f64);
    // Simplification tolerance in laser units: from almost nothing to 3 %.
    let base_eps = 0.0004 + opts.simplify as f64 / 100.0 * 0.03;
    let mut strokes: Vec<Path> = paths.iter().map(|p| simplified(p, base_eps)).filter(|p| !p.pts.is_empty()).collect();
    let travel_before = travel(&strokes) as f32;
    let mut dropped = 0;

    // The figure's own limits (T-296): keep the longest strokes.
    if strokes.len() > MAX_STROKES {
        let excess = strokes.len() - MAX_STROKES;
        dropped += excess;
        drop_shortest(&mut strokes, excess);
    }
    let mut eps = base_eps;
    while strokes.iter().map(|s| s.pts.len() + 1).sum::<usize>() > MAX_POINTS && eps < 0.2 {
        eps *= 1.5;
        strokes = strokes.iter().map(|p| simplified(p, eps)).collect();
    }
    strokes = order(strokes);

    // The point budget. Laser points come mostly from the length drawn
    // (densify) and the corners held; simplifying a lot would add corners
    // and spoil curves, so: a little more simplification if it helps,
    // then the smallest details are left out.
    let mut laser = laser_points(&strokes);
    if laser > opts.budget {
        let harder = (eps * 2.0).max(0.004);
        let candidate = order(strokes.iter().map(|p| simplified(p, harder)).collect());
        let fewer = laser_points(&candidate);
        if fewer < laser {
            strokes = candidate;
            laser = fewer;
            eps = harder;
        }
        let before = strokes.len();
        while laser > opts.budget && strokes.len() > 1 {
            // Several at a time when far over, one by one near the end.
            // The order stays good enough while dropping; it is redone
            // once at the end (the loop goes on if that is still over).
            while laser > opts.budget && strokes.len() > 1 {
                let excess = (laser - opts.budget) as f64 / laser as f64;
                let n = ((strokes.len() as f64 * excess * 0.5) as usize).clamp(1, strokes.len() - 1);
                drop_shortest(&mut strokes, n);
                laser = laser_points(&strokes);
            }
            strokes = order(strokes);
            laser = laser_points(&strokes);
        }
        if strokes.len() < before {
            dropped += before - strokes.len();
            warnings.push(format!(
                "budget de {} points : {} petit(s) tracé(s) laissé(s) de côté (augmentez le budget ou simplifiez l'image pour les garder)",
                opts.budget,
                before - strokes.len()
            ));
        }
        if eps > base_eps * 1.01 {
            warnings.push("budget de points : tracé simplifié davantage".into());
        }
    }
    let over_budget = laser > opts.budget;
    if over_budget {
        warnings.push(format!(
            "figure trop lourde : ≈ {laser} points laser pour un budget de {} ; réduisez la taille ou augmentez le budget, sinon la lecture espacera les points",
            opts.budget
        ));
    }

    let travel_after = travel(&strokes) as f32;
    let mut figure = Figure {
        name: name.to_string(),
        frames: vec![FigureFrame { strokes: strokes.iter().map(to_stroke).collect() }],
        ..Figure::default()
    };
    figure.validate()?;
    let stats = Stats {
        strokes: figure.frames[0].strokes.len(),
        points: figure.point_count(),
        laser_points: laser,
        budget: opts.budget,
        over_budget,
        travel_before,
        travel_after,
        dropped,
        ..Stats::default()
    };
    Ok((figure, stats))
}

/// Scale and centre everything into -size..size, keeping the aspect
/// ratio; sources have y down, the laser y up.
fn fit(paths: &mut [Path], size: f64) {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in paths.iter().flat_map(|p| &p.pts) {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    if lo[0] > hi[0] {
        return;
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let k = if span > 1e-12 { 2.0 * size / span } else { 1.0 };
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    for q in paths.iter_mut().flat_map(|p| p.pts.iter_mut()) {
        *q = [(q[0] - c[0]) * k, -(q[1] - c[1]) * k];
    }
}

fn dist(a: Pt, b: Pt) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn length(p: &Path) -> f64 {
    let mut l: f64 = p.pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    if p.closed && p.pts.len() > 2 {
        l += dist(p.pts[p.pts.len() - 1], p.pts[0]);
    }
    l
}

/// Distance from `p` to the segment a-b.
fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if l2 < 1e-18 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Ramer-Douglas-Peucker on an open polyline, with an explicit stack (no
/// recursion, whatever the input).
fn rdp(pts: &[Pt], eps: f64) -> Vec<Pt> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (mut best, mut best_d) = (0, eps);
        for i in a + 1..b {
            let d = seg_dist(pts[i], pts[a], pts[b]);
            if d > best_d {
                best = i;
                best_d = d;
            }
        }
        if best > 0 {
            keep[best] = true;
            stack.push((a, best));
            stack.push((best, b));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// A path with fewer points. Closed paths are split at their farthest
/// point from the start so both halves keep their shape.
fn simplified(p: &Path, eps: f64) -> Path {
    let mut pts: Vec<Pt> = Vec::with_capacity(p.pts.len());
    for &q in &p.pts {
        if pts.last().is_none_or(|&l| dist(l, q) > 1e-9) {
            pts.push(q);
        }
    }
    if p.closed && pts.len() > 1 && dist(pts[0], pts[pts.len() - 1]) <= 1e-9 {
        pts.pop();
    }
    let closed = p.closed && pts.len() > 2;
    let pts = if closed {
        let far = (1..pts.len()).max_by(|&i, &j| dist(pts[0], pts[i]).total_cmp(&dist(pts[0], pts[j]))).unwrap_or(1);
        let mut first = rdp(&pts[..=far], eps);
        let mut second: Vec<Pt> = pts[far..].to_vec();
        second.push(pts[0]);
        let second = rdp(&second, eps);
        first.extend_from_slice(&second[1..second.len() - 1]);
        first
    } else {
        rdp(&pts, eps)
    };
    // A closed path reduced to a sliver keeps at least a segment.
    let closed = closed && pts.len() > 2;
    Path { pts, closed, color: p.color }
}

/// Drops the `n` shortest strokes, keeping the others in order.
fn drop_shortest(strokes: &mut Vec<Path>, n: usize) {
    let mut idx: Vec<usize> = (0..strokes.len()).collect();
    idx.sort_by(|&a, &b| length(&strokes[a]).total_cmp(&length(&strokes[b])));
    let gone: std::collections::HashSet<usize> = idx.into_iter().take(n).collect();
    let mut i = 0;
    strokes.retain(|_| {
        i += 1;
        !gone.contains(&(i - 1))
    });
}

fn start(p: &Path) -> Pt {
    p.pts[0]
}

fn end(p: &Path) -> Pt {
    if p.closed {
        p.pts[0]
    } else {
        p.pts[p.pts.len() - 1]
    }
}

/// Blanked travel of a frame: between strokes and back to the start.
fn travel(strokes: &[Path]) -> f64 {
    let mut t: f64 = strokes.windows(2).map(|w| dist(end(&w[0]), start(&w[1]))).sum();
    if let (Some(a), Some(b)) = (strokes.last(), strokes.first()) {
        t += dist(end(a), start(b));
    }
    t
}

/// Drawing order, nearest neighbour: from the top-left, always go to the
/// closest start next. Open strokes may be drawn backwards, closed ones
/// start at their vertex closest to the beam.
fn order(mut strokes: Vec<Path>) -> Vec<Path> {
    let mut out = Vec::with_capacity(strokes.len());
    let mut at: Pt = [-1.0, 1.0];
    while !strokes.is_empty() {
        // (stroke, vertex to start at, reversed, distance)
        let mut best = (0, 0, false, f64::MAX);
        for (i, s) in strokes.iter().enumerate() {
            if s.closed {
                for (k, &q) in s.pts.iter().enumerate() {
                    let d = dist(at, q);
                    if d < best.3 {
                        best = (i, k, false, d);
                    }
                }
            } else {
                let (d0, d1) = (dist(at, s.pts[0]), dist(at, s.pts[s.pts.len() - 1]));
                if d0 < best.3 {
                    best = (i, 0, false, d0);
                }
                if d1 < best.3 {
                    best = (i, 0, true, d1);
                }
            }
        }
        let mut s = strokes.swap_remove(best.0);
        if s.closed {
            s.pts.rotate_left(best.1);
        } else if best.2 {
            s.pts.reverse();
        }
        at = end(&s);
        out.push(s);
    }
    out
}

fn to_stroke(p: &Path) -> Stroke {
    let mut points: Vec<[f32; 2]> = p.pts.iter().map(|q| [q[0] as f32, q[1] as f32]).collect();
    if p.closed {
        // Closed outlines repeat their first point.
        points.push(points[0]);
    }
    Stroke { points, color: p.color, lit: true }
}

/// Laser points per frame, exactly as the engine will draw it: the
/// figure's points (with blanked starts and dots) through `densify`,
/// plus the travel back to the start.
fn laser_points(strokes: &[Path]) -> usize {
    let frame = FigureFrame { strokes: strokes.iter().map(to_stroke).collect() };
    let fig = Figure { frames: vec![frame], ..Figure::default() };
    let pts = fig.frame_points(0, 1.0, |_| (1.0, 1.0, 1.0));
    let mut n = crate::engine::densify(&pts).len();
    if let (Some(a), Some(b)) = (pts.last(), pts.first()) {
        n += (((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt() / 0.03).ceil() as usize;
    }
    n
}

/// A colour a laser can show: black (or nearly) is invisible, so it
/// becomes `fallback`.
fn laser_color(c: [u8; 3], fallback: [u8; 3]) -> [u8; 3] {
    if c.iter().copied().max().unwrap_or(0) < 48 {
        fallback
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Path {
        Path { pts: vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]], closed: true, color: [255, 0, 0] }
    }

    #[test]
    fn sniffs_by_content_and_refuses_the_rest() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nxxxx").unwrap(), Kind::Png);
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap(), Kind::Jpeg);
        assert_eq!(sniff(b"<?xml version='1.0'?><svg></svg>").unwrap(), Kind::Svg);
        assert!(sniff(b"").unwrap_err().to_string().contains("vide"));
        assert!(sniff(&[0x1F, 0x8B, 0, 0]).unwrap_err().to_string().contains("svgz"));
        assert!(sniff(b"GIF89a....").unwrap_err().to_string().contains("format non reconnu"));
    }

    #[test]
    fn default_names_are_valid_figure_names() {
        assert_eq!(default_name("mon logo.svg"), "mon logo");
        assert_eq!(default_name("/tmp/../x/Été (2).png"), "Été 2");
        assert_eq!(default_name("..."), "Import");
        assert_eq!(default_name(""), "Import");
        assert!(crate::timeline::valid_show_name(&default_name(&"é".repeat(200))));
    }

    #[test]
    fn rdp_keeps_corners_and_drops_collinear_points() {
        let line: Vec<Pt> = (0..=100).map(|i| [i as f64 / 100.0, 0.0]).collect();
        assert_eq!(rdp(&line, 0.001), vec![[0.0, 0.0], [1.0, 0.0]]);
        let mut corner = line.clone();
        corner.extend((1..=100).map(|i| [1.0, i as f64 / 100.0]));
        assert_eq!(rdp(&corner, 0.001), vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        // A closed circle keeps its shape.
        let circle = Path { pts: (0..400).map(|i| { let a = i as f64 / 400.0 * std::f64::consts::TAU; [a.cos(), a.sin()] }).collect(), closed: true, color: [0; 3] };
        let s = simplified(&circle, 0.01);
        assert!(s.closed && s.pts.len() >= 8 && s.pts.len() < 40, "{}", s.pts.len());
    }

    #[test]
    fn nearest_neighbour_order_cuts_the_travel() {
        // A row of squares given in a scrambled order.
        let xs = [0, 7, 2, 9, 4, 1, 8, 3, 6, 5];
        let strokes: Vec<Path> = xs.iter().map(|&i| square(i as f64 * 0.2 - 1.0, 0.0, 0.1)).collect();
        let before = travel(&strokes);
        let ordered = order(strokes);
        assert_eq!(ordered.len(), 10);
        let after = travel(&ordered);
        assert!(after < before * 0.5, "{before} -> {after}");
        // Open strokes are reversed when their end is closer.
        let a = Path { pts: vec![[-1.0, 1.0], [0.0, 1.0]], closed: false, color: [0; 3] };
        let b = Path { pts: vec![[1.0, 1.0], [0.1, 1.0]], closed: false, color: [0; 3] };
        let o = order(vec![b, a]);
        assert_eq!(o[0].pts[0], [-1.0, 1.0]);
        assert_eq!(o[1].pts[0], [0.1, 1.0]);
    }

    #[test]
    fn finish_fits_the_frame_closes_outlines_and_keeps_the_budget() {
        let paths: Vec<Path> = (0..200).map(|i| square((i % 20) as f64 * 10.0, (i / 20) as f64 * 10.0, 4.0 + (i % 7) as f64)).collect();
        let opts = Options { budget: 400, ..Options::default() };
        let mut w = Vec::new();
        let (fig, stats) = finish(paths, &opts, "grille", &mut w).unwrap();
        assert!(stats.laser_points <= 400, "{stats:?}");
        assert!(!stats.over_budget);
        assert!(stats.dropped > 0 && !w.is_empty());
        for s in &fig.frames[0].strokes {
            assert_eq!(s.points.first(), s.points.last());
            assert!(s.points.iter().flatten().all(|v| v.abs() <= 0.9 + 1e-4));
        }
        // Measured the way the engine draws it.
        let pts = fig.frame_points(0, 1.0, |_| (1.0, 1.0, 1.0));
        assert!(crate::engine::densify(&pts).len() <= 400);
        // A figure too big even alone is kept but flagged.
        let big = vec![Path { pts: vec![[0.0, 0.0], [1.0, 0.0]], closed: false, color: [0, 0, 255] }];
        let (_, stats) = finish(big, &Options { budget: 100, size: 1.0, ..Options::default() }, "x", &mut w).unwrap();
        assert!(stats.over_budget);
        assert!(w.last().unwrap().contains("trop lourde"));
    }

    #[test]
    fn hostile_numbers_never_reach_the_figure() {
        let paths = vec![
            Path { pts: vec![[f64::NAN, 0.0], [1.0, 1.0]], closed: false, color: [255; 3] },
            Path { pts: vec![[0.0, 0.0], [1e300, 1.0]], closed: false, color: [255; 3] },
        ];
        let (fig, _) = finish(paths, &Options::default(), "x", &mut Vec::new()).unwrap();
        assert!(fig.frames[0].strokes.iter().flat_map(|s| &s.points).flatten().all(|v| v.is_finite() && v.abs() <= 1.0));
    }
}
