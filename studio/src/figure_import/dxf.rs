//! DXF → polylines (T-497). Our own reader of **ASCII** DXF (R12 to 2018),
//! written from Autodesk's public DXF reference: pairs of lines, a group
//! code then its value.
//!
//! Read: the `LAYER` table (colour, off / frozen), the `BLOCKS` section
//! and the `ENTITIES` section, model space only. Drawn: `LINE`,
//! `LWPOLYLINE` and 2D / 3D `POLYLINE` (bulges become arcs), `CIRCLE`,
//! `ARC`, `ELLIPSE`, `SPLINE` (B-spline evaluated from its control points
//! and knots, rational ones too; fit points only when there is nothing
//! else) and `INSERT` of a block one level deep (base point, scale,
//! rotation, position; a block inside a block is left out). Z is dropped:
//! the drawing is seen from above. Colours: ACI index (our own generated
//! 256-colour table), 24-bit colour (code 420), `BYLAYER` from the layer
//! table, `BYBLOCK` from the insert.
//!
//! Not drawn, with a warning naming them and their count: hatches, text,
//! dimensions, 3D entities (faces, solids, meshes, polyface polylines),
//! points, anything unknown. Binary DXF and DWG are refused by `sniff`.
//!
//! The file is untrusted: its size (`MAX_FILE_BYTES`), its entities
//! (`MAX_ENTITIES`), the vertices of one entity, the stored and drawn
//! points are capped; numbers must be finite and reasonable; nothing is
//! recursive; every error is a French message.

use super::{laser_color, Options, Path, Pt};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::{PI, TAU};

/// Entities read in the whole file (blocks included).
pub const MAX_ENTITIES: usize = 200_000;
/// Vertices, control points or knots of one entity.
const MAX_VERTICES: usize = 100_000;
/// Group code pairs of one entity.
const MAX_RECORD_PAIRS: usize = 4 * MAX_VERTICES + 1_000;
/// Points after flattening, drawn ones and the ones kept in blocks each.
const MAX_FLAT_POINTS: usize = 500_000;
/// Coordinates beyond this are refused as invalid data.
const MAX_COORD: f64 = 1e12;
/// Largest angle between two points of a flattened arc (4°).
const ARC_STEP: f64 = PI / 45.0;
/// Highest spline degree accepted.
const MAX_DEGREE: usize = 10;
/// Longest layer name kept as a part name.
const MAX_PART_NAME: usize = 32;

/// Whether the start of a file is an ASCII DXF: its first pair (after
/// `999` comments) is `0` / `SECTION`.
pub(super) fn looks_like(head: &str) -> bool {
    let mut lines = head.trim_start_matches('\u{feff}').lines();
    for _ in 0..16 {
        let (Some(code), Some(value)) = (lines.next(), lines.next()) else { return false };
        match code.trim() {
            "999" => continue,
            "0" => return value.trim() == "SECTION",
            _ => return false,
        }
    }
    false
}

/// A colour as an entity or a layer gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Color {
    ByLayer,
    ByBlock,
    Rgb([u8; 3]),
}

#[derive(Clone, Debug)]
struct Layer {
    color: [u8; 3],
    /// Off (negative colour) or frozen: its entities aren't drawn.
    hidden: bool,
}

/// A flattened entity, in the coordinates of its block (or of the drawing).
#[derive(Clone, Debug)]
struct Shape {
    pts: Vec<Pt>,
    closed: bool,
    color: Color,
    /// Index in `Reader::layer_names`.
    layer: usize,
}

/// One `0 / TYPE` item and the pairs that follow it.
struct Record<'a> {
    kind: &'a str,
    pairs: Vec<(i32, &'a str)>,
}

impl<'a> Record<'a> {
    fn text(&self, code: i32) -> Option<&'a str> {
        self.pairs.iter().find(|p| p.0 == code).map(|p| p.1)
    }
    fn int(&self, code: i32) -> Option<i64> {
        self.text(code).and_then(|v| v.parse::<i64>().ok().or_else(|| v.parse::<f64>().ok().filter(|f| f.is_finite()).map(|f| f as i64)))
    }
    /// A number; `Err` when present but not a reasonable finite number.
    fn num(&self, code: i32) -> Result<Option<f64>, ()> {
        self.text(code).map(number).transpose()
    }
    fn num_or(&self, code: i32, default: f64) -> Result<f64, ()> {
        Ok(self.num(code)?.unwrap_or(default))
    }
    fn point(&self, cx: i32) -> Result<Pt, ()> {
        Ok([self.num_or(cx, 0.0)?, self.num_or(cx + 10, 0.0)?])
    }
    /// Every value of `code`, in order.
    fn nums(&self, code: i32) -> Result<Vec<f64>, ()> {
        self.pairs.iter().filter(|p| p.0 == code).map(|p| number(p.1)).collect()
    }
    fn color(&self) -> Color {
        if let Some(c) = self.int(420) {
            return Color::Rgb(true_color(c));
        }
        match self.int(62) {
            Some(0) => Color::ByBlock,
            None | Some(256) => Color::ByLayer,
            Some(i) => Color::Rgb(aci(i.unsigned_abs().min(255) as u8)),
        }
    }
    /// The extrusion direction's sign: 1 for the usual +Z, -1 for a
    /// mirrored 2D entity, `None` for a plane that isn't the drawing's.
    fn extrusion(&self) -> Result<Option<f64>, ()> {
        let n = [self.num_or(210, 0.0)?, self.num_or(220, 0.0)?, self.num_or(230, 1.0)?];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-12 {
            return Ok(Some(1.0));
        }
        let z = n[2] / len;
        Ok(if z > 1.0 - 1e-9 {
            Some(1.0)
        } else if z < -1.0 + 1e-9 {
            Some(-1.0)
        } else {
            None
        })
    }
}

fn number(v: &str) -> Result<f64, ()> {
    match v.parse::<f64>() {
        Ok(f) if f.is_finite() && f.abs() <= MAX_COORD => Ok(f),
        _ => Err(()),
    }
}

/// Group code pairs, one record at a time.
struct Pairs<'a> {
    lines: std::str::Lines<'a>,
    line: usize,
    /// The `0 / TYPE` pair read ahead.
    next_kind: Option<&'a str>,
    truncated: bool,
}

