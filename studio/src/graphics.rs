//! Graphics (T-108, looks 34 to 36 of `docs/research/festival-looks.md`):
//! a 4-3-2-1 countdown for a DJ's entrance, a progress ring that closes as
//! the drop comes (the `clock` generator in tempo, see `progress_ring`),
//! and wireframe solids turning in perspective. Our own maths and our own
//! single-stroke font (`font.rs`), nothing taken from another product.
//!
//! - `countdown`: shows `count`, `count` − 1, … 1, one number per step
//!   (`steps_per_beat`: 0.25 = one per bar, 1 = one per beat), counted from
//!   the cue's start (a timeline event's start, or the bar a cue was
//!   launched in). Dark during the last 1/2 beat of the last step (at most
//!   half that step) and after it: the black before the drop. Centred;
//!   size × `DIGIT_H` = the height of a one-digit number (shared between
//!   the digits of a longer one).
//! - `clock` in tempo (`beat_sync`): a ring filling clockwise from twelve
//!   o'clock over `period_beats` (0 → 360°), then empty again; reversed
//!   (`direction` < 0), it empties instead. Four ticks mark the quarters
//!   so the ring reads even when it has just started.
//! - `wire3d`: a solid's edges (`a`: 0 cube, 1 octahedron, 2 icosahedron,
//!   3 square pyramid, 4 globe of parallels and meridians), deduplicated
//!   and chained into as few strokes as the solid allows (an Euler walk,
//!   see `chain`), so the beam is only blanked between edges that don't
//!   follow on. The solid turns one
//!   turn per `period_beats` around two axes (vertical and horizontal, the
//!   latter tilted so it is never seen edge-on), seen in perspective from
//!   `CAMERA` radii away. Vertices sit on the unit sphere and the
//!   perspective only ever shrinks them, so every point stays inside the
//!   disc of radius size (≤ 1) whatever the rotation.

use crate::beat;
use crate::generators::{BeamStyle, GenCtx, GenParams, Geometry, Stroke, Tint};
use std::collections::BTreeSet;
use std::f32::consts::{PI, TAU};

/// `countdown`: highest starting number.
pub const COUNT_MAX: u32 = 99;
/// `countdown`: steps per beat when unset or invalid (one number a bar).
pub const COUNTDOWN_STEPS: f32 = 0.25;
/// `countdown`: dark for this long at the end of the last step, in beats.
pub const COUNTDOWN_DARK: f64 = 0.5;
/// `countdown`: height of a one-digit number, × size.
const DIGIT_H: f32 = 1.6;
/// Progress ring: segments of a full ring, and the ticks' inner radius.
const RING_SEGMENTS: f32 = 96.0;
const TICK_IN: f32 = 0.85;
/// `wire3d`: number of solids, camera distance (in radii) and the
/// horizontal axis' fixed tilt (radians).
pub const SOLIDS: usize = 5;
pub const CAMERA: f32 = 3.0;
const TILT: f32 = 0.45;
/// `wire3d` globe: segments per full circle.
const GLOBE_SEGMENTS: usize = 16;

/// Draw generator `name`, or `None` if it isn't one of these.
pub fn generate(name: &str, p: &GenParams, ctx: &GenCtx) -> Option<Geometry> {
    let w = size(ctx.scale);
    match name {
        "countdown" => Some(countdown(p, ctx, w)),
        "wire3d" => Some(wire3d(p, ctx.t, w)),
        _ => None,
    }
}

fn size(scale: f32) -> f32 {
    if scale.is_finite() { scale.clamp(0.0, 1.0) } else { 0.0 }
}

/// One blanked point at the centre: dark, but never an empty frame (a DAC
/// may repeat the last frame it got).
fn dark() -> Geometry {
    Geometry::lines(vec![vec![(0.0, 0.0)]]).with_styles(vec![BeamStyle { intensity: 0.0, tint: Tint::Auto }])
}

// ---------- countdown ----------

