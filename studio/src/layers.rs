//! Cue layers: every cue plays on one of four layers (grid property, 1 by
//! default). The layers are drawn 1 → 4 into one frame, with blanked travel
//! between looks. A laser has no transparency: layers add up, and every
//! layer costs points, so each one has a dimmer, mute and solo, and the
//! mixer keeps the frame within a **point budget** (`pps / min fps`,
//! 750 = 30 kpps at 40 frames/s) so stacking cues never makes it flicker:
//! first the lit points of every layer are spaced out, then the
//! highest-numbered layers are cut.

use crate::engine::join_looks;
use crate::patterns::Point;
use serde::{Deserialize, Serialize};

pub const LAYER_COUNT: usize = 4;
pub const DEFAULT_POINT_BUDGET: usize = 750;
pub const MIN_POINT_BUDGET: usize = 100;
pub const MAX_POINT_BUDGET: usize = 10_000;

/// Lit points are never thinned below this fraction (1 point in 2): past
/// it the drawing falls apart, and cutting a layer is the better trade.
const MIN_KEEP: f32 = 0.5;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    /// 0..1, multiplies the colours of the layer's cues. 0 = the layer is off.
    pub dimmer: f32,
    pub mute: bool,
    /// While any layer is solo, only solo layers are drawn.
    pub solo: bool,
}

impl Default for Layer {
    fn default() -> Self {
        Self { dimmer: 1.0, mute: false, solo: false }
    }
}

/// The four layers and the point budget, saved to `studio-data/layers.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mixer {
    pub layers: [Layer; LAYER_COUNT],
    /// Most points in one frame (travel included).
    pub point_budget: usize,
}

impl Default for Mixer {
    fn default() -> Self {
        Self { layers: Default::default(), point_budget: DEFAULT_POINT_BUDGET }
    }
}

impl Mixer {
    pub fn sanitize(&mut self) {
        self.point_budget = self.point_budget.clamp(MIN_POINT_BUDGET, MAX_POINT_BUDGET);
        for l in &mut self.layers {
            l.dimmer = if l.dimmer.is_finite() { l.dimmer.clamp(0.0, 1.0) } else { 1.0 };
        }
    }

    /// `n` is 1-based. Out-of-range numbers fall back to layer 1.
    pub fn layer(&self, n: u8) -> &Layer {
        &self.layers[layer_index(n)]
    }

    pub fn layer_mut(&mut self, n: u8) -> &mut Layer {
        &mut self.layers[layer_index(n)]
    }

    /// Whether layer `n` reaches the output (not muted, not dimmed to 0,
    /// and solo if any layer is).
    pub fn audible(&self, n: u8) -> bool {
        let l = self.layer(n);
        let any_solo = self.layers.iter().any(|l| l.solo);
        !l.mute && l.dimmer > 0.0 && (!any_solo || l.solo)
    }
}

fn layer_index(n: u8) -> usize {
    (n.clamp(1, LAYER_COUNT as u8) - 1) as usize
}

/// What the budget did to this frame, for the UI.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MixReport {
    /// Points the audible layers asked for (travel included), before the budget.
    pub demand: usize,
    /// Points actually sent.
    pub points: usize,
    pub budget: usize,
    /// Lit points were spaced out to fit.
    pub decimated: bool,
    /// Layers cut to fit (1-based, highest first).
    pub dropped: Vec<u8>,
    /// Points each layer rendered (before dimmer, mute and budget).
    pub per_layer: [usize; LAYER_COUNT],
}

impl MixReport {
    pub fn over_budget(&self) -> bool {
        self.demand > self.budget
    }
}

