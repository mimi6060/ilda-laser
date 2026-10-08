//! OBJ reader (T-462): a generic Wavefront OBJ file of the operator's own
//! becomes a wireframe object (`wire::WireObject`).
//!
//! Only what a laser draws is read: `v x y z` (vertices), `f …` (faces of
//! 3 vertices or more; indices `a`, `a/b`, `a//c`, `a/b/c`, negative ones
//! relative to the end) and `l …` (polylines). Everything else (`vt`, `vn`,
//! `o`, `g`, `s`, `usemtl`, `mtllib`, comments…) is ignored without error:
//! no `.mtl` file, texture or any other file is ever opened, whatever the
//! OBJ names. Decimal point only; `\n` or `\r\n` line ends.
//!
//! Then: each side of a face is an edge, duplicates merged; an edge
//! between two faces that are coplanar (within `merge_coplanar_deg`) is
//! hidden, so a cube exported as 12 triangles still gives 12 edges; the
//! object is centred on its bounding box and scaled into the unit sphere;
//! the edges are chained (an Euler walk through shared vertices, then the
//! strokes in nearest-neighbour order) so the beam is blanked as little
//! as possible.
//!
//! The file is untrusted: its size, the vertices read and the edges kept
//! are capped (`MAX_OBJ_BYTES`, `MAX_VERTICES`, `MAX_RAW_EDGES`,
//! `wire::MAX_EDGES`), non-finite numbers are refused, faces and lines
//! naming a missing vertex are skipped and counted, every error is a
//! French message. The web handler runs it on its own thread and catches
//! panics.

use crate::wire::{WireObject, MAX_EDGES};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashMap;

pub const MAX_OBJ_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_VERTICES: usize = 50_000;
/// Distinct edges read before merging and simplification: bounds the
/// memory and time a file can take (« garder les n plus longues » can then
/// bring a dense model down to `MAX_EDGES`).
pub const MAX_RAW_EDGES: usize = 200_000;
/// Vertices of one face or polyline.
pub const MAX_FACE_VERTICES: usize = 1_000;
/// The coplanar threshold's range, in degrees.
pub const MAX_COPLANAR_DEG: f32 = 30.0;
/// Edges shorter than this (after normalisation) are dropped: a dot.
const MIN_EDGE: f32 = 1e-5;

/// Which axis of the file points up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
}