impl<'a> Pairs<'a> {
    fn pair(&mut self) -> Result<Option<(i32, &'a str)>> {
        let Some(code) = self.lines.next() else { return Ok(None) };
        self.line += 1;
        let Some(value) = self.lines.next() else {
            self.truncated = true;
            return Ok(None);
        };
        self.line += 1;
        let code = code.trim();
        let Ok(code) = code.parse::<i32>() else {
            bail!("DXF illisible : code de groupe invalide ligne {} (« {} ») ; seuls les DXF texte (ASCII) sont lus", self.line - 1, code.chars().take(20).collect::<String>());
        };
        if !(-5..=1071).contains(&code) {
            bail!("DXF illisible : code de groupe {code} inconnu ligne {}", self.line - 1);
        }
        Ok(Some((code, value.trim())))
    }

    /// The next record, `None` at the end of the file.
    fn record(&mut self) -> Result<Option<Record<'a>>> {
        let kind = match self.next_kind.take() {
            Some(k) => k,
            None => loop {
                // Stray pairs before the first `0` are skipped.
                match self.pair()? {
                    None => return Ok(None),
                    Some((0, k)) => break k,
                    Some(_) => {}
                }
            },
        };
        let mut pairs = Vec::new();
        while let Some((code, value)) = self.pair()? {
            if code == 0 {
                self.next_kind = Some(value);
                break;
            }
            if pairs.len() >= MAX_RECORD_PAIRS {
                bail!("DXF trop complexe : une entité {kind} a plus de {MAX_VERTICES} sommets");
            }
            pairs.push((code, value));
        }
        Ok(Some(Record { kind, pairs }))
    }
}

/// An `INSERT`: where and how a block is drawn.
struct Insert {
    base: Pt,
    at: Pt,
    scale: [f64; 2],
    rot: f64,
    mirror: f64,
}

impl Insert {
    fn apply(&self, p: Pt) -> Pt {
        let (x, y) = ((p[0] - self.base[0]) * self.scale[0], (p[1] - self.base[1]) * self.scale[1]);
        let (s, c) = self.rot.sin_cos();
        [(self.at[0] + x * c - y * s) * self.mirror, self.at[1] + x * s + y * c]
    }
}

#[derive(Default)]
struct Block {
    base: Pt,
    shapes: Vec<Shape>,
}

/// A `POLYLINE` waiting for its `VERTEX` records and `SEQEND`.
struct OpenPolyline {
    closed: bool,
    color: Color,
    layer: usize,
    mirror: f64,
    /// (point, bulge)
    vertices: Vec<(Pt, f64)>,
    bad: bool,
}

#[derive(Default)]
struct Reader {
    layers: HashMap<String, Layer>,
    layer_names: Vec<String>,
    layer_ids: HashMap<String, usize>,
    blocks: HashMap<String, Block>,
    /// Shapes of the drawing (model space), with inserts expanded.
    shapes: Vec<Shape>,
    drawn_points: usize,
    block_points: usize,
    entities: usize,
    ignored: BTreeMap<String, usize>,
    paper_space: usize,
    nested_inserts: usize,
    missing_blocks: usize,
    arrays: usize,
    bad_data: usize,
}

/// Where the record being read goes.
enum Target {
    Drawing,
    Block(String),
    Nowhere,
}

pub(super) fn parse(text: &str, opts: &Options, warnings: &mut Vec<String>) -> Result<(Vec<Path>, Vec<String>)> {
    let mut pairs = Pairs { lines: text.trim_start_matches('\u{feff}').lines(), line: 0, next_kind: None, truncated: false };
    let mut r = Reader::default();
    let mut section = String::new();
    let mut target = Target::Nowhere;
    let mut poly: Option<OpenPolyline> = None;
    let mut eof = false;
    while let Some(rec) = pairs.record()? {
        // A POLYLINE ends at its SEQEND, or at anything that isn't a VERTEX.
        if let Some(p) = poly.as_mut() {
            if rec.kind == "VERTEX" {
                Reader::vertex(p, &rec);
                continue;
            }
            if let Some(p) = poly.take() {
                r.end_polyline(p, &target)?;
            }
            if rec.kind == "SEQEND" {
                continue;
            }
        }
        match rec.kind {
            "SECTION" => {
                section = rec.text(2).unwrap_or("").to_ascii_uppercase();
                target = if section == "ENTITIES" { Target::Drawing } else { Target::Nowhere };
                continue;
            }
            "ENDSEC" => {
                section.clear();
                target = Target::Nowhere;
                continue;
            }
            "EOF" => {
                eof = true;
                break;
            }
            _ => {}
        }
        match section.as_str() {
            "TABLES" if rec.kind == "LAYER" => r.layer(&rec),
            "BLOCKS" => match rec.kind {
                "BLOCK" => {
                    let key = rec.text(2).unwrap_or("").to_ascii_uppercase();
                    let base = rec.point(10).unwrap_or([0.0, 0.0]);
                    // Model and paper space blocks hold no block content.
                    target = if key.starts_with("*MODEL_SPACE") || key.starts_with("*PAPER_SPACE") || r.blocks.len() >= MAX_ENTITIES {
                        Target::Nowhere
                    } else {
                        r.blocks.insert(key.clone(), Block { base, shapes: Vec::new() });
                        Target::Block(key)
                    };
                }
                "ENDBLK" => target = Target::Nowhere,
                _ => {
                    if let Target::Block(_) = target {
                        r.count()?;
                        poly = r.entity(&rec, &target)?;
                    }
                }
            },
            "ENTITIES" => {
                r.count()?;
                poly = r.entity(&rec, &target)?;
            }
            _ => {}
        }
    }
    if let Some(p) = poly.take() {
        r.end_polyline(p, &target)?;
    }
    if pairs.truncated || !eof {
        warnings.push("fichier DXF tronqué (pas de fin « EOF ») : seules les entités lues sont tracées".into());
    }
    r.warn(warnings);
    Ok(r.finish(opts, warnings))
}

impl Reader {
    fn count(&mut self) -> Result<()> {
        self.entities += 1;
        if self.entities > MAX_ENTITIES {
            bail!("DXF trop complexe : plus de {MAX_ENTITIES} entités");
        }
        Ok(())
    }

    fn layer(&mut self, rec: &Record) {
        let Some(name) = rec.text(2) else { return };
        let aci_index = rec.int(62).unwrap_or(7);
        let color = match rec.int(420) {
            Some(c) => true_color(c),
            None => aci(aci_index.unsigned_abs().min(255) as u8),
        };
        let frozen = rec.int(70).is_some_and(|f| f & 1 != 0);
        self.layers.insert(name.to_ascii_uppercase(), Layer { color, hidden: aci_index < 0 || frozen });
    }