/// Draw the rendered looks, as (layer, points), layer 1 first (looks of one
/// layer keep their order), within the point budget.
pub fn mix(rendered: Vec<(u8, Vec<Point>)>, mixer: &Mixer) -> (Vec<Point>, MixReport) {
    let budget = mixer.point_budget.max(1);
    let mut report = MixReport { budget, ..Default::default() };
    let mut looks: Vec<(u8, Vec<Point>)> = Vec::new();
    for (n, mut points) in rendered {
        let n = n.clamp(1, LAYER_COUNT as u8);
        report.per_layer[layer_index(n)] += points.len();
        if !mixer.audible(n) {
            continue;
        }
        let gain = mixer.layer(n).dimmer;
        if gain < 1.0 {
            for p in &mut points {
                p.r *= gain;
                p.g *= gain;
                p.b *= gain;
            }
        }
        looks.push((n, points));
    }
    looks.sort_by_key(|(n, _)| *n); // stable: a layer's cues keep their order

    let joined = join(&looks);
    report.demand = joined.len();
    if joined.len() <= budget {
        report.points = joined.len();
        return (joined, report);
    }
    loop {
        let (frame, fits) = thinned(&looks, budget);
        let layers_left = looks.last().map(|l| l.0) != looks.first().map(|l| l.0);
        if fits || !layers_left {
            // Fits, or a single layer is left: that one always plays
            // (thinned as far as allowed), rather than a dark output.
            report.decimated = true;
            report.points = frame.len();
            return (frame, report);
        }
        let top = looks.last().map_or(1, |l| l.0);
        looks.retain(|l| l.0 != top);
        report.dropped.push(top);
        let joined = join(&looks);
        if joined.len() <= budget {
            report.points = joined.len();
            return (joined, report);
        }
    }
}

fn join(looks: &[(u8, Vec<Point>)]) -> Vec<Point> {
    join_looks(looks.iter().map(|(_, p)| p.clone()).collect())
}

/// Space the lit points of every look out as little as possible (down to
/// `MIN_KEEP`) to fit the budget. Returns the thinnest try if none fits.
fn thinned(looks: &[(u8, Vec<Point>)], budget: usize) -> (Vec<Point>, bool) {
    let mut keep = 1.0f32;
    loop {
        keep = (keep - 0.05).max(MIN_KEEP);
        let frame = join_looks(looks.iter().map(|(_, p)| decimate(p, keep)).collect());
        if frame.len() <= budget || keep <= MIN_KEEP {
            let fits = frame.len() <= budget;
            return (frame, fits);
        }
    }
}

