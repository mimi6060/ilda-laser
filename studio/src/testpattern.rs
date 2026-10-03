//! Test patterns and alignment tools (T-404): seven patterns drawn by our
//! own code, to frame the field, check the geometry, balance the colours
//! and set the scan speed.
//!
//! None of them is a copy of a published test frame. The scan-speed one
//! only borrows the public *idea* of a circle that must touch the sides of
//! its square when the scanners keep up; its proportions and point count
//! are ours.
//!
//! A pattern replaces the output look until the operator leaves it (see
//! `select`): the cue deck keeps its cues and the timeline is paused, not
//! stopped. It then goes down the same output path as any look: `densify`,
//! calibration, every safety stage (horizon, zones, strobe limiter, static
//! beam guard, power caps) and the arming gate. Live modifiers, LFOs and
//! audio routes don't touch it (exact geometry). Selecting a pattern never
//! arms anything, and the mode is never saved: it is off at start-up.

use crate::patterns::Point;
use crate::Shared;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestPattern {
    CrossFrame,
    Grid,
    CircleSquare,
    ColorBars,
    WhiteRamp,
    ScanSpeed,
    Blanking,
}

/// UI names, in menu order.
pub const LABELS_FR: [(TestPattern, &str); 7] = [
    (TestPattern::CrossFrame, "Croix et cadre"),
    (TestPattern::Grid, "Grille"),
    (TestPattern::CircleSquare, "Cercle dans un carré"),
    (TestPattern::ColorBars, "Barres de couleur"),
    (TestPattern::WhiteRamp, "Rampe de blanc"),
    (TestPattern::ScanSpeed, "Vitesse de balayage"),
    (TestPattern::Blanking, "Extinction et retard couleur"),
];

/// Default pattern brightness: deliberately low, the power caps of the
/// output still apply on top.
pub const DEFAULT_BRIGHTNESS: f32 = 0.2;

/// The pattern mode. Memory only: never in a look, a project or a file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TestPatternState {
    pub active: Option<TestPattern>,
    /// 0..1, multiplies every colour of the pattern.
    pub brightness: f32,
    /// Reserved for T-012 (one pattern per zone/output): `None` = every
    /// output. Stored and reported, not used yet.
    pub target: Option<String>,
    /// The timeline was playing when the pattern took over: it resumes
    /// when the pattern is left.
    #[serde(skip)]
    pub resume_timeline: bool,
}

impl Default for TestPatternState {
    fn default() -> Self {
        Self { active: None, brightness: DEFAULT_BRIGHTNESS, target: None, resume_timeline: false }
    }
}

/// Shows `pattern` (or leaves the pattern mode with `None`). Entering
/// pauses a playing timeline; leaving resumes it where it was, unless the
/// operator moved it meanwhile or an emergency stop halted it. The cue
/// deck is never touched. Never arms.
pub fn select(s: &mut Shared, pattern: Option<TestPattern>) {
    s.test_pattern.active = pattern;
    if pattern.is_some() {
        hold_timeline(s);
    } else if std::mem::take(&mut s.test_pattern.resume_timeline)
        && s.timeline.transport == crate::timeline::Transport::Paused
        && !s.estop.is_latched()
    {
        let c = s.timeline_clock();
        s.timeline.play(&c);
    }
}

/// While a pattern is shown, a timeline that starts playing (from the
/// UI, MIDI...) is paused at once, and resumes with the look. Called by
/// the engine every frame. Returns the pattern to show, if any.
pub fn hold_timeline(s: &mut Shared) -> Option<(TestPattern, f32)> {
    let p = s.test_pattern.active?;
    if s.timeline.is_playing() {
        let c = s.timeline_clock();
        s.timeline.pause(&c);
        s.test_pattern.resume_timeline = true;
    }
    Some((p, s.test_pattern.brightness))
}

/// Parses a pattern name from the API (`"grid"`...).
pub fn parse(name: &str) -> Option<TestPattern> {
    serde_json::from_value(serde_json::Value::String(name.into())).ok()
}