    fn layer_id(&mut self, name: &str) -> usize {
        let key = name.to_ascii_uppercase();
        if let Some(&id) = self.layer_ids.get(&key) {
            return id;
        }
        self.layer_names.push(name.to_string());
        self.layer_ids.insert(key, self.layer_names.len() - 1);
        self.layer_names.len() - 1
    }

    fn ignore(&mut self, what: &str) {
        let name: String = what.chars().filter(|c| !c.is_control()).take(24).collect();
        *self.ignored.entry(name).or_default() += 1;
    }

    /// One entity record. Returns the POLYLINE it opens, if any.
    fn entity(&mut self, rec: &Record, target: &Target) -> Result<Option<OpenPolyline>> {
        if rec.int(67) == Some(1) {
            self.paper_space += 1;
            return Ok(None);
        }
        if rec.int(60) == Some(1) {
            // Invisible.
            return Ok(None);
        }
        let layer = self.layer_id(rec.text(8).unwrap_or("0"));
        let color = rec.color();
        let shape = |pts: Vec<Pt>, closed: bool| Shape { pts, closed, color, layer };
        let result: Result<Option<Shape>, ()> = (|| {
            Ok(match rec.kind {
                "LINE" => Some(shape(vec![rec.point(10)?, rec.point(11)?], false)),
                "LWPOLYLINE" => {
                    let Some(mirror) = rec.extrusion()? else { return Err(()) };
                    let closed = rec.int(70).is_some_and(|f| f & 1 != 0);
                    let mut vertices: Vec<(Pt, f64)> = Vec::new();
                    for &(code, value) in &rec.pairs {
                        match code {
                            10 => vertices.push(([number(value)?, 0.0], 0.0)),
                            20 => vertices.last_mut().ok_or(())?.0[1] = number(value)?,
                            42 => vertices.last_mut().ok_or(())?.1 = number(value)?,
                            _ => {}
                        }
                    }
                    if vertices.len() > MAX_VERTICES {
                        return Err(());
                    }
                    Some(shape(mirrored(bulged(&vertices, closed), mirror), closed))
                }
                "CIRCLE" | "ARC" => {
                    let Some(mirror) = rec.extrusion()? else { return Err(()) };
                    let (c, radius) = (rec.point(10)?, rec.num_or(40, 0.0)?);
                    if radius <= 0.0 {
                        return Err(());
                    }
                    let (pts, closed) = if rec.kind == "CIRCLE" {
                        (arc(c, radius, 0.0, TAU, false), true)
                    } else {
                        let a0 = rec.num_or(50, 0.0)?.to_radians().rem_euclid(TAU);
                        let mut a1 = rec.num_or(51, 360.0)?.to_radians().rem_euclid(TAU);
                        if a1 <= a0 {
                            a1 += TAU;
                        }
                        (arc(c, radius, a0, a1 - a0, true), false)
                    };
                    Some(shape(mirrored(pts, mirror), closed))
                }
                "ELLIPSE" => {
                    let Some(n) = rec.extrusion()? else { return Err(()) };
                    let (c, m) = (rec.point(10)?, rec.point(11)?);
                    let ratio = rec.num_or(40, 1.0)?;
                    let (t0, mut t1) = (rec.num_or(41, 0.0)?.rem_euclid(TAU), rec.num_or(42, TAU)?.rem_euclid(TAU));
                    if ratio <= 0.0 || ratio > 1e6 || m[0].hypot(m[1]) < 1e-12 {
                        return Err(());
                    }
                    if t1 <= t0 {
                        t1 += TAU;
                    }
                    let full = (t1 - t0 - TAU).abs() < 1e-9;
                    // Minor axis = N × major × ratio.
                    let minor = [-m[1] * n * ratio, m[0] * n * ratio];
                    let steps = arc_steps(t1 - t0);
                    let last = if full { steps - 1 } else { steps };
                    let pts = (0..=last)
                        .map(|i| {
                            let t = t0 + (t1 - t0) * i as f64 / steps as f64;
                            [c[0] + m[0] * t.cos() + minor[0] * t.sin(), c[1] + m[1] * t.cos() + minor[1] * t.sin()]
                        })
                        .collect();
                    Some(shape(pts, full))
                }
                "SPLINE" => {
                    let flags = rec.int(70).unwrap_or(0);
                    let closed = flags & 1 != 0;
                    let degree = rec.int(71).unwrap_or(3);
                    let ctrl: Vec<Pt> = rec.nums(10)?.into_iter().zip(rec.nums(20)?).map(|(x, y)| [x, y]).collect();
                    let fit: Vec<Pt> = rec.nums(11)?.into_iter().zip(rec.nums(21)?).map(|(x, y)| [x, y]).collect();
                    if ctrl.len() > MAX_VERTICES || fit.len() > MAX_VERTICES {
                        return Err(());
                    }
                    if !(1..=MAX_DEGREE as i64).contains(&degree) {
                        return Err(());
                    }
                    let pts = if ctrl.len() > degree as usize {
                        bspline(&ctrl, &rec.nums(40)?, &rec.nums(41)?, degree as usize)
                    } else if fit.len() >= 2 {
                        fit
                    } else {
                        return Err(());
                    };
                    // A closed spline ends where it starts: no repeat.
                    let mut pts = pts;
                    if closed && pts.len() > 2 && dist(pts[0], pts[pts.len() - 1]) < 1e-9 {
                        pts.pop();
                    }
                    let closed = closed && pts.len() > 2;
                    Some(shape(pts, closed))
                }
                _ => None,
            })
        })();
        match rec.kind {
            "LINE" | "LWPOLYLINE" | "CIRCLE" | "ARC" | "ELLIPSE" | "SPLINE" => match result {
                Ok(Some(s)) => self.push(s, target)?,
                Ok(None) => {}
                Err(()) => self.bad_data += 1,
            },
            "POLYLINE" => {
                let flags = rec.int(70).unwrap_or(0);
                if flags & (16 | 64) != 0 {
                    self.ignore("POLYLINE 3D (maillage)");
                    return Ok(None);
                }
                let mirror = match rec.extrusion() {
                    Ok(Some(m)) => m,
                    _ => {
                        self.bad_data += 1;
                        return Ok(None);
                    }
                };
                return Ok(Some(OpenPolyline { closed: flags & 1 != 0, color, layer, mirror, vertices: Vec::new(), bad: false }));
            }
            "VERTEX" | "SEQEND" => {}
            "INSERT" => match target {
                Target::Drawing => self.insert(rec, color, layer)?,
                _ => self.nested_inserts += 1,
            },
            "POINT" | "ATTRIB" | "ATTDEF" => self.ignore(rec.kind),
            other => self.ignore(other),
        }
        Ok(None)
    }

