//! Transitions between cues (T-158): *Coupe* (the default: nothing in
//! between), *Fondu* (the old cue dims while the new one comes up, both
//! drawn), *Fondu au noir* (the old one goes down, then the new one comes
//! up: fewer points) and *Morph* (both frames resampled to the same number
//! of points along their path, then positions and colours interpolated).
//!
//! A cue has a life cycle: *Démarrage* (fading in) → *Lecture* → *Fin*
//! (stopped, but still drawn while it fades out). The deck (cues.rs) keeps
//! the timings; this module turns them into what the engine does to each
//! rendered look, before the mixer and the whole safety chain.
//!
//! Safety: a fade only multiplies colours by a gain in 0..1, so it never
//! exceeds the cue's own brightness. A morph lights a sample only where
//! both frames are lit *on one stroke* across the step to it, so blanked
//! travel (the jump between two strokes) is never lit, whatever the point
//! counts (safety_invariants.rs checks it).

use crate::patterns::Point;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// No frame in between (the default, as before T-158).
    #[default]
    Cut,
    /// The old cue dims while the new one comes up; both are drawn.
    Fade,
    /// The old cue goes down in the first half, the new one comes up in
    /// the second: never both at once.
    FadeThroughBlack,
    /// The old frame turns into the new one, point by point.
    Morph,
}

pub const TRANSITION_KINDS: [TransitionKind; 4] = [TransitionKind::Cut, TransitionKind::Fade, TransitionKind::FadeThroughBlack, TransitionKind::Morph];
pub const TRANSITION_LABELS: [&str; 4] = ["Coupe", "Fondu", "Fondu au noir", "Morph"];

/// The grid bar's length choices (« Durée »).
pub const LENGTHS: [TransitionLength; 9] = [
    TransitionLength::Beats(0.25),
    TransitionLength::Beats(0.5),
    TransitionLength::Beats(1.0),
    TransitionLength::Beats(2.0),
    TransitionLength::Beats(4.0),
    TransitionLength::Seconds(0.5),
    TransitionLength::Seconds(1.0),
    TransitionLength::Seconds(2.0),
    TransitionLength::Seconds(4.0),
];
pub const LENGTH_LABELS: [&str; 9] = ["¼ temps", "½ temps", "1 temps", "2 temps", "4 temps", "0,5 s", "1 s", "2 s", "4 s"];
pub const MAX_BEATS: f32 = 32.0;
pub const MAX_SECONDS: f32 = 30.0;

/// How long a transition lasts: in beats of the tempo clock (read when it
/// starts) or in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionLength {
    Beats(f32),
    Seconds(f32),
}

impl Default for TransitionLength {
    fn default() -> Self {
        TransitionLength::Beats(1.0)
    }
}

impl TransitionLength {
    /// Seconds at `bpm` (120 when the clock has none).
    pub fn seconds(self, bpm: f64) -> f64 {
        let bpm = if bpm.is_finite() && bpm > 0.0 { bpm } else { 120.0 };
        match self {
            TransitionLength::Beats(b) => b as f64 * 60.0 / bpm,
            TransitionLength::Seconds(s) => s as f64,
        }
    }

    fn sanitized(self) -> Self {
        let clean = |v: f32, max: f32| if v.is_finite() { v.clamp(0.0, max) } else { 1.0 };
        match self {
            TransitionLength::Beats(b) => TransitionLength::Beats(clean(b, MAX_BEATS)),
            TransitionLength::Seconds(s) => TransitionLength::Seconds(clean(s, MAX_SECONDS)),
        }
    }
}

/// A transition setting: the grid's (global) one, or a cell's override.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transition {
    pub kind: TransitionKind,
    pub length: TransitionLength,
    /// Also for flash and solo presses (off: a flash reacts at once).
    pub in_flash: bool,
}

impl Transition {
    pub const CUT: Transition = Transition { kind: TransitionKind::Cut, length: TransitionLength::Beats(1.0), in_flash: false };

    pub fn sanitized(self) -> Self {
        Self { length: self.length.sanitized(), ..self }
    }