impl UpAxis {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "y" | "Y" => Some(UpAxis::Y),
            "z" | "Z" => Some(UpAxis::Z),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjOptions {
    /// Hide the edges between faces within this angle (degrees, 0..=30);
    /// 0 keeps every edge.
    pub merge_coplanar_deg: f32,
    pub up: UpAxis,
    /// Keep only the n longest edges.
    pub keep_longest: Option<usize>,
}

impl Default for ObjOptions {
    fn default() -> Self {
        Self { merge_coplanar_deg: 1.0, up: UpAxis::Y, keep_longest: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ObjReport {
    /// Vertices read.
    pub vertices: usize,
    /// Faces and polylines read.
    pub faces: usize,
    pub lines: usize,
    /// Edges kept (what the laser draws).
    pub edges: usize,
    /// Distinct edges before the coplanar merge and the simplification.
    pub edges_before: usize,
    /// Edges hidden between coplanar faces.
    pub coplanar: usize,
    /// Edges left out by « garder les n plus longues ».
    pub dropped: usize,
    /// Faces, lines and vertices skipped (a missing vertex, unreadable numbers).
    pub ignored: usize,
    /// Strokes drawn (each one starts with a blanked travel).
    pub strokes: usize,
    /// Laser points per frame at the cue's size (`WireObject::estimated_points`).
    pub est_points: usize,
}

/// The bytes of an uploaded file: size and text checks, then `parse_obj`.
pub fn parse_obj_bytes(bytes: &[u8], opts: &ObjOptions) -> Result<(WireObject, ObjReport)> {
    if bytes.is_empty() {
        bail!("Fichier OBJ illisible : le fichier est vide.");
    }
    if bytes.len() > MAX_OBJ_BYTES {
        bail!("Fichier OBJ trop gros : {} Mo au plus. Exportez l'objet avec moins de détails.", MAX_OBJ_BYTES / (1024 * 1024));
    }
    if bytes.contains(&0) {
        bail!("Fichier OBJ illisible : ce n'est pas un fichier texte (un OBJ s'exporte en texte).");
    }
    let Ok(text) = std::str::from_utf8(bytes) else { bail!("Fichier OBJ illisible : ce n'est pas un fichier texte (un OBJ s'exporte en texte).") };
    parse_obj(text, opts)
}

/// Resolves one OBJ index (1-based, negative = from the end) against the
/// vertices read so far.
fn index(token: &str, count: usize) -> Option<u32> {
    let first = token.split('/').next()?;
    let i: i64 = first.parse().ok()?;
    let n = count as i64;
    let k = match i {
        1.. => i - 1,
        ..=-1 => n + i,
        0 => return None,
    };
    (0..n).contains(&k).then_some(k as u32)
}

/// Newell's normal of a polygon (unit, or `None` when degenerate).
fn normal(v: &[[f64; 3]], face: &[u32]) -> Option<[f64; 3]> {
    let mut n = [0.0f64; 3];
    for (i, &a) in face.iter().enumerate() {
        let (p, q) = (v[a as usize], v[face[(i + 1) % face.len()] as usize]);
        n[0] += (p[1] - q[1]) * (p[2] + q[2]);
        n[1] += (p[2] - q[2]) * (p[0] + q[0]);
        n[2] += (p[0] - q[0]) * (p[1] + q[1]);
    }
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    (len > 1e-12 && len.is_finite()).then(|| n.map(|c| c / len))
}

/// The text of an OBJ file → a wireframe object (named later by the
/// caller) and what was read.
pub fn parse_obj(text: &str, opts: &ObjOptions) -> Result<(WireObject, ObjReport)> {
    if text.len() > MAX_OBJ_BYTES {
        bail!("Fichier OBJ trop gros : {} Mo au plus. Exportez l'objet avec moins de détails.", MAX_OBJ_BYTES / (1024 * 1024));
    }
    let mut report = ObjReport::default();
    let mut verts: Vec<[f64; 3]> = Vec::new();
    // Each distinct edge (low, high) → the faces it borders; from a
    // polyline: always kept.
    let mut edges: HashMap<(u32, u32), (Vec<u32>, bool)> = HashMap::new();
    let mut order: Vec<(u32, u32)> = Vec::new();
    let mut faces: Vec<Vec<u32>> = Vec::new();
    let mut add_edge = |a: u32, b: u32, face: Option<u32>, order: &mut Vec<(u32, u32)>| -> Result<()> {
        if a == b {
            return Ok(());
        }
        let key = (a.min(b), a.max(b));
        if !edges.contains_key(&key) {
            if order.len() >= MAX_RAW_EDGES {
                bail!(
                    "Objet trop détaillé : plus de {} arêtes dans le fichier. Simplifiez l'objet dans votre logiciel 3D avant de l'exporter.",
                    crate::wire::thousands(MAX_RAW_EDGES)
                );
            }
            order.push(key);
        }
        let entry = edges.entry(key).or_insert_with(|| (Vec::new(), false));
        match face {
            // Three faces are enough to know it is not between two; a
            // face that goes along it twice borders it once.
            Some(f) if entry.0.len() < 3 && !entry.0.contains(&f) => entry.0.push(f),
            Some(_) => {}
            None => entry.1 = true,
        }
        Ok(())
    };
    for (n, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let line = line.split('#').next().unwrap_or("");
        let mut tokens = line.split_whitespace();
        match tokens.next() {
            Some("v") => {
                if verts.len() >= MAX_VERTICES {
                    bail!(
                        "Objet trop détaillé : plus de {} sommets. Simplifiez l'objet dans votre logiciel 3D avant de l'exporter.",
                        crate::wire::thousands(MAX_VERTICES)
                    );
                }
                let xyz: Vec<f64> = tokens.by_ref().take(3).map_while(|t| t.parse::<f64>().ok()).collect();
                if xyz.len() < 3 {
                    // Still counted as a vertex, so later indices keep their meaning.
                    report.ignored += 1;
                    verts.push([f64::NAN; 3]);
                    continue;
                }
                if xyz.iter().any(|c| !c.is_finite() || c.abs() > 1e30) {
                    bail!("Fichier OBJ illisible : valeur non finie à la ligne {}.", n + 1);
                }
                verts.push([xyz[0], xyz[1], xyz[2]]);
            }
            Some(kind @ ("f" | "l")) => {
                let ids: Option<Vec<u32>> = tokens.by_ref().take(MAX_FACE_VERTICES + 1).map(|t| index(t, verts.len())).collect();
                let face = kind == "f";
                let min = if face { 3 } else { 2 };
                let Some(ids) = ids.filter(|ids| ids.len() >= min && ids.len() <= MAX_FACE_VERTICES && ids.iter().all(|&i| verts[i as usize][0].is_finite())) else {
                    report.ignored += 1;
                    continue;
                };
                if face {
                    let f = faces.len() as u32;
                    for (i, &a) in ids.iter().enumerate() {
                        add_edge(a, ids[(i + 1) % ids.len()], Some(f), &mut order)?;
                    }
                    faces.push(ids);
                    report.faces += 1;
                } else {
                    for w in ids.windows(2) {
                        add_edge(w[0], w[1], None, &mut order)?;
                    }
                    report.lines += 1;
                }
            }
            // vt, vn, o, g, s, usemtl, mtllib (never opened), anything else.
            _ => {}
        }
    }
    report.vertices = verts.len();
    if verts.iter().all(|v| !v[0].is_finite()) {
        bail!("Fichier OBJ illisible : aucun sommet.");
    }
    if order.is_empty() {
        bail!("Fichier OBJ illisible : aucune face ni ligne (un nuage de points ne se trace pas).");
    }
    report.edges_before = order.len();

    // Coplanar faces: the edge between them is hidden.
    let limit = if opts.merge_coplanar_deg.is_finite() { opts.merge_coplanar_deg.clamp(0.0, MAX_COPLANAR_DEG) } else { 1.0 };
    let normals: Vec<Option<[f64; 3]>> = if limit > 0.0 { faces.iter().map(|f| normal(&verts, f)).collect() } else { Vec::new() };
    let cos_limit = (limit as f64).to_radians().cos();
    let kept: Vec<(u32, u32)> = order
        .into_iter()
        .filter(|key| {
            let (around, from_line) = &edges[key];
            if *from_line || limit <= 0.0 || around.len() != 2 {
                return true;
            }
            match (normals[around[0] as usize], normals[around[1] as usize]) {
                // Either winding: a face turned the other way is still in the plane.
                (Some(a), Some(b)) => (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).abs() < cos_limit,
                _ => true,
            }
        })
        .collect();
    report.coplanar = report.edges_before - kept.len();

    // Normalise: centre of the bounding box of what is drawn, unit sphere,
    // the chosen axis up.
    let used = |f: &dyn Fn(&(u32, u32)) -> u32| kept.iter().map(f).collect::<Vec<_>>();
    let mut ends = used(&|e| e.0);
    ends.extend(used(&|e| e.1));
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for &i in &ends {
        for k in 0..3 {
            lo[k] = lo[k].min(verts[i as usize][k]);
            hi[k] = hi[k].max(verts[i as usize][k]);
        }
    }
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let radius = ends
        .iter()
        .map(|&i| {
            let v = verts[i as usize];
            ((v[0] - centre[0]).powi(2) + (v[1] - centre[1]).powi(2) + (v[2] - centre[2]).powi(2)).sqrt()
        })
        .fold(0.0f64, f64::max);
    if !(radius > 1e-12 && radius.is_finite()) {
        bail!("Fichier OBJ illisible : l'objet n'a pas de taille (tous ses sommets au même endroit).");
    }
    // Only the vertices the edges use, renumbered.
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut place = |i: u32| -> u32 {
        *remap.entry(i).or_insert_with(|| {
            let v = verts[i as usize];
            let p = [0, 1, 2].map(|k| ((v[k] - centre[k]) / radius) as f32);
            vertices.push(match opts.up {
                UpAxis::Y => p,
                // Z up: the file's Z becomes the vertical, its Y goes away from the viewer.
                UpAxis::Z => [p[0], p[2], -p[1]],
            });
            (vertices.len() - 1) as u32
        })
    };
    let mut list: Vec<(u32, u32)> = kept.iter().map(|&(a, b)| (place(a), place(b))).collect();
    let len = |&(a, b): &(u32, u32), v: &[[f32; 3]]| {
        let (p, q) = (v[a as usize], v[b as usize]);
        ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
    };
    list.retain(|e| len(e, &vertices) >= MIN_EDGE);
    if list.is_empty() {
        bail!("Fichier OBJ illisible : aucune arête à tracer.");
    }
    if let Some(n) = opts.keep_longest.filter(|&n| n < list.len()) {
        let n = n.max(1);
        // Longest first; ties keep the file's order (stable sort).
        list.sort_by(|a, b| len(b, &vertices).total_cmp(&len(a, &vertices)));
        report.dropped = list.len() - n;
        list.truncate(n);
    }
    if list.len() > MAX_EDGES {
        bail!("{}", crate::wire::too_detailed(list.len()));
    }
    let edges = chain(&vertices, &list);
    // Vertices no edge uses any more (after « garder ») are dropped.
    let mut used = vec![u32::MAX; vertices.len()];
    let mut kept_vertices = Vec::new();
    let mut renumber = |i: u32| -> u32 {
        if used[i as usize] == u32::MAX {
            used[i as usize] = kept_vertices.len() as u32;
            kept_vertices.push(vertices[i as usize]);
        }
        used[i as usize]
    };
    let edges: Vec<[u32; 2]> = edges.into_iter().map(|[a, b]| [renumber(a), renumber(b)]).collect();
    let obj = WireObject { vertices: kept_vertices, edges, ..WireObject::default() };
    report.edges = obj.edges.len();
    report.strokes = obj.strokes();
    report.est_points = obj.estimated_points();
    Ok((obj, report))
}

/// The edges in drawing order: chained into as few strokes as the graph
/// allows (`graphics::chain`, an Euler walk), then the strokes ordered
/// nearest-first from the last end, each one reversed if its other end is
/// closer. Every edge is in the result exactly once.
fn chain(vertices: &[[f32; 3]], edges: &[(u32, u32)]) -> Vec<[u32; 2]> {
    let pairs: Vec<(usize, usize)> = edges.iter().map(|&(a, b)| (a as usize, b as usize)).collect();
    let mut paths: Vec<Vec<usize>> = crate::graphics::chain(vertices.len(), &pairs);
    let dist = |a: usize, b: usize| {
        let (p, q) = (vertices[a], vertices[b]);
        (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)
    };
    let mut out = Vec::with_capacity(edges.len());
    let mut at: Option<usize> = None;
    while !paths.is_empty() {
        let (best, reverse) = match at {
            None => (0, false),
            Some(v) => {
                let mut best = (0, false, f32::MAX);
                for (i, p) in paths.iter().enumerate() {
                    let (s, e) = (dist(v, p[0]), dist(v, *p.last().unwrap_or(&p[0])));
                    if s < best.2 {
                        best = (i, false, s);
                    }
                    if e < best.2 {
                        best = (i, true, e);
                    }
                }
                (best.0, best.1)
            }
        };
        let mut path = paths.swap_remove(best);
        if reverse {
            path.reverse();
        }
        out.extend(path.windows(2).map(|w| [w[0] as u32, w[1] as u32]));
        at = path.last().copied();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A cube with six square faces, written by hand.
    pub const CUBE: &str = "# cube\nmtllib cube.mtl\no Cube\nv -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1\nv -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\nvn 0 0 1\nvt 0 0\nusemtl rouge\ns off\nf 1 2 3 4\nf 5 8 7 6\nf 1 5 6 2\nf 2 6 7 3\nf 3 7 8 4\nf 5 1 4 8\n";

    /// The same cube as 12 triangles, with the other index forms and CRLF.
    const CUBE_TRIS: &str = "v -1 -1 -1\r\nv 1 -1 -1\r\nv 1 1 -1\r\nv -1 1 -1\r\nv -1 -1 1\r\nv 1 -1 1\r\nv 1 1 1\r\nv -1 1 1\r\n\
        f 1/1 2/2 3/3\r\nf 1//1 3//1 4//1\r\nf 5/1/1 8/1/1 7/1/1\r\nf 5 7 6\r\nf 1 5 6\r\nf 1 6 2\r\nf 2 6 7\r\nf 2 7 3\r\nf 3 7 8\r\nf 3 8 4\r\nf 5 1 4\r\nf 5 4 8\r\n";

    fn parse(text: &str) -> (WireObject, ObjReport) {
        parse_obj(text, &ObjOptions::default()).unwrap()
    }

    fn err(text: &str) -> String {
        format!("{:#}", parse_obj(text, &ObjOptions::default()).unwrap_err())
    }

    /// Edges as sets of positions (independent of numbering and direction).
    fn edge_set(o: &WireObject) -> BTreeSet<[[i32; 3]; 2]> {
        let key = |i: u32| o.vertices[i as usize].map(|c| (c * 1000.0).round() as i32);
        o.edges.iter().map(|&[a, b]| { let (p, q) = (key(a), key(b)); if p < q { [p, q] } else { [q, p] } }).collect()
    }

    #[test]
    fn a_cube_of_square_faces_gives_12_edges() {
        let (o, r) = parse(CUBE);
        assert_eq!((r.vertices, r.faces, r.edges, r.ignored, r.coplanar), (8, 6, 12, 0, 0));
        assert_eq!(o.vertices.len(), 8);
        assert_eq!(edge_set(&o).len(), 12);
        // Each edge is in the drawing order once; the chained cube has at most 4 strokes.
        assert!(r.strokes <= 4, "{} strokes", r.strokes);
        assert_eq!(r.strokes, o.strokes());
        assert!(r.est_points > 0);
    }

    #[test]
    fn a_triangulated_cube_gives_12_edges_merged_18_without() {
        let (merged, r) = parse(CUBE_TRIS);
        assert_eq!((r.faces, r.edges, r.edges_before, r.coplanar), (12, 12, 18, 6));
        assert_eq!(edge_set(&merged), edge_set(&parse(CUBE).0), "the very same 12 edges, no diagonal");
        let (all, r) = parse_obj(CUBE_TRIS, &ObjOptions { merge_coplanar_deg: 0.0, ..Default::default() }).unwrap();
        assert_eq!((r.edges, r.coplanar, all.edges.len()), (18, 0, 18));
    }

    #[test]
    fn a_tetrahedron_and_polylines() {
        let (o, r) = parse("v 0 1 0\nv -1 -1 1\nv 1 -1 1\nv 0 -1 -1\nf 1 2 3\nf 1 3 4\nf 1 4 2\nf 2 4 3\n");
        assert_eq!((r.faces, r.edges, o.vertices.len()), (4, 6, 4));
        // Polylines: each segment an edge, shared ones once, never merged away.
        let (o, r) = parse("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nl 1 2 3 4 1\nl 2 1\n");
        assert_eq!((r.lines, r.edges, r.coplanar), (2, 4, 0));
        assert_eq!(o.strokes(), 1, "a closed square in one stroke");
    }

    #[test]
    fn negative_indices_and_unknown_lines() {
        let text = "# exporté\ng groupe\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0.5 0.5\nvn 0 0 1\nusemtl a\nmtllib ../../../etc/passwd\ncall autre.obj\nf -3/1/1 -2/1/1 -1/1/1\nfoo bar\n";
        let (o, r) = parse(text);
        assert_eq!((r.faces, r.edges, r.ignored), (1, 3, 0));
        assert_eq!(o.vertices.len(), 3);
    }

    #[test]
    fn missing_vertices_are_skipped_and_counted() {
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\nf 1 2 9\nf 0 1 2\nl 1 -7\nf 1 2\nf 1 x 3\nv 1 2\n";
        let (o, r) = parse(text);
        assert_eq!((r.faces, r.edges, r.ignored), (1, 3, 6));
        assert_eq!(o.edges.len(), 3);
    }

    #[test]
    fn non_finite_values_and_empty_or_binary_files_are_refused() {
        assert!(err("v 0 0 0\nv nan 0 0\nf 1 2 1\n").contains("valeur non finie à la ligne 2"));
        assert!(err("v 0 0 inf\n").contains("non finie"));
        assert!(err("v 1e40 0 0\n").contains("non finie"));
        assert!(err("").contains("aucun sommet"));
        assert!(err("# rien\nvt 0 0\n").contains("Fichier OBJ illisible : aucun sommet."));
        assert!(err("v 0 0 0\nv 1 1 1\n").contains("aucune face ni ligne"));
        assert!(err("v 1 1 1\nv 1 1 1\nl 1 2\n").contains("pas de taille"));
        let o = ObjOptions::default();
        assert!(format!("{:#}", parse_obj_bytes(b"", &o).unwrap_err()).contains("vide"));
        assert!(format!("{:#}", parse_obj_bytes(b"\x89PNG\r\n\x1a\n\0\0", &o).unwrap_err()).contains("pas un fichier texte"));
        assert!(format!("{:#}", parse_obj_bytes(&[0xff, 0xfe, b'v'], &o).unwrap_err()).contains("pas un fichier texte"));
        let huge = vec![b'#'; MAX_OBJ_BYTES + 1];
        assert!(format!("{:#}", parse_obj_bytes(&huge, &o).unwrap_err()).contains("trop gros : 8 Mo au plus"));
    }

    #[test]
    fn the_limits_hold_with_a_french_message() {
        // Too many vertices.
        let many: String = (0..=MAX_VERTICES).map(|i| format!("v {i} 0 0\n")).collect();
        assert!(err(&many).contains("plus de 50 000 sommets"));
        // A grid of 60 × 60 squares: 7 320 distinct edges, more than 2 000…
        let n = 60;
        let mut grid = String::new();
        for y in 0..=n {
            for x in 0..=n {
                grid += &format!("v {x} {y} {}\n", (x * y) % 3);
            }
        }
        for y in 0..n {
            for x in 0..n {
                let i = |x: usize, y: usize| y * (n + 1) + x + 1;
                grid += &format!("f {} {} {} {}\n", i(x, y), i(x + 1, y), i(x + 1, y + 1), i(x, y + 1));
            }
        }
        let all = ObjOptions { merge_coplanar_deg: 0.0, ..Default::default() };
        let e = format!("{:#}", parse_obj(&grid, &all).unwrap_err());
        assert!(e.starts_with("Objet trop détaillé : 7 320 arêtes, 2 000 au plus. Augmentez le seuil ou gardez moins d'arêtes."), "{e}");
        // …unless only the longest are kept.
        let (o, r) = parse_obj(&grid, &ObjOptions { keep_longest: Some(500), ..all }).unwrap();
        assert_eq!((o.edges.len(), r.dropped), (500, 7320 - 500));
        assert!(o.vertices.len() <= 1000);
        // A face with too many vertices is skipped.
        let fan: String = (0..1200).map(|i| format!("v {} {} 0\n", (i as f32).cos(), (i as f32).sin())).collect::<String>()
            + &format!("f {}\nl 1 2\n", (1..=1200).map(|i| i.to_string()).collect::<Vec<_>>().join(" "));
        let (_, r) = parse(&fan);
        assert_eq!((r.ignored, r.edges), (1, 1));
    }

    #[test]
    fn normalised_centred_in_the_unit_sphere_and_axis_up() {
        let text = "v 10 20 30\nv 14 20 30\nv 14 28 30\nv 10 28 32\nf 1 2 3 4\n";
        let (o, _) = parse(text);
        let radius = o.vertices.iter().map(|v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()).fold(0.0f32, f32::max);
        assert!((radius - 1.0).abs() < 1e-5, "{radius}");
        for k in 0..3 {
            let (lo, hi) = o.vertices.iter().map(|v| v[k]).fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c), b.max(c)));
            assert!((lo + hi).abs() < 1e-5, "axis {k} centred: {lo} {hi}");
        }
        // Z up: the file's Z extent (2) becomes the vertical one.
        let (z, _) = parse_obj(text, &ObjOptions { up: UpAxis::Z, ..Default::default() }).unwrap();
        let span = |o: &WireObject, k: usize| o.vertices.iter().map(|v| v[k]).fold(f32::MIN, f32::max) - o.vertices.iter().map(|v| v[k]).fold(f32::MAX, f32::min);
        assert!((span(&z, 1) - span(&o, 2)).abs() < 1e-5 && (span(&z, 2) - span(&o, 1)).abs() < 1e-5);
    }

    #[test]
    fn hostile_text_never_panics() {
        let mut seed = 99u64;
        let words = ["v", "f", "l", "vt", "1", "-1", "0", "2/3/4", "//", "/", "-0", "1e9", "nan", "9999999999999", "-9223372036854775808", "\r", "#", "mtllib", "\u{feff}"];
        for _ in 0..300 {
            let mut text = String::new();
            for _ in 0..200 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let w = words[(seed >> 33) as usize % words.len()];
                text.push_str(w);
                text.push(if (seed >> 20).is_multiple_of(5) { '\n' } else { ' ' });
            }
            if let Ok((o, r)) = parse_obj(&text, &ObjOptions { merge_coplanar_deg: 30.0, keep_longest: Some(3), up: UpAxis::Z }) {
                assert!(r.edges <= 3 && o.edges.iter().all(|e| (e[0] as usize) < o.vertices.len() && (e[1] as usize) < o.vertices.len()));
                assert!(o.vertices.iter().flatten().all(|c| c.abs() <= 1.0 + 1e-5));
            }
        }
    }
}