    /// A VERTEX of the open POLYLINE.
    fn vertex(poly: &mut OpenPolyline, rec: &Record) {
        // Spline frame control points are not on the curve.
        if rec.int(70).is_some_and(|f| f & 16 != 0) {
            return;
        }
        if poly.vertices.len() >= MAX_VERTICES {
            poly.bad = true;
            return;
        }
        match (rec.point(10), rec.num_or(42, 0.0)) {
            (Ok(p), Ok(b)) => poly.vertices.push((p, b)),
            _ => poly.bad = true,
        }
    }

    fn end_polyline(&mut self, p: OpenPolyline, target: &Target) -> Result<()> {
        if p.bad || p.vertices.is_empty() {
            self.bad_data += 1;
            return Ok(());
        }
        let pts = mirrored(bulged(&p.vertices, p.closed), p.mirror);
        self.push(Shape { pts, closed: p.closed, color: p.color, layer: p.layer }, target)
    }

    fn push(&mut self, mut s: Shape, target: &Target) -> Result<()> {
        if s.pts.is_empty() {
            return Ok(());
        }
        s.closed &= s.pts.len() > 2;
        match target {
            Target::Drawing => {
                self.drawn_points += s.pts.len();
                if self.drawn_points > MAX_FLAT_POINTS {
                    bail!("DXF trop complexe : plus de {MAX_FLAT_POINTS} points une fois les courbes tracées");
                }
                self.shapes.push(s);
            }
            Target::Block(name) => {
                self.block_points += s.pts.len();
                if self.block_points > MAX_FLAT_POINTS {
                    bail!("DXF trop complexe : plus de {MAX_FLAT_POINTS} points dans ses blocs");
                }
                if let Some(b) = self.blocks.get_mut(name) {
                    b.shapes.push(s);
                }
            }
            Target::Nowhere => {}
        }
        Ok(())
    }

    /// A block drawn in the drawing: its shapes, placed. Layer `0` takes
    /// the insert's layer, `BYBLOCK` its colour.
    fn insert(&mut self, rec: &Record, color: Color, layer: usize) -> Result<()> {
        let name = rec.text(2).unwrap_or("").to_ascii_uppercase();
        let Some(block) = self.blocks.get(&name) else {
            self.missing_blocks += 1;
            return Ok(());
        };
        let place = (|| -> Result<Insert, ()> {
            let sx = rec.num_or(41, 1.0)?;
            Ok(Insert {
                base: block.base,
                at: rec.point(10)?,
                scale: [sx, rec.num_or(42, sx)?],
                rot: rec.num_or(50, 0.0)?.to_radians(),
                mirror: rec.extrusion()?.ok_or(())?,
            })
        })();
        let Ok(place) = place else {
            self.bad_data += 1;
            return Ok(());
        };
        if rec.int(70).unwrap_or(1) > 1 || rec.int(71).unwrap_or(1) > 1 {
            self.arrays += 1;
        }
        let layer0 = self.layer_ids.get("0").copied();
        let placed: Vec<Shape> = block
            .shapes
            .iter()
            .map(|s| Shape {
                pts: s.pts.iter().map(|&p| place.apply(p)).collect(),
                closed: s.closed,
                color: if s.color == Color::ByBlock { color } else { s.color },
                layer: if Some(s.layer) == layer0 { layer } else { s.layer },
            })
            .collect();
        for s in placed {
            self.push(s, &Target::Drawing)?;
        }
        Ok(())
    }

    fn warn(&self, warnings: &mut Vec<String>) {
        if !self.ignored.is_empty() {
            let mut kinds: Vec<(&String, &usize)> = self.ignored.iter().collect();
            kinds.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let list: Vec<String> = kinds.iter().take(8).map(|(k, n)| format!("{k} ×{n}")).collect();
            let more = if kinds.len() > 8 { ", …" } else { "" };
            warnings.push(format!(
                "entité(s) ignorée(s) : {}{more} (textes, hachures, cotes et entités 3D ne sont pas tracés : décomposez-les en lignes dans votre logiciel de CAO)",
                list.join(", ")
            ));
        }
        if self.paper_space > 0 {
            warnings.push(format!("{} entité(s) de l'espace papier ignorée(s) : seul l'espace objet est tracé", self.paper_space));
        }
        if self.nested_inserts > 0 {
            warnings.push(format!("{} bloc(s) inséré(s) dans un autre bloc ignoré(s) : décomposez-les", self.nested_inserts));
        }
        if self.missing_blocks > 0 {
            warnings.push(format!("{} insertion(s) d'un bloc introuvable ignorée(s)", self.missing_blocks));
        }
        if self.arrays > 0 {
            warnings.push(format!("{} insertion(s) en réseau : seule la première copie est tracée", self.arrays));
        }
        if self.bad_data > 0 {
            warnings.push(format!("{} entité(s) aux données invalides ignorée(s)", self.bad_data));
        }
    }

    /// Colours resolved, hidden layers left out, y turned down (the
    /// common finish turns it back up); with « Par calque », one part per
    /// layer, in its colour.
    fn finish(self, opts: &Options, warnings: &mut Vec<String>) -> (Vec<Path>, Vec<String>) {
        let mut parts: Vec<String> = Vec::new();
        let mut part_of: HashMap<usize, u8> = HashMap::new();
        let (mut hidden, mut overflow) = (0, 0);
        let mut out = Vec::with_capacity(self.shapes.len());
        for s in self.shapes {
            let name = &self.layer_names[s.layer];
            let layer = self.layers.get(&name.to_ascii_uppercase());
            if layer.is_some_and(|l| l.hidden) {
                hidden += 1;
                continue;
            }
            let layer_color = layer.map_or([255, 255, 255], |l| l.color);
            let (color, part) = if opts.by_layer {
                let part = *part_of.entry(s.layer).or_insert_with(|| {
                    if parts.len() < crate::figures::MAX_PARTS as usize {
                        let clean: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_PART_NAME).collect();
                        parts.push(if clean.is_empty() { "Calque".into() } else { clean });
                        parts.len() as u8
                    } else {
                        0
                    }
                });
                if part == 0 {
                    overflow += 1;
                }
                (layer_color, part)
            } else {
                let c = match s.color {
                    Color::Rgb(c) => c,
                    // BYBLOCK outside a block: white, as CAD programs draw it.
                    Color::ByBlock => [255, 255, 255],
                    Color::ByLayer => layer_color,
                };
                (c, 0)
            };
            let pts = s.pts.into_iter().map(|p| [p[0], -p[1]]).collect();
            out.push(Path { pts, closed: s.closed, color: laser_color(color, opts.color), part });
        }
        if hidden > 0 {
            warnings.push(format!("{hidden} entité(s) sur des calques éteints ou gelés ignorée(s)"));
        }
        if overflow > 0 {
            let max = crate::figures::MAX_PARTS;
            warnings.push(format!("{overflow} tracé(s) de calques au-delà de {max} : sans partie (« Aucune »)"));
        }
        (out, parts)
    }
}