/// The pattern's points (normalised, before `densify`): closed outlines
/// repeat their first point, strokes are joined by blanked moves, and no
/// colour channel goes above `brightness`.
pub fn points(p: TestPattern, brightness: f32) -> Vec<Point> {
    let b = if brightness.is_finite() { brightness.clamp(0.0, 1.0) } else { 0.0 };
    let mut d = Drawing::default();
    let white = [b, b, b];
    match p {
        TestPattern::CrossFrame => {
            // The whole field's edge, then a centre cross.
            d.poly(&[(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)], white);
            d.line((-0.3, 0.0), (0.3, 0.0), white);
            d.line((0.0, -0.3), (0.0, 0.3), white);
        }
        TestPattern::Grid => {
            // 8 × 8 cells: a horizontal serpentine (the turns run along
            // the outer columns), then a vertical one.
            const E: f32 = 0.8;
            let at = |i: usize| -E + 2.0 * E * i as f32 / 8.0;
            let mut path = Vec::new();
            for i in 0..=8 {
                let (a, z) = if i % 2 == 0 { (-E, E) } else { (E, -E) };
                path.push((a, at(i)));
                path.push((z, at(i)));
            }
            d.poly(&path, white);
            path.clear();
            for i in 0..=8 {
                let (a, z) = if i % 2 == 0 { (E, -E) } else { (-E, E) };
                path.push((at(8 - i), a));
                path.push((at(8 - i), z));
            }
            d.poly(&path, white);
        }
        TestPattern::CircleSquare => {
            // Equal sides and a round circle when X/Y scales match; the
            // tick shows which way is up (rotation, mirroring).
            const R: f32 = 0.75;
            d.poly(&[(-R, -R), (R, -R), (R, R), (-R, R), (-R, -R)], white);
            d.circle(R, 144, white);
            d.line((0.0, R), (0.0, 0.95), white);
            d.line((0.0, 0.95), (0.1, 0.85), white);
        }
        TestPattern::ColorBars => {
            // One channel per stroke, then white.
            for (i, c) in [[b, 0.0, 0.0], [0.0, b, 0.0], [0.0, 0.0, b], white].into_iter().enumerate() {
                let y = 0.45 - 0.3 * i as f32;
                d.line((-0.8, y), (0.8, y), c);
            }
        }
        TestPattern::WhiteRamp => {
            // Ten steps, 10 % → 100 % of the pattern brightness, left to right.
            const X0: f32 = -0.9;
            const STEP: f32 = 0.18;
            d.jump((X0, 0.0));
            for k in 0..10 {
                let level = b * (k + 1) as f32 / 10.0;
                let x = X0 + STEP * (k + 1) as f32;
                d.to((x, 0.0), [level; 3]);
            }
        }
        TestPattern::ScanSpeed => {
            // A square and its inscribed circle, traced at the same point
            // spacing: if the scanners lag, the circle shrinks away from
            // the sides and the square's corners round off. Plus two
            // diameters to see the overshoot on sharp reversals.
            const R: f32 = 0.6;
            d.poly(&[(-R, -R), (R, -R), (R, R), (-R, R), (-R, -R)], white);
            d.circle(R, 126, white);
            d.line((-R, 0.0), (R, 0.0), white);
            d.line((0.0, R), (0.0, -R), white);
        }
        TestPattern::Blanking => {
            // Short dashes with blanked moves between them: tails or dots
            // show the blanking delay.
            for i in 0..8 {
                let x = -0.9 + 0.24 * i as f32;
                d.line((x, 0.4), (x + 0.12, 0.4), white);
            }
            // Hard colour changes along one stroke: a colour that lands
            // early or late shows the colour delay.
            let colours = [[b, 0.0, 0.0], [0.0, b, 0.0], [0.0, 0.0, b], white];
            d.jump((-0.9, -0.4));
            for i in 0..8 {
                d.to((-0.9 + 0.225 * (i + 1) as f32, -0.4), colours[i % 4]);
            }
        }
    }
    d.points
}

/// Builds strokes: the pen goes down with `to`, `jump` moves it blanked.
#[derive(Default)]
struct Drawing {
    points: Vec<Point>,
}

impl Drawing {
    /// Blanked move to `p`: the beam goes off where it is, travels dark,
    /// and the next stroke starts there.
    fn jump(&mut self, p: (f32, f32)) {
        if let Some(last) = self.points.last().copied() {
            if last.x == p.0 && last.y == p.1 {
                return;
            }
            self.points.push(Point::blanked(last.x, last.y));
        }
        self.points.push(Point::blanked(p.0, p.1));
    }