/// Keep about `keep` of the lit points, evenly spaced. Blanked points and
/// the ends of every lit stroke always stay, so the beam never draws
/// across a blanked jump and strokes keep their length.
pub fn decimate(points: &[Point], keep: f32) -> Vec<Point> {
    if keep >= 1.0 {
        return points.to_vec();
    }
    let lit = |i: usize| points.get(i).is_some_and(|p| p.is_lit());
    let mut acc = 0.0f32;
    let mut out = Vec::with_capacity((points.len() as f32 * keep) as usize + 8);
    for (i, &p) in points.iter().enumerate() {
        let stroke_end = !lit(i) || i == 0 || !lit(i - 1) || !lit(i + 1);
        acc += keep;
        if stroke_end || acc >= 1.0 {
            out.push(p);
            acc = acc.fract();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lit horizontal stroke of `n` points at height `y`, colour `c`.
    fn stroke(n: usize, y: f32, c: f32) -> Vec<Point> {
        (0..n).map(|i| Point { x: -0.5 + i as f32 / n as f32 * 0.02, y, r: c, g: c, b: c }).collect()
    }

    fn lit(frame: &[Point]) -> Vec<&Point> {
        frame.iter().filter(|p| p.is_lit()).collect()
    }

    #[test]
    fn layers_are_drawn_in_order_with_blanked_travel() {
        let m = Mixer::default();
        let (frame, report) = mix(vec![(2, stroke(10, 0.5, 0.8)), (1, stroke(10, -0.5, 0.4))], &m);
        assert_eq!(lit(&frame).len(), 20);
        assert_eq!(frame[0].y, -0.5, "layer 1 first");
        assert_eq!(frame.last().unwrap().y, 0.5);
        // Everything between the two strokes is blanked travel.
        let first_top = frame.iter().position(|p| p.y == 0.5).unwrap();
        assert!(frame[10..first_top].iter().all(|p| !p.is_lit()));
        assert_eq!((report.demand, report.points, report.decimated), (frame.len(), frame.len(), false));
        assert_eq!(report.per_layer, [10, 10, 0, 0]);
    }

    #[test]
    fn dimmer_scales_and_zero_removes_the_layer() {
        let mut m = Mixer::default();
        m.layer_mut(2).dimmer = 0.5;
        let (frame, _) = mix(vec![(1, stroke(4, -0.5, 0.8)), (2, stroke(4, 0.5, 0.8))], &m);
        assert!(lit(&frame).iter().filter(|p| p.y == 0.5).all(|p| (p.r - 0.4).abs() < 1e-6));
        m.layer_mut(2).dimmer = 0.0;
        let (frame, _) = mix(vec![(1, stroke(4, -0.5, 0.8)), (2, stroke(4, 0.5, 0.8))], &m);
        assert!(lit(&frame).iter().all(|p| p.y == -0.5), "only layer 1 is lit");
    }

    #[test]
    fn mute_and_solo() {
        let looks = || vec![(1, stroke(4, -0.5, 1.0)), (2, stroke(4, 0.0, 1.0)), (3, stroke(4, 0.5, 1.0))];
        let ys = |m: &Mixer| {
            let (frame, _) = mix(looks(), m);
            let mut ys: Vec<f32> = lit(&frame).iter().map(|p| p.y).collect();
            ys.dedup();
            ys
        };
        let mut m = Mixer::default();
        m.layer_mut(1).mute = true;
        assert_eq!(ys(&m), [0.0, 0.5]);
        m.layer_mut(2).solo = true;
        assert_eq!(ys(&m), [0.0], "solo layer 2: only it plays");
        m.layer_mut(3).solo = true;
        assert_eq!(ys(&m), [0.0, 0.5]);
        m.layer_mut(3).mute = true;
        assert_eq!(ys(&m), [0.0], "mute wins over solo");
    }

    #[test]
    fn over_budget_spaces_points_out_first() {
        let m = Mixer { point_budget: 300, ..Default::default() };
        let (frame, report) = mix(vec![(1, stroke(200, -0.5, 1.0)), (2, stroke(200, 0.5, 1.0))], &m);
        assert!(frame.len() <= 300, "{}", frame.len());
        assert!(report.decimated && report.dropped.is_empty());
        assert!(report.over_budget());
        // Both layers still there, each keeping its stroke ends.
        for y in [-0.5, 0.5] {
            let row: Vec<&Point> = lit(&frame).into_iter().filter(|p| p.y == y).collect();
            assert!(row.len() >= 100);
            assert_eq!(row.first().unwrap().x, -0.5);
            assert_eq!(row.last().unwrap().x, -0.5 + 199.0 / 200.0 * 0.02);
        }
    }

    #[test]
    fn then_cuts_the_highest_layers() {
        let m = Mixer::default();
        let heavy = |y| stroke(600, y, 1.0);
        let (frame, report) = mix(vec![(1, heavy(-0.6)), (2, heavy(-0.2)), (3, heavy(0.2)), (4, heavy(0.6))], &m);
        assert!(frame.len() <= DEFAULT_POINT_BUDGET, "{}", frame.len());
        assert_eq!(report.dropped, [4, 3]);
        assert!(lit(&frame).iter().any(|p| p.y == -0.6) && lit(&frame).iter().any(|p| p.y == -0.2));
        assert!(report.demand > 2400);
        assert_eq!(report.points, frame.len());
    }

    #[test]
    fn a_single_layer_is_never_cut() {
        let m = Mixer { point_budget: 100, ..Default::default() };
        let (frame, report) = mix(vec![(1, stroke(400, 0.0, 1.0)), (1, stroke(400, 0.5, 1.0))], &m);
        assert!(report.dropped.is_empty());
        assert!(!frame.is_empty());
        assert!(lit(&frame).len() >= 400, "thinned to at most 1 point in 2");
    }

    #[test]
    fn decimate_keeps_blanked_points_and_stroke_ends() {
        let mut pts = stroke(10, 0.0, 1.0);
        pts.push(Point::blanked(0.3, 0.3));
        pts.push(Point::blanked(0.4, 0.4));
        pts.extend(stroke(10, 0.5, 1.0));
        let d = decimate(&pts, 0.5);
        assert!(d.len() < pts.len());
        assert_eq!(d.iter().filter(|p| !p.is_lit()).count(), 2);
        // Ends of both strokes survive, so no lit segment crosses the jump.
        for w in d.windows(2) {
            if w[0].is_lit() && w[1].is_lit() {
                assert_eq!(w[0].y, w[1].y);
            }
        }
        assert_eq!(decimate(&pts, 1.0), pts);
    }

    #[test]
    fn mixer_json_defaults_and_sanitize() {
        let old: Mixer = serde_json::from_str("{}").unwrap();
        assert_eq!(old, Mixer::default());
        let mut m: Mixer = serde_json::from_str(r#"{"point_budget": 5, "layers": [{"dimmer": 3}, {}, {"mute": true}, {}]}"#).unwrap();
        m.sanitize();
        assert_eq!(m.point_budget, MIN_POINT_BUDGET);
        assert_eq!(m.layers[0].dimmer, 1.0);
        assert!(m.layers[2].mute && !m.audible(3));
    }
}