/// The number `countdown` shows at `beat_pos` beats from its start, `None`
/// when dark.
pub fn countdown_value(p: &GenParams, beat_pos: f64) -> Option<u32> {
    let n = p.count.clamp(1, COUNT_MAX);
    let spb = if p.steps_per_beat.is_finite() && p.steps_per_beat > 0.0 { p.steps_per_beat } else { COUNTDOWN_STEPS };
    let step_len = 1.0 / spb as f64;
    let dark_from = n as f64 * step_len - COUNTDOWN_DARK.min(step_len / 2.0);
    // Before the start (and NaN): the first number.
    let pos = if beat_pos.is_finite() { beat_pos.max(0.0) } else { 0.0 };
    if pos + 1e-9 >= dark_from {
        return None;
    }
    let k = beat::step_index(pos, spb).min(n as u64 - 1) as u32;
    Some(n - k)
}

fn countdown(p: &GenParams, ctx: &GenCtx, w: f32) -> Geometry {
    let Some(v) = countdown_value(p, ctx.beat_pos) else { return dark() };
    let text = v.to_string();
    // The font's cell is twice as tall as `text_strokes`' scale and sits
    // above the baseline: centred here on its own bounding box.
    let strokes = crate::fonts::text_strokes(&text, DIGIT_H * w / 2.0 / text.len() as f32);
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in strokes.iter().flatten() {
        for i in 0..2 {
            (lo[i], hi[i]) = (lo[i].min(q[i]), hi[i].max(q[i]));
        }
    }
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let strokes: Vec<Stroke> = strokes.into_iter().map(|s| s.into_iter().map(|[x, y]| (x - cx, y - cy)).collect()).collect();
    if strokes.is_empty() || w <= 0.0 { dark() } else { Geometry::lines(strokes) }
}

// ---------- progress ring ----------

/// How full the ring is at `beat_pos`, 0..1 (see the module doc).
pub fn ring_fill(p: &GenParams, ctx: &GenCtx) -> f32 {
    let period = if p.period_beats.is_finite() && p.period_beats > 0.0 { p.period_beats } else { 4.0 };
    ctx.cycle(&GenParams { period_beats: period, ..p.clone() })
}

/// A point at angle `a` (clockwise from twelve o'clock) on radius `r`.
fn on_dial(r: f32, a: f32) -> (f32, f32) {
    (r * a.sin(), r * a.cos())
}

/// The progress ring's strokes: four quarter ticks, then the arc of
/// `fill` (0..1) of a turn, radius `r`.
pub fn progress_ring(fill: f32, r: f32) -> Vec<Stroke> {
    let fill = if fill.is_finite() { fill.clamp(0.0, 1.0) } else { 0.0 };
    let mut strokes: Vec<Stroke> = (0..4).map(|q| {
        let a = q as f32 * TAU / 4.0;
        vec![on_dial(TICK_IN * r, a), on_dial(r, a)]
    }).collect();
    if fill > 1e-3 {
        let n = (RING_SEGMENTS * fill).ceil().max(1.0) as usize;
        strokes.push((0..=n).map(|i| on_dial(r, fill * TAU * i as f32 / n as f32)).collect());
    }
    strokes
}

// ---------- 3D wireframes ----------

type V3 = [f32; 3];

/// The solid `a` stands for (0 cube … 4 globe), rounded and clamped.
pub fn solid_index(a: f32) -> usize {
    if a.is_finite() { a.round().clamp(0.0, (SOLIDS - 1) as f32) as usize } else { 0 }
}