    /// Whether it changes anything (a zero length is a cut).
    pub fn is_cut(&self) -> bool {
        self.kind == TransitionKind::Cut || self.length.seconds(120.0) <= 0.0
    }
}

/// One running transition of a cue: how it fades in (`ActiveCue::fade`)
/// or out (an ending cue). Wall-clock seconds, like `ActiveCue::started_s`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub kind: TransitionKind,
    pub start_s: f64,
    pub length_s: f64,
    /// Morph: the cue instance on the other side (the one it turns from
    /// when fading in, into when fading out).
    pub partner: Option<u64>,
}

impl Fade {
    /// 0 at the start, 1 when done.
    pub fn progress(&self, now_s: f64) -> f32 {
        if self.length_s <= 0.0 {
            return 1.0;
        }
        (((now_s - self.start_s) / self.length_s) as f32).clamp(0.0, 1.0)
    }

    pub fn done(&self, now_s: f64) -> bool {
        self.progress(now_s) >= 1.0
    }

    /// Gain of a cue fading in at progress `p`.
    pub fn gain_in(kind: TransitionKind, p: f32) -> f32 {
        match kind {
            TransitionKind::FadeThroughBlack => (2.0 * p - 1.0).max(0.0),
            _ => p,
        }
    }

    /// Gain of a cue fading out at progress `p`.
    pub fn gain_out(kind: TransitionKind, p: f32) -> f32 {
        match kind {
            TransitionKind::FadeThroughBlack => (1.0 - 2.0 * p).max(0.0),
            _ => 1.0 - p,
        }
    }
}

/// Where a cue is in its life cycle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "phase", content = "progress", rename_all = "snake_case")]
pub enum CuePhase {
    Starting(f32),
    Playing,
    Finishing(f32),
}

/// What the engine does to one rendered look this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookFx {
    /// Colours × gain (0 = not drawn at all).
    Gain(f32),
    /// Replaced by the morph from look `from` at progress `p`.
    MorphFrom { from: u64, p: f32 },
    /// Drawn by its partner's morph: not on its own.
    Absorbed,
}

/// A rendered look: cue instance id, its points, and what the caller
/// carries along (layer, zones).
pub type Rendered<T> = (u64, Vec<Point>, T);

/// Apply this frame's transitions to the rendered looks `(id, points)`:
/// gains, morphs. Looks without an entry in `fx` are left untouched (with
/// every transition a cut, the frame is exactly what it was before).
pub fn apply<T>(looks: &mut Vec<Rendered<T>>, fx: &[(u64, LookFx)]) {
    if fx.is_empty() {
        return;
    }
    let find = |id: u64| fx.iter().find(|(i, _)| *i == id).map(|(_, f)| *f);
    // Morph sources, read before anything changes.
    let sources: Vec<(u64, Vec<Point>)> = fx
        .iter()
        .filter_map(|(_, f)| match f {
            LookFx::MorphFrom { from, .. } => looks.iter().find(|(i, _, _)| i == from).map(|(i, p, _)| (*i, p.clone())),
            _ => None,
        })
        .collect();
    for (id, points, _) in looks.iter_mut() {
        match find(*id) {
            Some(LookFx::Gain(g)) => scale(points, g),
            Some(LookFx::MorphFrom { from, p }) => match sources.iter().find(|(i, _)| *i == from) {
                Some((_, a)) => *points = morph(a, points, p),
                // The old frame is gone: fade in instead.
                None => scale(points, p),
            },
            Some(LookFx::Absorbed) | None => {}
        }
    }
    looks.retain(|(id, _, _)| !matches!(find(*id), Some(LookFx::Absorbed)));
}

/// Colours × `gain` (clamped to 0..1); 0 drops the points (nothing to draw).
pub fn scale(points: &mut Vec<Point>, gain: f32) {
    let g = if gain.is_finite() { gain.clamp(0.0, 1.0) } else { 0.0 };
    if g <= 0.0 {
        points.clear();
    } else if g < 1.0 {
        for p in points.iter_mut() {
            p.r *= g;
            p.g *= g;
            p.b *= g;
        }
    }
}