/// x mirrored for an entity drawn with a -Z extrusion.
fn mirrored(mut pts: Vec<Pt>, mirror: f64) -> Vec<Pt> {
    if mirror < 0.0 {
        for p in &mut pts {
            p[0] = -p[0];
        }
    }
    pts
}

fn dist(a: Pt, b: Pt) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Segments of an arc, a multiple of 4 so quadrant points are exact.
fn arc_steps(sweep: f64) -> usize {
    let n = (sweep.abs() / ARC_STEP).ceil().clamp(4.0, 512.0) as usize;
    n.div_ceil(4) * 4
}

/// Points of an arc from angle `a0`, turning `sweep` (radians, positive =
/// counter-clockwise); the end point only if `with_end`.
fn arc(c: Pt, r: f64, a0: f64, sweep: f64, with_end: bool) -> Vec<Pt> {
    let n = arc_steps(sweep);
    let last = if with_end { n } else { n - 1 };
    (0..=last)
        .map(|i| {
            let a = a0 + sweep * i as f64 / n as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// Polyline vertices with bulges (tan of a quarter of the arc's angle,
/// positive = counter-clockwise) → points. The closing segment of a
/// closed polyline is included but not its repeated first point.
fn bulged(vertices: &[(Pt, f64)], closed: bool) -> Vec<Pt> {
    let mut out = Vec::with_capacity(vertices.len());
    let n = vertices.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    if n > 0 {
        out.push(vertices[0].0);
    }
    for i in 0..segments {
        if out.len() > MAX_FLAT_POINTS {
            // `push` refuses it.
            break;
        }
        let ((p0, b), (p1, _)) = (vertices[i], vertices[(i + 1) % n]);
        let d = dist(p0, p1);
        if b.abs() > 1e-9 && d > 1e-12 && b.abs() < 1e6 {
            // Centre on the chord's left normal, (1 - b²) / 2b half-chords away.
            let (u, v) = ((p1[0] - p0[0]) / d, (p1[1] - p0[1]) / d);
            let off = (1.0 - b * b) / (2.0 * b) * d / 2.0;
            let c = [(p0[0] + p1[0]) / 2.0 - v * off, (p0[1] + p1[1]) / 2.0 + u * off];
            let r = dist(p0, c);
            let a0 = (p0[1] - c[1]).atan2(p0[0] - c[0]);
            let pts = arc(c, r, a0, 4.0 * b.atan(), false);
            out.extend_from_slice(&pts[1..]);
        }
        if i + 1 < n {
            out.push(p1);
        }
    }
    out
}

/// A B-spline (rational when every weight is given and positive) as
/// points. Missing or inconsistent knots are replaced by a clamped,
/// uniform vector.
fn bspline(ctrl: &[Pt], knots: &[f64], weights: &[f64], p: usize) -> Vec<Pt> {
    let n = ctrl.len();
    let valid = knots.len() == n + p + 1 && knots.windows(2).all(|w| w[0] <= w[1]) && knots[n] - knots[p] > 1e-12;
    let knots: Vec<f64> = if valid {
        knots.to_vec()
    } else {
        let inner = n - p;
        (0..n + p + 1).map(|i| (i.saturating_sub(p).min(inner)) as f64 / inner as f64).collect()
    };
    let rational = weights.len() == n && weights.iter().all(|&w| w > 1e-12);
    let w = |i: usize| if rational { weights[i] } else { 1.0 };
    let (t0, t1) = (knots[p], knots[n]);
    let samples = (n * 16).clamp(32, 4096);
    let mut out = Vec::with_capacity(samples + 1);
    let mut d = vec![[0.0; 3]; p + 1];
    for k in 0..=samples {
        let t = t0 + (t1 - t0) * k as f64 / samples as f64;
        // The knot span: knots[s] <= t < knots[s + 1], p <= s < n.
        let mut s = p;
        while s + 1 < n && knots[s + 1] <= t {
            s += 1;
        }
        for (j, dj) in d.iter_mut().enumerate() {
            let i = j + s - p;
            *dj = [ctrl[i][0] * w(i), ctrl[i][1] * w(i), w(i)];
        }
        for r in 1..=p {
            for j in (r..=p).rev() {
                let (a, b) = (knots[j + s - p], knots[j + 1 + s - r]);
                let alpha = if b - a > 1e-15 { (t - a) / (b - a) } else { 0.0 };
                let prev = d[j - 1];
                for (c, v) in d[j].iter_mut().enumerate() {
                    *v = (1.0 - alpha) * prev[c] + alpha * *v;
                }
            }
        }
        let q = d[p];
        if q[2].abs() > 1e-15 {
            out.push([q[0] / q[2], q[1] / q[2]]);
        }
    }
    out
}

/// A 24-bit colour, code 420: 0x00RRGGBB.
fn true_color(v: i64) -> [u8; 3] {
    let v = v as u32;
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// AutoCAD Colour Index → RGB, our own generated table: 1–9 named
/// colours, 10–249 24 hues × 5 brightness levels × full / half
/// saturation, 250–255 greys. Close to what CAD programs show, not a copy
/// of any program's table.
fn aci(i: u8) -> [u8; 3] {
    let hsv = |h: f64, s: f64, v: f64| -> [u8; 3] {
        let c = v * s;
        let hp = (h / 60.0) % 6.0;
        let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
        let (r, g, b) = match hp as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        [((r + m) * 255.0).round() as u8, ((g + m) * 255.0).round() as u8, ((b + m) * 255.0).round() as u8]
    };
    match i {
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        0 | 7 => [255, 255, 255],
        8 => [128, 128, 128],
        9 => [192, 192, 192],
        10..=249 => {
            let k = i - 10;
            let hue = (k / 10) as f64 * 15.0;
            let level = [1.0, 0.8, 0.6, 0.5, 0.3][(k % 10 / 2) as usize];
            let sat = if k.is_multiple_of(2) { 1.0 } else { 0.5 };
            hsv(hue, sat, level)
        }
        _ => {
            let g = [0.2, 0.33, 0.47, 0.6, 0.8, 1.0][(i - 250) as usize];
            let v = (g * 255.0_f64).round() as u8;
            [v, v, v]
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A DXF writer for the tests: every fixture is generated here.
    #[derive(Default)]
    pub struct Dxf {
        out: String,
    }

    impl Dxf {
        pub fn pair(&mut self, code: i32, value: impl std::fmt::Display) -> &mut Self {
            self.out.push_str(&format!("{code}\n{value}\n"));
            self
        }
        pub fn section(&mut self, name: &str) -> &mut Self {
            self.pair(0, "SECTION").pair(2, name)
        }
        pub fn endsec(&mut self) -> &mut Self {
            self.pair(0, "ENDSEC")
        }
        pub fn layer(&mut self, name: &str, aci: i32) -> &mut Self {
            self.pair(0, "LAYER").pair(2, name).pair(70, 0).pair(62, aci)
        }
        pub fn line(&mut self, layer: &str, a: Pt, b: Pt) -> &mut Self {
            self.pair(0, "LINE").pair(8, layer).pair(10, a[0]).pair(20, a[1]).pair(30, 0).pair(11, b[0]).pair(21, b[1]).pair(31, 0)
        }
        pub fn lwpolyline(&mut self, layer: &str, pts: &[(Pt, f64)], closed: bool) -> &mut Self {
            self.pair(0, "LWPOLYLINE").pair(8, layer).pair(90, pts.len()).pair(70, if closed { 1 } else { 0 });
            for (p, b) in pts {
                self.pair(10, p[0]).pair(20, p[1]);
                if *b != 0.0 {
                    self.pair(42, b);
                }
            }
            self
        }
        pub fn circle(&mut self, layer: &str, c: Pt, r: f64) -> &mut Self {
            self.pair(0, "CIRCLE").pair(8, layer).pair(10, c[0]).pair(20, c[1]).pair(40, r)
        }
        pub fn eof(&mut self) -> String {
            self.pair(0, "EOF");
            std::mem::take(&mut self.out)
        }
    }

    /// Our test drawing: a square, a circle, an arc, a polyline with a
    /// bulge, a spline and a block inserted twice, coloured by ACI, 24-bit
    /// colour and by layer.
    pub fn drawing() -> String {
        let mut d = Dxf::default();
        d.pair(999, "Laser Studio test drawing").section("HEADER").pair(9, "$ACADVER").pair(1, "AC1015").endsec();
        d.section("TABLES").pair(0, "TABLE").pair(2, "LAYER").pair(70, 3);
        d.layer("0", 7).layer("Rouge", 1).layer("Vert", 3).layer("Cache", -5);
        d.pair(0, "ENDTAB").endsec();
        d.section("BLOCKS");
        // A small triangle, base point (1, 1); its layer-0 side takes the insert's layer.
        d.pair(0, "BLOCK").pair(8, "0").pair(2, "Tri").pair(70, 0).pair(10, 1).pair(20, 1).pair(30, 0);
        d.lwpolyline("0", &[([1.0, 1.0], 0.0), ([2.0, 1.0], 0.0), ([1.0, 2.0], 0.0)], true);
        d.pair(0, "ENDBLK").endsec();
        d.section("ENTITIES");
        // Square, by layer (red).
        d.lwpolyline("Rouge", &[([0.0, 0.0], 0.0), ([10.0, 0.0], 0.0), ([10.0, 10.0], 0.0), ([0.0, 10.0], 0.0)], true);
        // Circle, ACI 5 (blue).
        d.circle("Vert", [20.0, 5.0], 3.0).pair(62, 5);
        // Arc, 24-bit orange, 0° → 90°, radius 2 around (30, 0).
        d.pair(0, "ARC").pair(8, "Vert").pair(420, 0xFF8000).pair(10, 30).pair(20, 0).pair(40, 2).pair(50, 0).pair(51, 90);
        // Polyline (0,20) → (2,20) with a half circle (bulge 1, counter-clockwise: dips to y = 19).
        d.lwpolyline("Vert", &[([0.0, 20.0], 1.0), ([2.0, 20.0], 0.0), ([2.0, 22.0], 0.0)], false);
        // Quadratic clamped spline: (40,0) (42,4) (44,0).
        d.pair(0, "SPLINE").pair(8, "Vert").pair(70, 8).pair(71, 2).pair(72, 6).pair(73, 3).pair(74, 0);
        for k in [0, 0, 0, 1, 1, 1] {
            d.pair(40, k);
        }
        for p in [[40.0, 0.0], [42.0, 4.0], [44.0, 0.0]] {
            d.pair(10, p[0]).pair(20, p[1]).pair(30, 0);
        }
        // Block at (50, 50), scaled 2, turned 90°.
        d.pair(0, "INSERT").pair(8, "Rouge").pair(2, "Tri").pair(10, 50).pair(20, 50).pair(41, 2).pair(42, 2).pair(50, 90);
        // A hatch, a text and a point: ignored, counted.
        d.pair(0, "HATCH").pair(8, "0").pair(0, "TEXT").pair(8, "0").pair(1, "Bonjour").pair(0, "POINT").pair(8, "0").pair(10, 1).pair(20, 1);
        // On a layer that is off: not drawn.
        d.line("Cache", [0.0, 0.0], [5.0, 5.0]);
        d.endsec();
        d.eof()
    }

    fn run(text: &str, opts: &Options) -> (Vec<Path>, Vec<String>, Vec<String>) {
        let mut w = Vec::new();
        let (paths, parts) = parse(text, opts, &mut w).unwrap();
        (paths, parts, w)
    }

    /// Points back in DXF coordinates (y up).
    fn pts(p: &Path) -> Vec<Pt> {
        p.pts.iter().map(|q| [q[0], -q[1]]).collect()
    }

    fn near(a: Pt, b: Pt) -> bool {
        dist(a, b) < 1e-3
    }

    fn has(p: &Path, q: Pt) -> bool {
        pts(p).iter().any(|&a| near(a, q))
    }

    #[test]
    fn recognises_ascii_dxf_only() {
        assert!(looks_like("  0\nSECTION\n  2\nHEADER\n"));
        assert!(looks_like("999\ncomment\n0\r\nSECTION\r\n"));
        assert!(!looks_like("0\nEOF\n"));
        assert!(!looks_like("<svg>"));
        assert!(!looks_like(""));
    }

    #[test]
    fn the_test_drawing_comes_out_right() {
        let (paths, parts, warnings) = run(&drawing(), &Options::default());
        assert!(parts.is_empty());
        assert_eq!(paths.len(), 6, "{paths:?}");
        // Square: four corners, closed, red by layer.
        let sq = &paths[0];
        assert!(sq.closed && sq.color == [255, 0, 0]);
        assert_eq!(pts(sq), vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        // Circle: closed, every point at radius 3, blue.
        let ci = &paths[1];
        assert!(ci.closed && ci.color == [0, 0, 255]);
        assert!(pts(ci).iter().all(|&p| (dist(p, [20.0, 5.0]) - 3.0).abs() < 1e-9));
        assert!(has(ci, [23.0, 5.0]) && has(ci, [20.0, 8.0]) && has(ci, [17.0, 5.0]) && has(ci, [20.0, 2.0]));
        // Arc: (32, 0) → (30, 2), 24-bit orange.
        let ar = &paths[2];
        assert!(!ar.closed && ar.color == [255, 128, 0]);
        assert!(near(pts(ar)[0], [32.0, 0.0]) && near(*pts(ar).last().unwrap(), [30.0, 2.0]));
        assert!(pts(ar).iter().all(|&p| (dist(p, [30.0, 0.0]) - 2.0).abs() < 1e-9));
        // Bulge: a half circle around (1, 20) below the chord, then the straight side.
        let bu = &paths[3];
        assert_eq!(bu.color, [0, 255, 0], "by layer « Vert »");
        let b = pts(bu);
        assert!(near(b[0], [0.0, 20.0]) && near(*b.last().unwrap(), [2.0, 22.0]));
        let arc_pts: Vec<Pt> = b.iter().copied().take_while(|p| p[1] <= 20.0 + 1e-9).collect();
        assert!(arc_pts.iter().all(|&p| (dist(p, [1.0, 20.0]) - 1.0).abs() < 1e-9));
        let low = arc_pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
        assert!((low - 19.0).abs() < 1e-3, "{low}");
        // Spline: the quadratic Bézier's ends and middle (0.25 P0 + 0.5 P1 + 0.25 P2).
        let sp = pts(&paths[4]);
        assert!(near(sp[0], [40.0, 0.0]) && near(*sp.last().unwrap(), [44.0, 0.0]));
        assert!(sp.iter().any(|&p| near(p, [42.0, 2.0])));
        // Block: base (1, 1) → (50, 50), × 2, 90°: (2,1) → (50, 52), (1,2) → (48, 50).
        let bl = &paths[5];
        assert!(bl.closed && bl.color == [255, 0, 0], "layer 0 inside the block takes the insert's layer");
        assert_eq!(bl.pts.len(), 3);
        assert!(has(bl, [50.0, 50.0]) && has(bl, [50.0, 52.0]) && has(bl, [48.0, 50.0]));
        // Ignored things are named and counted; the hidden layer too.
        let all = warnings.join(" | ");
        assert!(all.contains("HATCH ×1") && all.contains("TEXT ×1") && all.contains("POINT ×1"), "{all}");
        assert!(all.contains("calques éteints"), "{all}");
        assert!(!all.contains("tronqué"), "{all}");
        assert_eq!(paths.iter().filter(|p| p.color == [255, 0, 0]).count(), 2);
    }

    #[test]
    fn by_layer_gives_one_part_and_one_colour_per_layer() {
        let opts = Options { by_layer: true, ..Options::default() };
        let (paths, parts, _) = run(&drawing(), &opts);
        assert_eq!(parts, ["Rouge", "Vert"]);
        for p in &paths {
            match p.part {
                1 => assert_eq!(p.color, [255, 0, 0]),
                2 => assert_eq!(p.color, [0, 255, 0]),
                other => panic!("part {other}"),
            }
        }
        // Past eight layers, the rest has no part.
        let mut d = Dxf::default();
        d.section("ENTITIES");
        for i in 0..10 {
            d.line(&format!("C{i}"), [i as f64, 0.0], [i as f64, 1.0]);
        }
        d.endsec();
        let (paths, parts, w) = run(&d.eof(), &opts);
        assert_eq!(parts.len(), 8);
        assert_eq!(paths.iter().filter(|p| p.part == 0).count(), 2);
        assert!(w.iter().any(|w| w.contains("au-delà de 8")), "{w:?}");
    }

    #[test]
    fn aci_table_and_true_colours() {
        assert_eq!(aci(1), [255, 0, 0]);
        assert_eq!(aci(3), [0, 255, 0]);
        assert_eq!(aci(5), [0, 0, 255]);
        assert_eq!(aci(7), [255, 255, 255]);
        assert_eq!(aci(10), [255, 0, 0]);
        assert_eq!(aci(90), [0, 255, 0], "hue 120°");
        assert_eq!(aci(170), [0, 0, 255], "hue 240°");
        assert_eq!(aci(255), [255, 255, 255]);
        let distinct: std::collections::HashSet<[u8; 3]> = (0..=255u8).map(aci).collect();
        assert!(distinct.len() > 200);
        assert_eq!(true_color(0x123456), [0x12, 0x34, 0x56]);
        // BYBLOCK takes the insert's colour; ACI on the entity beats its layer.
        let mut d = Dxf::default();
        d.section("BLOCKS").pair(0, "BLOCK").pair(2, "B").pair(10, 0).pair(20, 0);
        d.line("0", [0.0, 0.0], [1.0, 0.0]).pair(62, 0);
        d.pair(0, "ENDBLK").endsec().section("ENTITIES");
        d.pair(0, "INSERT").pair(8, "0").pair(2, "B").pair(62, 6).pair(10, 0).pair(20, 0);
        d.line("0", [0.0, 0.0], [0.0, 1.0]).pair(62, 2);
        d.endsec();
        let (paths, _, w) = run(&d.eof(), &Options::default());
        assert_eq!(paths.iter().map(|p| p.color).collect::<Vec<_>>(), [[255, 0, 255], [255, 255, 0]], "{w:?}");
    }

    #[test]
    fn polylines_ellipses_and_mirrored_entities() {
        let mut d = Dxf::default();
        d.section("ENTITIES");
        // Old-style POLYLINE, closed, with a bulge on its closing side.
        d.pair(0, "POLYLINE").pair(8, "0").pair(66, 1).pair(70, 1);
        for (p, b) in [([0.0, 0.0], 0.0), ([4.0, 0.0], 0.0), ([4.0, 4.0], 0.0), ([0.0, 4.0], 1.0)] {
            d.pair(0, "VERTEX").pair(8, "0").pair(10, p[0]).pair(20, p[1]);
            if b != 0.0 {
                d.pair(42, b);
            }
        }
        d.pair(0, "SEQEND");
        // Full ellipse, major (3, 0), ratio 0.5.
        d.pair(0, "ELLIPSE").pair(8, "0").pair(10, 10).pair(20, 0).pair(11, 3).pair(21, 0).pair(40, 0.5).pair(41, 0).pair(42, TAU);
        // A circle on a -Z extrusion: its centre's x is mirrored.
        d.circle("0", [5.0, 0.0], 1.0).pair(210, 0).pair(220, 0).pair(230, -1);
        // Tilted plane: refused as 3D data.
        d.circle("0", [0.0, 0.0], 1.0).pair(210, 1).pair(220, 0).pair(230, 0);
        d.pair(0, "3DFACE").pair(8, "0");
        d.endsec();
        let (paths, _, w) = run(&d.eof(), &Options::default());
        assert_eq!(paths.len(), 3, "{paths:?}");
        let pl = pts(&paths[0]);
        assert!(paths[0].closed);
        assert_eq!(pl[..4], [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]);
        // Counter-clockwise half circle from (0,4) to (0,0): bulges out to x = -2.
        let left = pl.iter().map(|p| p[0]).fold(f64::MAX, f64::min);
        assert!((left + 2.0).abs() < 1e-3, "{left}");
        let el = pts(&paths[1]);
        assert!(paths[1].closed);
        assert!(has(&paths[1], [13.0, 0.0]) && has(&paths[1], [10.0, 1.5]) && has(&paths[1], [7.0, 0.0]));
        assert!(el.iter().all(|p| (((p[0] - 10.0) / 3.0).powi(2) + (p[1] / 1.5).powi(2) - 1.0).abs() < 1e-9));
        assert!(pts(&paths[2]).iter().all(|&p| (dist(p, [-5.0, 0.0]) - 1.0).abs() < 1e-9));
        let all = w.join(" | ");
        assert!(all.contains("3DFACE ×1") && all.contains("données invalides"), "{all}");
    }

    #[test]
    fn broken_and_hostile_files_never_panic() {
        let good = drawing();
        // Truncated anywhere: an error or a warning, never a panic.
        for cut in (0..good.len()).step_by(37) {
            let mut end = cut;
            while !good.is_char_boundary(end) {
                end -= 1;
            }
            let mut w = Vec::new();
            if let Ok((_, _)) = parse(&good[..end], &Options::default(), &mut w) {
                if end < good.len() - 8 {
                    assert!(w.iter().any(|w| w.contains("tronqué")), "cut {end}: {w:?}");
                }
            }
        }
        // Invalid group codes.
        let e = parse("0\nSECTION\nabc\nENTITIES\n", &Options::default(), &mut Vec::new()).unwrap_err().to_string();
        assert!(e.contains("code de groupe invalide ligne 3"), "{e}");
        assert!(parse("0\nSECTION\n99999\nx\n", &Options::default(), &mut Vec::new()).is_err());
        // Bad numbers: the entity is left out.
        let mut d = Dxf::default();
        d.section("ENTITIES").line("0", [0.0, 0.0], [1.0, 1.0]).pair(0, "CIRCLE").pair(10, "NaN").pair(20, 0).pair(40, 1);
        d.pair(0, "LINE").pair(10, "1e400").pair(20, 0).pair(11, 1).pair(21, 1);
        d.pair(0, "SPLINE").pair(71, 99).pair(10, 0).pair(20, 0).pair(10, 1).pair(20, 1);
        d.pair(0, "LWPOLYLINE").pair(20, 5);
        d.endsec();
        let (paths, _, w) = run(&d.eof(), &Options::default());
        assert_eq!(paths.len(), 1);
        assert!(w.iter().any(|w| w.contains("4 entité(s) aux données invalides")), "{w:?}");
        // Too many entities: refused.
        let mut d = Dxf::default();
        d.section("ENTITIES");
        for _ in 0..=MAX_ENTITIES {
            d.pair(0, "POINT");
        }
        let e = parse(&d.eof(), &Options::default(), &mut Vec::new()).unwrap_err().to_string();
        assert!(e.contains("200000 entités"), "{e}");
        // Light fuzz: random bytes and random mutations of a good file.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let lines: Vec<&str> = good.lines().collect();
        for _ in 0..300 {
            let mut l: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
            for _ in 0..1 + rnd() % 6 {
                let i = (rnd() % l.len() as u64) as usize;
                l[i] = match rnd() % 6 {
                    0 => String::new(),
                    1 => format!("{}", (rnd() % 2000) as i64 - 10),
                    2 => "-1e308".into(),
                    3 => "0".into(),
                    4 => "SEQEND".into(),
                    _ => "1e-300".into(),
                };
            }
            let text = l.join("\n");
            let _ = parse(&text, &Options::default(), &mut Vec::new());
            let bytes: Vec<u8> = (0..(rnd() % 400)).map(|_| rnd() as u8).collect();
            let _ = parse(&String::from_utf8_lossy(&bytes), &Options::default(), &mut Vec::new());
        }
        // Silly bulges, huge spline knots, empty polylines: no panic.
        let mut d = Dxf::default();
        d.section("ENTITIES").lwpolyline("0", &[([0.0, 0.0], 1e300), ([0.0, 0.0], -1e-320), ([1.0, 0.0], 1e5)], true);
        d.pair(0, "SPLINE").pair(71, 3).pair(40, 1e12).pair(40, -1e12);
        for i in 0..5 {
            d.pair(10, i).pair(20, 0);
        }
        d.pair(0, "POLYLINE").pair(70, 1).pair(0, "SEQEND");
        d.pair(0, "INSERT").pair(2, "nulle part");
        d.endsec();
        let (_, _, w) = run(&d.eof(), &Options::default());
        assert!(w.iter().any(|w| w.contains("bloc introuvable")), "{w:?}");
    }
}