fn normalized(v: V3) -> V3 {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn dist2(a: V3, b: V3) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Every pair of vertices at the shortest distance between two of them:
/// the edges of a regular solid.
fn nearest_pairs(v: &[V3]) -> Vec<(usize, usize)> {
    let mut d_min = f32::MAX;
    for i in 0..v.len() {
        for j in 0..v.len() {
            if i != j {
                d_min = d_min.min(dist2(v[i], v[j]));
            }
        }
    }
    let mut edges = Vec::new();
    for i in 0..v.len() {
        for j in 0..v.len() {
            // Both directions: `solid` deduplicates.
            if i != j && dist2(v[i], v[j]) < d_min * 1.01 {
                edges.push((i, j));
            }
        }
    }
    edges
}

/// Solid `kind`'s vertices (on the unit sphere) and its edges, each once.
pub fn solid(kind: usize) -> (Vec<V3>, Vec<(usize, usize)>) {
    let (v, raw): (Vec<V3>, Vec<(usize, usize)>) = match kind {
        // Octahedron: the six axis ends.
        1 => {
            let v = vec![[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
            let e = nearest_pairs(&v);
            (v, e)
        }
        // Icosahedron: (0, ±1, ±φ) and its cyclic permutations.
        2 => {
            let phi = (1.0 + 5f32.sqrt()) / 2.0;
            let mut v = Vec::new();
            for s1 in [-1.0, 1.0] {
                for s2 in [-1.0, 1.0] {
                    v.push(normalized([0.0, s1, s2 * phi]));
                    v.push(normalized([s1, s2 * phi, 0.0]));
                    v.push(normalized([s2 * phi, 0.0, s1]));
                }
            }
            let e = nearest_pairs(&v);
            (v, e)
        }
        // Square pyramid: apex on top, base a third of the way down.
        3 => {
            let (y, r) = (-1.0 / 3.0, (8f32 / 9.0).sqrt());
            let mut v = vec![[0.0, 1.0, 0.0]];
            v.extend((0..4).map(|i| {
                let a = i as f32 * TAU / 4.0 + PI / 4.0;
                [r * a.cos(), y, r * a.sin()]
            }));
            let e = (1..5).flat_map(|i| [(0, i), (i, i % 4 + 1)]).collect();
            (v, e)
        }
        // Globe: three parallels (±45° and the equator) and two great
        // circles through the poles, as short straight edges.
        4 => {
            let n = GLOBE_SEGMENTS;
            let mut v: Vec<V3> = vec![[0.0, 1.0, 0.0], [0.0, -1.0, 0.0]];
            let mut e = Vec::new();
            let lats = [PI / 4.0, 0.0, -PI / 4.0];
            for lat in lats {
                let first = v.len();
                v.extend((0..n).map(|i| {
                    let lon = i as f32 * TAU / n as f32;
                    [lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin()]
                }));
                e.extend((0..n).map(|i| (first + i, first + (i + 1) % n)));
            }
            // Meridians every quarter turn: pole → the three parallels → pole.
            for i in (0..n).step_by(n / 4) {
                let ring = |k: usize| 2 + k * n + i;
                e.extend([(0, ring(0)), (ring(0), ring(1)), (ring(1), ring(2)), (ring(2), 1)]);
            }
            (v, e)
        }
        // Cube: corners differing in one coordinate.
        _ => {
            let k = 1.0 / 3f32.sqrt();
            let v: Vec<V3> = (0..8).map(|i| [if i & 1 == 0 { -k } else { k }, if i & 2 == 0 { -k } else { k }, if i & 4 == 0 { -k } else { k }]).collect();
            let e = (0..8usize).flat_map(|i| [1usize, 2, 4].map(move |bit| (i, i ^ bit))).collect();
            (v, e)
        }
    };
    let edges: BTreeSet<(usize, usize)> = raw.into_iter().filter(|(a, b)| a != b).map(|(a, b)| (a.min(b), a.max(b))).collect();
    (v, edges.into_iter().collect())
}

/// Edges chained into paths of vertex indices, as few as there can be:
/// the vertices with an odd number of edges are paired by virtual edges,
/// an Euler circuit goes through everything (Hierholzer), and the circuit
/// is cut at the virtual edges (the blanked jumps). Every edge is in
/// exactly one path once.
pub fn chain(vertices: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut all = edges.to_vec();
    let degree = |v: usize| edges.iter().filter(|(a, b)| *a == v || *b == v).count();
    let odd: Vec<usize> = (0..vertices).filter(|&v| degree(v) % 2 == 1).collect();
    all.extend(odd.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])));
    let virtual_edge = |k: usize| k >= edges.len();
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); vertices];
    for (k, &(a, b)) in all.iter().enumerate() {
        adj[a].push((b, k));
        adj[b].push((a, k));
    }
    let mut used = vec![false; all.len()];
    let mut next = vec![0usize; vertices];
    let mut paths = Vec::new();
    while let Some(start) = (0..vertices).find(|&v| adj[v].iter().any(|(_, k)| !used[*k])) {
        // The circuit, backwards: (vertex, edge it was reached by).
        let mut stack: Vec<(usize, Option<usize>)> = vec![(start, None)];
        let mut circuit = Vec::new();
        while let Some(&(v, _)) = stack.last() {
            while next[v] < adj[v].len() && used[adj[v][next[v]].1] {
                next[v] += 1;
            }
            match adj[v].get(next[v]) {
                Some(&(u, k)) => {
                    used[k] = true;
                    stack.push((u, Some(k)));
                }
                None => circuit.extend(stack.pop()),
            }
        }
        // Consecutive vertices are joined by the earlier one's edge.
        let mut cut: Vec<Vec<usize>> = vec![vec![circuit[0].0]];
        for w in circuit.windows(2) {
            match w[0].1 {
                Some(k) if virtual_edge(k) => cut.push(vec![w[1].0]),
                _ => cut.last_mut().unwrap().push(w[1].0),
            }
        }
        // A closed circuit: its last piece runs on into its first.
        if cut.len() > 1 {
            let last = cut.pop().unwrap();
            cut[0] = last.into_iter().chain(cut[0].iter().skip(1).copied()).collect();
        }
        paths.extend(cut.into_iter().filter(|p| p.len() >= 2));
    }
    paths
}