    /// Lit stroke from the pen to `p`, in colour `c`.
    fn to(&mut self, p: (f32, f32), c: [f32; 3]) {
        if let Some(last) = self.points.last().copied().filter(|l| !l.is_lit()) {
            // Turn the beam on at the start of the stroke.
            self.points.push(Point::lit(last.x, last.y, c[0], c[1], c[2]));
        }
        self.points.push(Point::lit(p.0, p.1, c[0], c[1], c[2]));
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [f32; 3]) {
        self.poly(&[a, b], c);
    }

    fn poly(&mut self, path: &[(f32, f32)], c: [f32; 3]) {
        let Some((&first, rest)) = path.split_first() else { return };
        self.jump(first);
        if self.points.last().is_some_and(|l| l.is_lit()) {
            // Starting on the end of the last stroke: break it anyway, so
            // every stroke starts dark.
            self.points.push(Point::blanked(first.0, first.1));
        }
        for &p in rest {
            self.to(p, c);
        }
    }

    /// Closed circle from (r, 0), counter-clockwise, `n` segments.
    fn circle(&mut self, r: f32, n: usize, c: [f32; 3]) {
        let mut path: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                (r * a.cos(), r * a.sin())
            })
            .collect();
        path.push(path[0]);
        self.poly(&path, c);
    }
}