/// A sample of a frame resampled along its path: where, which colour, and
/// the lit stroke it lies on (`None` on blanked travel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub point: Point,
    pub stroke: Option<u32>,
}

/// A channel as the morph uses it: finite, within 0..1.
fn chan(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

fn clean(p: &Point) -> Point {
    if p.x.is_finite() && p.y.is_finite() {
        Point { x: p.x, y: p.y, r: chan(p.r), g: chan(p.g), b: chan(p.b) }
    } else {
        Point::blanked(0.0, 0.0)
    }
}

fn lerp(a: f32, b: f32, u: f32) -> f32 {
    a + (b - a) * u
}

/// `points` resampled to `n` samples evenly spaced along the path (travel
/// included). A sample is lit only on a segment whose two ends are lit,
/// and carries that lit run's number, so a step between two samples of
/// the same run never leaves the stroke.
pub fn resample(points: &[Point], n: usize) -> Vec<Sample> {
    if points.is_empty() || n == 0 {
        return Vec::new();
    }
    let pts: Vec<Point> = points.iter().map(clean).collect();
    if pts.len() == 1 {
        let lit = pts[0].is_lit();
        return vec![Sample { point: pts[0], stroke: lit.then_some(0) }; n];
    }
    // Lit runs: maximal sequences of lit points.
    let mut run = vec![0u32; pts.len()];
    let mut count = 0u32;
    for i in 0..pts.len() {
        if pts[i].is_lit() && (i == 0 || !pts[i - 1].is_lit()) {
            count += 1;
        }
        run[i] = count;
    }
    // Arc length, with a small share for still points (dwell), so they
    // keep some samples.
    const STILL: f32 = 1e-4;
    let mut cum = vec![0.0f32; pts.len()];
    for i in 1..pts.len() {
        let (dx, dy) = (pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
        cum[i] = cum[i - 1] + (dx * dx + dy * dy).sqrt() + STILL;
    }
    let total = cum[pts.len() - 1];
    let mut seg = 0usize;
    (0..n)
        .map(|k| {
            let s = if n == 1 { 0.0 } else { total * k as f32 / (n - 1) as f32 };
            while seg + 2 < pts.len() && cum[seg + 1] < s {
                seg += 1;
            }
            let (a, b) = (pts[seg], pts[seg + 1]);
            let span = cum[seg + 1] - cum[seg];
            let u = if span > 0.0 { ((s - cum[seg]) / span).clamp(0.0, 1.0) } else { 0.0 };
            let (x, y) = (lerp(a.x, b.x, u), lerp(a.y, b.y, u));
            if a.is_lit() && b.is_lit() {
                let point = Point { x, y, r: lerp(a.r, b.r, u), g: lerp(a.g, b.g, u), b: lerp(a.b, b.b, u) };
                Sample { point, stroke: Some(run[seg]) }
            } else {
                Sample { point: Point::blanked(x, y), stroke: None }
            }
        })
        .collect()
}

/// The frame `p` of the way from `a` to `b` (0 = `a`'s path, 1 = `b`'s),
/// with `max(a, b)` points whatever `p`. A sample is lit only when, in
/// both frames, it and the one before lie on one lit stroke: the step to
/// it is then a blend of two lit pieces of stroke, never of travel. Its
/// colour is the blend of both, so never brighter than the brighter one.
/// An empty side fades the other.
pub fn morph(a: &[Point], b: &[Point], p: f32) -> Vec<Point> {
    let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Vec::new(),
        (true, false) => {
            let mut out: Vec<Point> = b.iter().map(clean).collect();
            scale(&mut out, p);
            return out;
        }
        (false, true) => {
            let mut out: Vec<Point> = a.iter().map(clean).collect();
            scale(&mut out, 1.0 - p);
            return out;
        }
        _ => {}
    }
    let n = a.len().max(b.len()).max(2);
    let (ra, rb) = (resample(a, n), resample(b, n));
    let same_stroke = |s: &[Sample], k: usize| s[k].stroke.is_some() && s[k].stroke == s[k - 1].stroke;
    (0..n)
        .map(|k| {
            let (sa, sb) = (ra[k].point, rb[k].point);
            let (x, y) = (lerp(sa.x, sb.x, p), lerp(sa.y, sb.y, p));
            if k > 0 && same_stroke(&ra, k) && same_stroke(&rb, k) {
                Point { x, y, r: lerp(sa.r, sb.r, p), g: lerp(sa.g, sb.g, p), b: lerp(sa.b, sb.b, p) }
            } else {
                Point::blanked(x, y)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize, y: f32, c: f32) -> Vec<Point> {
        (0..n).map(|i| Point::lit(-0.5 + i as f32 / (n - 1) as f32, y, c, c, c)).collect()
    }

    #[test]
    fn resample_keeps_ends_and_count() {
        let pts = line(5, 0.2, 1.0);
        let r = resample(&pts, 11);
        assert_eq!(r.len(), 11);
        assert!((r[0].point.x + 0.5).abs() < 1e-5 && (r[10].point.x - 0.5).abs() < 1e-5);
        assert!(r.iter().all(|s| s.stroke == Some(1) && (s.point.y - 0.2).abs() < 1e-6));
        // Evenly spaced along the path.
        let steps: Vec<f32> = r.windows(2).map(|w| w[1].point.x - w[0].point.x).collect();
        assert!(steps.iter().all(|d| (d - 0.1).abs() < 1e-3), "{steps:?}");
        assert!(resample(&[], 4).is_empty());
        assert_eq!(resample(&pts[..1], 3).len(), 3);
    }

    #[test]
    fn resample_marks_travel_dark_and_numbers_strokes() {
        let mut pts = line(4, -0.5, 1.0);
        pts.push(Point::blanked(0.5, -0.5));
        pts.push(Point::blanked(-0.5, 0.5));
        pts.extend(line(4, 0.5, 1.0));
        let r = resample(&pts, 40);
        let strokes: Vec<Option<u32>> = r.iter().map(|s| s.stroke).collect();
        assert!(strokes.contains(&Some(1)) && strokes.contains(&Some(2)) && strokes.contains(&None));
        for s in &r {
            assert_eq!(s.stroke.is_some(), s.point.is_lit());
        }
    }

    #[test]
    fn morph_ends_on_each_frame_and_interpolates() {
        let (a, b) = (line(10, -0.5, 1.0), line(10, 0.5, 0.5));
        let start = morph(&a, &b, 0.0);
        assert!(start.iter().all(|p| (p.y + 0.5).abs() < 1e-6));
        let end = morph(&a, &b, 1.0);
        assert!(end.iter().all(|p| (p.y - 0.5).abs() < 1e-6));
        let mid = morph(&a, &b, 0.5);
        assert!(mid.iter().all(|p| p.y.abs() < 1e-6));
        assert!(mid.iter().skip(1).all(|p| (p.r - 0.75).abs() < 1e-5), "colours blend");
        assert!(!mid[0].is_lit(), "the first sample is where the beam arrives: dark");
    }

    #[test]
    fn morph_between_different_point_counts_has_a_stable_count() {
        let a = line(7, 0.0, 1.0);
        let mut b = line(30, 0.3, 1.0);
        b.push(Point::blanked(0.9, 0.9));
        b.extend(line(13, -0.3, 1.0));
        let n = a.len().max(b.len());
        for i in 0..=20 {
            let out = morph(&a, &b, i as f32 / 20.0);
            assert_eq!(out.len(), n);
            assert!(out.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        }
        assert_eq!(morph(&a[..1], &b[..1], 0.5).len(), 2);
        assert_eq!(morph(&[], &b, 0.5).len(), b.len());
        assert!(morph(&[], &[], 0.5).is_empty());
    }

    #[test]
    fn morph_never_lights_a_step_between_strokes() {
        // Two strokes far apart in `b`: the samples around b's jump are dark.
        let a = line(40, 0.0, 1.0);
        let mut b = line(20, -0.8, 1.0);
        b.extend(line(20, 0.8, 1.0));
        for p in [0.0, 0.3, 1.0] {
            let out = morph(&a, &b, p);
            for w in out.windows(2) {
                if w[1].is_lit() {
                    // A lit step stays within one of b's strokes (at p = 1),
                    // never crossing from y -0.8 to 0.8.
                    assert!((w[1].y - w[0].y).abs() < 0.2, "p {p}: lit step {:?} → {:?}", w[0], w[1]);
                }
            }
        }
    }

    #[test]
    fn morph_cleans_odd_colours() {
        let a = vec![Point::lit(0.0, 0.0, f32::NAN, 2.0, -1.0), Point::lit(0.1, 0.0, 0.5, 0.5, 0.5)];
        let out = morph(&a, &a, 0.5);
        assert!(out.iter().all(|p| [p.r, p.g, p.b].iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))));
    }

    #[test]
    fn gains_of_each_kind() {
        assert_eq!(Fade::gain_in(TransitionKind::Fade, 0.5), 0.5);
        assert_eq!(Fade::gain_out(TransitionKind::Fade, 0.5), 0.5);
        assert_eq!(Fade::gain_out(TransitionKind::FadeThroughBlack, 0.5), 0.0);
        assert_eq!(Fade::gain_in(TransitionKind::FadeThroughBlack, 0.5), 0.0);
        assert_eq!(Fade::gain_in(TransitionKind::FadeThroughBlack, 0.75), 0.5);
        assert_eq!(Fade::gain_out(TransitionKind::FadeThroughBlack, 0.25), 0.5);
        let f = Fade { kind: TransitionKind::Fade, start_s: 10.0, length_s: 1.0, partner: None };
        assert_eq!((f.progress(9.0), f.progress(10.5), f.progress(12.0)), (0.0, 0.5, 1.0));
        assert!(f.done(11.0) && !f.done(10.9));
        assert!(Fade { length_s: 0.0, ..f }.done(0.0));
    }

    #[test]
    fn apply_gains_morphs_and_drops_absorbed() {
        let mut looks = vec![(1u64, line(4, 0.0, 1.0), 'a'), (2, line(6, 0.5, 1.0), 'b'), (3, line(3, -0.5, 1.0), 'c')];
        let untouched = looks.clone();
        apply(&mut looks, &[]);
        assert_eq!(looks, untouched, "no transition: the frame is unchanged");
        apply(&mut looks, &[(1, LookFx::Absorbed), (2, LookFx::MorphFrom { from: 1, p: 0.0 }), (3, LookFx::Gain(0.5))]);
        assert_eq!(looks.iter().map(|l| l.2).collect::<Vec<_>>(), ['b', 'c']);
        assert_eq!(looks[0].1.len(), 6);
        assert!(looks[0].1.iter().all(|p| p.y.abs() < 1e-6), "p = 0: on the old frame");
        assert!(looks[1].1.iter().all(|p| p.r == 0.5));
        let mut gone = vec![(5u64, line(4, 0.0, 1.0), ())];
        apply(&mut gone, &[(5, LookFx::Gain(0.0))]);
        assert!(gone[0].1.is_empty(), "a look at 0 draws nothing");
        let mut orphan = vec![(6u64, line(4, 0.0, 1.0), ())];
        apply(&mut orphan, &[(6, LookFx::MorphFrom { from: 99, p: 0.25 })]);
        assert!(orphan[0].1.iter().all(|p| p.r == 0.25), "no old frame: a fade in");
    }

    #[test]
    fn transition_json_defaults_and_sanitize() {
        let t: Transition = serde_json::from_str("{}").unwrap();
        assert_eq!(t, Transition::default());
        assert_eq!(t.kind, TransitionKind::Cut);
        assert_eq!(t.length, TransitionLength::Beats(1.0));
        assert!(t.is_cut());
        let t: Transition = serde_json::from_str(r#"{"kind":"morph","length":{"seconds":99}}"#).unwrap();
        assert_eq!(t.sanitized().length, TransitionLength::Seconds(MAX_SECONDS));
        assert!(!t.is_cut());
        assert!(Transition { kind: TransitionKind::Fade, length: TransitionLength::Seconds(0.0), in_flash: false }.is_cut());
        assert_eq!(TransitionLength::Beats(2.0).seconds(120.0), 1.0);
        assert_eq!(TransitionLength::Beats(1.0).seconds(0.0), 0.5);
    }
}