/// `v` turned by `yaw` around the vertical axis then by `pitch` around the
/// horizontal one, seen in perspective: x, y on screen within the unit disc.
pub fn project(v: V3, yaw: f32, pitch: f32) -> (f32, f32) {
    let (sy, cy) = yaw.sin_cos();
    let (x, z) = (v[0] * cy + v[2] * sy, -v[0] * sy + v[2] * cy);
    let (sp, cp) = pitch.sin_cos();
    let (y, z) = (v[1] * cp - z * sp, v[1] * sp + z * cp);
    // z ≤ 1 on the unit sphere: the factor is at most 1.
    let k = (CAMERA - 1.0) / (CAMERA - z.min(1.0));
    (x * k, y * k)
}

fn wire3d(p: &GenParams, t: f32, w: f32) -> Geometry {
    let (v, edges) = solid(solid_index(p.a));
    let t = if t.is_finite() { t } else { 0.0 };
    let screen: Vec<(f32, f32)> = v.iter().map(|&q| {
        let (x, y) = project(q, t, t + TILT);
        (x * w, y * w)
    }).collect();
    Geometry::lines(chain(v.len(), &edges).into_iter().map(|path| path.into_iter().map(|i| screen[i]).collect()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::{colorize, generate as any, ColorMode, GENERATOR_NAMES};

    const NAMES: [&str; 2] = ["countdown", "wire3d"];

    fn at(beat_pos: f64) -> GenCtx {
        GenCtx { beat_pos, ..GenCtx::at_time(0.0, 0.0, 0.0, 0.7) }
    }

    fn count(n: u32, steps: f32) -> GenParams {
        GenParams { count: n, steps_per_beat: steps, beat_sync: true, ..Default::default() }
    }

    fn lit(g: &Geometry) -> bool {
        colorize(g, ColorMode::Solid, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), 0.0, 1.0).iter().any(|q| q.is_lit())
    }

    #[test]
    fn graphics_are_appended_after_the_textures() {
        let pos = |name: &str| GENERATOR_NAMES.iter().position(|n| *n == name).unwrap();
        for name in NAMES {
            assert!(pos(name) > pos("fat_beams"), "{name}: appended, saved looks refer to generators by name");
        }
        assert!(generate("clock", &GenParams::default(), &at(0.0)).is_none());
    }

    #[test]
    fn countdown_shows_4_3_2_1_one_a_bar_then_black() {
        let p = count(4, 0.25);
        let shown = |beat: f64| countdown_value(&p, beat);
        assert_eq!(shown(0.0), Some(4));
        assert_eq!(shown(3.99), Some(4));
        assert_eq!(shown(4.0), Some(3));
        assert_eq!(shown(8.0), Some(2));
        assert_eq!(shown(12.0), Some(1));
        assert_eq!(shown(15.4), Some(1));
        // Dark for the last 1/2 beat, and after.
        assert_eq!(shown(15.5), None);
        assert_eq!(shown(16.0), None);
        assert_eq!(shown(100.0), None);
        // Before its start, and with a broken clock: the first number.
        assert_eq!(shown(-3.0), Some(4));
        assert_eq!(shown(f64::NAN), Some(4));
        // Through the generator: the right glyph count, then a blanked point.
        let geo = |beat| generate("countdown", &p, &at(beat)).unwrap();
        assert_eq!(geo(0.5).strokes.len(), 4, "a 4 is four segments");
        assert_eq!(geo(13.0).strokes.len(), 2, "a 1 is two segments");
        assert!(lit(&geo(13.0)));
        let black = geo(15.75);
        assert!(!black.strokes.is_empty() && !lit(&black));
    }

    #[test]
    fn countdown_steps_follow_steps_per_beat() {
        // One number a beat: 8 … 1, dark from 7.5 beats.
        let p = count(8, 1.0);
        let row: Vec<Option<u32>> = (0..9).map(|b| countdown_value(&p, b as f64 + 0.25)).collect();
        assert_eq!(row, [Some(8), Some(7), Some(6), Some(5), Some(4), Some(3), Some(2), Some(1), None]);
        assert_eq!(countdown_value(&p, 7.5), None);
        // Short steps: dark for half the last step only.
        let fast = count(3, 4.0);
        assert_eq!(countdown_value(&fast, 0.5), Some(1));
        assert_eq!(countdown_value(&fast, 0.625), None);
        // Unset steps: one a bar. Out-of-range counts are clamped.
        assert_eq!(countdown_value(&count(4, 0.0), 4.0), Some(3));
        assert_eq!(countdown_value(&count(4, f32::NAN), 8.0), Some(2));
        assert_eq!(countdown_value(&count(0, 1.0), 0.0), Some(1));
        assert_eq!(countdown_value(&count(500, 1.0), 0.0), Some(COUNT_MAX));
    }

    #[test]
    fn progress_ring_is_half_full_at_half_the_period() {
        let p = GenParams { beat_sync: true, period_beats: 16.0, ..Default::default() };
        assert!((ring_fill(&p, &at(8.0)) - 0.5).abs() < 1e-6);
        assert!((ring_fill(&p, &at(4.0)) - 0.25).abs() < 1e-6);
        assert_eq!(ring_fill(&p, &at(16.0)), 0.0, "empty again each period");
        // Reversed: it empties.
        assert!((ring_fill(&GenParams { direction: -1, ..p.clone() }, &at(4.0)) - 0.75).abs() < 1e-6);
        // Through the clock: the arc ends at six o'clock, clockwise via three.
        let geo = any("clock", &p, &at(8.0)).unwrap();
        let arc = geo.strokes.last().unwrap();
        let (first, last) = (arc[0], *arc.last().unwrap());
        assert!(first.0.abs() < 1e-5 && (first.1 - 0.7).abs() < 1e-5, "starts at twelve: {first:?}");
        assert!(last.0.abs() < 1e-4 && (last.1 + 0.7).abs() < 1e-5, "half a turn: {last:?}");
        assert!(arc.iter().all(|q| q.0 >= -1e-5), "clockwise, through three o'clock");
        // Just started: the ticks still show.
        let start = any("clock", &p, &at(0.0)).unwrap();
        assert_eq!(start.strokes.len(), 4);
        assert!(lit(&start));
        // Free-running, it is still the wall clock (circle, 12 ticks, 3 hands).
        assert_eq!(any("clock", &GenParams::default(), &at(8.0)).unwrap().strokes.len(), 16);
    }

    #[test]
    fn solids_have_the_right_edges_each_once() {
        // (vertices, edges, every vertex's edge count)
        let want = [(8, 12, Some(3)), (6, 12, Some(4)), (12, 30, Some(5)), (5, 8, None), (2 + 3 * GLOBE_SEGMENTS, 3 * GLOBE_SEGMENTS + 16, None)];
        for (kind, (nv, ne, degree)) in want.into_iter().enumerate() {
            let (v, e) = solid(kind);
            assert_eq!((v.len(), e.len()), (nv, ne), "solid {kind}");
            assert!(v.iter().all(|q| (dist2(*q, [0.0; 3]) - 1.0).abs() < 1e-5), "solid {kind} on the unit sphere");
            let set: BTreeSet<_> = e.iter().collect();
            assert_eq!(set.len(), e.len(), "solid {kind}: deduplicated");
            assert!(e.iter().all(|(a, b)| a < b && *b < nv));
            if let Some(d) = degree {
                assert!((0..nv).all(|i| e.iter().filter(|(a, b)| *a == i || *b == i).count() == d), "solid {kind}");
            }
            // Chained: every edge drawn exactly once, along real edges.
            let paths = chain(nv, &e);
            let mut drawn: Vec<(usize, usize)> = paths.iter().flat_map(|p| p.windows(2).map(|w| (w[0].min(w[1]), w[0].max(w[1])))).collect();
            drawn.sort();
            assert_eq!(drawn, e, "solid {kind}");
            assert!(paths.len() < e.len(), "solid {kind}: edges that touch share a stroke");
        }
        assert_eq!(chain(8, &solid(0).1).len(), 4, "a cube needs 4 strokes (8 odd vertices)");
        assert_eq!(chain(6, &solid(1).1).len(), 1, "an octahedron is drawn in one stroke");
        assert_eq!((solid_index(-3.0), solid_index(1.4), solid_index(9.0), solid_index(f32::NAN)), (0, 1, 4, 0));
    }

    #[test]
    fn wire3d_stays_in_the_field_at_any_rotation() {
        for kind in 0..SOLIDS {
            let p = GenParams { a: kind as f32, ..Default::default() };
            for scale in [0.0, 0.5, 1.0, 1.5, f32::NAN] {
                for k in 0..400 {
                    let t = k as f32 * 0.0731 - 7.0;
                    let g = generate("wire3d", &p, &GenCtx::at_time(t, 0.0, 0.0, scale)).unwrap();
                    assert!(!g.strokes.is_empty());
                    for &(x, y) in g.strokes.iter().flatten() {
                        assert!(x.is_finite() && y.is_finite() && x * x + y * y <= 1.0 + 1e-5, "solid {kind} at t={t}: ({x}, {y})");
                    }
                }
            }
        }
        // Random orientations straight through the projection.
        for i in 0..2000u64 {
            let r = |j| crate::beat::seeded_rand(i, j) * 40.0 - 20.0;
            let v = normalized([r(0), r(1), r(2) + 1e-3]);
            let (x, y) = project(v, r(3), r(4));
            assert!(x * x + y * y <= 1.0 + 1e-5);
        }
    }

    #[test]
    fn wire3d_turns_one_turn_per_period_in_tempo() {
        let p = GenParams { a: 0.0, beat_sync: true, period_beats: 8.0, ..Default::default() };
        let draw = |t: f32| generate("wire3d", &p, &GenCtx::at_time(t, 0.0, 0.0, 0.6)).unwrap().strokes;
        let close = |a: &Vec<Stroke>, b: &Vec<Stroke>| a.iter().flatten().zip(b.iter().flatten()).all(|(p, q)| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4);
        assert!(close(&draw(0.0), &draw(TAU)), "a full turn comes back");
        assert!(!close(&draw(0.0), &draw(TAU / 8.0)), "it moves");
        // Perspective: the near face is drawn bigger than the far one.
        let (v, _) = solid(0);
        let near = project(v[7], 0.0, 0.0);
        let far = project(v[3], 0.0, 0.0);
        assert!(near.0.abs() > far.0.abs());
    }

    #[test]
    fn graphics_fit_the_budget_with_the_optimizer() {
        let on = crate::optimize::OptimizeParams { enabled: true, ..Default::default() };
        let cases = [("countdown", 0.0), ("countdown", 1.0), ("wire3d", 0.0), ("wire3d", 1.0), ("wire3d", 2.0), ("wire3d", 3.0), ("wire3d", 4.0)];
        for (name, a) in cases {
            let p = GenParams { count: 8, a, beat_sync: true, period_beats: 8.0, steps_per_beat: 1.0, ..Default::default() };
            let mut o = crate::optimize::Optimizer::new(on.clone());
            for k in 0..60 {
                let ctx = GenCtx { beat_pos: k as f64 * 0.1, ..GenCtx::at_time(k as f32 * 0.1, 0.0, 0.0, 1.0) };
                let pts = o.optimize(&colorize(&generate(name, &p, &ctx).unwrap(), ColorMode::Rainbow, (1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.0, 1.0));
                assert!(pts.len() <= 1000, "{name} {a}: {} points", pts.len());
            }
        }
    }
}