/// The pattern as the engine sends it into the safety stage: densified,
/// then calibrated (clamped to −1..1 by the calibration).
pub fn output_frame(p: TestPattern, brightness: f32, calibration: &crate::engine::Calibration) -> Vec<Point> {
    crate::engine::densify(&points(p, brightness))
        .into_iter()
        .map(|q| {
            let (x, y) = calibration.apply(q.x, q.y);
            Point { x, y, ..q }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{densify, Calibration};
    use crate::safety::{self, SafetySettings, StrobeLimiter};
    use crate::zones::{Zone, ZoneKind};

    const ALL: [TestPattern; 7] = [
        TestPattern::CrossFrame,
        TestPattern::Grid,
        TestPattern::CircleSquare,
        TestPattern::ColorBars,
        TestPattern::WhiteRamp,
        TestPattern::ScanSpeed,
        TestPattern::Blanking,
    ];

    /// Strokes: runs of consecutive lit points.
    fn strokes(pts: &[Point]) -> Vec<Vec<Point>> {
        let mut out: Vec<Vec<Point>> = Vec::new();
        let mut cur = Vec::new();
        for p in pts {
            if p.is_lit() {
                cur.push(*p);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    #[test]
    fn every_pattern_is_in_range_bright_enough_and_small_enough() {
        assert_eq!(LABELS_FR.map(|(p, _)| p), ALL, "one label per pattern, in menu order");
        for p in ALL {
            for b in [0.0, 0.2, 1.0] {
                let raw = points(p, b);
                assert!(!raw.is_empty(), "{p:?}");
                let dense = densify(&raw);
                assert!(dense.len() <= 1500, "{p:?}: {} points", dense.len());
                for q in &dense {
                    assert!((-1.0..=1.0).contains(&q.x) && (-1.0..=1.0).contains(&q.y), "{p:?} {q:?}");
                    assert!(q.r <= b + 1e-6 && q.g <= b + 1e-6 && q.b <= b + 1e-6, "{p:?} at {b}: {q:?}");
                }
                assert_eq!(dense.iter().any(Point::is_lit), b > 0.0, "{p:?}");
            }
            assert!(points(p, 7.0).iter().all(|q| q.r <= 1.0), "brightness clamped");
            assert!(!points(p, f32::NAN).iter().any(Point::is_lit), "NaN brightness is dark");
        }
    }

    #[test]
    fn strokes_start_dark_and_are_joined_by_blanked_moves() {
        for p in ALL {
            let pts = points(p, 1.0);
            assert!(!pts[0].is_lit(), "{p:?}: the first stroke starts with the beam off");
            // A blanked move never draws: the beam goes off where the
            // stroke ends and on where the next one starts, so every move
            // between strokes is entirely dark.
            for w in pts.windows(2) {
                if w[0].is_lit() != w[1].is_lit() {
                    assert_eq!((w[0].x, w[0].y), (w[1].x, w[1].y), "{p:?}: the beam switches while moving");
                }
            }
            assert!(strokes(&pts).len() >= 2 || p == TestPattern::WhiteRamp, "{p:?}");
        }
    }

    #[test]
    fn closed_outlines_repeat_their_first_point() {
        for p in [TestPattern::CrossFrame, TestPattern::CircleSquare, TestPattern::ScanSpeed] {
            let first = &strokes(&points(p, 1.0))[0];
            assert_eq!((first[0].x, first[0].y), (first.last().unwrap().x, first.last().unwrap().y), "{p:?}");
        }
        let circle = &strokes(&points(TestPattern::CircleSquare, 1.0))[1];
        assert_eq!((circle[0].x, circle[0].y), (circle.last().unwrap().x, circle.last().unwrap().y));
    }

    #[test]
    fn the_frame_reaches_the_field_edge_and_the_grid_has_eight_cells() {
        let pts = points(TestPattern::CrossFrame, 1.0);
        for corner in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            assert!(pts.iter().any(|p| p.is_lit() && (p.x, p.y) == corner), "{corner:?}");
        }
        let grid = points(TestPattern::Grid, 1.0);
        let mut ys: Vec<i32> = grid.iter().filter(|p| p.is_lit()).map(|p| (p.y * 1000.0).round() as i32).collect();
        ys.sort();
        ys.dedup();
        assert_eq!(ys.len(), 9, "9 lines = 8 cells");
    }

    #[test]
    fn colour_bars_light_one_channel_per_stroke() {
        let s = strokes(&points(TestPattern::ColorBars, 0.5));
        assert_eq!(s.len(), 4);
        for (i, stroke) in s.iter().take(3).enumerate() {
            for p in stroke {
                let ch = [p.r, p.g, p.b];
                for (c, v) in ch.iter().enumerate() {
                    assert_eq!(*v > 0.0, c == i, "stroke {i}: {p:?}");
                }
            }
        }
        assert!(s[3].iter().all(|p| p.r == 0.5 && p.g == 0.5 && p.b == 0.5), "then white");
    }

    #[test]
    fn the_white_ramp_climbs_in_ten_steps() {
        let pts = densify(&points(TestPattern::WhiteRamp, 1.0));
        let lit: Vec<&Point> = pts.iter().filter(|p| p.is_lit()).collect();
        assert!(lit.windows(2).all(|w| w[1].r >= w[0].r && w[1].x >= w[0].x), "rising left to right");
        let mut levels: Vec<i32> = lit.iter().map(|p| (p.r * 100.0).round() as i32).collect();
        levels.dedup();
        assert_eq!(levels, (1..=10).map(|k| k * 10).collect::<Vec<_>>());
        assert!(lit.iter().all(|p| p.r == p.g && p.g == p.b), "white");
    }

    #[test]
    fn the_scan_speed_circle_touches_its_square() {
        let pts = points(TestPattern::ScanSpeed, 1.0);
        let max_x = pts.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let circle = &strokes(&pts)[1];
        assert!(circle.iter().any(|p| (p.x - max_x).abs() < 1e-6 && p.y.abs() < 1e-6), "touches the right side");
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(parse("grid"), Some(TestPattern::Grid));
        assert_eq!(parse("scan_speed"), Some(TestPattern::ScanSpeed));
        assert_eq!(parse("nope"), None);
        let st = TestPatternState::default();
        assert_eq!((st.active, st.brightness), (None, 0.2));
    }

    /// A pattern is cut by a blanking zone like any look, after calibration.
    /// (The zone's edges avoid the pattern's sample grid: a sample lying
    /// exactly on a zone edge is a separate zones.rs matter.)
    #[test]
    fn a_blank_zone_cuts_the_pattern() {
        let zone = Zone { kind: ZoneKind::Blank, points: vec![[-0.951, -0.613], [0.013, -0.613], [0.013, 0.587], [-0.951, 0.587]], ..Default::default() };
        let cfg = SafetySettings { zones: vec![zone.clone()], ..Default::default() };
        let cal = Calibration { offset_x: 0.1, ..Default::default() };
        for p in ALL {
            let frame = output_frame(p, 0.2, &cal);
            let out = safety::apply(frame, 0.0, &cfg, &mut StrobeLimiter::default());
            assert_eq!(crate::zones::blank_violations(&out, std::slice::from_ref(&zone)), 0, "{p:?}");
            assert!(out.iter().all(|q| (-1.0..=1.0).contains(&q.x)));
        }
    }
}
